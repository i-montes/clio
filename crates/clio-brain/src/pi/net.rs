//! The way out of Pi's jail (ADR 0009, phase 3b). The jail has a network namespace of its own:
//! nothing but a loopback with nobody listening. Two small pieces make the provider reachable and
//! nothing else:
//!
//! - [`EgressProxy`], on the host, one per session: an HTTP `CONNECT` proxy on a unix socket
//!   that allows ONE `host:port`, taken from the provider's URL, and refuses everything else.
//!   TLS stays end to end: it only sees which address Pi asks for.
//! - [`forward_main`], inside the jail (`clio pi-net`): listens on `127.0.0.1` and forwards each
//!   connection to the proxy's socket, and starts Pi as its child. Pi finds it through
//!   `HTTPS_PROXY`: its HTTP stack installs undici's `EnvHttpProxyAgent` at startup.
//!
//! `clio pi-net probe` is the self-test's second half: from inside, the internet, a host loopback
//! port, a host abstract socket and a `CONNECT` to another host must all fail.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Where Pi finds the forwarder inside the jail (the jail's own loopback: no clash is possible).
pub const FORWARD_ADDR: &str = "127.0.0.1:18080";
/// Pi's proxy variables, as the jail sets them.
pub const PROXY_URL: &str = "http://127.0.0.1:18080";

/// The longest request head accepted: a `CONNECT` is a line and a few headers.
const MAX_HEAD: usize = 8 * 1024;
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_CONNECTIONS: usize = 64;
const MAX_DENIED: usize = 100;
/// How often the accept loop looks at the stop flag when nobody is connecting.
const ACCEPT_POLL: Duration = Duration::from_millis(5);
/// A unix socket path cannot be much longer than this.
const MAX_SOCKET_PATH: usize = 100;

static PROXIES: AtomicU32 = AtomicU32::new(0);

/// The destination `url` names, as `host:port` (host in lowercase; the port is the scheme's
/// default when the URL has none). A URL that reads as one host and could mean another (user
/// info, brackets, spaces) is not a destination.
pub fn allow_of(url: &str) -> Result<String, String> {
    let bad = |why: &str| {
        Err(format!(
            "la URL del proveedor no es un destino válido ({why})"
        ))
    };
    let Some((scheme, rest)) = url.split_once("://") else {
        return bad("falta el esquema");
    };
    let default = match scheme.to_ascii_lowercase().as_str() {
        "https" => 443,
        "http" => 80,
        _ => return bad("solo http o https"),
    };
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    if auth.is_empty()
        || auth.contains(['@', '[', ']', '\\', ' '])
        || auth.chars().any(char::is_control)
    {
        return bad("autoridad ambigua");
    }
    let (host, port) = match auth.rsplit_once(':') {
        Some((h, p)) => match p.parse::<u16>() {
            Ok(p) if p > 0 => (h, p),
            _ => return bad("puerto inválido"),
        },
        None => (auth, default),
    };
    let valid = !host.is_empty()
        && !host.starts_with(['.', '-'])
        && !host.ends_with(['.', '-'])
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    if !valid {
        return bad("host inválido");
    }
    Ok(format!("{}:{port}", host.to_ascii_lowercase()))
}

/// The proxy of one session. Dropping it stops it and removes its socket and folder.
pub struct EgressProxy {
    dir: PathBuf,
    sock: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    denied: Arc<Mutex<Vec<String>>>,
}

