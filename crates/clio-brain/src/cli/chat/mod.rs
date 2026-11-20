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
pub mod review;
pub mod run;
pub mod subagent;

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
    /// Host mode: everything of this session that still waits for the person, oldest first; each
    /// may be answered here from now on.
    fn waiting(&self) -> Vec<clio_toolhost::pending::Pending> {
        Vec::new()
    }
    /// The person's answer at the terminal to one of `new_pending` or `waiting`: yes runs it (its result goes
    /// to the model with the next message), no rejects it. Says what happened.
    fn answer(&self, id: &str, _yes: bool) -> Result<String, String> {
        Err(format!("{id} is not of this session"))
    }
    /// Under bypass or an open permission window: approves item `id` of this session as it,
    /// through the same path as the person's yes, recorded in its record before it runs, if it
    /// may answer it (`grant::answering`: bypass never Clio's own guards, a secret or a block
    /// change; a window only what was set aside after it opened, and never control, a service or
    /// an MCP server either). Never in a `suspect` session or for a round, never with the judge
    /// off. What the chat says.
    fn grant(&self, id: &str) -> Result<String, String> {
        Err(format!("{id} is not of this session"))
    }
    /// What the permission window or bypass approved in this session that the chat has not said
    /// yet, one quiet note each.
    fn granted_news(&self) -> Vec<String> {
        Vec::new()
    }
    /// A permission window that ended (`w`, with `acts`): its record goes with the next turn Clio
    /// starts by herself, as data, with her request to sum it up for the person, if this
    /// conversation may tell it (`pending::may_tell`). Whether it was queued.
    fn queue_summary(
        &self,
        _w: &clio_toolhost::grant::Window,
        _acts: &[clio_toolhost::grant::Act],
    ) -> bool {
        false
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
    /// Cancels a compaction in flight (Pi's `abort`), beside the `command` that waits for it.
    fn abort_compaction(&self) {}
    /// The subagents of this session's run so far, oldest first (S-73).
    fn subagents(&self) -> Vec<Value> {
        vec![]
    }
    /// Stops everything that belongs to the session now. Safe to repeat.
    fn shutdown(&self);
    /// What the last turn changed (files written, commands run, as the action log records
    /// them), or `None` when it changed nothing (a read-only turn).
    fn last_work(&self) -> Option<TurnWork> {
        None
    }
    /// The judge's closed question after a turn that changed something: does this work need an
    /// independent review? `Judged::yes` only when the judge said so; every decision is kept in
    /// the run's `reviews/judged.jsonl`.
    fn needs_review(&self, _work: &TurnWork) -> review::Judged {
        review::Judged::JudgeOff
    }
    /// A review by a second agent (`review::run`): its own Pi, tool host and run folder, the
    /// marks and the day's budget of this session. Runs beside a turn; `cancel` stops it.
    fn review(&self, _req: &ReviewRequest, _cancel: &Cancel) -> Result<Reviewed, String> {
        Err("reviews are not available in this session".into())
    }
    /// A review's findings go with the next message, as data, with Clio's request to answer
    /// them, once the judge read them like any outside text; `marked`: the reviewer ended marked
    /// (`Some(suspect)`), so they mark this session the same way, and are not given when the
    /// mark cannot be made. `ask` is Clio's request that goes with them.
    fn queue_findings(&self, _findings: &str, _marked: Option<bool>, _ask: &'static str) {}
    /// Skills (S-68), after a turn: the drafts `skill_write` stored since the last call (each
    /// once: id and name), and what the person is told of the skills' reports (one turned off).
    fn skill_news(&self) -> (Vec<(String, String)>, Vec<String>) {
        (Vec::new(), Vec::new())
    }
    /// What the model asked of what it created, free (S-69), after a turn or when the chat opens:
    /// the chat's notes of what was done now (`off`, or not done), and the runs, each for
    /// `manage_run` on a thread of its own. The model hears each outcome as data.
    fn manage_news(&self) -> (Vec<String>, Vec<crate::cli::manage::Request>) {
        (Vec::new(), Vec::new())
    }
    /// Runs one of `manage_news`'s runs; the chat's note of what came of it.
    fn manage_run(&self, _r: &crate::cli::manage::Request) -> String {
        String::new()
    }
    /// Reviews skill draft `id` (`review::run` on its text and check) and acts on the verdict:
    /// registers it, sets it aside for the person, or gives the findings back to the model.
    fn review_skill(&self, id: &str, _cancel: &Cancel) -> Result<Reviewed, String> {
        Err(format!("{id}: skills are not available in this session"))
    }
}

/// What came of a skill draft's review.
#[derive(Debug, Clone)]
pub enum SkillOutcome {
    /// Registered as version `n`: the chat says so in one line. `notes`: what the review that
    /// passed it still found, for the model once (as a review's findings).
    Registered {
        name: String,
        description: String,
        n: u32,
        notes: Option<String>,
    },
    /// Set aside (`pending` id): its Approval box, when Clio asks; `by`: bypass or the
    /// permission window (`grant::Kind::name`) answers it by itself at the chat's next check.
    Waits {
        id: String,
        ask: Box<Option<super::tui::ask::BlockAsk>>,
        by: Option<&'static str>,
    },
    /// Not registered (the review failed): the findings go to the model (`to_model`); `dropped`
    /// after the third failure in a row.
    Rejected {
        name: String,
        attempt: u32,
        dropped: bool,
        to_model: String,
    },
    /// A service draft's review failed (S-69): as `Rejected`, and the person is never asked.
    ServiceRejected {
        name: String,
        attempt: u32,
        dropped: bool,
        to_model: String,
    },
}

/// What a turn did that a review can check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnWork {
    /// What the person asked (empty for a turn Clio started).
    pub request: String,
    pub commands: Vec<String>,
    pub files: Vec<String>,
    /// The model's final message.
    pub answer: String,
}

