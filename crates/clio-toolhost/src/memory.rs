//! The memory tools. The memory is the agent's: `memory_search`, `memory_get`, `memory_timeline`
//! and `memory_context` read the active memories; `memory_save` (also `memory_propose`, its old
//! name, for one version), `memory_update`, `memory_forget` (to quarantine), `memory_pin`,
//! `memory_consolidate` and `memory_revert` (the agent's own changes) act at once (S-61, S-65). The
//! memory itself sits behind [`MemoryPort`] (the real one lives in `clio-brain`), so this crate
//! never opens a database.
//!
//! What comes back is text somebody wrote, maybe long ago, maybe hostile. It is cleaned first,
//! cut after cleaning (so a secret is never split by the cut), kept to one line per memory (a
//! memory cannot forge the line of another one) and wrapped as data (S-08, S-60). The session is
//! marked as tainted only when a memory is not the person's own (S-05): marking on every read
//! would tire the person and be ignored, never marking would make S-05 mean nothing.
//!
//! A session reads, saves and changes as often as it needs: there is no cap per session. What is
//! logged are facts (ids, counts, a short hash of the query), never the text of a query or of a
//! memory.

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
use serde_json::{Map, Value, json};

use crate::{DecisionOut, Request, Response, ToolHost, digest_hex};

pub(crate) const MEMORY_OPS: [&str; 13] = [
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
];

/// The old name of `memory_save`, still served for one version (a prompt or a client of before).
pub(crate) const OLD_SAVE: &str = "memory_propose";

const SEARCH_DESCRIPTION: &str = "Search the person's memory (active memories only, read-only)";
const GET_DESCRIPTION: &str = "Read one active memory of the person by id (read-only)";
const SAVE_DESCRIPTION: &str =
    "Save a memory: it is memory at once (Clio may refuse something unsafe or not worth keeping)";

const SUMMARY_DESCRIPTION: &str = "Save the summary of this session (goal, what was done, discoveries, next steps, files): the parts together, with their labels, may have 1200 characters (about 1115 of your own)";
/// The parts of a session summary, in the order they are written, and the two it cannot lack.
const SUMMARY_PARTS: [(&str, &str); 6] = [
    ("goal", "Objetivo"),
    ("instructions", "Instrucciones"),
    ("discoveries", "Descubrimientos"),
    ("accomplished", "Hecho"),
    ("next_steps", "Siguiente"),
    ("files", "Archivos"),
];

const UPDATE_DESCRIPTION: &str = "Correct one memory (its id) with a new text: it replaces it at once; the old one stays in the log";

const CONSOLIDATE_DESCRIPTION: &str =
    "Merge two to five memories into one; the originals are forgotten (quarantine), not erased";
/// How many memories one merge may join.
const MAX_MERGE: usize = 5;

const TIMELINE_DESCRIPTION: &str =
    "Read the memories around one, of its session (or its place), in order (read-only)";
const CONTEXT_DESCRIPTION: &str =
    "Read the last session's summary, the pinned memories and the most recent ones (read-only)";
const FORGET_DESCRIPTION: &str = "Forget a memory: it leaves the memory for quarantine, its record kept (the person can restore it)";
const PIN_DESCRIPTION: &str = "Pin a memory so it comes first in the context";
const REVERT_DESCRIPTION: &str = "Undo a change you made to the memory (its change number)";
const BLOCK_DESCRIPTION: &str = "Read one of the person's blocks whole, in order, each entry with its id: identidad (who Clio is and how she speaks), reglas (how she works and her limits), preferencias (who the person is), vigilancia (what she watches) (read-only)";
const TEXT_SIZE: &str = "the text must have 1 to 1200 characters";
/// The person's blocks: the only names `memory_block_edit` can change.
pub const BLOCK_NAMES: [&str; 4] = ["identidad", "reglas", "preferencias", "vigilancia"];
/// The memory's judge declining a text as not worth keeping (the brain's port says exactly this).
pub const NOT_WORTH_REMEMBERING: &str = "Clio did not save it: it does not look worth remembering";
const HIDDEN_CHARS: &str =
    "the text has invisible or control characters: write it again with visible characters only";
