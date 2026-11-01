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

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use clio_brain::pi::PiError;
use clio_brain::pi::install;
use clio_brain::pi::launch::{Launch, PiProcess, Provider, SessionTarget, read_api_key, start};
use clio_brain::pi::rpc::{Cancel, Event, RunSummary};
use clio_brain::pi::session::{GUARD_TOOLS, Toolhost, ensure_guard, guard_env};
use clio_brain::pi::sessions::{SessionInfo, list_sessions};
use serde_json::{Value, json};

use super::{Opened, Refresh, Row, RowKind, Session, SessionSource, Target};
use crate::cli::Res;
use crate::cli::home::{Config, Dirs};
use crate::cli::runs::{self, RunOpts};
use crate::cli::usage;

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
        let exe = std::env::current_exe().map_err(text)?;
        let toolhost = Toolhost::start(&exe, d.need_runtime()?, &paths, &run_dir, &work, &key)?;
        // 3. Pi
        let launch = Launch {
            guard: Some(ensure_guard(&install)?),
            guard_tools: GUARD_TOOLS.map(String::from).to_vec(),
            extra_env: guard_env(&paths, &toolhost)?,
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
}
