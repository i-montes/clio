//! The Pi harness seen from the interface (ADR 0007): sessions come from a [`SessionSource`], and
//! an open one is a [`Session`]. The TUI and `clio run --harness pi` only know these two traits.
//!
//! - [`local`]: this machine. Pi runs here, one `pi --mode rpc` per open session, with its own
//!   `clio toolhost` (the only thing that touches the working copy) beside it; both die with the
//!   session, with the interface, or with the process.
//! - [`RemoteSource`]: the same protocol over `ssh <host> clio serve-rpc` stdio. A placeholder:
//!   the trait exists, it lists nothing (a row nobody can open is noise), and opening says
//!   `remote: pending`.
//! - [`run`]: `clio run --harness pi`, a whole prompt without a terminal.

pub mod local;
pub mod run;

use std::path::PathBuf;
use std::sync::Arc;

use clio_brain::pi::PiError;
use clio_brain::pi::rpc::{Cancel, Event, RunSummary, SessionStats};
use serde_json::Value;

use super::Res;
use super::home::{Dirs, Mode};

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
/// is an error that says why; the default falls back to the builtin loop and says so on stderr
/// (the builtin loop is not the jailed one, so a Pi that broke must not go unnoticed).
pub fn resolve(flag: Option<&str>, dirs: &Dirs) -> Res<Harness> {
    let env = std::env::var("CLIO_HARNESS").ok().filter(|v| !v.is_empty());
    match flag.or(env.as_deref()) {
        Some("builtin") => Ok(Harness::Builtin),
        Some("pi") => local::ready(dirs).map(|()| Harness::Pi),
        None if TUI_PI_WHEN_INSTALLED => Ok(match local::ready(dirs) {
            Ok(()) => Harness::Pi,
            Err(why) => {
                eprintln!(
                    "warning: Pi is not available ({why}); using Clio's own loop, which does not run in Pi's jail. `--harness builtin` silences this warning."
                );
                Harness::Builtin
            }
        }),
        None => Ok(Harness::Builtin),
        Some(other) => Err(format!("--harness: {other:?} is not valid (pi or builtin)")),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Session,
    /// A row of the remote source, once it lists any (the drawer already draws them).
    #[allow(dead_code)]
    Remote,
}

/// One line of the selector (the "new session" line belongs to the screen, not to a source).
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
/// as having read outside content, and what the jail refused Pi to connect to (the whole list).
#[derive(Debug, Clone)]
pub struct Refresh {
    pub stats: SessionStats,
    pub tainted: bool,
    pub egress_denied: Vec<String>,
    /// How many values the host has struck out of what Pi read, in the whole session.
    pub masked: u32,
    /// What the judge withheld from the model, in the whole session, oldest first.
    pub withheld: Vec<Withheld>,
    /// The judge is on and did not answer at least once in this session.
    pub judge_silent: bool,
    /// Host mode: commands of this session still running in the background.
    pub jobs: usize,
}

/// A tool result the judge withheld from the model (the guard's `clio-guard.withheld` entry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Withheld {
    /// The model's tool: `bash`, `read`, `ls`, `find`.
    pub tool: String,
    /// Its path or command, as the host cleaned it.
    pub source: String,
    pub bytes: u64,
    /// Where the person can read it; `None` if it could not be kept.
    pub file: Option<String>,
}

pub struct Opened {
    pub session: Arc<dyn Session>,
    /// A host-mode session: the tools act on the real machine.
    pub host: bool,
    pub id: String,
    /// The folder the session works on (its copy).
    pub workspace: String,
    /// Pi's messages so far (a reopened session has them).
    pub history: Vec<Value>,
    pub refresh: Option<Refresh>,
    /// Clio speaks first: the interface starts the first turn itself, with no words of the person.
    pub presentation: bool,
    /// Quiet notes for the chat (the session file changed after it was closed).
    pub notes: Vec<String>,
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
    /// The person's answer to one of the guard's dialogs (`ask_person`): the value, or `None`
    /// when they skipped it.
    fn ui_respond(&self, _id: &str, _value: Option<&str>) {}
    fn refresh(&self) -> Result<Refresh, PiError>;
    /// Before closing a session the person worked in: asks the model, under a short cap, to save
    /// the session's summary. Nothing when it does not apply. Runs once.
    fn summarize(&self) {}
    /// Host mode: what this session set aside for the person since the last call, each once.
    fn new_pending(&self) -> Vec<clio_toolhost::pending::Pending> {
        Vec::new()
    }
    /// The person's answer at the terminal to one of `new_pending`: yes runs it (its result goes
    /// to the model with the next message), no rejects it. Says what happened.
    fn answer(&self, id: &str, _yes: bool) -> Result<String, String> {
        Err(format!("{id} is not of this session"))
    }
    /// The person's answer to a secret the model asked for (`request_secret`): the value goes to
    /// the vault, `None` declines it. What the model is told with the next message.
    fn answer_secret(
        &self,
        id: &str,
        _value: Option<&clio_vault::SecretValue>,
    ) -> Result<String, String> {
        Err(format!("{id} is not of this session"))
    }
    /// The judge flagged this session (`suspect`): a secret box says so.
    fn suspect(&self) -> bool {
        false
    }
    /// One of Pi's commands (`set_model`, `compact`...) for the chat's slash commands, sent only
    /// while no turn runs; Pi's `data`, or why not.
    fn command(&self, kind: &str, _fields: Value) -> Result<Value, String> {
        Err(format!("{kind}: not available in this session"))
    }
    /// Stops everything that belongs to the session now. Safe to repeat.
    fn shutdown(&self);
}

pub trait SessionSource: Send + Sync {
    fn list(&self) -> Vec<Row>;
    fn open(&self, target: &Target) -> Result<Opened, String>;
    /// The conversation `clio` reopens (a `Row::reference`): the first listed session that is
    /// Clio's, of the mode a new session would have and, in copy mode, of `project`. `None`: none.
    fn last(&self, _project: &std::path::Path) -> Option<String> {
        None
    }
}

/// The same Pi protocol over `ssh <host> clio serve-rpc` stdio. Not built yet: the source is
/// there so the selector, the keys and the trait do not change when it is; until then it lists
/// no row, since none could be opened.
pub struct RemoteSource;

const REMOTE_PENDING: &str = "remote: pending (ssh <host> clio serve-rpc does not exist yet)";

impl SessionSource for RemoteSource {
    fn list(&self) -> Vec<Row> {
        Vec::new()
    }

    fn open(&self, _: &Target) -> Result<Opened, String> {
        Err(REMOTE_PENDING.into())
    }
}

/// The sources of this process: this machine, and the remote placeholder. `mode`: `--host` or
/// `--copy`, over the configuration.
pub fn sources(dirs: Arc<Dirs>, mode: Option<Mode>) -> Vec<Box<dyn SessionSource>> {
    vec![
        Box::new(local::LocalSource::new(dirs).with_mode(mode)),
        Box::new(RemoteSource),
    ]
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_remote_source_lists_nothing_and_never_opens() {
        let r = RemoteSource;
        assert!(r.list().is_empty());
        let e = r.open(&Target::Existing {
            reference: "x".into(),
        });
        assert!(e.err().unwrap().contains("remote: pending"));
    }
}
