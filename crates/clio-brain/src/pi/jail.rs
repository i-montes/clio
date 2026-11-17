//! Pi inside `bwrap` (ADR 0009, phases 3a and 3b). Pi and its npm dependencies run as the person, so
//! they could connect to the vault's sockets, read `~/.ssh`, or write the working copy and skip
//! the toolhost's rules. Inside the jail Pi sees only what it needs, at the same absolute paths
//! it has outside (no argument or variable changes):
//!
//! - the system, read-only, and the certificates TLS needs;
//! - Node and Pi's install folder (which holds the guard), read-only;
//! - Pi's own `agent/`, `sessions/` and `home/`, read-write;
//! - the prompt file, read-only, and the folder of the toolhost's socket (made for it alone),
//!   the only way out to an effect;
//! - the working copy, **read-only**: every change goes through the toolhost.
//! - with the claude connector, the copy's `.claude/`, `.pi/` and `.mcp.json` covered, and Clio's
//!   managed settings for Claude Code at `/etc/claude-code/managed-settings.json`, read-only.
//!
//! - the `clio` binary and the folder of the egress proxy's socket (see `net`): the only way out
//!   to the network, which is a private one. Pi reaches the provider's address through them and
//!   nothing else: no host loopback, no abstract sockets, no internet.
//!
//! It fails closed: on Linux Pi starts only if `bwrap` works, the proxy starts, and a self-test
//! from inside passes. `CLIO_PI_JAIL=off` is the only way around it, and `clio doctor` shows it
//! as a FAIL.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use clio_node::jail::Jail;

use super::PiError;
use super::launch::{Launch, hardened_env};
use super::net::{EgressProxy, FORWARD_ADDR, PROXY_URL, allow_of};

const SELFTEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Runs inside the jail: the working copy is there and cannot be written, and none of the decoys
/// (files placed outside what the jail mounts) can be seen. The write is tried in a subshell: a
/// failed redirection on a special builtin like `:` ends a POSIX shell, and here failing is the
/// right answer.
const PROBE: &str = r#"cwd=$1; shift
[ -d "$cwd" ] || { echo "the working copy is not mounted"; exit 11; }
( : > "$cwd/.clio-jail-probe" ) 2>/dev/null && { rm -f "$cwd/.clio-jail-probe"; echo "the working copy is writable"; exit 12; }
for p in "$@"; do [ -e "$p" ] && { echo "visible from inside: $p"; exit 13; }; done
exit 0"#;

/// What `doctor` says about Pi's jail.
pub enum Status {
    /// `bwrap` (this version) starts here; the self-test runs at every start.
    Ready(String),
    /// `CLIO_PI_JAIL=off`.
    Off,
    /// Not Linux: there is no jail to build.
    Unsupported,
    /// Linux, but the jail cannot start: Pi will not start either.
    Unavailable(String),
}

pub fn status() -> Status {
    if switched_off() {
        return Status::Off;
    }
    if !cfg!(target_os = "linux") {
        return Status::Unsupported;
    }
    match Jail::detect().and_then(|j| j.probe().map(|()| j.bwrap_version())) {
        Ok(v) => Status::Ready(v),
        Err(why) => Status::Unavailable(why),
    }
}

fn switched_off() -> bool {
    std::env::var("CLIO_PI_JAIL").is_ok_and(|v| v == "off")
}

/// A jail that passed its self-test for one launch. It owns the session's egress proxy: keep it
/// alive as long as Pi runs.
pub struct PiJail {
    bwrap: PathBuf,
    proxy: EgressProxy,
}

/// The policy. `Ok(None)`: Pi runs without a jail (switched off, or a system that cannot have
/// one). `Err`: it must not start.
pub fn prepare(l: &Launch) -> Result<Option<PiJail>, PiError> {
    if switched_off() || !cfg!(target_os = "linux") {
        return Ok(None);
    }
    let refuse = |why: String| {
        PiError::Spawn(format!(
            "Pi no arranca: la jaula de Pi no se pudo preparar ({why}). Sin la jaula Pi tendría tus mismos permisos; instala bubblewrap (bwrap) y habilita los espacios de nombres de usuario, o pon CLIO_PI_JAIL=off bajo tu responsabilidad (ADR 0009)"
        ))
    };
    let jail = Jail::detect().map_err(&refuse)?;
    // The one address Pi may reach: the provider's, from its URL.
    let allow = allow_of(&l.provider.base_url).map_err(&refuse)?;
    let proxy = EgressProxy::start(&egress_parent(l), &allow).map_err(&refuse)?;
    let j = PiJail {
        bwrap: jail.bwrap().to_path_buf(),
        proxy,
    };
    j.selftest(l).map_err(refuse)?;
    j.proxy.clear_denied();
    Ok(Some(j))
}

