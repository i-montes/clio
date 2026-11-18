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
pub const GUARD_TOOLS: [&str; 22] = [
    "read",
    "write",
    "edit",
    "bash",
    "find",
    "ls",
    "memory_search",
    "memory_get",
    "memory_timeline",
    "memory_context",
    "memory_save",
    "memory_session_summary",
    "memory_update",
    "memory_consolidate",
    "memory_forget",
    "memory_revert",
    "memory_pin",
    "memory_block",
    "memory_block_edit",
    "ask_person",
    "request_secret",
    "todo_write",
];
/// What it registers on top in host mode (`HOST_TOOL_NAMES`): the background commands' tools.
pub const HOST_GUARD_TOOLS: [&str; 2] = ["job_output", "job_stop"];
pub const PROMPT_NAME: &str = "clio-agent@21";
pub const PROMPT: &str = include_str!("../../../../prompts/system/clio-agent@21.md");
/// The prompt of a host-mode session: the tools act on the person's real machine.
/// What the tool host is taught to strike out (`CLIO_TOOLHOST_SCRUB_JSON`): the model's key.
pub fn scrub_json(key: &str) -> String {
    json!({ crate::connector::MODEL_KEY_NAME: key }).to_string()
}

pub const HOST_PROMPT_NAME: &str = "clio-host@5";
pub const HOST_PROMPT: &str = include_str!("../../../../prompts/system/clio-host@5.md");
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

/// 128 random bits as hex, from the system.
fn random_token() -> Result<String, String> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| format!("could not make a random key: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

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
    write_named_prompt(paths, PROMPT_NAME, PROMPT)
}

fn write_named_prompt(paths: &PiPaths, name: &str, body: &str) -> Result<PathBuf, String> {
    let dir = paths.root.join("prompts");
    private_dir(&dir).map_err(text)?;
    let p = dir.join(format!("{name}.md"));
    write_private(&p, body.as_bytes()).map_err(text)?;
    Ok(p)
}

/// The guard's own variables (prompt file and version, toolhost socket) for `Launch::extra_env`.
/// `host`: the host-mode prompt and `CLIO_GUARD_HOST=1`, which the guard and Pi's jail read.
/// `person`: someone answers the guard's questions (the chat), `CLIO_GUARD_PERSON=1`; without it
/// (`clio run`) `ask_person` answers at once that nobody is there.
pub fn guard_env(
    paths: &PiPaths,
    toolhost: &Toolhost,
    host: bool,
    person: bool,
) -> Result<Vec<(String, String)>, String> {
    let (name, body) = if host {
        (HOST_PROMPT_NAME, HOST_PROMPT)
    } else {
        (PROMPT_NAME, PROMPT)
    };
    let mut env = vec![
        (
            "CLIO_GUARD_PROMPT_FILE".into(),
            write_named_prompt(paths, name, body)?.display().to_string(),
        ),
        ("CLIO_GUARD_PROMPT_VERSION".into(), name.into()),
        (
            "CLIO_TOOLHOST_SOCKET".into(),
            toolhost.socket.display().to_string(),
        ),
    ];
    if host {
        env.push((HOST_ENV.into(), "1".into()));
    }
    if person {
        env.push((PERSON_ENV.into(), "1".into()));
    }
    Ok(env)
}

/// Set in Pi's environment for a host-mode session.
pub const HOST_ENV: &str = "CLIO_GUARD_HOST";
/// Set in Pi's environment when a person answers the guard's dialogs.
pub const PERSON_ENV: &str = "CLIO_GUARD_PERSON";

/// What a tool host may be given besides its folders.
#[derive(Default, Clone, Copy)]
pub struct ToolhostOpts<'a> {
    /// The person's data folder and the vector models' (best first): the memory tools.
    pub memory: Option<(&'a Path, &'a [PathBuf])>,
    /// Extra variables for the service (what it needs to judge proposals with Jev).
    pub extra_env: &'a [(String, String)],
    /// Host mode: the tools act on the real machine (`clio toolhost --host-*`).
    pub host: Option<HostOpts<'a>>,
    /// A person can be asked for a secret (`request_secret`): where requests wait (`Dirs.data/host`),
    /// the run, and the vault's person socket.
    pub secrets: Option<(&'a Path, &'a str, &'a Path)>,
}

/// The `--host-*` options of `clio toolhost`.
#[derive(Clone, Copy)]
pub struct HostOpts<'a> {
    pub home: &'a Path,
    /// `Dirs.data/host`.
    pub state: &'a Path,
    /// Clio's own folders.
    pub control: &'a [PathBuf],
    /// The person's `host.json`, when there is one.
    pub rules: Option<&'a Path>,
    pub run: &'a str,
}