/// What a review is asked: a description, the subject (text and paths) and a check (may be
/// empty: the reviewer designs one).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewRequest {
    pub what: String,
    pub subject: String,
    pub paths: Vec<String>,
    pub check: String,
}

impl ReviewRequest {
    /// `/review [what]` or the judge: the last turn's work, described by `what` or by the
    /// person's request.
    pub fn of_work(what: Option<&str>, work: Option<&TurnWork>) -> Self {
        let Some(w) = work else {
            return Self {
                what: what.unwrap_or_default().to_owned(),
                ..Self::default()
            };
        };
        let mut subject = format!("What the person asked:\n{}\n\nWhat was done:\n", w.request);
        for c in &w.commands {
            subject.push_str(&format!("- ran: {c}\n"));
        }
        for f in &w.files {
            subject.push_str(&format!("- wrote: {f}\n"));
        }
        subject.push_str(&format!("\nWhat the agent said at the end:\n{}", w.answer));
        Self {
            what: what.map_or_else(
                || format!("the work done for this request: {}", w.request),
                str::to_owned,
            ),
            subject,
            paths: w.files.clone(),
            check: String::new(),
        }
    }
}

/// A review's result: the answer, and how it ran.
#[derive(Debug, Clone)]
pub struct Reviewed {
    pub answer: clio_brain::pi::review::Answer,
    /// The model it ran on.
    pub model: String,
    /// Why it did not run on the reviewer's model (the connector does not offer it).
    pub fallback: Option<String>,
    pub secs: u64,
    /// The commands it ran (`bash` calls).
    pub commands: usize,
    /// The reviewer ended marked (it read what is not the person's): `Some(suspect)`.
    pub marked: Option<bool>,
    /// The reviewer's whole answer, kept in the review's folder.
    pub full: Option<String>,
    pub usage: clio_brain::pi::rpc::Usage,
    /// A skill draft's review: what came of it.
    pub skill: Option<SkillOutcome>,
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