impl EgressProxy {
    /// Makes `<parent>/egress-<pid>-<n>/` (0700) with the socket inside, and allows only `allow`
    /// (`host:port`, lowercase, as `allow_of` gives it).
    pub fn start(parent: &Path, allow: &str) -> Result<Self, String> {
        let dir = parent.join(format!(
            "egress-{}-{}",
            std::process::id(),
            PROXIES.fetch_add(1, Ordering::SeqCst)
        ));
        let sock = dir.join("proxy.sock");
        if sock.as_os_str().len() > MAX_SOCKET_PATH {
            return Err(format!(
                "la ruta del socket del proxy ({}) es demasiado larga para un socket Unix",
                sock.display()
            ));
        }
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?;
        let listener = UnixListener::bind(&sock).map_err(|e| {
            let _ = std::fs::remove_dir(&dir);
            format!("{}: {e}", sock.display())
        })?;
        std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let denied = Arc::new(Mutex::new(Vec::new()));
        let active = Arc::new(AtomicUsize::new(0));
        let (flag, log, allow) = (stop.clone(), denied.clone(), allow.to_owned());
        // Not a blocking `accept`: stopping must not depend on the socket file still being there
        // to connect to (the runtime folder can be gone before the proxy is dropped).
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("{}: {e}", sock.display()))?;
        let thread = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                let conn = match listener.accept() {
                    Ok((conn, _)) => {
                        // On macOS an accepted socket inherits the listener's non-blocking mode
                        // (on Linux it does not): every read would return at once, and a tunnel
                        // would end before it began.
                        let _ = conn.set_nonblocking(false);
                        conn
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(ACCEPT_POLL);
                        continue;
                    }
                    Err(_) => continue,
                };
                if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                    active.fetch_sub(1, Ordering::SeqCst);
                    continue; // dropped: too many at once
                }
                let (allow, log, active) = (allow.clone(), log.clone(), active.clone());
                std::thread::spawn(move || {
                    handle(conn, &allow, &log);
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        Ok(Self {
            dir,
            sock,
            stop,
            thread: Some(thread),
            denied,
        })
    }

    /// The unix socket the forwarder inside the jail connects to.
    pub fn socket(&self) -> &Path {
        &self.sock
    }

    /// The folder that holds only that socket: the one the jail mounts.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Forgets what was refused so far. For after the self-test, which asks for a destination it
    /// knows will be refused: that is not something Pi did, and the person must not be shown it.
    pub fn clear_denied(&self) {
        self.denied
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// What Pi (or whatever runs in the jail) asked for and was refused, oldest first.
    pub fn denied(&self) -> Vec<String> {
        self.denied
            .lock()
            .map(|d| d.clone())
            .unwrap_or_else(|e| e.into_inner().clone())
    }
}

impl Drop for EgressProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_file(&self.sock);
        let _ = std::fs::remove_dir(&self.dir);
    }
}

fn remember(log: &Mutex<Vec<String>>, what: String) {
    let mut d = log.lock().unwrap_or_else(|e| e.into_inner());
    if d.len() < MAX_DENIED {
        d.push(what);
    }
}

/// The request head (up to the blank line) and whatever followed it in the same read.
fn read_head(c: &mut UnixStream) -> Result<(String, Vec<u8>), &'static str> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(end + 4);
            buf.truncate(end);
            return Ok((String::from_utf8_lossy(&buf).into_owned(), rest));
        }
        if buf.len() > MAX_HEAD {
            return Err("431 Request Header Fields Too Large");
        }
        match c.read(&mut chunk) {
            Ok(0) | Err(_) => return Err("400 Bad Request"),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

/// Answers a refusal and closes without cutting the answer off. Closing with the client's bytes
/// still unread resets the connection on some systems (macOS, with its small socket buffers), and
/// the client then never sees the status: so stop sending, then read and drop what is still
/// arriving, for a moment and up to a limit.
fn refuse(c: &mut UnixStream, status: &str) {
    reply(c, status);
    let _ = c.shutdown(std::net::Shutdown::Write);
    let _ = c.set_read_timeout(Some(Duration::from_millis(200)));
    let (mut sink, mut dropped) = ([0u8; 4096], 0usize);
    while dropped < 256 * 1024 {
        match c.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(n) => dropped += n,
        }
    }
}