pub struct Toolhost {
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    pub socket: PathBuf,
    /// What the service wants to see before it believes the brain about what the person said. Made
    /// here, put in the service's own environment (Pi never gets it) and shown by nobody else.
    pub said_token: String,
    sock_dir: PathBuf,
    /// Host mode: the units its `Systemd` launcher started are stopped with it.
    host: bool,
}

static TOOLHOSTS: AtomicU32 = AtomicU32::new(0);

impl Toolhost {
    /// `exe` is a `clio` binary; `runtime` the folder for the 0700 socket folder (kept short:
    /// Unix sockets cannot be longer than about 100 bytes); `run_dir` holds `scratch/`,
    /// `toolhost/` and `toolhost.log`; `HOME` is Clio's own Pi home. `memory` is the person's
    /// data folder and, if there is one, the vector model's: the service reads the memory (read
    /// only) and is told where the model is because its `HOME` is not the person's.
    pub fn start(
        exe: &Path,
        runtime: &Path,
        paths: &PiPaths,
        run_dir: &Path,
        work: &Path,
        key: &Secret,
        opts: ToolhostOpts<'_>,
    ) -> Result<Self, String> {
        let ToolhostOpts {
            memory,
            extra_env,
            host,
            secrets,
        } = opts;
        // Units a `clio` killed before it could stop them do not run forever as the person.
        // `CLIO_JAIL=off` has no commands: no launcher, and systemd is not asked either.
        if host.is_some() && !std::env::var("CLIO_JAIL").is_ok_and(|v| v == "off") {
            clio_node::host::stop_leftover_units();
        }
        private_dir(runtime).map_err(text)?;
        let sock_dir = runtime.join(format!(
            "th-{}-{}",
            std::process::id(),
            TOOLHOSTS.fetch_add(1, Ordering::SeqCst)
        ));
        let socket = sock_dir.join("toolhost.sock");
        if socket.as_os_str().len() > 100 {
            return Err(format!(
                "the socket path ({}) is too long for a Unix socket: use a shorter XDG_RUNTIME_DIR",
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
        let said_token = random_token()?;
        let mut cmd = Command::new(exe);
        cmd.arg("toolhost");
        if let Some((data, model)) = memory {
            cmd.arg("--memory-data").arg(data);
            for model in model {
                cmd.arg("--memory-model").arg(model);
            }
        }
        if let Some(h) = host {
            cmd.arg("--host-home")
                .arg(h.home)
                .arg("--host-state")
                .arg(h.state)
                .arg("--host-run")
                .arg(h.run);
            for c in h.control {
                cmd.arg("--host-control").arg(c);
            }
            if let Some(r) = h.rules {
                cmd.arg("--host-rules").arg(r);
            }
        }
        if let Some((state, run, person)) = secrets {
            cmd.arg("--secret-state")
                .arg(state)
                .arg("--secret-run")
                .arg(run)
                .arg("--vault-person")
                .arg(person);
        }
        let mut child = cmd
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
            .envs(extra_env.iter().map(|(k, v)| (k, v)))
            .env("CLIO_SAID_TOKEN", &said_token)
            .env("HOME", paths.home_dir())
            .env("PATH", "/usr/bin:/bin")
            .env("CLIO_TOOLHOST_SCRUB_JSON", scrub_json(key.expose()))
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(logf))
            .spawn()
            .map_err(|e| format!("could not start clio toolhost: {e}"))?;
        let stdin = child.stdin.take();
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            return Err("clio toolhost has no output".into());
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
            said_token,
            sock_dir,
            host: host.is_some(),
        };
        // On any failure `this` drops here and stops the process.
        let line = rx.recv_timeout(TOOLHOST_READY).map_err(|_| {
            format!(
                "clio toolhost did not say READY in time (see {})",
                log.display()
            )
        })?;
        if line.strip_prefix("READY ").map(|s| Path::new(s.trim())) != Some(&this.socket) {
            return Err(format!(
                "clio toolhost started badly (see {})",
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
            if self.host {
                clio_node::host::stop_units_of(c.id());
            }
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
    fn the_guard_tools_are_the_ones_the_extension_registers() {
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
        let host = index
            .lines()
            .find(|l| l.contains("export const HOST_TOOL_NAMES"))
            .unwrap();
        for t in HOST_GUARD_TOOLS {
            assert!(host.contains(&format!("\"{t}\"")), "{t}");
        }
        assert_eq!(host.matches('"').count(), HOST_GUARD_TOOLS.len() * 2);
    }

    #[test]
    fn the_prompt_is_the_one_the_guard_checks() {
        // The model once took "Jev" (the judge) for the person's name: the judge is not named to it.
        assert!(!PROMPT.contains("Jev"));
        assert!(PROMPT.starts_with("version: 21\n"));
    }
}
