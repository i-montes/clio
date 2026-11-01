//! The local source: Pi on this machine, and the whole lifecycle of what surrounds it, in one
//! place. Opening a session does, in this order, and undoes it in the reverse order on any
//! failure, on `shutdown`, on `Drop` (panic included) and when the interface dies:
//!
//! 1. the working copy: a NEW session is a `runs::prepare` run (the project copied to
//!    `runs/<id>/sandbox` with its manifest, so `clio diff|apply|undo <id>` work on it); a listed
//!    one must be a session whose folder is exactly that copy;
//! 2. `clio toolhost` (this same executable) on that copy, with `HOME` set to Clio's own Pi home
//!    (the jail self-test plants its decoy there, never in the person's home), a 0700 socket
//!    folder under `$XDG_RUNTIME_DIR/clio`, and the API key taught to its output filter;
//! 3. Pi through `pi::launch::start`, hardened, with the `clio-guard` extension, exactly the
//!    tools the guard registers, and the guard's own variables (prompt file and version, socket).
//!
//! Pi and the toolhost both exit when their stdin closes, so nothing is left behind even if this
//! process is killed with `SIGKILL`.

use std::io::{BufRead, BufReader};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clio_brain::pi::install::{self, PiInstall};
use clio_brain::pi::launch::{Launch, PiProcess, Provider, SessionTarget, read_api_key, start};
use clio_brain::pi::rpc::{Cancel, Event, RunSummary};
use clio_brain::pi::sessions::{SessionInfo, list_sessions};
use clio_brain::pi::{PiError, PiPaths};
use serde_json::{Value, json};

use super::{Opened, Refresh, Row, RowKind, Session, SessionSource, Target};
use crate::cli::home::{Config, Dirs};
use crate::cli::runs::{self, RunOpts};
use crate::cli::usage;
use crate::cli::{Res, mkdir_private, write_private};

/// EXACTLY what `clio-guard` registers (see `adapters/pi/README.md`).
pub const GUARD_TOOLS: [&str; 6] = ["read", "write", "edit", "bash", "find", "ls"];
const PROMPT_NAME: &str = "clio-agent@4";
const PROMPT: &str = include_str!("../../../../../prompts/system/clio-agent@4.md");
/// The guard travels inside the binary so an installed `clio` needs no repository; a checkout
/// that already has `extensions/clio-guard` next to Pi's `node_modules` uses its own.
const GUARD_FILES: [(&str, &str); 4] = [
    (
        "index.ts",
        include_str!("../../../../../adapters/pi/extensions/clio-guard/index.ts"),
    ),
    (
        "data.ts",
        include_str!("../../../../../adapters/pi/extensions/clio-guard/data.ts"),
    ),
    (
        "host.ts",
        include_str!("../../../../../adapters/pi/extensions/clio-guard/host.ts"),
    ),
    (
        "prompt.ts",
        include_str!("../../../../../adapters/pi/extensions/clio-guard/prompt.ts"),
    ),
];
const TOOLHOST_READY: Duration = Duration::from_secs(60);
const TOOLHOST_GRACE: Duration = Duration::from_secs(3);