/// Where the proxy's folder goes: next to the toolhost's (both short-lived, both 0700, both in
/// the runtime folder), or the system's temporary folder when there is no toolhost.
fn egress_parent(l: &Launch) -> PathBuf {
    l.extra_env
        .iter()
        .find(|(k, _)| k == "CLIO_TOOLHOST_SOCKET")
        .and_then(|(_, v)| Path::new(v).parent()?.parent().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir)
}

fn strings(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| (*s).to_owned()).collect()
}

impl PiJail {
    /// `argv` run inside the jail with Pi's environment. The environment is set on `bwrap`
    /// itself, never with `--setenv`: the arguments of a process are readable by every user of
    /// the machine, and the provider's key is in that environment.
    pub fn command(&self, l: &Launch, session_id: &str, argv: &[OsString]) -> Command {
        let mut c = Command::new(&self.bwrap);
        // `current_dir` is for whoever looks at this process from outside (`bwrap` keeps it);
        // inside, `--chdir` puts Pi in the same folder.
        // Inside, the forwarder comes first: it listens on the jail's loopback, sends each
        // connection to the proxy, and starts `argv` as its child.
        let mut inner: Vec<OsString> = vec![
            l.helper.clone().into_os_string(),
            "pi-net".into(),
            "--listen".into(),
            FORWARD_ADDR.into(),
            "--upstream".into(),
            self.proxy.socket().as_os_str().to_owned(),
            "--".into(),
        ];
        inner.extend(argv.iter().cloned());
        c.args(self.args(l))
            .arg("--")
            .args(&inner)
            .env_clear()
            .envs(hardened_env(l, session_id))
            .envs(proxy_env())
            .current_dir(&l.cwd);
        c
    }

    /// What Pi (or anything else in the jail) asked the proxy for and was refused.
    pub fn egress_denied(&self) -> Vec<String> {
        self.proxy.denied()
    }

