//! The memory tools: `memory_search` and `memory_get` read the person's active memories;
//! `memory_propose` suggests one, and a suggestion only ever lands in quarantine, where the person
//! decides (S-61). The memory itself sits behind [`MemoryPort`] (the real one lives in
//! `clio-brain`), so this crate never opens a database.
//!
//! What comes back is text somebody wrote, maybe long ago, maybe hostile. It is cleaned first,
//! cut after cleaning (so a secret is never split by the cut), kept to one line per memory (a
//! memory cannot forge the line of another one) and wrapped as data (S-08, S-60). The session is
//! marked as tainted only when a memory is not the person's own (S-05): marking on every read
//! would tire the person and be ignored, never marking would make S-05 mean nothing.
//!
//! A session may spend 20 calls and 24 KiB of returned text (S-47). The count lives in a file of the
//! run folder, so a restarted service of the same run does not give a fresh budget (the MCP server
//! has a folder per process, so its budget is per process), and what is logged are facts
//! (ids, counts, a short hash of the query), never the text of a query or of a memory.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clio_core::Effect;
use clio_gateway::{ArgKind, ArgSpec, Egress, Source, ToolDecl, wrap_untrusted};
use clio_vault::count_masks;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{DecisionOut, Request, Response, ToolHost, digest_hex};

pub(crate) const MEMORY_OPS: [&str; 12] = [
    "memory_search",
    "memory_get",
    "memory_timeline",
    "memory_context",
    "memory_propose",
    "memory_session_summary",
    "memory_update",
    "memory_consolidate",
    "memory_forget",
    "memory_promote",
    "memory_revert",
    "memory_pin",
];

const SEARCH_DESCRIPTION: &str = "Search the person's memory (active memories only, read-only)";
const GET_DESCRIPTION: &str = "Read one active memory of the person by id (read-only)";
const PROPOSE_DESCRIPTION: &str = "Propose a memory: Clio decides whether it is kept, replaces another, was known, or waits for the person's review";

const SUMMARY_DESCRIPTION: &str = "Save the summary of this session (goal, what was done, discoveries, next steps, files): Clio decides it like a proposal";
/// The parts of a session summary, in the order they are written, and the two it cannot lack.
const SUMMARY_PARTS: [(&str, &str); 6] = [
    ("goal", "Objetivo"),
    ("instructions", "Instrucciones"),
    ("discoveries", "Descubrimientos"),
    ("accomplished", "Hecho"),
    ("next_steps", "Siguiente"),
    ("files", "Archivos"),
];

const UPDATE_DESCRIPTION: &str = "Propose a new text for one memory (its id): Clio decides whether it replaces it, like any proposal";

const CONSOLIDATE_DESCRIPTION: &str = "Merge two to five memories into one because the person asked (Jev checks the person's words); the originals are archived, not erased";
/// How many memories one merge may join.
const MAX_MERGE: usize = 5;

const TIMELINE_DESCRIPTION: &str =
    "Read the memories around one, of its session (or its place), in order (read-only)";
const CONTEXT_DESCRIPTION: &str =
    "Read the last session's summary, the pinned memories and the most recent ones (read-only)";
const FORGET_DESCRIPTION: &str =
    "Archive a memory because the person asked (it is not erased; Jev checks the person's words)";
const PROMOTE_DESCRIPTION: &str = "Accept a memory that waits for review because the person asked (Jev checks the person's words)";
const PIN_DESCRIPTION: &str = "Pin a memory so it comes first in the context, because the person asked (Jev checks the person's words)";
const REVERT_DESCRIPTION: &str =
    "Undo a change Jev made to the memory because the person asked (Jev checks the person's words)";

/// Changes a session may ask for through these three tools (S-47, S-65).
pub(crate) const MAX_CHANGES: u32 = 10;
const NO_CHANGES: &str =
    "this memory cannot make that change: the person can do it with `clio memory`";

/// Longest proposal, in characters, after cleaning (`Qué: … · Por qué: … · Dónde: … · Aprendido: …`).
const MAX_PROPOSAL_CHARS: usize = 1200;
/// Longest title.
const MAX_TITLE_CHARS: usize = 120;
/// The categories a proposal may have (the same names as the memory's).
pub const CATEGORIES: [&str; 8] = [
    "decision",
    "architecture",
    "bugfix",
    "pattern",
    "config",
    "discovery",
    "preference",
    "learning",
];
/// How many memories a timeline may show on each side.
const MAX_AROUND: usize = 5;
/// A memory read in full (`memory_get`) may be this long; a search shows a preview.
const MAX_FULL_LINE_CHARS: usize = 1600;
/// Proposals a TAINTED session may make (S-47, S-61): what it read may be hostile, and what it
/// proposes is the model's own words after reading it.
pub(crate) const MAX_PROPOSALS: u32 = 5;
/// Proposals a session that has read nothing hostile may make. The memory's own quota per hour is
/// the real rate limit; this only stops a loop that proposes without end.
pub(crate) const MAX_PROPOSALS_CLEAN: u32 = 30;