const BLOCK_EDIT_DESCRIPTION: &str = "Change one of the person's blocks as the person asked in this conversation: add an entry, replace one or remove one. In a session that read outside content it is set aside and written only if the person approves it";
/// What a session that read outside content hears when it tries to change a block and nobody can
/// be asked (no person, as in `clio run`): the person's own words only enter from a clean session
/// or with the person's approval (S-05).
pub const BLOCK_TAINTED: &str = "this session read content from outside: identity and what is watched are only changed in a clean session. Open a new session and ask for it there.";

const NO_CHANGES: &str =
    "this memory cannot make that change: the person can do it with `clio memory`";

/// Longest proposal, in characters, after cleaning (`Qué: … · Por qué: … · Dónde: … · Aprendido: …`).
const MAX_PROPOSAL_CHARS: usize = 1200;
/// Longest part of a session summary; the six parts with their labels must still fit in
/// `MAX_PROPOSAL_CHARS` (the labels and separators take 85).
const MAX_SUMMARY_PART_CHARS: usize = 600;
/// Longest title.
const MAX_TITLE_CHARS: usize = 120;
/// The categories a proposal may have (the same names as the memory's).
pub const CATEGORIES: [&str; 9] = [
    "decision",
    "architecture",
    "bugfix",
    "pattern",
    "config",
    "discovery",
    "preference",
    "profile",
    "learning",
];
/// How many memories a timeline may show on each side.
const MAX_AROUND: usize = 5;
/// A memory read in full (`memory_get`) may be this long; a search shows a preview.
const MAX_FULL_LINE_CHARS: usize = 1600;

const MAX_QUERY_CHARS: usize = 200;
const DEFAULT_LIMIT: usize = 5;
const MAX_LIMIT: usize = 8;
/// A whole memory line, `[m1] kind · origin · date: text` included.
const MAX_LINE_CHARS: usize = 600;
/// A whole answer, delimiters included.
const MAX_ANSWER_BYTES: usize = 4096;
/// Room kept for the line that says how many memories did not fit (the note is counted apart).
const OMITTED_ROOM: usize = 96;
const MAX_ERROR_CHARS: usize = 300;
const MAX_NOTE_CHARS: usize = 200;
/// Last night's line of the sleep, at most.
const MAX_MORNING_CHARS: usize = 600;
/// Neighbors shown under a memory read in full, ids of neighbors after each search result, and
/// how long a neighbor's preview is (the same as clio-memory's tools).
const NEAR_GET: usize = 5;
const NEAR_SEARCH: usize = 3;
const PREVIEW_CHARS: usize = 60;
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
    /// Where it lives: `personal`, `project X`, `lesson from X`; empty when it does not say.
    pub place: String,
    /// How it stands to other memories, as Jev judged it or as it replaced one: `contradice a m12`.
    pub notes: Vec<String>,
    /// The memories nearest to it, best first: what the model can walk to from it.
    pub near: Vec<Near>,
}

/// A memory near another one: its number, who wrote it and a few words of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Near {
    pub id: i64,
    pub origin: Origin,
    /// Its title or text, uncut: cleaned (vault values crossed out) before it is cut to a preview.
    pub preview: String,
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
            near: Vec::new(),
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
    /// It is memory now.
    Active,
    /// It replaced an earlier memory of the same fact.
    Replaced,
    /// It was already known: nothing was added.
    Known,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposed {
    pub id: i64,
    pub saved: Saved,
    /// The change (operation) the agent made, so it can undo it (`memory_revert`).
    pub change: Option<i64>,
    /// An existing memory the saved one is nearly identical to: the model may correct instead.
    pub similar: Option<Near>,
}

/// A change to one of the person's blocks, as `memory_block_edit` asks for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockEdit {
    /// A new last entry.
    Add(String),
    /// A new text for entry `id`, in its place.
    Replace(i64, String),
    /// Entry `id` leaves the block (and is forgotten, restorable).
    Remove(i64),
}

/// What the tool host tells the port about the call: whether the session read anything hostile
/// (what it saves is marked `externo`, and Jev is told).
#[derive(Debug, Clone, Default)]
pub struct Judging {
    pub tainted: bool,
    /// The Pi session the call comes from.
    pub session: String,
}

/// What the judge said about outside text a tool read (`MemoryPort::judge_text`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextVerdict {
    /// The person did not turn a judge on: nothing was asked.
    NoJudge,
    Clean,
    /// A clear «it tries to give an AI agent instructions»: the text is withheld from the model.
    Injection,
    /// The judge is on but did not answer (down, error, timeout, unsure of its own input).
    NoAnswer,
}

