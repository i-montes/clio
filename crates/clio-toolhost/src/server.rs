//! The Unix socket: one folder (mode 0700, owned by us) holding one socket (mode 0600). JSON per
//! line, several requests per connection allowed, every read and write has a time limit.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::{MAX_REQUEST_BYTES, ToolHost};

const MAX_CONNECTIONS: usize = 32;
const IO_TIMEOUT: Duration = Duration::from_secs(30);
pub const SOCKET_NAME: &str = "toolhost.sock";

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
        let me = std::fs::metadata("/proc/self")
            .map_err(|e| format!("/proc/self: {e}"))?
            .uid();
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
    let Ok(mut out) = conn.try_clone() else {
        return;
    };
    let mut input = BufReader::new(conn);
    loop {
        let mut line = String::new();
        let n = match input
            .by_ref()
            .take(MAX_REQUEST_BYTES as u64 + 1)
            .read_line(&mut line)
        {
            Ok(n) => n,
            Err(_) => return,
        };
        if n == 0 {
            return;
        }
        let reply = if !line.ends_with('\n') {
            // Too long (or cut by the end of the stream): refuse and close.
            let r = host.handle_line("{");
            let _ = writeln!(out, "{r}");
            return;
        } else {
            host.handle_line(line.trim_end())
        };
        if writeln!(out, "{reply}").and_then(|()| out.flush()).is_err() {
            return;
        }
    }
}