const MAX_QUERY_CHARS: usize = 200;
const DEFAULT_LIMIT: usize = 5;
const MAX_LIMIT: usize = 8;
/// A whole memory line, `[m1] kind · origin · date: text` included.
const MAX_LINE_CHARS: usize = 600;
/// A whole answer, delimiters included.
const MAX_ANSWER_BYTES: usize = 4096;
/// Room kept for the line that says how many memories did not fit (the note is counted apart).
const OMITTED_ROOM: usize = 96;
const MAX_CALLS: u32 = 20;
const MAX_BUDGET_BYTES: u64 = 24 * 1024;
/// With less than this left the budget counts as spent: an answer needs room for its delimiters
/// and its notes, and the cap on returned text is hard.
const MIN_ROOM: u64 = 1024;
const MAX_ERROR_CHARS: usize = 300;
const MAX_NOTE_CHARS: usize = 200;
/// How long the memory has to answer before the call fails.
pub(crate) const MEMORY_TIMEOUT: Duration = Duration::from_secs(10);

/// Who wrote a memory. Only `Person` is what the person said; the other two can carry text that
/// came from outside, so reading them taints the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Person,
    Agent,
    External,
}

impl Origin {
    fn label(self) -> &'static str {
        match self {
            Origin::Person => "persona",
            Origin::Agent => "agente",
            Origin::External => "externo",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MemoryItem {
    pub id: i64,
    pub kind: String,
    pub origin: Origin,
    pub date: String,
    pub text: String,
    pub title: Option<String>,
    pub category: Option<String>,
    /// Where it lives: `proyecto`, `personal`, `lección de X`; empty when it does not say.
    pub place: String,
    /// How it stands to other memories, as Jev judged it or as it replaced one: `contradice a m12`.
    pub notes: Vec<String>,
}

impl MemoryItem {
    /// An item with no title, category or place.
    pub fn plain(id: i64, kind: &str, origin: Origin, date: &str, text: &str) -> Self {
        Self {
            id,
            kind: kind.to_owned(),
            origin,
            date: date.to_owned(),
            text: text.to_owned(),
            title: None,
            category: None,
            place: String::new(),
            notes: Vec::new(),
        }
    }
}

/// What the agent proposes, checked and cleaned by the service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalDraft {
    pub text: String,
    pub title: Option<String>,
    /// One of `CATEGORIES`.
    pub category: Option<String>,
    pub kind: ProposalKind,
}

#[derive(Debug, Clone)]
pub struct Found {
    pub items: Vec<MemoryItem>,
    /// The memory judged that nothing was relevant enough.
    pub abstained: bool,
    /// Something the model should know about how the search went (e.g. text only, no vectors).
    pub note: Option<String>,
}

/// What kind of memory is proposed. There is no third kind a model can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalKind {
    Semantic,
    Procedural,
    /// The summary of a session (`memory_session_summary`), never asked for with `kind`.
    Session,
}

/// What the memory did with a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saved {
    /// It is memory now (the judge, Jev, said it is new).
    Active,
    /// It replaced an earlier memory of the same fact.
    Replaced,
    /// It was already known: nothing was added.
    Known,
    /// It waits for the person's review.
    Waiting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Proposed {
    pub id: i64,
    pub saved: Saved,
    /// The change (operation) the judge made, so the model can undo it if the person asks.
    pub change: Option<i64>,
}

/// What the tool host tells the port about the call so it can judge it: whether the session read
/// anything hostile, and the person's last words in it (what they said themselves, never what the
/// model reports they said).
#[derive(Debug, Clone, Default)]
pub struct Judging {
    pub tainted: bool,
    pub person_said: Vec<String>,
    /// The Pi session the call comes from.
    pub session: String,
}

/// What the tool host needs from the memory: active memories of the person's scope to read, and
/// one way to write: a proposal that waits for the person. There is no way to promote, archive,
/// revert or purge through it, and nothing here says who the writer is or how far to trust it:
/// the port decides that (S-60, S-61).
pub trait MemoryPort: Send + Sync {
    /// Hands `text` to the memory as the agent's, from outside content: the memory decides (the
    /// judge when there is one, else the rules: quarantine for the person). `session` is only for
    /// the record. Returns the id and what became of it.
    fn propose(
        &self,
        draft: &ProposalDraft,
        session: &str,
        judging: &Judging,
    ) -> Result<Proposed, String>;
    /// A new text for memory `id`, decided like a proposal (the judge, or the person's review).
    fn update(
        &self,
        _id: i64,
        _draft: &ProposalDraft,
        _session: &str,
        _judging: &Judging,
    ) -> Result<Proposed, String> {
        Err("this memory cannot update; propose a new one".into())
    }
    /// One memory instead of `ids` (same place, facts), because the person asked: the port checks
    /// their words with the judge, saves the merged one and archives the originals.
    fn consolidate(
        &self,
        _ids: &[i64],
        _draft: &ProposalDraft,
        _session: &str,
        _judging: &Judging,
    ) -> Result<Proposed, String> {
        Err(NO_CHANGES.into())
    }
    /// The memories around `id` (its session, else its place), oldest first, `id` included.
    fn timeline(&self, _id: i64, _before: usize, _after: usize) -> Result<Vec<MemoryItem>, String> {
        Err("this memory has no timeline".into())
    }
    /// The last session's summary, the pinned memories and the most recent ones.
    fn context(&self) -> Result<Vec<MemoryItem>, String> {
        Err("this memory has no context".into())
    }
    /// Archives memory `id` (reversible, not an erasure) because the person asked. The port checks
    /// the person's words in `judging`; the model's claim is not evidence.
    fn forget(&self, _id: i64, _judging: &Judging) -> Result<(), String> {
        Err(NO_CHANGES.into())
    }
    /// Accepts the candidate `id` that waits for review, because the person asked.
    fn promote(&self, _id: i64, _judging: &Judging) -> Result<(), String> {
        Err(NO_CHANGES.into())
    }
    /// Pins memory `id` (it comes first in the context), because the person asked.
    fn pin(&self, _id: i64, _judging: &Judging) -> Result<(), String> {
        Err(NO_CHANGES.into())
    }
    /// Undoes `change`, one the judge made, because the person asked.
    fn undo(&self, _change: i64, _judging: &Judging) -> Result<(), String> {
        Err(NO_CHANGES.into())
    }
    fn search(&self, query: &str, limit: usize) -> Result<Found, String>;
    /// The same error for "does not exist", "is not active" and "is another scope's": which one
    /// it is must not leak.
    fn get(&self, id: i64) -> Result<MemoryItem, String>;
}

