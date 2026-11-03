//! The memory tools (`memory_search`, `memory_get`): the agent reads the person's active memories,
//! read only. The memory itself sits behind [`MemoryPort`] (the real one lives in `clio-brain`),
//! so this crate never opens a database.
//!
//! What comes back is text somebody wrote, maybe long ago, maybe hostile. It is cleaned first,
//! cut after cleaning (so a secret is never split by the cut), kept to one line per memory (a
//! memory cannot forge the line of another one) and wrapped as data (S-08, S-60). The session is
//! marked as tainted only when a memory is not the person's own (S-05): marking on every read
//! would tire the person and be ignored, never marking would make S-05 mean nothing.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use clio_core::Effect;
use clio_gateway::{ArgKind, ArgSpec, Egress, Source, ToolDecl, wrap_untrusted};
use clio_vault::count_masks;
use serde_json::{Map, Value, json};

use crate::{DecisionOut, Request, Response, ToolHost, digest_hex};

pub(crate) const MEMORY_OPS: [&str; 2] = ["memory_search", "memory_get"];

const SEARCH_DESCRIPTION: &str = "Search the person's memory (active memories only, read-only)";
const GET_DESCRIPTION: &str = "Read one active memory of the person by id (read-only)";

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
}

#[derive(Debug, Clone)]
pub struct Found {
    pub items: Vec<MemoryItem>,
    /// The memory judged that nothing was relevant enough.
    pub abstained: bool,
    /// Something the model should know about how the search went (e.g. text only, no vectors).
    pub note: Option<String>,
}

/// What the tool host needs from the memory: active memories of the person's scope, read only.
/// There is no way to write, promote, archive or purge through it (S-60).
pub trait MemoryPort: Send + Sync {
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
        _ => vec![a("id", ArgKind::Int, true)],
    }
}

/// The declarations for the gateway, and each one's description hash (S-15).
pub(crate) fn decls() -> Vec<ToolDecl> {
    [
        ("memory_search", SEARCH_DESCRIPTION),
        ("memory_get", GET_DESCRIPTION),
    ]
    .into_iter()
    .map(|(name, description)| ToolDecl {
        name: name.into(),
        version: "1".into(),
        effect: Effect::Read,
        egress: Egress::None,
        description_hash: digest_hex(description),
        args: arg_specs(name),
    })
    .collect()
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
        let (tx, rx) = mpsc::channel();
        let port = Arc::clone(port);
        std::thread::spawn(move || {
            let ran = catch_unwind(AssertUnwindSafe(|| f(port.as_ref())));
            let _ = tx.send(ran.unwrap_or_else(|_| Err("the memory tool panicked".into())));
        });
        rx.recv_timeout(self.memory_timeout)
            .unwrap_or_else(|_| Err("the memory did not answer in time".into()))
    }

    /// `memory_search` / `memory_get`, after the gateway let the call through. Memory does not go
    /// through the executor: nothing here touches the working copy.
    pub(crate) fn memory_call(
        &self,
        req: &Request,
        args: &Map<String, Value>,
        out: DecisionOut,
    ) -> Response {
        let Some(port) = self.memory.clone() else {
            return self.memory_failed(req, out, "there is no memory");
        };
        let found = if req.tool == "memory_search" {
            let query = self.tidy(
                args.get("query")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                MAX_QUERY_CHARS,
            );
            if query.is_empty() {
                return self.memory_failed(req, out, "the query is empty");
            }
            let limit = args.get("limit").map_or(DEFAULT_LIMIT, |v| {
                let n = v.as_i64().unwrap_or(i64::MAX);
                usize::try_from(n.clamp(1, MAX_LIMIT as i64)).unwrap_or(MAX_LIMIT)
            });
            self.ask_port(&port, move |p| p.search(&query, limit))
        } else {
            // The gateway checked it is an integer; a u64 past i64::MAX is not an id.
            let Some(id) = args.get("id").and_then(Value::as_i64).filter(|id| *id > 0) else {
                return self.memory_failed(req, out, "the id must be a positive integer");
            };
            self.ask_port(&port, move |p| p.get(id)).map(|item| Found {
                items: vec![item],
                abstained: false,
                note: None,
            })
        };
        let found = match found {
            Ok(f) => f,
            Err(why) => return self.memory_failed(req, out, &why),
        };

        // The session is marked for what the memory returned, shown or not.
        if found.items.iter().any(|i| i.origin != Origin::Person) {
            self.mark(&req.session, "memory");
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

        let room = MAX_ANSWER_BYTES
            - wrap_untrusted(Source::Memory, "").len()
            - OMITTED_ROOM
            - note.as_ref().map_or(0, |n| n.len() + 8);
        let (mut lines, mut ids, mut masked, mut used) = (Vec::new(), Vec::new(), 0, 0);
        for it in &found.items {
            let (kind, date) = (self.tidy(&it.kind, 32), self.tidy(&it.date, 32));
            let text = self.clean(&it.text);
            masked += count_masks(&text).saturating_sub(count_masks(&it.text));
            let head = format!("[m{}] {kind} · {} · {date}: ", it.id, it.origin.label());
            let text = cut(
                &one_line(&text),
                MAX_LINE_CHARS.saturating_sub(head.chars().count()),
            );
            let line = format!("{head}{text}");
            if used + line.len() + 1 > room {
                break;
            }
            used += line.len() + 1;
            ids.push(it.id);
            lines.push(line);
        }
        let left = found.items.len() - ids.len();
        if left > 0 {
            lines.push(format!(
                "[+{left} more memories left out: the answer is limited to {MAX_ANSWER_BYTES} bytes]"
            ));
        }
        if let Some(n) = note.filter(|n| !n.is_empty()) {
            lines.push(format!("[note] {n}"));
        }
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
