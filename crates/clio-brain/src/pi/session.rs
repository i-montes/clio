//! What surrounds a Pi session on this machine, shared by `clio` (chat, `clio run --harness pi`)
//! and the bench: the guard extension that travels inside the binary, the system prompt file the
//! guard checks, and `clio toolhost` running on the session's working copy.

use std::io::{BufRead, BufReader};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use serde_json::json;
use sha2::{Digest, Sha256};

use super::install::PiInstall;
use super::launch::Secret;
use super::{PiPaths, private_dir, write_private};

/// EXACTLY what `clio-guard` registers (see `adapters/pi/README.md`).
pub const GUARD_TOOLS: [&str; 6] = ["read", "write", "edit", "bash", "find", "ls"];
pub const PROMPT_NAME: &str = "clio-agent@4";
pub const PROMPT: &str = include_str!("../../../../prompts/system/clio-agent@4.md");
/// The guard travels inside the binary so an installed `clio` needs no repository; a checkout
/// that already has `extensions/clio-guard` next to Pi's `node_modules` uses its own.
pub const GUARD_FILES: [(&str, &str); 4] = [
    (
        "index.ts",
        include_str!("../../../../adapters/pi/extensions/clio-guard/index.ts"),
    ),
    (
        "data.ts",
        include_str!("../../../../adapters/pi/extensions/clio-guard/data.ts"),
    ),
    (
        "host.ts",
        include_str!("../../../../adapters/pi/extensions/clio-guard/host.ts"),
    ),
    (
        "prompt.ts",
        include_str!("../../../../adapters/pi/extensions/clio-guard/prompt.ts"),
    ),
];
const TOOLHOST_READY: Duration = Duration::from_secs(60);
const TOOLHOST_GRACE: Duration = Duration::from_secs(3);