pub(crate) fn arg_specs(tool: &str) -> Vec<ArgSpec> {
    let a = |name: &str, kind, required| ArgSpec {
        name: name.into(),
        kind,
        required,
    };
    match tool {
        "memory_search" => vec![
            a("query", ArgKind::Text, true),
            a("limit", ArgKind::Int, false),
        ],
        "memory_propose" => vec![
            a("text", ArgKind::Text, true),
            a("kind", ArgKind::Text, true),
            a("title", ArgKind::Text, false),
            a("category", ArgKind::Text, false),
        ],
        "memory_session_summary" => SUMMARY_PARTS
            .iter()
            .map(|(name, _)| {
                a(
                    name,
                    ArgKind::Text,
                    matches!(*name, "goal" | "accomplished"),
                )
            })
            .collect(),
        "memory_update" => vec![
            a("id", ArgKind::Int, true),
            a("text", ArgKind::Text, true),
            a("title", ArgKind::Text, false),
            a("category", ArgKind::Text, false),
        ],
        "memory_consolidate" => vec![
            a("ids", ArgKind::Text, true),
            a("text", ArgKind::Text, true),
            a("title", ArgKind::Text, false),
            a("category", ArgKind::Text, false),
        ],
        "memory_timeline" => vec![
            a("id", ArgKind::Int, true),
            a("before", ArgKind::Int, false),
            a("after", ArgKind::Int, false),
        ],
        "memory_context" => vec![],
        "memory_revert" => vec![a("change", ArgKind::Int, true)],
        _ => vec![a("id", ArgKind::Int, true)],
    }
}

/// The arguments of a memory tool as the gateway checks them: name, JSON type and whether it is
/// required. Other callers (the MCP server) build their schema from this, not from a copy.
pub fn memory_args(tool: &str) -> Vec<(String, &'static str, bool)> {
    arg_specs(tool)
        .into_iter()
        .map(|a| {
            let ty = match a.kind {
                ArgKind::Int => "integer",
                _ => "string",
            };
            (a.name, ty, a.required)
        })
        .collect()
}

/// The declarations for the gateway, and each one's description hash (S-15).
pub(crate) fn decls() -> Vec<ToolDecl> {
    [
        ("memory_search", SEARCH_DESCRIPTION),
        ("memory_get", GET_DESCRIPTION),
        ("memory_timeline", TIMELINE_DESCRIPTION),
        ("memory_context", CONTEXT_DESCRIPTION),
        ("memory_propose", PROPOSE_DESCRIPTION),
        ("memory_session_summary", SUMMARY_DESCRIPTION),
        ("memory_update", UPDATE_DESCRIPTION),
        ("memory_consolidate", CONSOLIDATE_DESCRIPTION),
        ("memory_forget", FORGET_DESCRIPTION),
        ("memory_promote", PROMOTE_DESCRIPTION),
        ("memory_revert", REVERT_DESCRIPTION),
        ("memory_pin", PIN_DESCRIPTION),
    ]
    .into_iter()
    .map(|(name, description)| ToolDecl {
        name: name.into(),
        version: "1".into(),
        // A proposal changes the memory file, but only by adding a candidate nobody trusts.
        effect: if matches!(
            name,
            "memory_search" | "memory_get" | "memory_timeline" | "memory_context"
        ) {
            Effect::Read
        } else {
            Effect::LocalReversible
        },
        egress: Egress::None,
        description_hash: digest_hex(description),
        args: arg_specs(name),
    })
    .collect()
}

/// `"12, m15 18"` as distinct positive ids, 2 to `MAX_MERGE` of them.
fn merge_ids(s: &str) -> Option<Vec<i64>> {
    let mut ids = Vec::new();
    for part in s
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
    {
        let n: i64 = part
            .trim_start_matches('m')
            .parse()
            .ok()
            .filter(|n| *n > 0)?;
        if ids.contains(&n) {
            return None;
        }
        ids.push(n);
    }
    (2..=MAX_MERGE).contains(&ids.len()).then_some(ids)
}