fn reply(c: &mut UnixStream, status: &str) {
    // One write: `write!` on a socket sends each piece of the format on its own, and a reader
    // that reads once can then see only the start of the answer.
    let msg = format!("HTTP/1.1 {status}\r\nConnection: close\r\n\r\n");
    let _ = c.write_all(msg.as_bytes());
    let _ = c.flush();
}

fn handle(mut c: UnixStream, allow: &str, log: &Mutex<Vec<String>>) {
    let _ = c.set_read_timeout(Some(HEAD_TIMEOUT));
    let (head, rest) = match read_head(&mut c) {
        Ok(h) => h,
        Err(status) => return refuse(&mut c, status),
    };
    let line = head.lines().next().unwrap_or("");
    let mut parts = line.split_ascii_whitespace();
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return refuse(&mut c, "400 Bad Request");
    };
    if !version.starts_with("HTTP/1.") {
        return refuse(&mut c, "400 Bad Request");
    }
    // Only a tunnel. A plain request would make the proxy fetch for Pi, and read what it gets.
    if method != "CONNECT" {
        remember(log, format!("{method} (no es CONNECT)"));
        return refuse(&mut c, "405 Method Not Allowed");
    }
    if target.to_ascii_lowercase() != allow {
        remember(log, target.chars().take(120).collect());
        return refuse(&mut c, "403 Forbidden");
    }
    let Some(up) = connect_upstream(allow) else {
        return refuse(&mut c, "502 Bad Gateway");
    };
    reply_open(&mut c);
    let _ = c.set_read_timeout(None);
    tunnel(c, up, &rest);
}

fn reply_open(c: &mut UnixStream) {
    let _ = c.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n");
    let _ = c.flush();
}

fn connect_upstream(allow: &str) -> Option<TcpStream> {
    allow
        .to_socket_addrs()
        .ok()?
        .find_map(|a| TcpStream::connect_timeout(&a, CONNECT_TIMEOUT).ok())
}

/// Copies both ways until either side closes. `first` is what the client sent after the head.
fn tunnel(mut c: UnixStream, mut up: TcpStream, first: &[u8]) {
    if !first.is_empty() && up.write_all(first).is_err() {
        return;
    }
    let (Ok(mut c2), Ok(mut up2)) = (c.try_clone(), up.try_clone()) else {
        return;
    };
    let t = std::thread::spawn(move || {
        let _ = std::io::copy(&mut c2, &mut up2);
        let _ = up2.shutdown(std::net::Shutdown::Write);
    });
    let _ = std::io::copy(&mut up, &mut c);
    let _ = c.shutdown(std::net::Shutdown::Write);
    let _ = t.join();
}

// ---- inside the jail ----------------------------------------------------------------------------

/// `clio pi-net --listen ADDR --upstream SOCKET -- COMMAND [ARGS...]`, run INSIDE the jail: it
/// forwards every connection to `ADDR` into `SOCKET` and runs the command as its child (its
/// input and output are the command's), ending with it. Returns the command's exit code.
/// `clio pi-net probe ...` runs the self-test's network half instead.
pub fn forward_main(args: Vec<String>) -> Result<i32, String> {
    if args.first().map(String::as_str) == Some("probe") {
        return probe_main(&args[1..]);
    }
    let (mut listen, mut upstream, mut cmd) = (None, None, Vec::new());
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--listen" => listen = it.next(),
            "--upstream" => upstream = it.next(),
            "--" => {
                cmd = it.collect();
                break;
            }
            other => return Err(format!("pi-net: argumento desconocido {other}")),
        }
    }
    let (Some(listen), Some(upstream)) = (listen, upstream) else {
        return Err("pi-net: faltan --listen y --upstream".into());
    };
    let Some((program, rest)) = cmd.split_first() else {
        return Err("pi-net: falta el comando después de --".into());
    };
    let listener = TcpListener::bind(&listen).map_err(|e| format!("pi-net: {listen}: {e}"))?;
    let upstream = PathBuf::from(upstream);
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let up = upstream.clone();
            std::thread::spawn(move || forward(conn, &up));
        }
    });
    let status = Command::new(program)
        .args(rest)
        .status()
        .map_err(|e| format!("pi-net: {program}: {e}"))?;
    Ok(status.code().unwrap_or_else(|| {
        use std::os::unix::process::ExitStatusExt;
        128 + status.signal().unwrap_or(0)
    }))
}