fn text(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// The guard: the checkout's own next to Pi's `node_modules` if there is one, else the copy that
/// travels inside this binary, written beside the installed Pi (refreshed when the binary's
/// differs). The result is the entry file, which is what `pi -e` and the launch checks take.
pub fn ensure_guard(install: &PiInstall) -> Result<PathBuf, String> {
    let dir = install.dir.join("extensions/clio-guard");
    let marker = dir.join(".clio-embedded");
    let entry = |d: &Path| d.join("index.ts").canonicalize().map_err(text);
    if dir.join("index.ts").is_file() && !marker.exists() {
        return entry(&dir);
    }
    let sum: String = Sha256::digest(GUARD_FILES.map(|f| f.1).concat().as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if std::fs::read_to_string(&marker).ok().as_deref() != Some(&sum) {
        private_dir(&dir).map_err(text)?;
        for (name, body) in GUARD_FILES {
            write_private(&dir.join(name), body.as_bytes()).map_err(text)?;
        }
        write_private(&marker, sum.as_bytes()).map_err(text)?;
    }
    entry(&dir)
}

pub fn write_prompt(paths: &PiPaths) -> Result<PathBuf, String> {
    let dir = paths.root.join("prompts");
    private_dir(&dir).map_err(text)?;
    let p = dir.join(format!("{PROMPT_NAME}.md"));
    write_private(&p, PROMPT.as_bytes()).map_err(text)?;
    Ok(p)
}

/// The guard's own variables (prompt file and version, toolhost socket) for `Launch::extra_env`.
pub fn guard_env(paths: &PiPaths, toolhost: &Toolhost) -> Result<Vec<(String, String)>, String> {
    Ok(vec![
        (
            "CLIO_GUARD_PROMPT_FILE".into(),
            write_prompt(paths)?.display().to_string(),
        ),
        ("CLIO_GUARD_PROMPT_VERSION".into(), PROMPT_NAME.into()),
        (
            "CLIO_TOOLHOST_SOCKET".into(),
            toolhost.socket.display().to_string(),
        ),
    ])
}

/// `clio toolhost` running on the session's copy.
pub struct Toolhost {
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    pub socket: PathBuf,
    sock_dir: PathBuf,
}

static TOOLHOSTS: AtomicU32 = AtomicU32::new(0);

impl Toolhost {
    /// `exe` is a `clio` binary; `runtime` the folder for the 0700 socket folder (kept short:
    /// Unix sockets cannot be longer than about 100 bytes); `run_dir` holds `scratch/`,
    /// `toolhost/` and `toolhost.log`; `HOME` is Clio's own Pi home.
    pub fn start(
        exe: &Path,
        runtime: &Path,
        paths: &PiPaths,
        run_dir: &Path,
        work: &Path,
        key: &Secret,
    ) -> Result<Self, String> {
        private_dir(runtime).map_err(text)?;
        let sock_dir = runtime.join(format!(
            "th-{}-{}",
            std::process::id(),
            TOOLHOSTS.fetch_add(1, Ordering::SeqCst)
        ));
        let socket = sock_dir.join("toolhost.sock");
        if socket.as_os_str().len() > 100 {
            return Err(format!(
                "la ruta del socket ({}) es demasiado larga para un socket Unix: usa un XDG_RUNTIME_DIR más corto",
                socket.display()
            ));
        }
        private_dir(&sock_dir).map_err(text)?;
        let (scratch, effects) = (run_dir.join("scratch"), run_dir.join("toolhost"));
        private_dir(&scratch).map_err(text)?;
        private_dir(&effects).map_err(text)?;
        private_dir(&paths.home_dir()).map_err(text)?;
        let log = run_dir.join("toolhost.log");
        let logf = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&log)
            .map_err(|e| format!("{}: {e}", log.display()))?;
        let jail = if std::env::var("CLIO_JAIL").is_ok_and(|v| v == "off") {
            "off"
        } else {
            "auto"
        };
        let mut child = Command::new(exe)
            .arg("toolhost")
            .arg("--socket-dir")
            .arg(&sock_dir)
            .arg("--work")
            .arg(work)
            .arg("--scratch")
            .arg(&scratch)
            .arg("--run-dir")
            .arg(&effects)
            .args(["--jail", jail])
            .env_clear()
            .env("HOME", paths.home_dir())
            .env("PATH", "/usr/bin:/bin")
            .env(
                "CLIO_TOOLHOST_SCRUB_JSON",
                json!({"opencode_api_key": key.expose()}).to_string(),
            )
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(logf))
            .spawn()
            .map_err(|e| format!("no pude iniciar clio toolhost: {e}"))?;
        let stdin = child.stdin.take();
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            return Err("clio toolhost sin salida".into());
        };
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for l in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = tx.send(l);
            }
        });
        let this = Self {
            child: Mutex::new(Some(child)),
            stdin: Mutex::new(stdin),
            socket,
            sock_dir,
        };
        // On any failure `this` drops here and stops the process.
        let line = rx.recv_timeout(TOOLHOST_READY).map_err(|_| {
            format!(
                "clio toolhost no dijo READY a tiempo (mira {})",
                log.display()
            )
        })?;
        if line.strip_prefix("READY ").map(|s| Path::new(s.trim())) != Some(&this.socket) {
            return Err(format!(
                "clio toolhost arrancó mal (mira {})",
                log.display()
            ));
        }
        Ok(this)
    }

    /// Closes its stdin (its orderly stop), waits a moment, then kills; removes the socket folder.
    pub fn stop(&self) {
        if let Ok(mut g) = self.stdin.lock() {
            *g = None;
        }
        if let Ok(mut g) = self.child.lock()
            && let Some(mut c) = g.take()
        {
            let end = Instant::now() + TOOLHOST_GRACE;
            while Instant::now() < end && matches!(c.try_wait(), Ok(None)) {
                std::thread::sleep(Duration::from_millis(20));
            }
            if matches!(c.try_wait(), Ok(None)) {
                let _ = c.kill();
            }
            let _ = c.wait();
        }
        let _ = std::fs::remove_dir_all(&self.sock_dir);
    }
}

impl Drop for Toolhost {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_guard_tools_are_the_six_the_extension_registers() {
        // Same list as TOOL_NAMES in adapters/pi/extensions/clio-guard/index.ts.
        let (_, index) = GUARD_FILES[0];
        let line = index
            .lines()
            .find(|l| l.contains("export const TOOL_NAMES"))
            .unwrap();
        for t in GUARD_TOOLS {
            assert!(line.contains(&format!("\"{t}\"")), "{t}");
        }
        assert_eq!(line.matches('"').count(), GUARD_TOOLS.len() * 2);
    }

    #[test]
    fn the_prompt_is_the_one_the_guard_checks() {
        assert!(PROMPT.starts_with("version: 4"));
    }
}