/// The parts of a summary that were given, as typed, each after its label: the vault and the
/// checks of a proposal see it whole, then it is flattened like any proposal.
fn summary_text(args: &Map<String, Value>) -> Result<String, &'static str> {
    let mut parts = Vec::new();
    for (name, label) in SUMMARY_PARTS {
        match args.get(name).and_then(Value::as_str).map(str::trim) {
            Some(v) if !v.is_empty() => parts.push(format!("{label}: {v}")),
            _ if matches!(name, "goal" | "accomplished") => {
                return Err("a summary needs its goal and what was accomplished");
            }
            _ => {}
        }
    }
    Ok(parts.join(" · "))
}

/// Whitespace of any kind becomes one space and what is left of the control characters goes: one
/// line, nothing a terminal or the model's framing could take as structure.
fn one_line(s: &str) -> String {
    let kept: String = s
        .chars()
        .filter(|c| c.is_whitespace() || !c.is_control())
        .collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// What a session has spent of the memory budget. A hard cap (S-47): the file is read before
/// every call and written before the port is asked, so a call is counted even if the host dies
/// in the middle of it.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Budget {
    calls: u32,
    bytes: u64,
}

impl Budget {
    /// No file is a fresh budget; a file that cannot be read or understood is an error, never a
    /// fresh budget.
    fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| format!("it is not valid: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Whole or not at all: a temp file, then `rename`. `create_new`, so a planted link is not
    /// followed.
    fn save(&self, path: &Path) -> Result<(), String> {
        let tmp = path.with_extension("json.tmp");
        let _ = std::fs::remove_file(&tmp);
        let text = serde_json::to_string(self).map_err(|e| e.to_string())?;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| e.to_string())?;
        f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }
}

/// What one call leaves in `memory.jsonl`. No text of the query or of a memory, ever.
#[derive(Default)]
struct Facts {
    ids: Vec<i64>,
    /// The change (operation) an undo was about: not a memory id.
    change: Option<i64>,
    /// This call marked the session (a memory that is not the person's).
    marked: bool,
    query_hash: Option<String>,
}

impl ToolHost {
    /// Cleaning first (the scrubber must see the whole text), then one line, then the cut.
    fn tidy(&self, s: &str, max: usize) -> String {
        cut(&one_line(&self.clean(s)), max)
    }

    fn memory_failed(&self, req: &Request, out: DecisionOut, why: &str) -> Response {
        let content = cut(
            &format!("{} failed: {}", req.tool, one_line(&self.clean(why))),
            MAX_ERROR_CHARS,
        );
        Response {
            id: req.id.clone(),
            ok: false,
            content,
            data: Value::Null,
            decision: out,
            tainted: self.is_tainted(&req.session),
            external: false,
        }
    }

