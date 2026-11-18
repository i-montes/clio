//! The Unix socket: one folder (mode 0700, owned by us) holding one socket (mode 0600). JSON per
//! line, several requests per connection allowed, every read and write has a time limit.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::{MAX_REQUEST_BYTES, ToolHost};

const MAX_CONNECTIONS: usize = 32;
const IO_TIMEOUT: Duration = Duration::from_secs(30);
pub const SOCKET_NAME: &str = "toolhost.sock";

/// The effective uid of this process. Not from `/proc/self` (macOS has no `/proc`, so the server
/// could never start there) and not from `id -u` (a poisoned PATH would answer for it).
fn current_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// The listening socket. Dropping it stops the accept loop and removes the socket file.
pub struct Server {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    /// `dir` must exist, be a folder, have mode 0700 and belong to this user: a socket in a
    /// folder others can enter is refused (fail closed).
    pub fn start(host: Arc<ToolHost>, dir: &Path) -> Result<Self, String> {
        let meta = std::fs::metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let me = current_uid();
        if !meta.is_dir() || meta.permissions().mode() & 0o777 != 0o700 || meta.uid() != me {
            return Err(format!(
                "{} must be a folder with mode 0700 owned by this user",
                dir.display()
            ));
        }
        let path = dir.join(SOCKET_NAME);
        let listener = UnixListener::bind(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicUsize::new(0));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            for conn in listener.incoming() {
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(conn) = conn else { continue };
                if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                    active.fetch_sub(1, Ordering::SeqCst);
                    continue; // dropped: too many at once
                }
                let (host, active) = (host.clone(), active.clone());
                std::thread::spawn(move || {
                    serve(&host, conn);
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        Ok(Self {
            path,
            stop,
            thread: Some(thread),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&self.path); // wakes the accept loop
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn serve(host: &ToolHost, conn: UnixStream) {
    if conn.set_read_timeout(Some(IO_TIMEOUT)).is_err()
        || conn.set_write_timeout(Some(IO_TIMEOUT)).is_err()
    {
        return;
    }
    let (Ok(mut out), Ok(probe)) = (conn.try_clone(), conn.try_clone()) else {
        return;
    };
    let mut input = BufReader::new(conn);
    // One request at a time: what the client sends while one runs is read here, at most one
    // byte, and given back to the next request.
    let early: Mutex<Vec<u8>> = Mutex::default();
    loop {
        let mut line = std::mem::take(&mut *early.lock().unwrap_or_else(PoisonError::into_inner));
        let n = match input
            .by_ref()
            .take(MAX_REQUEST_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
        {
            Ok(n) => n,
            Err(_) => return,
        };
        if n == 0 && line.is_empty() {
            return;
        }
        let reply = match String::from_utf8(line) {
            Ok(l) if l.ends_with('\n') => {
                host.handle_line_until(l.trim_end(), &|| client_gone(&probe, &early))
            }
            // Too long, cut by the end of the stream, or not text: refuse and close.
            _ => {
                let r = host.handle_line("{");
                let _ = writeln!(out, "{r}");
                return;
            }
        };
        if writeln!(out, "{reply}").and_then(|()| out.flush()).is_err() {
            return;
        }
    }
}

/// While a request runs: has the client closed its end (it aborted, or it died)? A read that
/// would wait means it is still there. A byte it sent early is kept for the next request.
fn client_gone(probe: &UnixStream, early: &Mutex<Vec<u8>>) -> bool {
    let mut kept = early.lock().unwrap_or_else(PoisonError::into_inner);
    if !kept.is_empty() || probe.set_nonblocking(true).is_err() {
        return false;
    }
    let mut b = [0u8; 1];
    let r = (&*probe).read(&mut b);
    let _ = probe.set_nonblocking(false);
    match r {
        Ok(0) => true,
        Ok(_) => {
            kept.push(b[0]);
            false
        }
        Err(e) => !matches!(
            e.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
        ),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_uid_is_the_owner_of_what_this_process_creates() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("f");
        std::fs::write(&f, b"").unwrap();
        assert_eq!(current_uid(), std::fs::metadata(&f).unwrap().uid());
    }
}