    /// The exact arguments of `bwrap`, public so a test (and a reviewer) can read what Pi sees.
    pub fn args(&self, l: &Launch) -> Vec<String> {
        let mut a = strings(&[
            // `--unshare-all` includes the network: a namespace of its own, with a loopback
            // that nobody listens on. The way out is the forwarder and the proxy (see `net`).
            "--unshare-all",
            // No `--die-with-parent`: it is tied to the THREAD that started `bwrap`, and a
            // session is opened from a worker thread that ends right after. Pi (and so the jail)
            // ends by itself when its input closes, also if `clio` is killed.
            "--new-session",
            "--cap-drop",
            "ALL",
            "--ro-bind",
            "/usr",
            "/usr",
        ]);
        for top in ["/bin", "/sbin", "/lib", "/lib32", "/lib64"] {
            match std::fs::symlink_metadata(top) {
                Ok(m) if m.is_symlink() => {
                    if let Ok(t) = std::fs::read_link(top) {
                        a.extend(["--symlink".into(), t.display().to_string(), top.into()]);
                    }
                }
                Ok(m) if m.is_dir() => a.extend(["--ro-bind".into(), top.into(), top.into()]),
                _ => {}
            }
        }
        // The loader's cache and the certificates TLS needs, and nothing else of /etc: no DNS
        // files, the proxy resolves the provider's name on the host.
        for f in ["/etc/ld.so.cache", "/etc/ssl", "/etc/ca-certificates"] {
            if Path::new(f).exists() {
                a.extend(["--ro-bind".into(), f.into(), f.into()]);
            }
        }
        a.extend(strings(&[
            "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp",
        ]));
        // From here on, mounts that may sit under /tmp (the tmpfs above comes first).
        let mut seen = BTreeSet::new();
        let mut bind = |a: &mut Vec<String>, flag: &str, p: &Path| {
            let s = p.display().to_string();
            if seen.insert((flag.to_owned(), s.clone())) {
                a.extend([flag.to_owned(), s.clone(), s]);
            }
        };
        let node_root = l.node.path.parent().and_then(Path::parent);
        if let Some(root) = node_root.filter(|r| r.parent().is_some() && !r.starts_with("/usr")) {
            bind(&mut a, "--ro-bind", root);
        }
        bind(&mut a, "--ro-bind", &l.install.dir);
        // The guard lives inside the install folder, already mounted. One that does not (a
        // symlinked checkout) is mounted too: an extension is a folder with an `index.ts` and
        // its siblings, and that folder is its own; a loose file is mounted alone, because the
        // folder around it holds other things.
        if let Some(g) = l
            .guard
            .as_deref()
            .filter(|g| !g.starts_with(&l.install.dir))
        {
            let own = g.file_name().is_some_and(|n| n == "index.ts");
            bind(&mut a, "--ro-bind", g.parent().filter(|_| own).unwrap_or(g));
        }
        for (k, v) in &l.extra_env {
            let p = Path::new(v);
            match k.as_str() {
                // The prompt: that one file. The socket: its own folder (made for it alone).
                "CLIO_GUARD_PROMPT_FILE" => bind(&mut a, "--ro-bind", p),
                "CLIO_TOOLHOST_SOCKET" => {
                    if let Some(d) = p.parent() {
                        bind(&mut a, "--bind", d);
                    }
                }
                _ => {}
            }
        }
        bind(&mut a, "--ro-bind", &l.helper);
        bind(&mut a, "--bind", self.proxy.dir());
        for rw in [
            l.paths.agent_dir(),
            l.paths.sessions_dir(),
            l.paths.home_dir(),
        ] {
            bind(&mut a, "--bind", &rw);
        }
        bind(&mut a, "--ro-bind", &l.cwd);
        if l.provider.extension.is_some() {
            claude_masks(&mut a, l);
        }
        a.extend(["--chdir".into(), l.cwd.display().to_string()]);
        a
    }

    /// Files placed where the jail must not show them: Clio's data folder, the folder that holds
    /// the vault's sockets (the one above the toolhost's) and the person's home.
    fn decoys(&self, l: &Launch) -> Vec<PathBuf> {
        let name = format!(".pi-jail-decoy-{}", std::process::id());
        let mut dirs: Vec<PathBuf> = Vec::new();
        dirs.extend(l.paths.root.parent().map(Path::to_path_buf));
        for (k, v) in &l.extra_env {
            if k == "CLIO_TOOLHOST_SOCKET" {
                dirs.extend(
                    Path::new(v)
                        .parent()
                        .and_then(Path::parent)
                        .map(Path::to_path_buf),
                );
            }
        }
        dirs.extend(std::env::var_os("HOME").map(PathBuf::from));
        let mut out: Vec<PathBuf> = dirs.into_iter().map(|d| d.join(&name)).collect();
        out.sort();
        out.dedup();
        out
    }