    /// Runs the port off this thread, with a deadline and a net for panics: a memory that hangs
    /// or blows up is an error of this one call, never of the host.
    // shortcut: a port that never returns leaves its thread behind (the call itself fails on
    // time); a port that can hang for good needs a process of its own.
    fn ask_port<T: Send + 'static>(
        &self,
        port: &Arc<dyn MemoryPort>,
        f: impl FnOnce(&dyn MemoryPort) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        self.ask_port_for(port, f, self.memory_timeout)
    }

    fn ask_port_for<T: Send + 'static>(
        &self,
        port: &Arc<dyn MemoryPort>,
        f: impl FnOnce(&dyn MemoryPort) -> Result<T, String> + Send + 'static,
        timeout: Duration,
    ) -> Result<T, String> {
        let (tx, rx) = mpsc::channel();
        let port = Arc::clone(port);
        std::thread::spawn(move || {
            let ran = catch_unwind(AssertUnwindSafe(|| f(port.as_ref())));
            let _ = tx.send(ran.unwrap_or_else(|_| Err("the memory tool panicked".into())));
        });
        rx.recv_timeout(timeout)
            .unwrap_or_else(|_| Err("the memory did not answer in time".into()))
    }

    fn budget_denied(&self, req: &Request, why: &str) -> Response {
        // Not through the gateway: this is not a policy denial, so the breaker does not count it.
        self.refuse(
            &req.id,
            "S-47/memory",
            why.to_owned(),
            self.is_tainted(&req.session),
        )
    }

    /// `memory_search` / `memory_get`, after the gateway let the call through: the budget first
    /// (S-47), then the call, then the facts. Anything that goes wrong with the budget or the log
    /// denies the call: no read goes uncounted or unlogged.
    pub(crate) fn memory_call(
        &self,
        req: &Request,
        args: &Map<String, Value>,
        out: DecisionOut,
    ) -> Response {
        if matches!(
            req.tool.as_str(),
            "memory_propose" | "memory_session_summary" | "memory_update" | "memory_consolidate"
        ) {
            return self.propose_call(req, args, out);
        }
        if matches!(
            req.tool.as_str(),
            "memory_forget" | "memory_promote" | "memory_revert" | "memory_pin"
        ) {
            return self.change_call(req, args, out);
        }
        let path = self.run_dir.join("memory-budget.json");
        let mut budget = match Budget::load(&path) {
            Ok(b) => b,
            Err(why) => return self.budget_denied(req, &format!("the memory budget: {why}")),
        };
        let left = MAX_BUDGET_BYTES.saturating_sub(budget.bytes);
        if budget.calls >= MAX_CALLS || left < MIN_ROOM {
            return self.budget_denied(
                req,
                "this session spent its memory budget (20 calls or 24 KiB of text)",
            );
        }
        // Counted before the port is asked: failed and rejected calls spend it too.
        budget.calls += 1;
        if let Err(why) = budget.save(&path) {
            return self.budget_denied(req, &format!("the memory budget cannot be kept: {why}"));
        }
        let cap = usize::try_from(left).map_or(MAX_ANSWER_BYTES, |l| l.min(MAX_ANSWER_BYTES));
        let mut facts = Facts::default();
        let resp = self.memory_answer(req, args, out, cap, &mut facts);
        // Only what a memory said counts: an error or a refusal is not text of the memory.
        let bytes = if resp.ok { resp.content.len() } else { 0 };
        budget.bytes += bytes as u64;
        let logged = budget
            .save(&path)
            .and_then(|()| self.log_facts(req, &facts, bytes));
        match logged {
            Ok(()) => resp,
            Err(why) => self.budget_denied(req, &format!("the memory log cannot be kept: {why}")),
        }
    }

    /// `memory_propose`, after the gateway let the call through: the proposals a session may make
    /// (S-47) are counted in a file before the port is asked, as the reads are, and the log has to
    /// be kept or the call is denied. A proposal never marks the session: it carries what the
    /// model decided to say, not what a memory said.
    /// `memory_forget`, `memory_promote` and `memory_revert`: what the person asks the model to do
    /// with their memory. The port checks the person's own words (reported by the brain, never the
    /// model's claim) with Jev; without a judge it says no, and the person uses `clio memory`.
    fn change_call(&self, req: &Request, args: &Map<String, Value>, out: DecisionOut) -> Response {
        let Some(port) = self.memory.clone() else {
            return self.memory_failed(req, out, "there is no memory");
        };
        let key = if req.tool == "memory_revert" {
            "change"
        } else {
            "id"
        };
        let Some(n) = args.get(key).and_then(Value::as_i64).filter(|n| *n > 0) else {
            return self.memory_failed(req, out, &format!("{key} must be a positive number"));
        };
        let path = self.run_dir.join("memory-changes.json");
        let mut budget = match Budget::load(&path) {
            Ok(b) => b,
            Err(why) => return self.budget_denied(req, &format!("the change count: {why}")),
        };
        if budget.calls >= MAX_CHANGES {
            return self.budget_denied(
                req,
                &format!("this session asked for its memory changes ({MAX_CHANGES}); the person can use `clio memory`"),
            );
        }
        budget.calls += 1;
        if let Err(why) = budget.save(&path) {
            return self.budget_denied(req, &format!("the change count cannot be kept: {why}"));
        }
        let judging = Judging {
            tainted: self.is_tainted(&req.session),
            person_said: self.person_said(&req.session),
            session: req.session.clone(),
        };
        let tool = req.tool.clone();
        // Longer than a read: Jev, the vault and the writer may all take their time, and a change
        // reported as failed must not happen afterwards in the background.
        let done = self.ask_port_for(
            &port,
            move |p| match tool.as_str() {
                "memory_forget" => p.forget(n, &judging),
                "memory_promote" => p.promote(n, &judging),
                "memory_pin" => p.pin(n, &judging),
                _ => p.undo(n, &judging),
            },
            self.memory_timeout * 3,
        );
        let mut facts = Facts::default();
        let resp = match done {
            Ok(()) => {
                // One request of the person is one change: the next one needs new words.
                self.forget_said(&req.session);
                if req.tool == "memory_revert" {
                    facts.change = Some(n);
                } else {
                    facts.ids.push(n);
                }
                let content = match req.tool.as_str() {
                    "memory_forget" => {
                        "forgotten: it is archived, not erased; the person can restore it"
                    }
                    "memory_promote" => "promoted: it is part of the person's memory now",
                    "memory_pin" => {
                        "pinned: it comes first in the context; the person can unpin it with `clio memory`"
                    }
                    _ => "undone: the change was reverted",
                };
                Response {
                    id: req.id.clone(),
                    ok: true,
                    content: content.into(),
                    data: json!({ "done": true }),
                    decision: out,
                    tainted: self.is_tainted(&req.session),
                    external: false,
                }
            }
            Err(why) => self.memory_failed(req, out, &why),
        };
        match self.log_facts(req, &facts, 0) {
            Ok(()) => resp,
            Err(why) => self.budget_denied(req, &format!("the memory log cannot be kept: {why}")),
        }
    }

    fn propose_call(&self, req: &Request, args: &Map<String, Value>, out: DecisionOut) -> Response {
        // A merge archives memories: it is also one of the session's ten changes (S-65).
        if req.tool == "memory_consolidate" {
            let path = self.run_dir.join("memory-changes.json");
            let mut changes = match Budget::load(&path) {
                Ok(b) => b,
                Err(why) => return self.budget_denied(req, &format!("the change count: {why}")),
            };
            if changes.calls >= MAX_CHANGES {
                return self.budget_denied(
                    req,
                    &format!("this session asked for its memory changes ({MAX_CHANGES}); the person can use `clio memory`"),
                );
            }
            changes.calls += 1;
            if let Err(why) = changes.save(&path) {
                return self.budget_denied(req, &format!("the change count cannot be kept: {why}"));
            }
        }
        let path = self.run_dir.join("memory-proposals.json");
        let mut budget = match Budget::load(&path) {
            Ok(b) => b,
            Err(why) => return self.budget_denied(req, &format!("the proposal count: {why}")),
        };
        let cap = if self.is_tainted(&req.session) {
            MAX_PROPOSALS
        } else {
            MAX_PROPOSALS_CLEAN
        };
        if budget.calls >= cap {
            return self.budget_denied(
                req,
                &format!("this session made its memory proposals ({cap}); the person reviews what is there"),
            );
        }
        budget.calls += 1;
        if let Err(why) = budget.save(&path) {
            return self.budget_denied(req, &format!("the proposal count cannot be kept: {why}"));
        }
        let mut facts = Facts::default();
        let resp = self.propose_answer(req, args, out, &mut facts);
        match self.log_facts(req, &facts, 0) {
            Ok(()) => resp,
            Err(why) => self.budget_denied(req, &format!("the memory log cannot be kept: {why}")),
        }
    }

    fn propose_answer(
        &self,
        req: &Request,
        args: &Map<String, Value>,
        out: DecisionOut,
        facts: &mut Facts,
    ) -> Response {
        let Some(port) = self.memory.clone() else {
            return self.memory_failed(req, out, "there is no memory");
        };
        let (typed, kind) = if req.tool == "memory_session_summary" {
            match summary_text(args) {
                Ok(t) => (t, ProposalKind::Session),
                Err(why) => return self.memory_failed(req, out, why),
            }
        } else if matches!(req.tool.as_str(), "memory_update" | "memory_consolidate") {
            let text = args.get("text").and_then(Value::as_str).unwrap_or_default();
            (text.to_owned(), ProposalKind::Semantic)
        } else {
            let kind = match args.get("kind").and_then(Value::as_str) {
                Some("semantic") => ProposalKind::Semantic,
                Some("procedural") => ProposalKind::Procedural,
                _ => return self.memory_failed(req, out, "kind must be semantic or procedural"),
            };
            let text = args.get("text").and_then(Value::as_str).unwrap_or_default();
            (text.to_owned(), kind)
        };
        let typed = typed.as_str();
        // The vault's cleaner sees the text before anything is proposed, as typed AND as it will be
        // proposed (flattened to one line): a secret with spaces in it must not get through by
        // swapping a space for a newline, a tab or a no-break space, which flattening puts back.
        // A text that had a secret in it is not proposed at all: a memory with a hole where a
        // secret was is useless, and the model had no business keeping it.
        let flat = one_line(typed);
        let (Some(from_typed), Some(from_flat)) = (self.try_clean(typed), self.try_clean(&flat))
        else {
            return self.memory_failed(req, out, "the vault could not filter the text");
        };
        if count_masks(&from_typed) > count_masks(typed)
            || count_masks(&from_flat) > count_masks(&flat)
        {
            return self.memory_failed(
                req,
                out,
                "the text had a secret in it: nothing was proposed",
            );
        }
        let text = one_line(&from_typed);
        let chars = text.chars().count();
        if chars == 0 || chars > MAX_PROPOSAL_CHARS {
            return self.memory_failed(
                req,
                out,
                &format!("the text must have 1 to {MAX_PROPOSAL_CHARS} characters"),
            );
        }
        // The title is checked like the text: cleaned by the vault, refused if it had a secret.
        let title = match args.get("title").and_then(Value::as_str) {
            None => None,
            Some(typed) => {
                let flat = one_line(typed);
                let Some(cleaned) = self.try_clean(&flat) else {
                    return self.memory_failed(req, out, "the vault could not filter the title");
                };
                if count_masks(&cleaned) > count_masks(&flat) {
                    return self.memory_failed(
                        req,
                        out,
                        "the title had a secret in it: nothing was proposed",
                    );
                }
                // The memory also caps a title at 256 bytes: a title of wide characters can reach it.
                if cleaned.chars().count() > MAX_TITLE_CHARS || cleaned.len() > 256 {
                    return self.memory_failed(
                        req,
                        out,
                        &format!("the title must have at most {MAX_TITLE_CHARS} characters"),
                    );
                }
                Some(cleaned).filter(|t| !t.is_empty())
            }
        };
        let category = match args.get("category").and_then(Value::as_str) {
            None => None,
            Some(c) => match CATEGORIES.iter().find(|k| k.eq_ignore_ascii_case(c.trim())) {
                Some(k) => Some((*k).to_owned()),
                None => {
                    return self.memory_failed(
                        req,
                        out,
                        &format!("category must be one of {}", CATEGORIES.join(", ")),
                    );
                }
            },
        };
        let draft = ProposalDraft {
            text,
            title,
            category,
            kind,
        };
        // A merge names two to five memories ("12, m15"), each once.
        let merge: Option<Vec<i64>> = if req.tool == "memory_consolidate" {
            let ids = merge_ids(args.get("ids").and_then(Value::as_str).unwrap_or_default());
            match ids {
                Some(v) => Some(v),
                None => {
                    return self.memory_failed(
                        req,
                        out,
                        &format!(
                            "ids must be 2 to {MAX_MERGE} different memory numbers, like \"12, 15\""
                        ),
                    );
                }
            }
        } else {
            None
        };
        let merged_ids = merge.clone().unwrap_or_default();
        // An update names the memory it is for; a bad id is refused before anyone is asked.
        let target = if req.tool == "memory_update" {
            match args.get("id").and_then(Value::as_i64).filter(|n| *n > 0) {
                Some(id) => Some(id),
                None => return self.memory_failed(req, out, "id must be a positive number"),
            }
        } else {
            None
        };
        let session = req.session.clone();
        let judging = Judging {
            tainted: self.is_tainted(&req.session),
            person_said: self.person_said(&req.session),
            session: req.session.clone(),
        };
        // Up to four questions to Jev (place, decision, the person's words, relation) and the
        // writer: a proposal reported as failed must not be saved afterwards in the background.
        match self.ask_port_for(
            &port,
            move |p| match (merge, target) {
                (Some(ids), _) => p.consolidate(&ids, &draft, &session, &judging),
                (None, Some(id)) => p.update(id, &draft, &session, &judging),
                (None, None) => p.propose(&draft, &session, &judging),
            },
            self.memory_timeout * 3,
        ) {
            Ok(done) if req.tool == "memory_consolidate" => {
                facts.ids.push(done.id);
                facts.ids.extend(merged_ids.iter().copied());
                // One request of the person is one change: the next one needs new words.
                self.forget_said(&req.session);
                Response {
                    id: req.id.clone(),
                    ok: true,
                    content: format!(
                        "merged into m{}: the originals are archived, not erased (the person can restore them)",
                        done.id
                    ),
                    data: json!({ "proposed": true, "result": "merged", "change": done.change }),
                    decision: out,
                    tainted: self.is_tainted(&req.session),
                    external: false,
                }
            }
            Ok(done) => {
                facts.ids.push(done.id);
                let (content, how) = match done.saved {
                    Saved::Active => ("saved: it is part of the person's memory now", "saved"),
                    Saved::Replaced => (
                        "saved: it replaced an earlier memory of the same fact",
                        "replaced",
                    ),
                    Saved::Known => ("already known: nothing was added", "known"),
                    Saved::Waiting => (
                        "proposed: it waits for the person's review and is not memory until they accept it",
                        "waiting",
                    ),
                };
                // The change number is only for undoing, if the person asks: `memory_revert`.
                let content = match done.change {
                    _ if done.saved == Saved::Waiting => format!(
                        "proposed (m{}): it waits for the person's review and is not memory until they accept it",
                        done.id
                    ),
                    Some(c) if matches!(done.saved, Saved::Active | Saved::Replaced) => {
                        format!(
                            "{content} (change {c}: if the person asks you to undo it, use memory_revert with it)"
                        )
                    }
                    _ => content.to_owned(),
                };
                Response {
                    id: req.id.clone(),
                    ok: true,
                    content,
                    data: json!({ "proposed": true, "result": how, "change": done.change }),
                    decision: out,
                    tainted: self.is_tainted(&req.session),
                    external: false,
                }
            }
            Err(why) => self.memory_failed(req, out, &why),
        }
    }

    fn log_facts(&self, req: &Request, facts: &Facts, bytes: usize) -> Result<(), String> {
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        let row = json!({
            "at": at,
            "tool": req.tool,
            "results": facts.ids.len(),
            "ids": facts.ids,
            "change": facts.change,
            "bytes": bytes,
            "tainted": facts.marked,
            "query_hash": facts.query_hash,
        });
        let path = self.run_dir.join("memory.jsonl");
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let mut line = row.to_string();
        line.push('\n');
        f.write_all(line.as_bytes()).map_err(|e| e.to_string())
    }

    /// The call itself, with its answer cut to `cap` bytes.
    fn memory_answer(
        &self,
        req: &Request,
        args: &Map<String, Value>,
        out: DecisionOut,
        cap: usize,
        facts: &mut Facts,
    ) -> Response {
        let Some(port) = self.memory.clone() else {
            return self.memory_failed(req, out, "there is no memory");
        };
        let found = if req.tool == "memory_search" {
            let typed = args
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default();
            // Not `tidy`: the port gets the query cut as it is, with no ellipsis, and a withheld
            // stand-in text must never be taken for a query.
            let Some(cleaned) = self.try_clean(typed) else {
                return self.memory_failed(req, out, "the vault could not filter the query");
            };
            let query: String = one_line(&cleaned).chars().take(MAX_QUERY_CHARS).collect();
            if query.is_empty() {
                return self.memory_failed(req, out, "the query is empty");
            }
            facts.query_hash = Some(digest_hex(&query)[..8].to_owned());
            let limit = args.get("limit").map_or(DEFAULT_LIMIT, |v| {
                let n = v.as_i64().unwrap_or(i64::MAX);
                usize::try_from(n.clamp(1, MAX_LIMIT as i64)).unwrap_or(MAX_LIMIT)
            });
            self.ask_port(&port, move |p| p.search(&query, limit))
        } else if req.tool == "memory_context" {
            self.ask_port(&port, |p| p.context()).map(|items| Found {
                items,
                abstained: false,
                note: Some(
                    "the last session's summary, then what is pinned, then the most recent".into(),
                ),
            })
        } else {
            // The gateway checked it is an integer; a u64 past i64::MAX is not an id.
            let Some(id) = args.get("id").and_then(Value::as_i64).filter(|id| *id > 0) else {
                return self.memory_failed(req, out, "the id must be a positive integer");
            };
            if req.tool == "memory_timeline" {
                let side = |k: &str| {
                    args.get(k).and_then(Value::as_i64).map_or(2, |n| {
                        usize::try_from(n.clamp(0, MAX_AROUND as i64)).unwrap_or(MAX_AROUND)
                    })
                };
                let (before, after) = (side("before"), side("after"));
                self.ask_port(&port, move |p| p.timeline(id, before, after))
                    .map(|items| Found {
                        items,
                        abstained: false,
                        note: None,
                    })
            } else {
                self.ask_port(&port, move |p| p.get(id)).map(|item| Found {
                    items: vec![item],
                    abstained: false,
                    note: None,
                })
            }
        };
        let found = match found {
            Ok(f) => f,
            Err(why) => return self.memory_failed(req, out, &why),
        };

        // The session is marked for what the memory returned, shown or not.
        if found.items.iter().any(|i| i.origin != Origin::Person) {
            self.mark(&req.session, "memory");
            facts.marked = true;
        }
        let note = found.note.as_deref().map(|n| self.tidy(n, MAX_NOTE_CHARS));
        if found.items.is_empty() {
            let mut content = String::from("nothing relevant in memory");
            if let Some(n) = note.filter(|n| !n.is_empty()) {
                content = format!("{content} ({n})");
            }
            return Response {
                id: req.id.clone(),
                ok: true,
                content,
                data: json!({ "ids": [], "count": 0, "masked": 0 }),
                decision: out,
                tainted: self.is_tainted(&req.session),
                external: false,
            };
        }

        // Saturating all the way: `cap` can be the 1 KiB floor and a port's note can be longer than
        // that, and a wrapped `room` would lift the hard cap.
        let mut room = cap
            .saturating_sub(wrap_untrusted(Source::Memory, "").len())
            .saturating_sub(OMITTED_ROOM);
        // The lines after the memories are kept only while they fit in what is left of the cap;
        // a note that does not fit is dropped, whole, never cut.
        let mut tail = Vec::new();
        if found.abstained {
            tail.push("[the memory was unsure: these results may not be relevant]".to_owned());
        }
        if let Some(n) = note.filter(|n| !n.is_empty()) {
            tail.push(format!("[note] {n}"));
        }
        tail.retain(|l| match room.checked_sub(l.len() + 1) {
            Some(r) => {
                room = r;
                true
            }
            None => false,
        });
        let (mut lines, mut ids, mut masked, mut used) = (Vec::new(), Vec::new(), 0, 0);
        // A memory read on its own is shown whole (within the cap); a list shows previews.
        let max_line = if req.tool == "memory_get" {
            MAX_FULL_LINE_CHARS
        } else {
            MAX_LINE_CHARS
        };
        for it in &found.items {
            let (kind, date) = (self.tidy(&it.kind, 32), self.tidy(&it.date, 32));
            let mut head = format!("[m{}] {kind} · {} · {date}", it.id, it.origin.label());
            if let Some(c) = it.category.as_deref() {
                head.push_str(&format!(" · {}", self.tidy(c, 32)));
            }
            if !it.place.is_empty() {
                head.push_str(&format!(" · {}", self.tidy(&it.place, 100)));
            }
            for n in it.notes.iter().take(4) {
                head.push_str(&format!(" · {}", self.tidy(n, 60)));
            }
            head.push_str(": ");
            // The title goes before the text, in the same line, cleaned like it.
            let body = match it.title.as_deref() {
                Some(t) => format!("{} — {}", self.tidy(t, MAX_TITLE_CHARS), it.text),
                None => it.text.clone(),
            };
            let cleaned = self.clean(&body);
            let text = cut(
                &one_line(&cleaned),
                max_line.saturating_sub(head.chars().count()),
            );
            let line = format!("{head}{text}");
            if used + line.len() + 1 > room {
                break;
            }
            used += line.len() + 1;
            // Only memories that are shown: one left out was never in front of the model.
            masked += count_masks(&cleaned).saturating_sub(count_masks(&body));
            ids.push(it.id);
            lines.push(line);
        }
        let left = found.items.len() - ids.len();
        facts.ids.clone_from(&ids);
        if left > 0 {
            lines.push(format!(
                "[+{left} more memories left out: the answer is limited to {cap} bytes]"
            ));
        }
        lines.extend(tail);
        Response {
            id: req.id.clone(),
            ok: true,
            content: wrap_untrusted(Source::Memory, &lines.join("\n")),
            data: json!({ "ids": ids, "count": ids.len(), "masked": masked }),
            decision: out,
            tainted: self.is_tainted(&req.session),
            external: true,
        }
    }
}