fn forward(mut c: TcpStream, upstream: &Path) {
    let Ok(mut up) = UnixStream::connect(upstream) else {
        return;
    };
    let (Ok(mut c2), Ok(mut up2)) = (c.try_clone(), up.try_clone()) else {
        return;
    };
    let t = std::thread::spawn(move || {
        let _ = std::io::copy(&mut c2, &mut up2);
        let _ = up2.shutdown(std::net::Shutdown::Write);
    });
    let _ = std::io::copy(&mut up, &mut c);
    let _ = c.shutdown(std::net::Shutdown::Write);
    let _ = t.join();
}

/// What the self-test asks the jail's network to refuse. Prints one line per check; exit 0 only if
/// every one failed to get out.
fn probe_main(args: &[String]) -> Result<i32, String> {
    let (mut tcp, mut abstract_name, mut denied) = (None, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--tcp-port" => tcp = it.next().and_then(|p| p.parse::<u16>().ok()),
            "--abstract" => abstract_name = it.next().cloned(),
            "--denied" => denied = it.next().cloned(),
            other => return Err(format!("pi-net probe: argumento desconocido {other}")),
        }
    }
    let mut bad = 0;
    let mut check = |name: &str, got_out: bool| {
        if got_out {
            bad += 1;
            println!("pi-net: {name}: ALCANZABLE desde dentro de la jaula");
        }
    };
    let timeout = Duration::from_secs(3);
    let internet = std::net::SocketAddr::from(([1, 1, 1, 1], 443));
    check(
        "internet directo",
        TcpStream::connect_timeout(&internet, timeout).is_ok(),
    );
    if let Some(p) = tcp {
        let lo = std::net::SocketAddr::from(([127, 0, 0, 1], p));
        check(
            "loopback del anfitrión",
            TcpStream::connect_timeout(&lo, timeout).is_ok(),
        );
    }
    if let Some(name) = &abstract_name {
        check("socket abstracto del anfitrión", abstract_reachable(name));
    }
    if let Some(target) = denied {
        let refused = TcpStream::connect(FORWARD_ADDR)
            .ok()
            .and_then(|mut s| {
                s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
                write!(s, "CONNECT {target} HTTP/1.1\r\n\r\n").ok()?;
                // The status line, read to its end: one `read` can return only a part of it.
                let (mut line, mut b) = (Vec::new(), [0u8; 1]);
                while !line.ends_with(b"\r\n") && line.len() < 256 {
                    if s.read(&mut b).ok()? == 0 {
                        break;
                    }
                    line.push(b[0]);
                }
                Some(String::from_utf8_lossy(&line).contains(" 403 "))
            })
            .unwrap_or(false);
        check("el proxy deja pasar un destino ajeno", !refused);
    }
    Ok(i32::from(bad > 0))
}

/// Whether an abstract unix socket of the host can be connected to (Linux only).
#[cfg(target_os = "linux")]
fn abstract_reachable(name: &str) -> bool {
    use std::os::linux::net::SocketAddrExt;
    std::os::unix::net::SocketAddr::from_abstract_name(name)
        .ok()
        .is_some_and(|a| UnixStream::connect_addr(&a).is_ok())
}

#[cfg(not(target_os = "linux"))]
fn abstract_reachable(_: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_forwarder_needs_its_arguments() {
        for args in [
            vec![],
            vec!["--listen".into(), "127.0.0.1:0".into()],
            vec![
                "--listen".into(),
                "127.0.0.1:0".into(),
                "--upstream".into(),
                "/x".into(),
            ],
            vec!["--bogus".into()],
        ] {
            assert!(forward_main(args).is_err());
        }
    }
}