fn text(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn path_var() -> String {
    std::env::var("PATH").unwrap_or_default()
}

/// Can Pi run here at all: Node 22.19+ and the pinned install?
pub fn ready(dirs: &Dirs) -> Res<()> {
    install::find_node(&path_var()).map_err(text)?;
    install::locate(&crate::cli::pi::paths(dirs)).map_err(text)?;
    Ok(())
}

pub struct LocalSource {
    dirs: Arc<Dirs>,
}

impl LocalSource {
    pub fn new(dirs: Arc<Dirs>) -> Self {
        Self { dirs }
    }

    /// The only folder a session of Clio may work in: the copy of its run.
    fn copy_of(&self, id: &str) -> PathBuf {
        self.dirs.runs.join(id).join("sandbox")
    }

    fn is_ours(&self, s: &SessionInfo) -> bool {
        let want = self.copy_of(&s.id);
        match (&s.cwd_path, want.canonicalize()) {
            (Some(have), Ok(want)) => have.canonicalize().is_ok_and(|h| h == want),
            _ => false,
        }
    }
}

impl SessionSource for LocalSource {
    fn list(&self) -> Vec<Row> {
        let list = list_sessions(&crate::cli::pi::paths(&self.dirs).sessions_dir());
        list.sessions
            .iter()
            .map(|s| Row {
                kind: RowKind::Session,
                title: s.title.clone(),
                detail: s.cwd.clone(),
                messages: s.messages,
                last_ms: Some(s.last_activity_ms),
                reference: s.path.display().to_string(),
                unavailable: (!self.is_ours(s)).then(|| {
                    "no es una copia de trabajo de Clio (otra herramienta la creó): aquí no se abre"
                        .to_owned()
                }),
            })
            .collect()
    }

    fn open(&self, target: &Target) -> Result<Opened, String> {
        let d = &*self.dirs;
        let cfg = Config::load(d)?;
        let paths = crate::cli::pi::paths(d);
        let node = install::find_node(&path_var()).map_err(text)?;
        let install = install::locate(&paths).map_err(text)?;
        let key = read_api_key("OPENCODE_API_KEY", &d.key_file()).ok_or(
            "falta la clave del modelo: ponla en OPENCODE_API_KEY o en ~/.config/clio/opencode-api-key",
        )?;
        // 1. the working copy
        let (id, session) = match target {
            Target::New { project } => {
                let p = runs::prepare(
                    d,
                    &RunOpts {
                        workspace: project.clone(),
                        max_steps: None,
                        allow_broad: false,
                        instruction: "sesión de Pi".into(),
                    },
                )?;
                (p.id.clone(), SessionTarget::New { id: Some(p.id) })
            }
            Target::Existing { reference } => {
                // Only a listed session, and only one whose folder is its own copy.
                let list = list_sessions(&paths.sessions_dir());
                let s = list
                    .sessions
                    .iter()
                    .find(|s| s.path.display().to_string() == *reference)
                    .ok_or("esa sesión ya no está en la carpeta de sesiones de Pi")?;
                if !self.is_ours(s) {
                    return Err("la sesión no es una copia de trabajo de Clio: no se abre".into());
                }
                (s.id.clone(), SessionTarget::Open(reference.clone()))
            }
        };
        let run_dir = d.runs.join(&id);
        let work = self.copy_of(&id);
        // 2. the toolhost
        let toolhost = Toolhost::start(d, &paths, &run_dir, &work, &key)?;
        // 3. Pi
        let launch = Launch {
            guard: Some(ensure_guard(&install)?),
            guard_tools: GUARD_TOOLS.map(String::from).to_vec(),
            extra_env: vec![
                (
                    "CLIO_GUARD_PROMPT_FILE".into(),
                    write_prompt(&paths)?.display().to_string(),
                ),
                ("CLIO_GUARD_PROMPT_VERSION".into(), PROMPT_NAME.into()),
                (
                    "CLIO_TOOLHOST_SOCKET".into(),
                    toolhost.socket.display().to_string(),
                ),
            ],
            provider: provider_of(d, &cfg),
            api_key: Some(key),
            cwd: work.clone(),
            name: None,
            install,
            node,
            paths,
            session,
        };
        let pi = start(&launch).map_err(text)?;
        let history = pi.client.get_messages().map_err(text)?;
        let s = ChatSession {
            pi,
            toolhost,
            down: Mutex::new(false),
        };
        let refresh = s.refresh().ok();
        Ok(Opened {
            id: s.pi.session_id.clone(),
            workspace: work.display().to_string(),
            history,
            refresh,
            session: Arc::new(s),
        })
    }
}

fn provider_of(d: &Dirs, cfg: &Config) -> Provider {
    let m = &cfg.model;
    let mut p = Provider::opencode_go(&m.model);
    p.id.clone_from(&m.provider);
    if let Some(u) = &m.base_url {
        p.base_url.clone_from(u);
    }
    p.context_window = usage::window_for(d, &m.provider, &m.model).0;
    // Micro-dollars per million tokens to dollars per million tokens.
    p.dollars_per_mtok = (
        m.price_in_micro_per_mtok as f64 / 1e6,
        m.price_out_micro_per_mtok as f64 / 1e6,
    );
    p
}

/// The guard: the checkout's own next to Pi's `node_modules` if there is one, else the copy that
/// travels inside this binary, written beside the installed Pi (refreshed when the binary's
/// differs). The result is the entry file, which is what `pi -e` and the launch checks take.
fn ensure_guard(install: &PiInstall) -> Result<PathBuf, String> {
    let dir = install.dir.join("extensions/clio-guard");
    let marker = dir.join(".clio-embedded");
    let entry = |d: &Path| d.join("index.ts").canonicalize().map_err(text);
    if dir.join("index.ts").is_file() && !marker.exists() {
        return entry(&dir);
    }
    let sum = crate::cli::sha_hex(GUARD_FILES.map(|f| f.1).concat().as_bytes());
    if std::fs::read_to_string(&marker).ok().as_deref() != Some(&sum) {
        mkdir_private(&dir)?;
        for (name, body) in GUARD_FILES {
            write_private(&dir.join(name), body.as_bytes())?;
        }
        write_private(&marker, sum.as_bytes())?;
    }
    entry(&dir)
}

fn write_prompt(paths: &PiPaths) -> Result<PathBuf, String> {
    let dir = paths.root.join("prompts");
    mkdir_private(&dir)?;
    let p = dir.join(format!("{PROMPT_NAME}.md"));
    write_private(&p, PROMPT.as_bytes())?;
    Ok(p)
}

/// `clio toolhost` running on the session's copy.
struct Toolhost {
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    socket: PathBuf,
    sock_dir: PathBuf,
}

static TOOLHOSTS: AtomicU32 = AtomicU32::new(0);

impl Toolhost {
    fn start(
        d: &Dirs,
        paths: &PiPaths,
        run_dir: &Path,
        work: &Path,
        key: &clio_brain::pi::launch::Secret,
    ) -> Res<Self> {
        let rt = d.need_runtime()?;
        mkdir_private(rt)?;
        // A short path: Unix sockets cannot be longer than about 100 bytes.
        let sock_dir = rt.join(format!(
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
        mkdir_private(&sock_dir)?;
        let (scratch, effects) = (run_dir.join("scratch"), run_dir.join("toolhost"));
        mkdir_private(&scratch)?;
        mkdir_private(&effects)?;
        mkdir_private(&paths.home_dir())?;
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
        let exe = std::env::current_exe().map_err(text)?;
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
    fn stop(&self) {
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

/// An open session: Pi, and the toolhost beside it. Field order is the order they are dropped:
/// Pi first, then the toolhost.
struct ChatSession {
    pi: PiProcess,
    toolhost: Toolhost,
    down: Mutex<bool>,
}

impl Session for ChatSession {
    fn run_prompt(
        &self,
        text: &str,
        cancel: &Cancel,
        on_event: &mut dyn FnMut(&Event),
    ) -> Result<RunSummary, PiError> {
        self.pi.client.run_prompt(text, cancel, on_event)
    }

    fn steer(&self, text: &str) -> Result<(), PiError> {
        self.pi.client.steer(text)
    }

    fn refresh(&self) -> Result<Refresh, PiError> {
        let stats = self.pi.client.get_session_stats()?;
        // The guard writes `clio-guard.taint` into the session when it read outside content.
        let entries = self.pi.client.request("get_entries", json!({}))?;
        let tainted = entries["entries"]
            .as_array()
            .is_some_and(|a| a.iter().any(is_taint));
        Ok(Refresh { stats, tainted })
    }

    fn shutdown(&self) {
        if let Ok(mut g) = self.down.lock() {
            if std::mem::replace(&mut *g, true) {
                return;
            }
            self.pi.client.kill();
            self.toolhost.stop();
        }
    }
}

impl Drop for ChatSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn is_taint(e: &Value) -> bool {
    e["customType"] == "clio-guard.taint" && e["data"]["tainted"] == true
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn only_the_guards_taint_entry_counts() {
        assert!(is_taint(
            &json!({"type":"custom","customType":"clio-guard.taint","data":{"tainted":true}})
        ));
        assert!(!is_taint(
            &json!({"type":"custom","customType":"clio-guard.tools","data":{"tainted":true}})
        ));
        assert!(!is_taint(
            &json!({"type":"custom","customType":"clio-guard.taint","data":{}})
        ));
    }

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