/// What the tool host needs from the memory: active memories to read, and the agent's own writes.
/// There is no way to promote, archive or purge through it (the person's, with `clio memory`), and
/// nothing here says who the writer is or which origin it gets: the port decides that (S-60, S-61).
pub trait MemoryPort: Send + Sync {
    /// Saves `text` as the agent's, at once. `session` is only for the record. Returns the id and
    /// what became of it; a refusal (a secret, the quota, Jev's clear «no») is the error.
    fn propose(
        &self,
        draft: &ProposalDraft,
        session: &str,
        judging: &Judging,
    ) -> Result<Proposed, String>;
    /// A new text for memory `id`: it replaces it at once.
    fn update(
        &self,
        _id: i64,
        _draft: &ProposalDraft,
        _session: &str,
        _judging: &Judging,
    ) -> Result<Proposed, String> {
        Err("this memory cannot update; propose a new one".into())
    }
    /// One memory instead of `ids` (same place, facts): the merged one is saved and the originals
    /// are forgotten.
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
    /// What the memory's sleep did last night, in its own words (ids and reasons, never memory
    /// text): `memory_context` starts with it. `None` when it did nothing or it was long ago.
    fn morning(&self) -> Option<String> {
        None
    }
    /// Forgets memory `id`: to quarantine, its record kept, restorable.
    fn forget(&self, _id: i64, _judging: &Judging) -> Result<(), String> {
        Err(NO_CHANGES.into())
    }
    /// Pins memory `id` (it comes first in the context).
    fn pin(&self, _id: i64, _judging: &Judging) -> Result<(), String> {
        Err(NO_CHANGES.into())
    }
    /// Undoes `change`, one the agent made itself.
    fn undo(&self, _change: i64, _judging: &Judging) -> Result<(), String> {
        Err(NO_CHANGES.into())
    }
    /// Every entry of the person's block `name`, whole and in order.
    fn block(&self, _name: &str) -> Result<Vec<MemoryItem>, String> {
        Err("this memory has no blocks".into())
    }
    /// Changes the person's block `name` with the person's authority: only from a clean session.
    fn block_edit(
        &self,
        _name: &str,
        _edit: &BlockEdit,
        _judging: &Judging,
    ) -> Result<MemoryItem, String> {
        Err(NO_CHANGES.into())
    }
    /// Is `text`, outside content a tool just read from `source`, trying to give an AI agent
    /// instructions or to manipulate it? Asked through the same judge (and the same process and
    /// key) that guards what the agent saves. The caller bounds how long it waits.
    fn judge_text(&self, _text: &str, _source: &str) -> TextVerdict {
        TextVerdict::NoJudge
    }
    /// `judge_text` for a caller that waits up to `timeout` (a skill from outside, read window by
    /// window by a judge that may be slow): the judge's own limit for the call is that long.
    fn judge_text_for(&self, text: &str, source: &str, _timeout: Duration) -> TextVerdict {
        self.judge_text(text, source)
    }
    /// The person turned a judge on for this service (Jev): without one no permission window or
    /// bypass acts (`grant`), and an open one closes.
    fn judge_on(&self) -> bool {
        false
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
        "memory_save" | OLD_SAVE => vec![
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
        "memory_block" => vec![a("name", ArgKind::Text, true)],
        "memory_block_edit" => vec![
            a("name", ArgKind::Text, true),
            a("action", ArgKind::Text, true),
            a("id", ArgKind::Int, false),
            a("text", ArgKind::Text, false),
        ],
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
        ("memory_save", SAVE_DESCRIPTION),
        ("memory_session_summary", SUMMARY_DESCRIPTION),
        ("memory_update", UPDATE_DESCRIPTION),
        ("memory_consolidate", CONSOLIDATE_DESCRIPTION),
        ("memory_forget", FORGET_DESCRIPTION),
        ("memory_revert", REVERT_DESCRIPTION),
        ("memory_pin", PIN_DESCRIPTION),
        ("memory_block", BLOCK_DESCRIPTION),
        ("memory_block_edit", BLOCK_EDIT_DESCRIPTION),
    ]
    .into_iter()
    .map(|(name, description)| ToolDecl {
        name: name.into(),
        version: "1".into(),
        // A write changes the memory file, and every change stays in its log (revertible).
        effect: if matches!(
            name,
            "memory_search" | "memory_get" | "memory_timeline" | "memory_context" | "memory_block"
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
fn summary_text(args: &Map<String, Value>) -> Result<String, String> {
    let mut parts = Vec::new();
    for (name, label) in SUMMARY_PARTS {
        match args.get(name).and_then(Value::as_str).map(str::trim) {
            Some(v) if one_line(v).chars().count() > MAX_SUMMARY_PART_CHARS => {
                return Err(format!(
                    "{name} has {} characters; a part may have {MAX_SUMMARY_PART_CHARS}",
                    one_line(v).chars().count()
                ));
            }
            Some(v) if !v.is_empty() => parts.push(format!("{label}: {v}")),
            _ if matches!(name, "goal" | "accomplished") => {
                return Err("a summary needs its goal and what was accomplished".into());
            }
            _ => {}
        }
    }
    Ok(parts.join(" · "))
}

/// A write's own arguments, checked before the write is counted.
struct Asked {
    typed: String,
    kind: ProposalKind,
    category: Option<String>,
    merge: Option<Vec<i64>>,
    target: Option<i64>,
}

/// Checks a write's arguments without the vault or the memory: kind, text length (measured
/// flattened, as it will be proposed), title length, category, ids. A refusal here is the model's
/// own mistake and does not spend a save; the secret checks come after the count.
fn asked(req: &Request, args: &Map<String, Value>) -> Result<Asked, String> {
    let (typed, kind) = if req.tool == "memory_session_summary" {
        (summary_text(args)?, ProposalKind::Session)
    } else if matches!(req.tool.as_str(), "memory_update" | "memory_consolidate") {
        let text = args.get("text").and_then(Value::as_str).unwrap_or_default();
        (text.to_owned(), ProposalKind::Semantic)
    } else {
        let kind = match args.get("kind").and_then(Value::as_str) {
            Some("semantic") => ProposalKind::Semantic,
            Some("procedural") => ProposalKind::Procedural,
            _ => return Err("kind must be semantic or procedural".into()),
        };
        let text = args.get("text").and_then(Value::as_str).unwrap_or_default();
        (text.to_owned(), kind)
    };
    let chars = one_line(&typed).chars().count();
    if chars == 0 || chars > MAX_PROPOSAL_CHARS {
        return Err(if kind == ProposalKind::Session {
            format!(
                "the summary has {chars} characters with its labels; the limit is {MAX_PROPOSAL_CHARS}"
            )
        } else {
            format!("the text has {chars} characters; it must have 1 to {MAX_PROPOSAL_CHARS}")
        });
    }
    if let Some(t) = args.get("title").and_then(Value::as_str).map(one_line)
        && (t.chars().count() > MAX_TITLE_CHARS || t.len() > 256)
    {
        return Err(format!(
            "the title must have at most {MAX_TITLE_CHARS} characters"
        ));
    }
    let category = match args.get("category").and_then(Value::as_str) {
        None => None,
        Some(c) => match CATEGORIES.iter().find(|k| k.eq_ignore_ascii_case(c.trim())) {
            Some(k) => Some((*k).to_owned()),
            None => return Err(format!("category must be one of {}", CATEGORIES.join(", "))),
        },
    };
    // A merge names two to five memories ("12, m15"), each once.
    let merge = if req.tool == "memory_consolidate" {
        match merge_ids(args.get("ids").and_then(Value::as_str).unwrap_or_default()) {
            Some(v) => Some(v),
            None => {
                return Err(format!(
                    "ids must be 2 to {MAX_MERGE} different memory numbers, like \"12, 15\""
                ));
            }
        }
    } else {
        None
    };
    // An update names the memory it is for.
    let target = if req.tool == "memory_update" {
        match args.get("id").and_then(Value::as_i64).filter(|n| *n > 0) {
            Some(id) => Some(id),
            None => return Err("id must be a positive number".into()),
        }
    } else {
        None
    };
    Ok(Asked {
        typed,
        kind,
        category,
        merge,
        target,
    })
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

/// What one call leaves in `memory.jsonl`. No text of the query or of a memory, ever.
#[derive(Default)]
struct Facts {
    ids: Vec<i64>,
    /// Neighbors whose words were shown with a memory read in full.
    near: Vec<i64>,
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

    pub(crate) fn ask_port_for<T: Send + 'static>(
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

    /// `memory_search` / `memory_get`, after the gateway let the call through: the call, then the
    /// facts. A log that cannot be kept denies the call: no read goes unlogged.
    pub(crate) fn memory_call(
        &self,
        req: &Request,
        args: &Map<String, Value>,
        out: DecisionOut,
    ) -> Response {
        // Before the memory is asked: a write that could not be logged must not happen.
        if let Err(why) = self.open_log() {
            return self.budget_denied(req, &format!("the memory log cannot be kept: {why}"));
        }
        if matches!(
            req.tool.as_str(),
            "memory_save" | "memory_session_summary" | "memory_update" | "memory_consolidate"
        ) {
            return self.propose_call(req, args, out);
        }
        if matches!(
            req.tool.as_str(),
            "memory_forget" | "memory_revert" | "memory_pin"
        ) {
            return self.change_call(req, args, out);
        }
        if req.tool == "memory_block_edit" {
            return self.block_edit_call(req, args, out);
        }
        // A block is read whole: what Clio watches can be longer than one answer of the others.
        let cap = if req.tool == "memory_block" {
            usize::MAX
        } else {
            MAX_ANSWER_BYTES
        };
        let mut facts = Facts::default();
        let resp = self.memory_answer(req, args, out, cap, &mut facts);
        // Only what a memory said counts: an error or a refusal is not text of the memory.
        let bytes = if resp.ok { resp.content.len() } else { 0 };
        match self.log_facts(req, &facts, bytes) {
            Ok(()) => resp,
            Err(why) => self.budget_denied(req, &format!("the memory log cannot be kept: {why}")),
        }
    }

    /// `memory_forget`, `memory_pin` and `memory_revert`: the agent's own changes (S-65).
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
        let judging = Judging {
            tainted: self.is_tainted(&req.session),
            session: req.session.clone(),
        };
        let tool = req.tool.clone();
        // Longer than a read: Jev, the vault and the writer may all take their time, and a change
        // reported as failed must not happen afterwards in the background.
        let done = self.ask_port_for(
            &port,
            move |p| match tool.as_str() {
                "memory_forget" => p.forget(n, &judging),
                "memory_pin" => p.pin(n, &judging),
                _ => p.undo(n, &judging),
            },
            self.memory_timeout * 3,
        );
        let mut facts = Facts::default();
        let resp = match done {
            Ok(()) => {
                if req.tool == "memory_revert" {
                    facts.change = Some(n);
                } else {
                    facts.ids.push(n);
                }
                let content = match req.tool.as_str() {
                    "memory_forget" => {
                        "forgotten: it is in quarantine, not erased; the person can restore it"
                    }
                    "memory_pin" => "pinned: it comes first in the context",
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

    /// `memory_save` and the other writes, after the gateway let the call through: a session
    /// saves as often as it needs (the memory's hourly quota is the brake on a loop), and the log has to be kept or the call is denied. A save never marks the session: it carries
    /// what the model decided to say, not what a memory said.
    fn propose_call(&self, req: &Request, args: &Map<String, Value>, out: DecisionOut) -> Response {
        // What the model got wrong in its own arguments is refused before anything is counted: it
        // asks neither the vault nor the memory, so it tells a probe nothing (S-61).
        let asked = match asked(req, args) {
            Ok(a) => a,
            Err(why) => return self.memory_failed(req, out, &why),
        };
        let mut facts = Facts::default();
        let resp = self.propose_answer(req, args, asked, out, &mut facts);
        match self.log_facts(req, &facts, 0) {
            Ok(()) => resp,
            Err(why) => self.budget_denied(req, &format!("the memory log cannot be kept: {why}")),
        }
    }

    fn propose_answer(
        &self,
        req: &Request,
        args: &Map<String, Value>,
        asked: Asked,
        out: DecisionOut,
        facts: &mut Facts,
    ) -> Response {
        let Some(port) = self.memory.clone() else {
            return self.memory_failed(req, out, "there is no memory");
        };
        let Asked {
            typed,
            kind,
            category,
            merge,
            target,
        } = asked;
        let text = match self.text_to_keep(&typed) {
            Ok(t) => t,
            Err(why) => return self.memory_failed(req, out, why),
        };
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
        let summary = kind == ProposalKind::Session;
        let draft = ProposalDraft {
            text,
            title,
            category,
            kind,
        };
        let merged_ids = merge.clone().unwrap_or_default();
        let session = req.session.clone();
        let judging = Judging {
            tainted: self.is_tainted(&req.session),
            session: req.session.clone(),
        };
        // Up to three questions to Jev (the guard, the place, a relation) and the writer: a save
        // reported as failed must not be saved afterwards in the background.
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
                Response {
                    id: req.id.clone(),
                    ok: true,
                    content: format!(
                        "merged into m{}: the originals are forgotten (quarantine), not erased",
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
                    Saved::Active => ("saved: it is memory now", "saved"),
                    Saved::Replaced => (
                        "saved: it replaced an earlier memory of the same fact",
                        "replaced",
                    ),
                    Saved::Known => ("already known: nothing was added", "known"),
                };
                // The change number is for undoing it: `memory_revert`.
                let mut content = match done.change {
                    Some(c) if matches!(done.saved, Saved::Active | Saved::Replaced) => {
                        format!(
                            "{content} (m{}, change {c}: memory_revert undoes it)",
                            done.id
                        )
                    }
                    _ => content.to_owned(),
                };
                // Only its number: this answer is not framed as data, so no memory text goes in it.
                if let (Some(twin), false) = (&done.similar, done.saved == Saved::Known) {
                    content.push_str(&format!(
                        " · very similar to m{0}: if it corrects or contradicts it, use memory_update or memory_consolidate (memory_get m{0} shows it)",
                        twin.id
                    ));
                }
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
            // The summary is Clio's own bookkeeping: declined as not worth keeping, there is
            // nothing for the model to report, so it is no failure. An unsafe one stays an error.
            Err(why) if summary && why == NOT_WORTH_REMEMBERING => Response {
                id: req.id.clone(),
                ok: true,
                content: "nothing was saved and nothing needs saving: go on".into(),
                data: json!({ "proposed": false, "result": "declined" }),
                decision: out,
                tainted: self.is_tainted(&req.session),
                external: false,
            },
            Err(why) => self.memory_failed(req, out, &why),
        }
    }

    /// A text to write, cleaned by the vault as typed AND as it will be written (flattened to one
    /// line): a secret with spaces in it must not get through by swapping a space for a newline, a
    /// tab or a no-break space, which flattening puts back. A text that had a secret in it is not
    /// written at all: a memory with a hole where a secret was is useless, and the model had no
    /// business keeping it.
    fn text_to_keep(&self, typed: &str) -> Result<String, &'static str> {
        let flat = one_line(typed);
        let (Some(from_typed), Some(from_flat)) = (self.try_clean(typed), self.try_clean(&flat))
        else {
            return Err("the vault could not filter the text");
        };
        if count_masks(&from_typed) > count_masks(typed)
            || count_masks(&from_flat) > count_masks(&flat)
        {
            return Err("the text had a secret in it: nothing was proposed");
        }
        let text = one_line(&from_typed);
        let chars = text.chars().count();
        if chars == 0 || chars > MAX_PROPOSAL_CHARS {
            return Err(TEXT_SIZE);
        }
        Ok(text)
    }

    /// `memory_block_edit`: the person's words. A clean session writes them at once; a session
    /// that read outside content (`tainted`, `suspect`) sets them aside for the person, who sees
    /// the exact change and approves it (`pending::approve_block`), and the port never hears of it
    /// here; with nobody to ask, it is refused. Its arguments are checked first.
    fn block_edit_call(
        &self,
        req: &Request,
        args: &Map<String, Value>,
        out: DecisionOut,
    ) -> Response {
        let marked = self.is_tainted(&req.session);
        // Where a change waits for the person: host mode, or a chat (where secrets wait too).
        let waits = self
            .host
            .as_ref()
            .map(|h| (h.cfg.state.as_path(), h.cfg.run.as_str()))
            .or_else(|| {
                self.secrets
                    .as_ref()
                    .map(|c| (c.state.as_path(), c.run.as_str()))
            });
        if marked && waits.is_none() {
            return Response {
                id: req.id.clone(),
                ok: false,
                content: BLOCK_TAINTED.into(),
                data: Value::Null,
                decision: DecisionOut {
                    outcome: "deny".into(),
                    rule: "S-05/block".into(),
                    reason: BLOCK_TAINTED.into(),
                },
                tainted: true,
                external: false,
            };
        }
        let name = one_line(args.get("name").and_then(Value::as_str).unwrap_or_default());
        let id = args.get("id").and_then(Value::as_i64).filter(|n| *n > 0);
        let text = args.get("text").and_then(Value::as_str).map(str::to_owned);
        // The model's own mistakes are refused before anything is counted.
        let edit = match (args.get("action").and_then(Value::as_str), id, text) {
            (Some("add"), _, Some(t)) => BlockEdit::Add(t),
            (Some("replace"), Some(id), Some(t)) => BlockEdit::Replace(id, t),
            (Some("remove"), Some(id), _) => BlockEdit::Remove(id),
            (Some("add" | "replace" | "remove"), _, _) => {
                return self.memory_failed(
                    req,
                    out,
                    "add needs text, replace needs id and text, remove needs id",
                );
            }
            _ => return self.memory_failed(req, out, "action must be add, replace or remove"),
        };
        if let BlockEdit::Add(t) | BlockEdit::Replace(_, t) = &edit
            && one_line(t).chars().count() > MAX_PROPOSAL_CHARS
        {
            return self.memory_failed(req, out, TEXT_SIZE);
        }
        // What waits for the person must be exactly what the Approval box shows: a text the
        // display would clean (an invisible or control character) is refused, not cleaned.
        if let BlockEdit::Add(t) | BlockEdit::Replace(_, t) = &edit
            && marked
            && waits.is_some()
            && t.chars()
                .any(|c| (c.is_control() && !c.is_whitespace()) || crate::is_deceptive(c))
        {
            return self.memory_failed(req, out, HIDDEN_CHARS);
        }
        let edit = match edit {
            BlockEdit::Add(t) => self.text_to_keep(&t).map(BlockEdit::Add),
            BlockEdit::Replace(id, t) => self.text_to_keep(&t).map(|t| BlockEdit::Replace(id, t)),
            remove => Ok(remove),
        };
        let edit = match edit {
            Ok(e) => e,
            Err(why) => return self.memory_failed(req, out, why),
        };
        let remove = matches!(edit, BlockEdit::Remove(_));
        let Some(port) = self.memory.clone() else {
            return self.memory_failed(req, out, "there is no memory");
        };
        if let Some(at) = waits.filter(|_| marked) {
            return self.block_aside(req, out, at, &port, name, edit);
        }
        // The port checks the taint too: it gets the session's own.
        let judging = Judging {
            tainted: self.is_tainted(&req.session),
            session: req.session.clone(),
        };
        let block = name.clone();
        let done = self.ask_port_for(
            &port,
            move |p| p.block_edit(&block, &edit, &judging),
            self.memory_timeout * 3,
        );
        let mut facts = Facts::default();
        let resp = match done {
            Ok(it) => {
                facts.ids.push(it.id);
                let content = if remove {
                    format!(
                        "removed from «{}»: m{} is in quarantine, the person can restore it",
                        self.tidy(&name, 40),
                        it.id
                    )
                } else {
                    format!("saved in «{}» as m{}", self.tidy(&name, 40), it.id)
                };
                Response {
                    id: req.id.clone(),
                    ok: true,
                    content,
                    data: json!({ "done": true, "id": it.id }),
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

    /// A marked session's block change, set aside exactly as it would be written (the text
    /// already cleaned) with, for a replace or a remove, the entry's text now: the person sees
    /// both, and the approval writes nothing if that entry changed meanwhile.
    fn block_aside(
        &self,
        req: &Request,
        out: DecisionOut,
        at: (&Path, &str),
        port: &Arc<dyn MemoryPort>,
        name: String,
        edit: BlockEdit,
    ) -> Response {
        let (action, id, text) = match &edit {
            BlockEdit::Add(t) => ("add", None, Some(t.clone())),
            BlockEdit::Replace(n, t) => ("replace", Some(*n), Some(t.clone())),
            BlockEdit::Remove(n) => ("remove", Some(*n), None),
        };
        let mut args = json!({ "name": name, "action": action });
        // A block that does not exist is the model's mistake: nothing waits for it.
        if !BLOCK_NAMES.contains(&name.as_str()) {
            return self.memory_failed(
                req,
                out,
                &format!(
                    "there is no block «{}»: {}",
                    self.tidy(&name, 40),
                    BLOCK_NAMES.join(", ")
                ),
            );
        }
        if let Some(n) = id {
            let block = name.clone();
            let entries = match self.ask_port(port, move |p| p.block(&block)) {
                Ok(e) => e,
                Err(why) => return self.memory_failed(req, out, &why),
            };
            let Some(old) = entries.into_iter().find(|e| e.id == n) else {
                return self.memory_failed(
                    req,
                    out,
                    &format!("m{n} is not an entry of «{}»", self.tidy(&name, 40)),
                );
            };
            args["id"] = n.into();
            args["old"] = old.text.into();
        }
        if let Some(t) = text {
            args["text"] = t.into();
        }
        self.aside_at(at, req, args, (crate::ask::Class::Memory, None), &|| false)
    }

    fn open_log(&self) -> Result<std::fs::File, String> {
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(self.run_dir.join("memory.jsonl"))
            .map_err(|e| e.to_string())
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
            "near": facts.near,
            "change": facts.change,
            "bytes": bytes,
            "tainted": facts.marked,
            "query_hash": facts.query_hash,
        });
        let mut f = self.open_log()?;
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
        } else if req.tool == "memory_block" {
            let name = one_line(args.get("name").and_then(Value::as_str).unwrap_or_default());
            self.ask_port(&port, move |p| p.block(&name))
                .map(|items| Found {
                    items,
                    abstained: false,
                    note: None,
                })
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
        // Last night's sleep goes first, outside the frame: the sleep's own words, cleaned.
        let morning = (req.tool == "memory_context")
            .then(|| port.morning())
            .flatten()
            .map(|l| format!("{}\n", self.tidy(&l, MAX_MORNING_CHARS)))
            .unwrap_or_default();

        // The session is marked for what the memory returned, shown or not: a memory read in full
        // also returns its neighbors' words (a search, only their numbers).
        let previews = req.tool == "memory_get";
        if found.items.iter().any(|i| {
            i.origin != Origin::Person
                || (previews && i.near.iter().any(|n| n.origin != Origin::Person))
        }) {
            self.mark(&req.session, "memory");
            facts.marked = true;
        }
        let note = found.note.as_deref().map(|n| self.tidy(n, MAX_NOTE_CHARS));
        if found.items.is_empty() {
            let mut content = format!("{morning}nothing relevant in memory");
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

        // Saturating all the way: a port's note can be longer than `cap`, and a wrapped `room`
        // would lift it.
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
        let mut ids_after: Vec<(usize, String)> = Vec::new();
        let (mut lines, mut ids, mut masked, mut used) = (Vec::new(), Vec::new(), 0, 0);
        // A memory read on its own is shown whole (within the cap); a list shows previews.
        let max_line = if matches!(req.tool.as_str(), "memory_get" | "memory_block") {
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
            if !previews && !it.near.is_empty() {
                let near: Vec<String> = it
                    .near
                    .iter()
                    .take(NEAR_SEARCH)
                    .map(|n| format!("m{}", n.id))
                    .collect();
                ids_after.push((lines.len(), format!(" · cerca: {}", near.join(", "))));
            }
            used += line.len() + 1;
            // Only memories that are shown: one left out was never in front of the model.
            masked += count_masks(&cleaned).saturating_sub(count_masks(&body));
            ids.push(it.id);
            lines.push(line);
            if previews {
                // After the memory, in what room is left: fewer neighbors, never less of it.
                let named: Vec<String> = it
                    .near
                    .iter()
                    .take(NEAR_GET)
                    .map(|n| {
                        let outside = if n.origin == Origin::External {
                            " (externo)"
                        } else {
                            ""
                        };
                        format!(
                            "m{}{outside} «{}»",
                            n.id,
                            self.tidy(&n.preview, PREVIEW_CHARS)
                        )
                    })
                    .collect();
                for k in (1..=named.len()).rev() {
                    let near_line = format!("Cerca: {}", named[..k].join(" · "));
                    if used + near_line.len() < room {
                        used += near_line.len() + 1;
                        lines.push(near_line);
                        for n in it.near.iter().take(k) {
                            facts.near.push(n.id);
                            masked += count_masks(&self.clean(&n.preview))
                                .saturating_sub(count_masks(&n.preview));
                        }
                        break;
                    }
                }
            }
        }
        // Neighbor ids go in the room the results left: they never cost a result.
        for (at, near) in ids_after {
            if used + near.len() <= room {
                used += near.len();
                lines[at].push_str(&near);
            }
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
            content: format!(
                "{morning}{}",
                wrap_untrusted(Source::Memory, &lines.join("\n"))
            ),
            data: json!({ "ids": ids, "count": ids.len(), "masked": masked }),
            decision: out,
            tainted: self.is_tainted(&req.session),
            external: true,
        }
    }
}
