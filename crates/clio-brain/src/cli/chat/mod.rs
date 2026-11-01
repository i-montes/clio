//! The Pi harness seen from the interface (ADR 0007): sessions come from a [`SessionSource`], and
//! an open one is a [`Session`]. The TUI and `clio run --harness pi` only know these two traits.
//!
//! - [`local`]: this machine. Pi runs here, one `pi --mode rpc` per open session, with its own
//!   `clio toolhost` (the only thing that touches the working copy) beside it; both die with the
//!   session, with the interface, or with the process.
//! - [`RemoteSource`]: the same protocol over `ssh <host> clio serve-rpc` stdio. A placeholder:
//!   the trait and the selector row exist, opening says `remoto: pendiente`.
//! - [`run`]: `clio run --harness pi`, a whole prompt without a terminal.

pub mod local;
pub mod run;

use std::path::PathBuf;
use std::sync::Arc;

use clio_brain::pi::PiError;
use clio_brain::pi::rpc::{Cancel, Event, RunSummary, SessionStats};
use serde_json::Value;

use super::Res;
use super::home::Dirs;

/// Which agent runs under the interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    Pi,
    Builtin,
}

/// ONE LINE to change what `clio` and `clio run` use when nobody says: `true` is Pi when it is
/// installed, and the builtin loop otherwise; `false` is always the builtin loop. `--harness` and
/// `CLIO_HARNESS` override it either way.
pub const TUI_PI_WHEN_INSTALLED: bool = true;

/// `--harness`, then `CLIO_HARNESS`, then the default above. Asking for `pi` when it cannot run
/// is an error that says why; the default quietly falls back to the builtin loop.
pub fn resolve(flag: Option<&str>, dirs: &Dirs) -> Res<Harness> {
    let env = std::env::var("CLIO_HARNESS").ok().filter(|v| !v.is_empty());
    match flag.or(env.as_deref()) {
        Some("builtin") => Ok(Harness::Builtin),
        Some("pi") => local::ready(dirs).map(|()| Harness::Pi),
        None if TUI_PI_WHEN_INSTALLED && local::ready(dirs).is_ok() => Ok(Harness::Pi),
        None => Ok(Harness::Builtin),
        Some(other) => Err(format!("--harness: {other:?} no es válido (pi o builtin)")),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Session,
    Remote,
}

/// One line of the selector (the "nueva sesión" line belongs to the screen, not to a source).
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub kind: RowKind,
    pub title: String,
    /// The folder (or the host) the session is about.
    pub detail: String,
    pub messages: u32,
    pub last_ms: Option<i64>,
    /// What to hand back to [`SessionSource::open`].
    pub reference: String,
    /// Why this row cannot be opened (shown, not hidden).
    pub unavailable: Option<String>,
}

pub enum Target {
    /// A new session on a copy of this project.
    New { project: PathBuf },
    /// A listed one (`Row::reference`).
    Existing { reference: String },
}

/// What the status bar needs after a run: Pi's own totals and whether the guard marked the session
/// as having read outside content.
#[derive(Debug, Clone, Copy)]
pub struct Refresh {
    pub stats: SessionStats,
    pub tainted: bool,
}

pub struct Opened {
    pub session: Arc<dyn Session>,
    pub id: String,
    /// The folder the session works on (its copy).
    pub workspace: String,
    /// Pi's messages so far (a reopened session has them).
    pub history: Vec<Value>,
    pub refresh: Option<Refresh>,
}

/// An open session. Every method may be called from any thread: `steer` comes while another
/// thread waits in `run_prompt`.
pub trait Session: Send + Sync {
    /// Sends a prompt and pumps events until Pi settles (or `cancel` aborts it).
    fn run_prompt(
        &self,
        text: &str,
        cancel: &Cancel,
        on_event: &mut dyn FnMut(&Event),
    ) -> Result<RunSummary, PiError>;
    /// Adds a message to the run in progress.
    fn steer(&self, text: &str) -> Result<(), PiError>;
    fn refresh(&self) -> Result<Refresh, PiError>;
    /// Stops everything that belongs to the session now. Safe to repeat.
    fn shutdown(&self);
}

pub trait SessionSource: Send + Sync {
    fn list(&self) -> Vec<Row>;
    fn open(&self, target: &Target) -> Result<Opened, String>;
}

/// The same Pi protocol over `ssh <host> clio serve-rpc` stdio. Not built yet: the row is there
/// so the selector, the keys and the trait do not change when it is.
pub struct RemoteSource;

const REMOTE_PENDING: &str = "remoto: pendiente (ssh <host> clio serve-rpc todavía no existe)";

impl SessionSource for RemoteSource {
    fn list(&self) -> Vec<Row> {
        vec![Row {
            kind: RowKind::Remote,
            title: "remoto: pendiente".into(),
            detail: "ssh <host> clio serve-rpc".into(),
            messages: 0,
            last_ms: None,
            reference: String::new(),
            unavailable: Some(REMOTE_PENDING.into()),
        }]
    }

    fn open(&self, _: &Target) -> Result<Opened, String> {
        Err(REMOTE_PENDING.into())
    }
}

/// The sources of this process: this machine, and the remote placeholder.
pub fn sources(dirs: Arc<Dirs>) -> Vec<Box<dyn SessionSource>> {
    vec![
        Box::new(local::LocalSource::new(dirs)),
        Box::new(RemoteSource),
    ]
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_remote_source_says_it_is_pending_and_never_opens() {
        let r = RemoteSource;
        let rows = r.list();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, RowKind::Remote);
        assert!(
            rows[0]
                .unavailable
                .as_deref()
                .unwrap()
                .contains("pendiente")
        );
        let e = r.open(&Target::Existing {
            reference: "x".into(),
        });
        assert!(e.err().unwrap().contains("remoto: pendiente"));
    }
}