    /// Starts the jail the way Pi will be started and checks it from inside, twice: the files
    /// (the copy cannot be written, the decoys cannot be seen) and the network (nothing but the
    /// proxy gets out). Any failure here (a mount that cannot be made, `bwrap` refused, a decoy
    /// visible, the copy writable, a way out that works) means Pi does not start.
    fn selftest(&self, l: &Launch) -> Result<(), String> {
        struct Decoys(Vec<PathBuf>);
        impl Drop for Decoys {
            fn drop(&mut self) {
                for p in &self.0 {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
        let placed = Decoys(
            self.decoys(l)
                .into_iter()
                .filter(|p| std::fs::write(p, b"decoy").is_ok())
                .collect(),
        );
        let mut files: Vec<OsString> = vec![
            "/bin/sh".into(),
            "-c".into(),
            PROBE.into(),
            "probe".into(),
            l.cwd.clone().into_os_string(),
        ];
        files.extend(placed.0.iter().map(|p| p.clone().into_os_string()));
        self.run_probe(l, &files)?;
        // Things on the host that Pi must not reach: a loopback port and an abstract socket.
        let host_port = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = host_port.local_addr().map_err(|e| e.to_string())?.port();
        let abstract_socket = abstract_listener();
        let mut net: Vec<OsString> = vec![
            l.helper.clone().into_os_string(),
            "pi-net".into(),
            "probe".into(),
            "--tcp-port".into(),
            port.to_string().into(),
            "--denied".into(),
            "selftest.invalid:443".into(),
        ];
        if let Some((name, _)) = &abstract_socket {
            net.extend(["--abstract".into(), name.into()]);
        }
        self.run_probe(l, &net)
    }

    fn run_probe(&self, l: &Launch, argv: &[OsString]) -> Result<(), String> {
        let mut child = self
            .command(l, "selftest", argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("no se pudo iniciar bwrap: {e}"))?;
        let t = Instant::now();
        let status = loop {
            if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
                break s;
            }
            if t.elapsed() > SELFTEST_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                return Err("la autoprueba no terminó a tiempo".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        if status.success() {
            return Ok(());
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        let said = String::from_utf8_lossy(&[out.stdout, out.stderr].concat())
            .trim()
            .lines()
            .take(3)
            .collect::<Vec<_>>()
            .join(" | ");
        Err(format!(
            "la autoprueba falló (salida {}): {said}",
            status
                .code()
                .map_or("por señal".to_owned(), |c| c.to_string())
        ))
    }
}

/// The bridge runs Claude Code in the working copy, and Claude Code (and the bridge) load code
/// and settings from there: `.claude/` (hooks, `env`, `apiKeyHelper`), `.pi/claude-bridge.json`
/// (another Claude Code binary, MCP turned back on) and `.mcp.json`. The copy is hostile content
/// (ADR 0009): they are covered with an empty folder or `/dev/null`. A link is followed only to
/// a target inside the copy; one outside it points at something the jail does not mount, or at
/// Pi's own folders, which `launch::reset_claude_settings` empties. Claude Code also gets Clio's
/// managed settings, read-only: no hook runs, whatever any other source says.
fn claude_masks(a: &mut Vec<String>, l: &Launch) {
    let inside = |name: &str| {
        let root = l.cwd.canonicalize().ok()?;
        let real = l.cwd.join(name).canonicalize().ok()?;
        real.strip_prefix(&root).ok().map(|r| l.cwd.join(r))
    };
    for d in [".claude", ".pi"] {
        if let Some(p) = inside(d).filter(|p| p.is_dir()) {
            a.extend(["--tmpfs".into(), p.display().to_string()]);
        }
    }
    if let Some(p) = inside(".mcp.json").filter(|p| p.is_file()) {
        a.extend([
            "--ro-bind".into(),
            "/dev/null".into(),
            p.display().to_string(),
        ]);
    }
    a.extend([
        "--ro-bind".into(),
        l.paths.claude_policy().display().to_string(),
        CLAUDE_POLICY_IN_JAIL.into(),
    ]);
}

/// Where Claude Code 2.1 reads managed settings on Linux.
pub const CLAUDE_POLICY_IN_JAIL: &str = "/etc/claude-code/managed-settings.json";

/// An abstract unix socket on the host (Linux only): the jail must not be able to connect to it.
#[cfg(target_os = "linux")]
fn abstract_listener() -> Option<(String, std::os::unix::net::UnixListener)> {
    use std::os::linux::net::SocketAddrExt;
    let name = format!(
        "clio-jail-probe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos())
    );
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(&name).ok()?;
    let l = std::os::unix::net::UnixListener::bind_addr(&addr).ok()?;
    Some((name, l))
}

#[cfg(not(target_os = "linux"))]
fn abstract_listener() -> Option<(String, std::os::unix::net::UnixListener)> {
    None
}

/// Pi's proxy variables: everything it sends over the network goes to the forwarder, none of it is
/// exempt (`NO_PROXY` empty).
fn proxy_env() -> Vec<(&'static str, &'static str)> {
    vec![
        ("HTTPS_PROXY", PROXY_URL),
        ("HTTP_PROXY", PROXY_URL),
        ("https_proxy", PROXY_URL),
        ("http_proxy", PROXY_URL),
        ("NO_PROXY", ""),
        ("no_proxy", ""),
    ]
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::pi::PiPaths;
    use crate::pi::install::{NodeInfo, PiInstall};
    use crate::pi::launch::SessionTarget;

    fn args_for(connector: &str, cwd: &Path, data: &Path) -> Vec<String> {
        let l = Launch {
            install: PiInstall {
                version: "1.4.0".into(),
                dir: "/p/1.4.0".into(),
                bin: "/p/1.4.0/node_modules/.bin/pi".into(),
            },
            node: NodeInfo {
                path: "/n/bin/node".into(),
                version: (22, 23, 2),
            },
            paths: PiPaths::under_data(data),
            provider: crate::connector::by_name(connector)
                .unwrap()
                .pi_launch_provider("m"),
            api_key: None,
            cwd: cwd.into(),
            session: SessionTarget::New { id: None },
            name: None,
            guard: None,
            guard_tools: vec![],
            extra_env: vec![],
            helper: "/p/clio".into(),
        };
        let j = PiJail {
            bwrap: "/usr/bin/bwrap".into(),
            proxy: EgressProxy::start(data, "api.anthropic.com:443").unwrap(),
        };
        j.args(&l)
    }

    // Finding 1 of the claude-bridge review: a working copy's `.claude/settings.json` hooks ran in
    // the jail and read the token. With the bridge they are covered, and the policy is mounted.
    #[test]
    fn with_the_bridge_the_copys_claude_settings_are_masked_and_the_policy_is_mounted() {
        let t = tempfile::tempdir().unwrap();
        let cwd = t.path().join("proj");
        std::fs::create_dir_all(cwd.join(".claude")).unwrap();
        std::fs::create_dir_all(cwd.join(".pi")).unwrap();
        std::fs::write(cwd.join(".mcp.json"), "{}").unwrap();
        let s = |p: &Path| p.display().to_string();
        let has = |a: &[String], want: &[String]| a.windows(want.len()).any(|w| w == want);
        let a = args_for("claude", &cwd, t.path());
        let at = |want: &[String]| a.windows(want.len()).position(|w| w == want).unwrap();
        let copy = at(&["--ro-bind".into(), s(&cwd), s(&cwd)]);
        for want in [
            vec!["--tmpfs".into(), s(&cwd.join(".claude"))],
            vec!["--tmpfs".into(), s(&cwd.join(".pi"))],
            vec![
                "--ro-bind".into(),
                "/dev/null".into(),
                s(&cwd.join(".mcp.json")),
            ],
        ] {
            assert!(at(&want) > copy, "{want:?} after the copy: {a:?}");
        }
        let policy = PiPaths::under_data(t.path()).claude_policy();
        assert!(has(
            &a,
            &[
                "--ro-bind".into(),
                s(&policy),
                "/etc/claude-code/managed-settings.json".into()
            ]
        ));
        // A link to a folder outside the copy is not followed (that folder is not mounted).
        std::fs::remove_dir(cwd.join(".pi")).unwrap();
        std::os::unix::fs::symlink("/usr", cwd.join(".pi")).unwrap();
        let a = args_for("claude", &cwd, t.path());
        assert!(!has(&a, &["--tmpfs".into(), "/usr".into()]));
        assert!(!has(&a, &["--tmpfs".into(), s(&cwd.join(".pi"))]));
        // Another connector: nothing of this.
        let o = args_for("opencode", &cwd, t.path());
        assert!(
            !o.iter()
                .any(|x| x.contains(".claude") || x.contains(".mcp.json"))
        );
        assert!(!o.iter().any(|x| x.contains("claude-code")));
        assert_eq!(
            o.iter().filter(|x| *x == "--tmpfs").count(),
            1,
            "only /tmp: {o:?}"
        );
    }

    #[test]
    fn the_probe_script_has_no_unbalanced_quotes() {
        // `sh -n` parses without running: a typo in the probe would otherwise look like a jail failure.
        let o = Command::new("/bin/sh")
            .args(["-n", "-c", PROBE])
            .output()
            .unwrap_or_else(|e| panic!("sh: {e}"));
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
}
