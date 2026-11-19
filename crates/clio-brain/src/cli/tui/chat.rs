//! The chat over a Pi session: the conversation, what the agent does as it does it (text as it
//! streams, one compact line per tool), the field to write in, and, with debug on (Ctrl-D), the
//! raw events Pi sends. Use and context are in the status bar. Everything Pi, the model or a tool
//! said is stored as it came and sanitized when drawn.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};

use clio_brain::pi::rpc::{Event, UsageMeter};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use serde_json::Value;

use super::app::{App, Cmd, OpenTarget, Screen, clean, section};
use super::keys::{Act, lookup};
use super::text::{self, wrap, wrap_ranges};
use super::theme::Theme;
use super::{composer, markdown, sanitize};
use crate::cli::chat::Refresh;
use crate::cli::usage::{self, RunLimits, Snapshot};

/// Raw events kept for the debug pane.
const RAW_KEEP: usize = 300;
/// Bytes kept of each event in the debug log.
const RAW_CLIP: usize = 1024;
/// Bytes of text kept per conversation item: what Pi streams is not trusted to be small.
const MAX_TEXT: usize = 1 << 20;
/// Bytes kept of an error or a note.
const MAX_NOTE: usize = 4096;
/// Text kept from what Pi streams, in all: past it the conversation stops growing.
const MAX_HELD: usize = 16 << 20;
/// Items kept: the oldest go first.
const MAX_ITEMS: usize = 20_000;

/// `{e:?}` cut at `RAW_CLIP` bytes. It stops writing at the limit instead of building the whole
/// text first: an event can be tens of megabytes.
fn debug_clip(e: &Event) -> String {
    use std::fmt::Write;
    struct Clip(String);
    impl Write for Clip {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            let room = RAW_CLIP.saturating_sub(self.0.len());
            if s.len() <= room {
                self.0.push_str(s);
                return Ok(());
            }
            let mut end = room;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            self.0.push_str(&s[..end]);
            self.0.push('…');
            Err(std::fmt::Error)
        }
    }
    let mut w = Clip(String::new());
    let _ = write!(w, "{e:?}");
    w.0
}

/// Adds `more` to a conversation item, up to `MAX_TEXT` for the item and `left` for the whole
/// conversation, and says once that the rest was dropped. Returns the bytes added.
fn append_capped(text: &mut String, more: &str, left: usize) -> usize {
    const MARK: &str = "\n[… the rest was left out]";
    let room = MAX_TEXT.saturating_sub(text.len()).min(left);
    if more.len() <= room {
        text.push_str(more);
        return more.len();
    }
    let mut end = room;
    while !more.is_char_boundary(end) {
        end -= 1;
    }
    text.push_str(&more[..end]);
    if !text.ends_with(MARK) {
        text.push_str(MARK);
    }
    end
}

/// `s` cut at `max` bytes, on a character boundary.
fn clip_to(s: &str, max: usize) -> String {
    let mut text = String::new();
    if s.len() <= max {
        s.clone_into(&mut text);
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        text.push_str(&s[..end]);
        text.push('…');
    }
    text
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Ok,
    Failed,
    /// The guard or the gateway said no.
    Denied,
    /// Host mode: set aside for the person's approval (`set aside <id>: …`).
    SetAside,
    /// Still running when its turn settled or was aborted: it will not end on its own.
    Stopped,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    User(String),
    /// The agent's text; `live` while it streams.
    Assistant {
        text: String,
        live: bool,
    },
    Tool {
        id: String,
        name: String,
        summary: String,
        /// The model's own few words on why (the step's title), if it gave them.
        reason: String,
        state: ToolState,
        ms: Option<u64>,
    },
    /// Compaction, retries: one quiet line.
    Note(String),
    Error(String),
}

pub struct Chat {
    /// Which `Cmd::ChatOpen` this chat is waiting for.
    pub seq: u64,
    pub opening: bool,
    /// Why it could not open.
    pub failed: Option<String>,
    pub session_id: String,
    pub workspace: String,
    pub title: String,
    pub items: Vec<Item>,
    pub input: String,
    /// Where the keys write in `input`: a byte position (see `composer::clamp`).
    pub cursor: usize,
    pub running: bool,
    /// When the turn on screen started.
    pub turn_ms: u64,
    pub aborting: bool,
    pub meter: UsageMeter,
    /// Model calls (turns) seen since the chat opened.
    pub turns: u32,
    pub started_ms: u64,
    pub tainted: bool,
    /// How many of the jail's refused connections the chat has already said.
    egress_seen: usize,
    /// The total of struck-out values the chat has already said.
    masked_seen: u32,
    /// How many of the judge's withheld results the chat has already said.
    withheld_seen: usize,
    /// The chat said the judge did not answer (once per session).
    judge_said: bool,
    pub raw: VecDeque<String>,
    /// Bytes of text taken from Pi's events so far (see `MAX_HELD`).
    held: usize,
    /// Lines scrolled up from the newest.
    pub scroll: usize,
    /// How far up the last draw let it go.
    pub scroll_max: Cell<usize>,
    pub view_h: Cell<u16>,
    pub view_w: Cell<u16>,
    /// The columns of the composer's text, at the last draw (for ↑ and ↓ in it).
    pub input_w: Cell<u16>,
    /// The message sent while the session opens or compacts, sent once it can be.
    pub queued: Option<String>,
    /// The slash commands' menu over the composer, while it is open.
    pub menu: Option<super::commands::Menu>,
    /// The person's own commands, as read when the menu last opened.
    pub customs: Vec<super::commands::Custom>,
    /// `/compact` runs: nothing is sent until it ends.
    pub compacting: bool,
    /// The compaction running is Clio's own, the context being over the cap.
    pub auto_compact: bool,
    /// "Still over the cap" was said: not again until the context is back under it.
    pub over_cap_said: bool,
    /// The model's checklist (`todo_write`), pinned above the status line.
    pub todo: Vec<super::todo::Item>,
    /// Host mode: commands running in the background, as of the last turn's end.
    // shortcut: counted when a turn ends, so one that ends between turns still shows; poll if that misleads.
    pub jobs: usize,
    streamed: bool,
    /// The text items as the last draw rendered them, by their place in the whole conversation
    /// (`dropped` + index), with the fingerprint of what went into them: parsing a long
    /// conversation's markdown on every frame is slow. Cleared when an item is taken out.
    pub(super) rendered: RefCell<HashMap<usize, (u64, Vec<Line<'static>>)>>,
    /// Items dropped from the front (past `MAX_ITEMS`) so far.
    dropped: usize,
    /// The Pi session file of a session opened from the list: its history is read from there.
    pub file: Option<String>,
    /// What is shown of the file starts at this line, and the file has messages before it.
    pub earlier: Option<usize>,
    /// The page before is being read.
    pub loading_earlier: bool,
    /// The history came from the file: Pi's own copy (`get_messages`) is not drawn.
    pub from_file: bool,
    /// `clio` is reopening the last conversation by itself: if it cannot, a draft.
    pub resume: bool,
    /// How many times an answer's markdown was parsed (a test counts it).
    pub(super) parsed: Cell<usize>,
    /// The reviews of this session.
    pub review: super::review::Review,
    /// What of this session waits for the person, and when Clio asks.
    pub approvals: super::approvals::Approvals,
}

impl Default for Chat {
    fn default() -> Self {
        Self::opening(0, 0)
    }
}

impl Chat {
    pub fn opening(seq: u64, now_ms: u64) -> Self {
        Self {
            seq,
            opening: true,
            failed: None,
            session_id: String::new(),
            workspace: String::new(),
            title: String::new(),
            items: vec![],
            input: String::new(),
            cursor: 0,
            running: false,
            turn_ms: now_ms,
            aborting: false,
            meter: UsageMeter::default(),
            turns: 0,
            started_ms: now_ms,
            tainted: false,
            egress_seen: 0,
            masked_seen: 0,
            withheld_seen: 0,
            judge_said: false,
            raw: VecDeque::new(),
            held: 0,
            scroll: 0,
            scroll_max: Cell::new(0),
            view_h: Cell::new(0),
            view_w: Cell::new(0),
            input_w: Cell::new(0),
            queued: None,
            menu: None,
            customs: vec![],
            compacting: false,
            auto_compact: false,
            over_cap_said: false,
            todo: vec![],
            jobs: 0,
            streamed: false,
            rendered: RefCell::default(),
            dropped: 0,
            file: None,
            earlier: None,
            loading_earlier: false,
            from_file: false,
            resume: false,
            parsed: Cell::new(0),
            review: super::review::Review::default(),
            approvals: super::approvals::Approvals::default(),
        }
    }

    /// A new session that does not exist yet: its first message creates it.
    pub fn draft(now_ms: u64) -> Self {
        Self {
            opening: false,
            title: "new session".into(),
            ..Self::opening(0, now_ms)
        }
    }

    /// A turn starts now.
    pub fn start_turn(&mut self, now_ms: u64) {
        // Any turn gives the model what came of the approvals, and an ended window's record.
        self.approvals.answered = false;
        self.approvals.summary = false;
        self.running = true;
        self.aborting = false;
        self.turn_ms = now_ms;
    }

    /// Puts `text` in the composer, the cursor at its end.
    pub fn set_input(&mut self, text: String) {
        self.cursor = text.len();
        self.input = text;
    }

    /// A message that could not go out is back in the composer, in front of anything typed since,
    /// and out of the conversation.
    pub fn unsend(&mut self, text: String) {
        self.items.retain(|i| *i != Item::User(text.clone()));
        self.rendered.take();
        let typed = std::mem::take(&mut self.input);
        self.set_input(if typed.is_empty() {
            text
        } else {
            format!("{text}\n{typed}")
        });
    }

    /// Scrolls the conversation `n` lines up (negative: down), within what the last draw allowed.
    pub fn scroll_by(&mut self, n: isize) {
        self.scroll = self
            .scroll
            .saturating_add_signed(n)
            .min(self.scroll_max.get());
    }

    pub fn is_draft(&self) -> bool {
        !self.opening && self.failed.is_none() && self.session_id.is_empty()
    }

    /// The session is open: its history, and Pi's totals if it gave them.
    pub fn opened(
        &mut self,
        id: String,
        workspace: String,
        history: &[Value],
        refresh: Option<Refresh>,
        now_ms: u64,
    ) {
        self.opening = false;
        self.session_id = id;
        self.workspace = workspace;
        self.started_ms = now_ms;
        // In front of what is already there (a message sent while it opened).
        let rest = std::mem::take(&mut self.items);
        for m in history {
            self.add_history(m);
        }
        self.items.extend(rest);
        self.rendered.take();
        if let Some(r) = refresh {
            self.refresh(&r);
        }
    }

    /// A page of the session's file in front of what is shown: its messages through the same
    /// conversion as Pi's, a quiet line where the file has a compaction, and one on top while the
    /// file has messages before the page (`earlier`, the line the page starts at).
    pub fn add_page(&mut self, entries: &[Value], earlier: Option<usize>) {
        if self.items.first() == Some(&Item::Note(EARLIER.into())) {
            self.items.remove(0);
        }
        let rest = std::mem::take(&mut self.items);
        if earlier.is_some() {
            self.items.push(Item::Note(EARLIER.into()));
        }
        for e in entries {
            match e["type"].as_str() {
                Some("message") => self.add_history(&e["message"]),
                Some("compaction") => self.items.push(Item::Note(SUMMARISED.into())),
                _ => {}
            }
        }
        self.items.extend(rest);
        self.earlier = earlier;
        self.from_file = true;
        // The items moved: what the last draw rendered is by place.
        self.rendered.take();
    }

    fn add_history(&mut self, m: &Value) {
        let text = content_text(&m["content"]);
        match m["role"].as_str() {
            Some("user") if !text.is_empty() => {
                // What the person sees is what they typed: the brain's envelope stays out.
                let typed = clio_brain::pi::envelope::strip(&text);
                // A turn the review started has no words of the person: only its findings.
                if typed.trim_start().starts_with(super::review::FINDINGS_MARK) {
                    self.items
                        .push(Item::Note(super::review::REVIEW_TURN.into()));
                } else {
                    // What came of the approvals rode with it: the quiet lines it had live.
                    let (typed, outcomes) = super::approvals::outcomes(typed);
                    let started = typed.is_empty() && !outcomes.is_empty();
                    self.items.extend(outcomes.into_iter().map(Item::Note));
                    if started {
                        self.items
                            .push(Item::Note(super::approvals::APPROVALS_TURN.into()));
                    } else if !typed.is_empty() {
                        self.items.push(Item::User(typed.to_owned()));
                    }
                }
            }
            Some("assistant") => {
                if !text.is_empty() {
                    self.items.push(Item::Assistant { text, live: false });
                }
                for b in m["content"].as_array().into_iter().flatten() {
                    if b["type"] == "toolCall" {
                        self.items.push(Item::Tool {
                            id: b["id"].as_str().unwrap_or("").to_owned(),
                            name: b["name"].as_str().unwrap_or("?").to_owned(),
                            summary: summarize(b["name"].as_str().unwrap_or(""), &b["arguments"]),
                            reason: reason_of(&b["arguments"]),
                            state: ToolState::Ok,
                            ms: None,
                        });
                    }
                }
            }
            Some("toolResult") => {
                let id = m["toolCallId"].as_str().unwrap_or("");
                let state = state_of(
                    m["toolName"].as_str().unwrap_or(""),
                    m["isError"] == true,
                    &text,
                );
                if let Some(Item::Tool { state: s, .. }) = self
                    .items
                    .iter_mut()
                    .rev()
                    .find(|i| matches!(i, Item::Tool { id: x, .. } if x == id))
                {
                    *s = state;
                }
            }
            _ => {}
        }
    }

    pub fn refresh(&mut self, r: &Refresh) {
        self.meter.refresh(&r.stats);
        // No chat line: the `externo` mark in the status bar is enough.
        self.tainted |= r.tainted;
        self.egress(&r.egress_denied);
        self.masked(r.masked);
        self.judged(&r.withheld, r.judge_silent);
        self.jobs = r.jobs;
    }

    /// The judge withheld outside text from the model, or did not answer: say it quietly, with
    /// where to read what was withheld. `withheld` is the whole session's list so far.
    pub fn judged(&mut self, withheld: &[super::super::chat::Withheld], silent: bool) {
        for w in withheld.iter().skip(self.withheld_seen) {
            let read = w.file.as_deref().map_or_else(
                || " (it could not be kept)".to_owned(),
                |f| format!(": read it with less {f}"),
            );
            self.items.push(Item::Note(format!(
                "the judge flagged {} bytes from {} {} as instructions for an AI; the model did not see them{read}",
                w.bytes, w.tool, w.source
            )));
        }
        self.withheld_seen = self.withheld_seen.max(withheld.len());
        if silent && !self.judge_said {
            self.judge_said = true;
            self.items.push(Item::Note(
                "the judge did not answer: outside text reached the model unjudged, marked as data"
                    .into(),
            ));
        }
    }

    /// The host struck values out of what Pi read (S-18): say how many are new, quietly. The
    /// person supervises here; `total` is the whole session's count so far.
    pub fn masked(&mut self, total: u32) {
        if total > self.masked_seen {
            let n = total - self.masked_seen;
            let (one, many) = ("value", "values");
            self.items.push(Item::Note(format!(
                "Clio redacted {n} key-shaped {} in what Pi read: the model does not see them",
                if n == 1 { one } else { many }
            )));
        }
        self.masked_seen = self.masked_seen.max(total);
    }

    /// Pi's jail refused a connection (ADR 0009): say it once, as a quiet note. The person
    /// supervises here, and a refused destination is exactly what is worth seeing. `denied` is
    /// the whole list so far; only what is new since the last call is added.
    pub fn egress(&mut self, denied: &[String]) {
        for d in denied.iter().skip(self.egress_seen) {
            self.items.push(Item::Note(format!(
                "the jail refused a connection from Pi to {d}"
            )));
        }
        self.egress_seen = denied.len();
    }

    fn last_live(&mut self) -> Option<&mut String> {
        match self.items.last_mut() {
            Some(Item::Assistant { text, live: true }) => Some(text),
            _ => None,
        }
    }

    /// The turn is over: a tool still drawn as running did not end, it was stopped; a checklist
    /// with every item done is not pinned any more.
    pub fn stop_tools(&mut self) {
        if self
            .todo
            .iter()
            .all(|(s, _)| *s == super::todo::Status::Done)
        {
            self.todo.clear();
        }
        for i in &mut self.items {
            if let Item::Tool { state, .. } = i
                && *state == ToolState::Running
            {
                *state = ToolState::Stopped;
            }
        }
    }

    pub fn on_event(&mut self, e: &Event) {
        let (n0, left) = (self.items.len(), MAX_HELD.saturating_sub(self.held));
        let note = |s: &str| clip_to(s, MAX_NOTE.min(left));
        self.meter.observe(e);
        self.raw.push_back(debug_clip(e));
        if self.raw.len() > RAW_KEEP {
            self.raw.pop_front();
        }
        match e {
            Event::AgentStart => self.running = true,
            Event::AgentSettled { .. } => {
                self.running = false;
                self.stop_tools();
            }
            Event::TurnEnd => self.turns += 1,
            Event::MessageStart(m) if m.role == "assistant" => self.streamed = false,
            Event::MessageUpdate {
                kind,
                delta: Some(d),
                ..
            } if kind == "text_delta" => {
                self.streamed = true;
                // A new item is paid for at the end of the event; only an append is paid here.
                match self.last_live() {
                    Some(t) => self.held += append_capped(t, d, left),
                    None => {
                        let mut text = String::new();
                        append_capped(&mut text, d, left);
                        self.items.push(Item::Assistant { text, live: true });
                    }
                }
            }
            Event::MessageEnd(m) if m.role == "assistant" => {
                if self.streamed {
                    if let Some(Item::Assistant { live, .. }) = self.items.last_mut() {
                        *live = false;
                    }
                } else if !m.text.is_empty() {
                    let mut text = String::new();
                    append_capped(&mut text, &m.text, left);
                    self.items.push(Item::Assistant { text, live: false });
                }
                if m.stop_reason.as_deref() == Some("error") {
                    self.items.push(Item::Error(
                        m.error_message
                            .as_deref()
                            .map_or_else(|| "the model returned an error".into(), note),
                    ));
                }
            }
            Event::ToolStart { id, name, args } => self.items.push(Item::Tool {
                id: id.clone(),
                name: name.clone(),
                summary: summarize(name, args),
                reason: reason_of(args),
                state: ToolState::Running,
                ms: None,
            }),
            Event::ToolEnd {
                id,
                name,
                result,
                is_error,
                duration_ms,
            } => {
                let state = state_of(name, *is_error, &content_text(&result["content"]));
                if let Some(Item::Tool { state: s, ms, .. }) = self
                    .items
                    .iter_mut()
                    .rev()
                    .find(|i| matches!(i, Item::Tool { id: x, .. } if x == id))
                {
                    *s = state;
                    *ms = *duration_ms;
                }
            }
            Event::CompactionStart { reason } => self
                .items
                .push(Item::Note(format!("compacting the context ({reason})…"))),
            Event::CompactionEnd {
                aborted,
                error,
                tokens_before,
                tokens_after,
                ..
            } => self.items.push(Item::Note(match (aborted, error) {
                (_, Some(e)) => format!("compaction failed: {}", note(e)),
                (true, _) => "compaction cancelled".into(),
                _ => format!(
                    "context compacted{}",
                    match (tokens_before, tokens_after) {
                        (Some(a), Some(b)) =>
                            format!(": {} → {} tok", usage::tokens(*a), usage::tokens(*b)),
                        _ => String::new(),
                    }
                ),
            })),
            Event::AutoRetryStart {
                attempt,
                max_attempts,
                delay_ms,
                error,
            } => self.items.push(Item::Note(format!(
                "retry {attempt}/{max_attempts} in {} ({})",
                usage::ms_label(*delay_ms),
                note(error)
            ))),
            Event::AutoRetryEnd {
                success: false,
                final_error,
                ..
            } => self.items.push(Item::Error(format!(
                "the retries were not enough: {}",
                note(final_error.as_deref().unwrap_or_default())
            ))),
            Event::ExtensionError { error, .. } => self
                .items
                .push(Item::Note(format!("extension warning: {}", note(error)))),
            // Only the checklist's widget: any other key is not drawn.
            Event::ExtensionUiRequest {
                method,
                widget_key,
                widget_lines,
                ..
            } if method == "setWidget" && widget_key == super::todo::KEY => {
                self.todo = super::todo::parse(widget_lines.as_deref().unwrap_or_default());
            }
            _ => {}
        }
        // What this event added is paid from the budget; the oldest items go past the cap.
        self.held += self.items[n0.min(self.items.len())..]
            .iter()
            .map(|i| match i {
                Item::Assistant { text, .. } | Item::Note(text) | Item::Error(text) => text.len(),
                _ => 0,
            })
            .sum::<usize>();
        if self.items.len() > MAX_ITEMS {
            let extra = self.items.len() - MAX_ITEMS;
            self.items.drain(..extra);
            self.dropped += extra;
            // A gap would open between what is shown and the page before it.
            self.earlier = None;
        }
    }

    pub fn snapshot(&self, now_ms: u64, fallback: (u64, bool, RunLimits)) -> Snapshot {
        let t = self.meter.total();
        let c = self.meter.context;
        let window = c.map(|c| c.window).filter(|w| *w > 0);
        Snapshot {
            tokens_in: t.input + t.cache_read,
            tokens_out: t.output,
            cost_micro: (t.cost * 1e6).round().max(0.0) as u64,
            steps: self.turns,
            secs: now_ms.saturating_sub(self.started_ms) / 1000,
            limits: RunLimits::default(),
            ctx: c.and_then(|c| c.tokens),
            window: window.unwrap_or(fallback.0),
            window_known: window.is_some() || fallback.1,
            // Not shown: nearly every useful session reads a file, so the mark would always be on.
            tainted: false,
        }
    }
}

/// The quiet line on top while the session's file has earlier messages.
pub const EARLIER: &str = "earlier messages · scroll up";
/// The quiet line where the file has a compaction.
pub const SUMMARISED: &str = "earlier messages were summarised here";

/// At the very top of what is shown: the page before it, read from the session's file.
pub fn earlier(app: &mut App) -> Vec<Cmd> {
    let c = &mut app.chat;
    match (&c.file, c.earlier) {
        (Some(file), Some(before)) if !c.loading_earlier && c.scroll >= c.scroll_max.get() => {
            c.loading_earlier = true;
            vec![Cmd::LoadHistory {
                seq: c.seq,
                file: file.clone(),
                before,
            }]
        }
        _ => vec![],
    }
}

/// The text blocks of a Pi message content (a string, or blocks).
pub fn content_text(c: &Value) -> String {
    match c {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// A tool that failed because the guard or the gateway refused it is "denied", not "failed"; one the
/// toolhost set aside for the person (`clio_toolhost::pending`) is "set aside". Only for drawing:
/// nothing ever decides on this state. Only the wrapped answer counts, except for
/// `memory_block_edit`, whose answer comes from the tool host bare (a memory result is never
/// wrapped): a bare "set aside" from any other tool is a failure, so its output cannot fake one.
fn state_of(tool: &str, is_error: bool, text: &str) -> ToolState {
    let t = text.to_lowercase();
    if !is_error {
        ToolState::Ok
    } else if unwrapped(text).is_some_and(set_aside)
        || (tool == "memory_block_edit" && set_aside(text) && text.contains(": «memory» "))
    {
        ToolState::SetAside
    } else if t.contains("clio-guard:") || t.contains("denied") || t.contains("denegad") {
        ToolState::Denied
    } else {
        ToolState::Failed
    }
}

/// What is inside the host's wrapper of a tool's result (`clio_gateway::wrap_untrusted`, as a
/// tool result or a file), when the text is exactly that wrapper around it, hash included.
fn unwrapped(text: &str) -> Option<&str> {
    use clio_gateway::{Source, wrap_untrusted};
    let (_, rest) = text.split_once('\n')?;
    let (inner, _) = rest.rsplit_once('\n')?;
    [Source::ToolResult, Source::File]
        .into_iter()
        .any(|s| wrap_untrusted(s, inner) == text)
        .then_some(inner)
}

/// The toolhost's answer for an action set aside: `set aside <hex id>: «<class>» …`, or
/// `apartado <hex id>: «<class>» …` as older versions said it (old sessions are reopened).
fn set_aside(text: &str) -> bool {
    let Some((id, rest)) = text
        .strip_prefix("set aside ")
        .or_else(|| text.strip_prefix("apartado "))
        .and_then(|r| r.split_once(": «"))
    else {
        return false;
    };
    let class = rest.split_once('»').map_or("", |(c, _)| c);
    !id.is_empty()
        && id.bytes().all(|b| b.is_ascii_hexdigit())
        && !class.is_empty()
        && class.chars().all(char::is_alphabetic)
}

/// The model's own words on why it uses a tool (`reason`), one line.
fn reason_of(args: &Value) -> String {
    args["reason"]
        .as_str()
        .unwrap_or("")
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_owned()
}

/// What a tool was asked, in a few words.
pub fn summarize(name: &str, args: &Value) -> String {
    let pick = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| args[*k].as_str())
            .map(str::to_owned)
    };
    let s = match name {
        "bash" => pick(&["command"]),
        "read" | "write" | "edit" | "ls" => pick(&["path"]),
        "find" => pick(&["pattern", "path"]),
        _ => None,
    }
    .unwrap_or_else(|| {
        let j = args.to_string();
        if j == "null" || j == "{}" {
            String::new()
        } else {
            j
        }
    });
    s.lines().next().unwrap_or("").to_owned()
}

pub fn key(app: &mut App, k: &KeyEvent) -> Vec<Cmd> {
    if let Some(cmds) = super::commands::key(app, k) {
        return cmds;
    }
    let before = app.chat.input.clone();
    let cmds = key_composer(app, k);
    super::commands::sync(app, &before);
    cmds
}

fn key_composer(app: &mut App, k: &KeyEvent) -> Vec<Cmd> {
    // Esc stops an automatic compaction (a `/compact` the person asked for is waited for).
    let c = &mut app.chat;
    if k.code == KeyCode::Esc && c.compacting && c.auto_compact && !c.running && !c.aborting {
        c.aborting = true;
        c.items
            .push(Item::Note("stopping the automatic compaction…".into()));
        return vec![Cmd::CompactAbort];
    }
    let ctx = app.ctx();
    let Some(act) = lookup(ctx, k) else {
        return edit(app, k);
    };
    match act {
        Act::Quit => {
            if !app.busy_to_quit() {
                app.quit = true;
            }
            return vec![];
        }
        Act::Sessions => {
            app.screen = Screen::Sessions;
            return vec![];
        }
        Act::Help => {
            app.open_help();
            return vec![];
        }
        Act::Open => return send(app),
        _ => {}
    }
    let page = isize::try_from(app.chat.view_h.get().max(2) - 1).unwrap_or(1);
    let w = usize::from(app.chat.input_w.get());
    let c = &mut app.chat;
    if act == Act::Cancel && !c.aborting {
        c.aborting = true;
        c.items.push(Item::Note("aborting the turn…".into()));
        return vec![Cmd::ChatAbort];
    }
    let (s, at) = (&mut c.input, &mut c.cursor);
    match act {
        Act::Clear => composer::delete_to_line_start(s, at),
        Act::Newline => composer::insert(s, at, "\n"),
        Act::Left => *at = composer::left(s, *at),
        Act::Right => *at = composer::right(s, *at),
        Act::LineStart => *at = composer::line_start(s, *at),
        Act::LineEnd => *at = composer::line_end(s, *at),
        Act::WordLeft => *at = composer::word_left(s, *at),
        Act::WordRight => *at = composer::word_right(s, *at),
        Act::DelBack => composer::backspace(s, at),
        Act::DelNext => composer::delete(s, at),
        Act::DelWord => composer::delete_word(s, at),
        // Inside a message of several rows they move the cursor; past its edge, the conversation.
        Act::Up => match composer::vertical(s, *at, w, true) {
            Some(p) => *at = p,
            None => c.scroll_by(1),
        },
        Act::Down => match composer::vertical(s, *at, w, false) {
            Some(p) => *at = p,
            None => c.scroll_by(-1),
        },
        Act::PageUp => c.scroll_by(page),
        Act::PageDown => c.scroll_by(-page),
        Act::Top => c.scroll = c.scroll_max.get(),
        Act::Bottom => c.scroll = 0,
        _ => {}
    }
    if matches!(act, Act::Up | Act::PageUp | Act::Top) {
        return earlier(app);
    }
    vec![]
}

fn send(app: &mut App) -> Vec<Cmd> {
    // A command is only what the person typed with `/` first; ` /x` is a message.
    if app.chat.input.starts_with('/') {
        return super::commands::enter(app);
    }
    let text = app.chat.input.trim().to_owned();
    if text.is_empty() {
        return vec![];
    }
    app.chat.set_input(String::new());
    send_text(app, text)
}

/// Sends `text` as the person's message: it opens a new session, steers the turn running, or
/// starts a turn. Never read as a command (a command of the person's ends up here already
/// expanded).
pub fn send_text(app: &mut App, text: String) -> Vec<Cmd> {
    let c = &mut app.chat;
    if text.trim().is_empty() {
        return vec![];
    }
    if c.is_draft() {
        c.items.push(Item::User(text.clone()));
        c.queued = Some(text);
        c.opening = true;
        app.chat_seq += 1;
        c.seq = app.chat_seq;
        return vec![Cmd::ChatOpen {
            seq: app.chat_seq,
            target: OpenTarget::New {
                project: app.cwd.clone(),
            },
        }];
    }
    // The session is starting or compacting: the message goes out once it can (one at a time).
    if (c.opening || c.compacting) && c.queued.is_none() && c.failed.is_none() {
        c.scroll = 0;
        c.items.push(Item::User(text.clone()));
        c.queued = Some(text);
        return vec![];
    }
    if c.compacting || c.opening || c.failed.is_some() {
        app.flash = Some(if c.compacting {
            "Pi is compacting the context: wait for it to finish".into()
        } else {
            "the session is not ready yet".into()
        });
        // Nothing is lost: what was going out is back in the composer.
        if c.input.is_empty() {
            c.set_input(text);
        }
        return vec![];
    }
    c.scroll = 0;
    c.items.push(Item::User(text.clone()));
    if c.running {
        return vec![Cmd::ChatSteer { text }];
    }
    c.start_turn(app.now_ms);
    vec![Cmd::ChatSend { text }]
}

/// Plain characters go in at the cursor. Nothing that could drive the terminal or hide text is
/// taken (a typed bidi mark, say): what the composer holds is always safe to draw as it is.
fn edit(app: &mut App, k: &KeyEvent) -> Vec<Cmd> {
    if let KeyCode::Char(c) = k.code
        && !c.is_control()
        && !clio_brain::pi::text::is_deceptive(c)
        && !k
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        let mut buf = [0u8; 4];
        let ch = &mut app.chat;
        let was_empty = ch.input.is_empty();
        composer::insert(&mut ch.input, &mut ch.cursor, c.encode_utf8(&mut buf));
        if was_empty && c == '/' {
            return super::commands::open(app);
        }
    }
    vec![]
}

/// Pasted text goes through the sanitizer: no escape sequences, and it never presses Enter.
pub fn paste(app: &mut App, text: &str) {
    let before = app.chat.input.clone();
    let ch = &mut app.chat;
    composer::insert(&mut ch.input, &mut ch.cursor, &sanitize::multiline(text));
    super::commands::sync(app, &before);
}

/// What a tool does, as a title ("Read file"), like Aria's steps.
fn title(name: &str, reason: &str) -> String {
    if !reason.is_empty() {
        return clean(reason);
    }
    match name {
        "read" => "Read file".into(),
        "write" => "Write file".into(),
        "edit" => "Edit file".into(),
        "bash" => "Run command".into(),
        "ls" => "List folder".into(),
        "find" => "Find files".into(),
        other => clean(other),
    }
}

/// What a step works on: a file by its name, a command or a pattern.
fn chip(name: &str, summary: &str) -> String {
    if matches!(name, "read" | "write" | "edit") {
        return summary.rsplit('/').next().unwrap_or_default().to_owned();
    }
    summary.to_owned()
}

/// A kind of item. A blank line goes between two items, except inside a run of tools (a group)
/// or of notes.
#[derive(PartialEq, Clone, Copy)]
enum Kind {
    User,
    Agent,
    Tools,
    Notes,
}

/// The widest a group's titles get before they are cut.
const LABEL_MAX: usize = 30;
/// The most rows of text the composer shows; past them it scrolls.
const INPUT_ROWS: usize = 6;

/// `text` wrapped to `w` columns after `lead` (styled) on the first row and as many spaces on
/// the others.
fn hanging(text: &str, w: usize, lead: Span<'static>, style: Style) -> Vec<Line<'static>> {
    let indent = " ".repeat(text::width(&lead.content));
    let room = w.saturating_sub(indent.len());
    wrap(&sanitize::multiline(text), room)
        .into_iter()
        .enumerate()
        .map(|(i, row)| {
            let first = if i == 0 {
                lead.clone()
            } else {
                Span::raw(indent.clone())
            };
            Line::from(vec![first, Span::styled(row, style)])
        })
        .collect()
}

/// An O(1) fingerprint of a text item as it is drawn (its kind, length and ends, the width, the
/// theme, whether it streams); `None` for a tool, which is drawn fresh (one line, a spinner).
/// Items only grow at the end, so this changes whenever what they show does.
fn fingerprint(it: &Item, w: usize, t: Theme) -> Option<u64> {
    let (tag, text, live) = match it {
        Item::User(s) => (0u8, s, false),
        Item::Assistant { text, live } => (1, text, *live),
        Item::Note(s) => (2, s, false),
        Item::Error(s) => (3, s, false),
        Item::Tool { .. } => return None,
    };
    let b = text.as_bytes();
    let ends = (&b[..b.len().min(32)], &b[b.len().saturating_sub(32)..]);
    let mut h = DefaultHasher::new();
    (tag, b.len(), ends, live, w, t).hash(&mut h);
    Some(h.finish())
}

/// The rows of a text item (not a tool) at `w` columns.
fn render_item(c: &Chat, it: &Item, w: usize, t: Theme) -> Vec<Line<'static>> {
    match it {
        // The accent bar marks the person, on every row.
        Item::User(text) => wrap(&sanitize::multiline(text), w.saturating_sub(2))
            .into_iter()
            .map(|row| {
                Line::from(vec![
                    Span::styled(format!("{} ", t.bar_left()), t.accent_line()),
                    Span::styled(row, t.bold()),
                ])
            })
            .collect(),
        Item::Assistant { text, live } => {
            c.parsed.set(c.parsed.get() + 1);
            markdown::render(text, w.saturating_sub(2), t, *live)
                .into_iter()
                .map(|l| {
                    let mut spans = vec![Span::raw("  ")];
                    spans.extend(l.spans);
                    Line::from(spans)
                })
                .collect()
        }
        Item::Note(text) => hanging(
            text,
            w,
            Span::styled(format!("  {} ", t.dot()), t.dim()),
            t.dim(),
        ),
        Item::Error(text) => hanging(
            text,
            w,
            Span::styled(format!("  {} ", t.cross()), t.fail()),
            t.fail(),
        ),
        Item::Tool { .. } => vec![],
    }
}

/// The conversation at `w` columns, then `tail`, seen through a window `h` rows high that is
/// `app.chat.scroll` rows up from the newest: the rows in the window, where it starts, and how
/// far up it can go. Only the rows in the window are built: the text items' rows come from what
/// the last draw rendered (`Chat::rendered`), so a frame over a long, finished conversation
/// parses nothing and copies only what it shows.
fn conversation(
    app: &App,
    t: Theme,
    w: usize,
    tail: Vec<Line<'static>>,
    h: usize,
) -> (Vec<Line<'static>>, usize, usize) {
    let c = &app.chat;
    // The titles of a group of tools share one width, so what they worked on starts in one
    // column, and so do their endings, so those summaries are cut in one column too.
    let label_max = (w * 2 / 5).clamp(12, LABEL_MAX);
    let mut widths = vec![(0, 0); c.items.len()];
    let mut i = 0;
    while i < c.items.len() {
        let run = c.items[i..]
            .iter()
            .take_while(|it| matches!(it, Item::Tool { .. }))
            .count();
        let widest =
            |f: &dyn Fn(&Item) -> usize| c.items[i..i + run].iter().map(f).max().unwrap_or(0);
        let label = widest(&|it| match it {
            Item::Tool { name, reason, .. } => text::width(&title(name, reason)),
            _ => 0,
        });
        let tail = widest(&|it| match it {
            Item::Tool { state, ms, .. } => tool_tail(t, *state, *ms)
                .iter()
                .map(|s| text::width(s))
                .sum(),
            _ => 0,
        });
        widths[i..i + run].fill((label.min(label_max), tail));
        i += run.max(1);
    }
    // Each item's blank row before it (between kinds, and before each message) and its rows.
    let mut cache = c.rendered.borrow_mut();
    if cache.len() > c.items.len() {
        cache.retain(|k, _| *k >= c.dropped);
    }
    let mut last: Option<Kind> = None;
    let mut sizes = Vec::with_capacity(c.items.len());
    for (n, it) in c.items.iter().enumerate() {
        let kind = match it {
            Item::User(_) => Kind::User,
            Item::Assistant { .. } => Kind::Agent,
            Item::Tool { .. } => Kind::Tools,
            Item::Note(_) | Item::Error(_) => Kind::Notes,
        };
        let gap = usize::from(last != Some(kind) || matches!(kind, Kind::User | Kind::Agent));
        last = Some(kind);
        let rows = match fingerprint(it, w, t) {
            None => 1,
            Some(key) => {
                let e = cache.entry(c.dropped + n).or_insert((!key, vec![]));
                if e.0 != key {
                    *e = (key, render_item(c, it, w, t));
                }
                e.1.len()
            }
        };
        sizes.push((gap, rows));
    }
    let total = sizes.iter().map(|(g, r)| g + r).sum::<usize>() + tail.len();
    let max = total.saturating_sub(h);
    let start = max - c.scroll.min(max);
    let end = (start + h).min(total);
    let mut lines = Vec::with_capacity(end - start);
    let mut y = 0;
    for (n, (it, (gap, rows))) in c.items.iter().zip(sizes).enumerate() {
        let (a, b) = (y, y + gap + rows);
        y = b;
        if b <= start {
            continue;
        }
        if a >= end {
            break;
        }
        // Row `a` is the blank one when there is one; the item's own rows follow.
        let from = start.max(a + gap) - (a + gap);
        let to = end.min(b) - (a + gap);
        if gap == 1 && a >= start {
            lines.push(Line::from(""));
        }
        match it {
            Item::Tool {
                name,
                summary,
                reason,
                state,
                ms,
                ..
            } => {
                if from < to {
                    lines.push(tool_line(
                        app,
                        (w, widths[n].0, widths[n].1),
                        name,
                        summary,
                        reason,
                        *state,
                        *ms,
                    ));
                }
            }
            _ => {
                if let Some((_, rows)) = cache.get(&(c.dropped + n)) {
                    lines.extend(rows[from.min(to)..to].iter().cloned());
                }
            }
        }
    }
    let skip = start.max(y) - y;
    lines.extend(
        tail.into_iter()
            .skip(skip)
            .take(end.saturating_sub(y.max(start))),
    );
    (lines, start, max)
}

/// How a tool ended, for its right end: the word when it did not end well, and how long it took.
fn tool_tail(t: Theme, state: ToolState, ms: Option<u64>) -> [String; 3] {
    let word = match state {
        ToolState::Running | ToolState::Ok => "",
        ToolState::Failed => "failed",
        ToolState::Denied => "denied",
        ToolState::SetAside => "set aside",
        ToolState::Stopped => "stopped",
    };
    let dur = match ms {
        Some(ms) if ms >= 10_000 => usage::duration(ms / 1000),
        Some(ms) => usage::ms_label(ms),
        None => String::new(),
    };
    let sep = if word.is_empty() || dur.is_empty() {
        String::new()
    } else {
        format!(" {} ", t.dot())
    };
    [word.to_owned(), sep, dur]
}

/// One tool: its mark, its title (padded to `label_w`, the group's), what it worked on (cut where
/// the group's endings, `tail_w` wide, begin) and, at the right edge, how it ended and how long it
/// took.
fn tool_line(
    app: &App,
    (w, label_w, tail_w): (usize, usize, usize),
    name: &str,
    summary: &str,
    reason: &str,
    state: ToolState,
    ms: Option<u64>,
) -> Line<'static> {
    let t = app.theme;
    let (mark, style) = match state {
        ToolState::Running => (t.spinner(app.now_ms), t.accent_line()),
        ToolState::Ok => (t.check(), t.ok()),
        ToolState::Failed | ToolState::Denied => (t.cross(), t.fail()),
        ToolState::SetAside => (t.aside(), t.warn()),
        ToolState::Stopped => (t.stopped(), t.warn()),
    };
    let [word, sep, dur] = tool_tail(t, state, ms);
    let own_tail = text::width(&word) + text::width(&sep) + text::width(&dur);
    let mut label = text::fit(&title(name, reason), label_w.max(4), t.ellipsis());
    label.push_str(&" ".repeat(label_w.saturating_sub(text::width(&label))));
    let used = 4 + text::width(&label);
    let room = w.saturating_sub(used + 3 + tail_w.max(own_tail) + 2);
    let summary = clean(&chip(name, summary));
    let summary = if room >= 4 && !summary.is_empty() {
        format!("   {}", text::fit(&summary, room, t.ellipsis()))
    } else {
        String::new()
    };
    let pad = w.saturating_sub(used + text::width(&summary) + own_tail);
    Line::from(vec![
        Span::raw("  "),
        Span::styled(format!("{mark} "), style),
        Span::raw(label),
        Span::styled(summary, t.dim()),
        Span::raw(" ".repeat(pad)),
        Span::styled(word, style),
        Span::styled(sep, t.dim()),
        Span::styled(dur, t.dim()),
    ])
}

/// The empty new session: who and where, in the middle.
fn welcome(app: &App) -> Vec<Line<'static>> {
    let t = app.theme;
    let place = if app.host {
        "on this server".to_owned()
    } else {
        format!("in a copy of {}", clean(&folder_name(&app.cwd)))
    };
    vec![
        Line::from(Span::styled("clio", t.accent())),
        Line::from(Span::styled("How can I help?", t.bold())),
        Line::from(Span::styled(place, t.dim())),
    ]
}

fn folder_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned())
}

/// The line above the composer: what the session is doing now, the last flash, and whether
/// there is more of the conversation below.
fn status_line(app: &App, w: usize) -> Line<'static> {
    let t = app.theme;
    let c = &app.chat;
    let mut spans: Vec<Span> = vec![Span::raw("  ")];
    if c.opening {
        spans.push(Span::styled(
            format!("{} ", t.spinner(app.now_ms)),
            t.accent_line(),
        ));
        spans.push(Span::styled(
            if app.host {
                "starting…"
            } else {
                "copying the project and starting…"
            },
            t.dim(),
        ));
    } else if c.running && c.aborting {
        spans.push(Span::styled(format!("{} aborting…", t.bang()), t.warn()));
    } else if c.running {
        let secs = app.now_ms.saturating_sub(c.turn_ms) / 1000;
        spans.push(Span::styled(
            format!("{} ", t.spinner(app.now_ms)),
            t.accent_line(),
        ));
        spans.push(Span::styled(
            format!(
                "working {d} {} {d} Esc stops",
                usage::duration(secs),
                d = t.dot()
            ),
            t.dim(),
        ));
    }
    if let Some(r) = super::review::status(app) {
        if spans.len() > 1 {
            spans.push(Span::styled(format!(" {} ", t.dot()), t.dim()));
        }
        spans.push(Span::styled(
            format!("{} ", t.spinner(app.now_ms)),
            t.accent_line(),
        ));
        spans.push(Span::styled(r, t.dim()));
    }
    if let Some(w) = super::approvals::status(app) {
        if spans.len() > 1 {
            spans.push(Span::styled(format!(" {} ", t.dot()), t.dim()));
        }
        spans.push(Span::styled(w, t.accent_line()));
    }
    if c.jobs > 0 {
        if spans.len() > 1 {
            spans.push(Span::styled(format!(" {} ", t.dot()), t.dim()));
        }
        let s = if c.jobs == 1 { "" } else { "s" };
        spans.push(Span::styled(format!("{} job{s}", c.jobs), t.accent_line()));
    }
    if let Some(msg) = &app.flash {
        if spans.len() > 1 {
            spans.push(Span::styled(format!(" {} ", t.dot()), t.dim()));
        }
        spans.push(Span::styled(clean(msg), t.accent_line()));
    }
    let more = if c.scroll > 0 && c.scroll_max.get() > 0 {
        format!("{} more below", t.down())
    } else {
        String::new()
    };
    let used: usize = spans.iter().map(|s| text::width(&s.content)).sum();
    let room = w.saturating_sub(text::width(&more) + 1);
    if used > room {
        // The flash is the long part: it is cut, the state never.
        let cut = text::fit(
            &spans.iter().map(|s| s.content.as_ref()).collect::<String>(),
            room,
            t.ellipsis(),
        );
        return Line::from(vec![
            Span::styled(cut, t.dim()),
            Span::styled(more, t.dim()),
        ]);
    }
    spans.push(Span::raw(
        " ".repeat(w.saturating_sub(used + text::width(&more))),
    ));
    spans.push(Span::styled(more, t.dim()));
    Line::from(spans)
}

/// The right of the chat's footer: tokens in and out, the cost when there is one, and how full
/// the context is; then shorter forms, for a narrow footer (the context always stays).
pub(super) fn meter(app: &App) -> Vec<Vec<Span<'static>>> {
    let t = app.theme;
    let s = app.chat.snapshot(app.now_ms, app.fallback());
    let mut text = format!(
        "{}{} {}{}",
        t.up(),
        usage::tokens(s.tokens_in),
        t.down(),
        usage::tokens(s.tokens_out)
    );
    if s.cost_micro > 0 {
        text.push_str(&format!(" {} {}", t.dot(), usage::usd(s.cost_micro)));
    }
    text.push_str(&format!(" {} context ", t.dot()));
    let pct = match s.ctx_percent() {
        Some(p) => Span::styled(format!("{p} %"), t.level(p)),
        None => Span::styled(t.rule().to_owned(), t.dim()),
    };
    vec![
        vec![Span::styled(text, t.dim()), pct.clone()],
        vec![Span::styled("context ", t.dim()), pct.clone()],
        vec![Span::styled("ctx ", t.dim()), pct],
    ]
}

/// The Pi harness's screen under the top bar: the drawer of sessions when it is open, and the
/// chat across the width left: the conversation, a status line, the composer and the footer.
pub fn draw_screen(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let c = &app.chat;
    let (side, rest) = if app.screen == Screen::Sessions {
        let dw = if area.width < super::sessions::WIDE {
            area.width
        } else {
            super::sessions::DRAWER_W
        };
        let [side, rest] =
            Layout::horizontal([Constraint::Length(dw), Constraint::Min(0)]).areas(area);
        (side, rest)
    } else {
        (Rect::default(), area)
    };
    app.sessions.area.set(side);
    app.sessions.chat_area.set(rest);
    if side.width > 0 {
        super::sessions::draw_drawer(f, side, app);
    }
    if rest.width < 16 {
        return;
    }
    // The whole width left, with one column of margin on each side.
    let cw = rest.width - 2;
    let col = Rect::new(rest.x + 1, rest.y, cw, rest.height);

    // The composer: its rows of text, and the one the cursor is on kept in sight.
    let text_w = usize::from(cw.saturating_sub(6)).max(2);
    c.input_w.set(u16::try_from(text_w).unwrap_or(u16::MAX));
    let rows = wrap_ranges(&c.input, text_w);
    let (cur_row, cur_col) = composer::locate(&c.input, c.cursor, text_w);
    let shown = rows.len().clamp(1, INPUT_ROWS);
    let top = cur_row.saturating_sub(INPUT_ROWS - 1);
    let dbg_h: u16 = if app.debug {
        (area.height / 3).clamp(4, 10)
    } else {
        0
    };
    let [conv, dbg, todo, status, input, foot] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(dbg_h),
        Constraint::Length(super::todo::height(&c.todo)),
        Constraint::Length(1),
        Constraint::Length(u16::try_from(shown).unwrap_or(1) + 2),
        Constraint::Length(1),
    ])
    .areas(col);

    // The conversation, from the top while it fits, then anchored to the newest line.
    let text_area = Rect {
        width: conv.width.saturating_sub(2),
        ..conv
    };
    c.view_h.set(conv.height);
    c.view_w.set(text_area.width);
    let w = usize::from(text_area.width);
    let mut tail = vec![];
    if let Some(e) = &c.failed {
        tail.push(Line::from(""));
        tail.extend(hanging(
            e,
            w,
            Span::styled(format!("  {} ", t.cross()), t.fail()),
            t.fail(),
        ));
        tail.push(Line::from(Span::styled(
            "    Ctrl-S opens the sessions",
            t.dim(),
        )));
    }
    let h = usize::from(conv.height);
    let (shown_rows, start, max) = conversation(app, t, w, tail, h);
    c.scroll_max.set(max);
    if c.is_draft() && c.items.is_empty() {
        let lines = welcome(app);
        let y = conv.y + conv.height.saturating_sub(3) / 2;
        f.render_widget(
            Paragraph::new(lines).alignment(Alignment::Center),
            Rect::new(conv.x, y, conv.width, conv.height.min(3)),
        );
    } else {
        f.render_widget(Paragraph::new(shown_rows), text_area);
        if max > 0 {
            let mut st = ScrollbarState::new(max + 1)
                .position(start)
                .viewport_content_length(h);
            let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some(t.rule_v()))
                .track_style(t.faint())
                .thumb_symbol(if t.unicode { "┃" } else { "#" })
                .thumb_style(t.dim());
            f.render_stateful_widget(bar, conv, &mut st);
        }
    }

    if app.debug {
        let mut lines = vec![section(t, "Pi events (raw)", dbg.width)];
        let room = usize::from(dbg.height).saturating_sub(1);
        let skip = c.raw.len().saturating_sub(room);
        lines.extend(c.raw.iter().skip(skip).map(|r| {
            Line::from(Span::styled(
                text::fit(&clean(r), usize::from(dbg.width), t.ellipsis()),
                t.dim(),
            ))
        }));
        f.render_widget(Paragraph::new(lines), dbg);
    }

    super::todo::draw(f, todo, t, &c.todo);
    f.render_widget(
        Paragraph::new(status_line(app, usize::from(status.width))),
        status,
    );

    // The composer has the keys unless the drawer, a dialog or a box does.
    let focused = app.screen == Screen::Chat
        && app.confirm.is_none()
        && app.notice.is_none()
        && app.modal.is_none();
    let block = Block::new()
        .borders(Borders::ALL)
        .border_set(t.border())
        .border_style(if focused { t.accent_line() } else { t.faint() });
    let inner = block.inner(input);
    f.render_widget(block, input);
    let prompt = Span::styled(format!(" {} ", t.prompt()), t.accent_line());
    let mut lines = Vec::new();
    if c.input.is_empty() {
        lines.push(Line::from(vec![
            prompt,
            Span::styled("Type a message…", t.dim()),
        ]));
    } else {
        for (i, (a, b)) in rows.iter().enumerate().skip(top).take(shown) {
            let lead = if i == 0 {
                prompt.clone()
            } else {
                Span::raw("   ")
            };
            lines.push(Line::from(vec![
                lead,
                Span::raw(sanitize::line(&c.input[*a..*b])),
            ]));
        }
    }
    f.render_widget(Paragraph::new(lines), inner);
    if focused {
        super::commands::draw(f, input, conv.y, app);
    }
    if focused {
        let x = inner.x + 3 + u16::try_from(cur_col).unwrap_or(0);
        let y = inner.y + u16::try_from(cur_row - top).unwrap_or(0);
        f.set_cursor_position((x.min(inner.right().saturating_sub(1)), y));
    }

    let foot = Rect::new(foot.x + 2, foot.y, foot.width.saturating_sub(3), 1);
    super::app::footer(f, foot, app, 4, meter(app));
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use clio_brain::pi::rpc::{MessageInfo, Usage};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use serde_json::json;

    fn chat() -> Chat {
        let mut c = Chat::opening(1, 0);
        c.opened("sid".into(), String::new(), &[], None, 0);
        c
    }

    fn delta(s: &str) -> Event {
        Event::MessageUpdate {
            usage: Some(Usage {
                input: 10,
                output: 2,
                cost: 0.5,
                ..Usage::default()
            }),
            kind: "text_delta".into(),
            delta: Some(s.into()),
        }
    }

    #[test]
    fn text_streams_into_one_assistant_item_and_usage_follows() {
        let mut c = chat();
        c.on_event(&Event::MessageStart(MessageInfo {
            role: "assistant".into(),
            ..MessageInfo::default()
        }));
        c.on_event(&delta("hola "));
        c.on_event(&delta("mundo"));
        assert_eq!(
            c.items,
            vec![Item::Assistant {
                text: "hola mundo".into(),
                live: true
            }]
        );
        assert_eq!(
            c.snapshot(0, (1000, false, RunLimits::default())).tokens_in,
            10
        );
        c.on_event(&Event::MessageEnd(MessageInfo {
            role: "assistant".into(),
            text: "hola mundo".into(),
            usage: Some(Usage {
                input: 10,
                output: 2,
                cost: 0.5,
                ..Usage::default()
            }),
            ..MessageInfo::default()
        }));
        assert!(matches!(&c.items[0], Item::Assistant { live: false, .. }));
        c.on_event(&Event::TurnEnd);
        let s = c.snapshot(5000, (1000, false, RunLimits::default()));
        assert_eq!(
            (s.tokens_out, s.steps, s.secs, s.cost_micro),
            (2, 1, 5, 500_000)
        );
    }

    #[test]
    fn a_step_is_titled_by_the_models_reason_or_by_what_the_tool_does() {
        let mut c = chat();
        let start = |id: &str, name: &str, args: Value| Event::ToolStart {
            id: id.into(),
            name: name.into(),
            args,
        };
        c.on_event(&start(
            "1",
            "bash",
            json!({"command":"ls","reason":" Review the structure\nmás"}),
        ));
        c.on_event(&start("2", "read", json!({"path":"src/a.rs"})));
        let titles: Vec<_> = c
            .items
            .iter()
            .map(|i| match i {
                Item::Tool { name, reason, .. } => title(name, reason),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(titles, ["Review the structure", "Read file"]);
    }

    #[test]
    fn a_tool_is_one_line_that_ends_ok_failed_or_denied() {
        let mut c = chat();
        let start = |id: &str, name: &str, args: Value| Event::ToolStart {
            id: id.into(),
            name: name.into(),
            args,
        };
        let end = |id: &str, err: bool, text: &str, ms| Event::ToolEnd {
            id: id.into(),
            name: "x".into(),
            result: json!({"content":[{"type":"text","text":text}]}),
            is_error: err,
            duration_ms: Some(ms),
        };
        c.on_event(&start("1", "bash", json!({"command":"ls -la\nsecond"})));
        c.on_event(&start("2", "write", json!({"path":".git/config"})));
        c.on_event(&start("3", "read", json!({"path":"nope"})));
        c.on_event(&end("1", false, "ok", 12));
        c.on_event(&end(
            "2",
            true,
            "clio-guard: .git is a folder nothing here may write",
            1,
        ));
        c.on_event(&end("3", true, "ENOENT", 3));
        let states: Vec<_> = c
            .items
            .iter()
            .map(|i| match i {
                Item::Tool { state, summary, .. } => (*state, summary.clone()),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            states,
            vec![
                (ToolState::Ok, "ls -la".to_owned()),
                (ToolState::Denied, ".git/config".to_owned()),
                (ToolState::Failed, "nope".to_owned())
            ]
        );
    }

    // Sessions saved before the text was English say `denegado`: reopened, it is still a denial.
    #[test]
    fn a_denial_in_an_old_spanish_session_is_still_drawn_as_denied() {
        assert_eq!(
            state_of("bash", true, "denegado por la regla S-16"),
            ToolState::Denied
        );
        assert_eq!(
            state_of("bash", true, "denied by rule S-16"),
            ToolState::Denied
        );
        assert_eq!(state_of("bash", true, "ENOENT"), ToolState::Failed);
    }

    // Host mode: a call the toolhost set aside for the person is its own state, drawn in the
    // warning color with «apartado», never as a failure; and a group's summaries start in one
    // column and its endings end at the right edge. Review: the test used the bare answer and
    // passed while the real one, which comes wrapped by the host, was drawn as «failed».
    #[test]
    fn a_set_aside_call_is_apartado_and_a_group_lines_up() {
        use clio_gateway::{Source, wrap_untrusted};
        let answer =
            "set aside 2bdcb88e: «delete» needs the person's approval. Do not try it another way.";
        let old = "apartado 2bdcb88e: «borrar» necesita el visto bueno de la persona. No lo intentes de otra forma.";
        // A session saved before the text was English is drawn the same way.
        assert_eq!(
            state_of("bash", true, &wrap_untrusted(Source::ToolResult, old)),
            ToolState::SetAside
        );
        let real = wrap_untrusted(Source::ToolResult, answer);
        assert!(real.starts_with("<<<UNTRUSTED-DATA tool-result "));
        assert_eq!(state_of("bash", true, &real), ToolState::SetAside);
        assert_eq!(
            state_of("bash", true, &wrap_untrusted(Source::File, answer)),
            ToolState::SetAside
        );
        // Not the exact wrapper, or not the exact answer inside it: the text cannot fake it.
        for fake in [
            answer.to_owned(),
            wrap_untrusted(Source::ToolResult, &format!("salida:\n{answer}")),
            wrap_untrusted(Source::ToolResult, "set aside x y: «delete»"),
            wrap_untrusted(Source::ToolResult, "set aside zz: «delete»"),
            wrap_untrusted(Source::ToolResult, "apartado zz: «borrar»"),
            wrap_untrusted(Source::Web, answer),
            real.replace("2bdcb88e:", "2bdcb88f:"),
            format!("{real}\nmás"),
        ] {
            assert_eq!(state_of("bash", true, &fake), ToolState::Failed, "{fake}");
        }
        // A block change in a marked session: the tool host answers it bare, live and in history.
        let memory = "set aside 2bdcb88e: «memory» needs the person's approval. Do not try it another way. Go on with the rest and say so at the end.";
        assert_eq!(
            state_of("memory_block_edit", true, memory),
            ToolState::SetAside
        );
        let mut c = chat();
        c.on_event(&Event::ToolStart {
            id: "m1".into(),
            name: "memory_block_edit".into(),
            args: json!({}),
        });
        c.on_event(&Event::ToolEnd {
            id: "m1".into(),
            name: "memory_block_edit".into(),
            result: json!({"content": [{"type": "text", "text": memory}]}),
            is_error: true,
            duration_ms: None,
        });
        assert!(matches!(
            c.items.last(),
            Some(Item::Tool {
                state: ToolState::SetAside,
                ..
            })
        ));
        let mut c = chat();
        c.add_history(&json!({"role": "assistant", "content": [{"type": "toolCall", "id": "m1", "name": "memory_block_edit", "arguments": {}}]}));
        c.add_history(&json!({"role": "toolResult", "toolCallId": "m1", "toolName": "memory_block_edit", "isError": true, "content": [{"type": "text", "text": memory}]}));
        assert!(matches!(
            c.items.last(),
            Some(Item::Tool {
                state: ToolState::SetAside,
                ..
            })
        ));
        // Bare from any other tool, or not a «memory» item: a failure.
        assert_eq!(state_of("bash", true, memory), ToolState::Failed);
        assert_eq!(state_of("memory_save", true, memory), ToolState::Failed);
        assert_eq!(
            state_of("memory_block_edit", true, answer),
            ToolState::Failed
        );
        let mut a = super::super::tests_gallery::pi(true);
        a.chat = chat();
        let tool = |reason: &str, cmd: &str, state, ms| Item::Tool {
            id: String::new(),
            name: "bash".into(),
            summary: cmd.into(),
            reason: reason.into(),
            state,
            ms,
        };
        a.chat.items = vec![
            tool(
                "Delete old volumes",
                "rm -r /var/lib/docker/volumes/viejo",
                ToolState::SetAside,
                Some(400),
            ),
            tool("Look", "du -sh /var", ToolState::Ok, Some(1_500)),
            tool(
                "A very long title that does not fit in thirty columns",
                "ls",
                ToolState::Failed,
                Some(80_000),
            ),
        ];
        let screen = super::super::tests_gallery::screen_text(&a, 100, 30);
        let rows: Vec<&str> = screen.lines().filter(|l| l.contains("   ")).collect();
        let row = |s: &str| *rows.iter().find(|r| r.contains(s)).unwrap();
        let (r1, r2, r3) = (row("rm -r"), row("du -sh"), row("ls  "));
        assert!(
            r1.contains("◇ Delete") && r1.contains("set aside") && !r1.contains("failed"),
            "{screen}"
        );
        assert!(r3.contains('…') && r3.contains("failed"), "{screen}");
        let col = |r: &str, s: &str| text::width(&r[..r.find(s).unwrap()]);
        assert_eq!(col(r1, "rm -r"), col(r2, "du -sh"), "{screen}");
        assert_eq!(col(r2, "du -sh"), col(r3, "ls  "), "{screen}");
        let end = |r: &str, s: &str| col(r, s) + text::width(s);
        assert_eq!(end(r1, "0.4 s"), end(r2, "1.5 s"), "{screen}");
        assert_eq!(end(r2, "1.5 s"), end(r3, "1 min 20 s"), "{screen}");
    }

    #[test]
    fn what_the_judge_withheld_is_a_quiet_note_with_where_to_read_it() {
        use crate::cli::chat::Withheld;
        let mut c = chat();
        let notes = |c: &Chat| -> Vec<String> {
            c.items
                .iter()
                .filter_map(|i| match i {
                    Item::Note(n) => Some(n.clone()),
                    _ => None,
                })
                .collect()
        };
        let w = Withheld {
            tool: "bash".into(),
            source: "journalctl -u app".into(),
            bytes: 812,
            file: Some("/r/withheld/1.txt".into()),
        };
        c.judged(std::slice::from_ref(&w), false);
        c.judged(std::slice::from_ref(&w), false);
        let n = notes(&c);
        assert_eq!(n.len(), 1, "{n:?}");
        assert!(
            n[0].contains("812 bytes from bash journalctl -u app"),
            "{n:?}"
        );
        assert!(n[0].contains("less /r/withheld/1.txt"), "{n:?}");
        // A silent judge is said once.
        c.judged(std::slice::from_ref(&w), true);
        c.judged(&[w], true);
        let n = notes(&c);
        assert_eq!(n.len(), 2, "{n:?}");
        assert!(n[1].contains("the judge did not answer"), "{n:?}");
    }

    #[test]
    fn values_struck_out_are_a_quiet_note_when_the_total_grows() {
        let mut c = chat();
        let notes = |c: &Chat| -> Vec<String> {
            c.items
                .iter()
                .filter_map(|i| match i {
                    Item::Note(n) => Some(n.clone()),
                    _ => None,
                })
                .collect()
        };
        c.masked(0);
        assert!(notes(&c).is_empty());
        c.masked(2);
        c.masked(2); // the same total after the next turn: nothing new to say
        assert_eq!(notes(&c).len(), 1, "{:?}", notes(&c));
        assert!(notes(&c)[0].contains("2 key-shaped values") && notes(&c)[0].contains("redacted"));
        c.masked(3);
        assert_eq!(notes(&c).len(), 2);
        assert!(
            notes(&c)[1].contains("1 key-shaped value ")
                || notes(&c)[1].contains("1 key-shaped value"),
            "{:?}",
            notes(&c)
        );
    }

    #[test]
    fn a_connection_the_jail_refused_is_a_quiet_note_once() {
        let mut c = chat();
        let notes = |c: &Chat| -> Vec<String> {
            c.items
                .iter()
                .filter_map(|i| match i {
                    Item::Note(n) => Some(n.clone()),
                    _ => None,
                })
                .collect()
        };
        c.egress(&[]);
        assert!(notes(&c).is_empty());
        let first = vec!["example.com:443".to_owned()];
        c.egress(&first);
        c.egress(&first); // the same list again, after the next turn: nothing new to say
        assert_eq!(notes(&c).len(), 1, "{:?}", notes(&c));
        assert!(notes(&c)[0].contains("example.com:443") && notes(&c)[0].contains("jail"));
        let more = vec!["example.com:443".to_owned(), "GET (not CONNECT)".to_owned()];
        c.egress(&more);
        assert_eq!(notes(&c).len(), 2, "only the new one is added");
        assert!(notes(&c)[1].contains("GET"));
    }

    #[test]
    fn compaction_and_retries_are_quiet_notes_and_events_are_kept_raw() {
        let mut c = chat();
        c.on_event(&Event::CompactionStart {
            reason: "threshold".into(),
        });
        c.on_event(&Event::CompactionEnd {
            reason: "threshold".into(),
            aborted: false,
            will_retry: false,
            error: None,
            tokens_before: Some(90_000),
            tokens_after: Some(12_000),
        });
        c.on_event(&Event::AutoRetryStart {
            attempt: 1,
            max_attempts: 3,
            delay_ms: 2000,
            error: "overloaded".into(),
        });
        let notes: Vec<_> = c
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Note(n) => Some(n.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(notes.len(), 3);
        assert!(notes[1].contains("90k") && notes[1].contains("12k"));
        assert_eq!(c.raw.len(), 3);
        for _ in 0..RAW_KEEP + 10 {
            c.on_event(&Event::TurnStart);
        }
        assert_eq!(c.raw.len(), RAW_KEEP);
    }

    // Second audit, finding 2: Pi is jailed but not trusted, and `clio` holds what it streams. A
    // stream of huge events must leave a small state behind (the log of events keeps clips, the
    // assistant's text has a cap) and must not cost time per byte of the event.
    #[test]
    fn a_stream_of_huge_events_leaves_a_bounded_state() {
        let mut c = Chat::opening(1, 0);
        let huge = "x".repeat(8 << 20);
        let t = std::time::Instant::now();
        for _ in 0..40 {
            c.on_event(&Event::MessageUpdate {
                usage: None,
                kind: "text_delta".into(),
                delta: Some(huge.clone()),
            });
            c.on_event(&Event::ToolEnd {
                id: "t".into(),
                name: "read".into(),
                result: json!({"content":[{"type":"text","text": huge}]}),
                is_error: false,
                duration_ms: None,
            });
            c.on_event(&Event::AutoRetryEnd {
                success: false,
                attempt: 1,
                final_error: Some(huge.clone()),
            });
        }
        let held: usize = c.raw.iter().map(String::len).sum();
        assert!(held <= RAW_KEEP * 2048, "the event log holds {held} bytes");
        let text: usize = c
            .items
            .iter()
            .map(|i| match i {
                Item::Assistant { text, .. } | Item::Error(text) | Item::Note(text) => text.len(),
                _ => 0,
            })
            .sum();
        assert!(
            text <= MAX_HELD + (1 << 20),
            "the conversation holds {text} bytes of text"
        );
        assert!(
            text >= MAX_HELD / 2,
            "the budget is spent, not skipped: {text}"
        );
        assert!(
            t.elapsed() < std::time::Duration::from_secs(5),
            "took {:?}",
            t.elapsed()
        );
        // A normal answer is shown whole.
        let mut c = Chat::opening(1, 0);
        c.on_event(&Event::MessageUpdate {
            usage: None,
            kind: "text_delta".into(),
            delta: Some("hola, esto cabe".into()),
        });
        assert!(
            matches!(&c.items[..], [Item::Assistant { text, .. }] if text == "hola, esto cabe")
        );
    }

    #[test]
    fn history_shows_what_the_person_typed_without_the_envelope() {
        let mut c = Chat::opening(1, 0);
        let wrapped = clio_brain::pi::envelope::wrap("2026-11-03 18:42 -05:00", "hola, Clio");
        let h = vec![json!({"role":"user","content":[{"type":"text","text":wrapped}]})];
        c.opened("sid".into(), String::new(), &h, None, 0);
        assert!(
            matches!(&c.items[..], [Item::User(t)] if t == "hola, Clio"),
            "{:?}",
            c.items.len()
        );
    }

    #[test]
    fn history_comes_back_as_items() {
        let mut c = Chat::opening(1, 0);
        let h = vec![
            json!({"role":"system","content":"x"}),
            json!({"role":"user","content":[{"type":"text","text":"hola"}]}),
            json!({"role":"assistant","content":[{"type":"text","text":"qué tal"},{"type":"toolCall","id":"t1","name":"read","arguments":{"path":"a.txt"}}]}),
            json!({"role":"toolResult","toolCallId":"t1","isError":true,"content":[{"type":"text","text":"ENOENT"}]}),
        ];
        c.opened("sid".into(), String::new(), &h, None, 0);
        assert_eq!(c.items.len(), 3);
        assert!(
            matches!(&c.items[2], Item::Tool { state: ToolState::Failed, summary, .. } if summary == "a.txt")
        );
    }

    fn press(app: App, code: KeyCode, mods: KeyModifiers) -> (App, Vec<Cmd>) {
        super::super::app::update(
            app,
            super::super::app::Input::Key(KeyEvent::new(code, mods)),
        )
    }

    fn typed(mut app: App, text: &str) -> App {
        for c in text.chars() {
            app = press(app, KeyCode::Char(c), KeyModifiers::NONE).0;
        }
        app
    }

    fn open_chat() -> App {
        let mut app = App::new(
            "/p/proj",
            Theme {
                color: false,
                unicode: true,
                rgb: false,
            },
        );
        app.use_pi();
        app.screen = Screen::Chat;
        app.chat = Chat::opening(1, 0);
        app.chat.opened("sid".into(), String::new(), &[], None, 0);
        app
    }

    // The first time, Clio speaks first: the chat starts a turn with no words of the person, and
    // nothing of the person's shows for it, now or when the session is opened again.
    #[test]
    fn a_session_that_presents_starts_its_own_turn_and_shows_no_words_of_the_person() {
        use super::super::app::{Bg, ChatInfo, Input, update};
        let opened = |presentation| {
            let mut app = open_chat();
            app.chat = Chat::opening(1, 0);
            let info = ChatInfo {
                id: "sid".into(),
                workspace: String::new(),
                history: vec![],
                refresh: None,
                presentation,
                notes: vec![],
            };
            update(
                app,
                Input::Bg(Bg::ChatOpened {
                    seq: 1,
                    result: Ok(Box::new(info)),
                }),
            )
        };
        let (app, cmds) = opened(true);
        assert_eq!(
            cmds,
            vec![Cmd::ChatSend {
                text: String::new()
            }]
        );
        assert!(app.chat.running && app.chat.items.is_empty());
        let (app, cmds) = opened(false);
        // Idle: only whether to ask about what waits is checked.
        assert!(!app.chat.running);
        assert!(
            cmds.iter().all(|c| matches!(c, Cmd::CheckApprovals { .. })),
            "{cmds:?}"
        );
        // Reopened: the turn's message is only Clio's envelope.
        let mut c = Chat::opening(1, 0);
        let wrapped = clio_brain::pi::envelope::wrap("2026-11-03 18:42 -05:00", "");
        let h = vec![json!({"role":"user","content":[{"type":"text","text":wrapped}]})];
        c.opened("sid".into(), String::new(), &h, None, 0);
        assert!(c.items.is_empty());
    }

    #[test]
    fn enter_sends_then_steers_esc_aborts_then_leaves_and_ctrl_c_quits() {
        let none = KeyModifiers::NONE;
        let (app, cmds) = press(open_chat(), KeyCode::Enter, none);
        assert!(
            cmds.is_empty() && !app.chat.running,
            "an empty message sends nothing"
        );
        let (app, cmds) = press(typed(app, "hola"), KeyCode::Enter, none);
        assert_eq!(
            cmds,
            vec![Cmd::ChatSend {
                text: "hola".into()
            }]
        );
        assert!(app.chat.running && app.chat.input.is_empty());
        assert_eq!(app.ctx(), super::super::keys::Ctx::ChatRun);
        let (app, cmds) = press(typed(app, "mejor así"), KeyCode::Enter, none);
        assert_eq!(
            cmds,
            vec![Cmd::ChatSteer {
                text: "mejor así".into()
            }]
        );
        let (app, cmds) = press(app, KeyCode::Esc, none);
        assert_eq!(cmds, vec![Cmd::ChatAbort]);
        let (app, cmds) = press(app, KeyCode::Esc, none);
        assert!(cmds.is_empty(), "a second Esc does not abort twice");
        // The run ends: Esc does nothing, Ctrl-S opens the sessions with the session still open;
        // Ctrl-C quits.
        let (mut app, _) = super::super::app::update(
            app,
            super::super::app::Input::Bg(super::super::app::Bg::ChatSettled {
                result: Ok(clio_brain::pi::rpc::RunSummary::default()),
                refresh: None,
            }),
        );
        assert!(!app.chat.running);
        let (idle, cmds) = press(std::mem::replace(&mut app, open_chat()), KeyCode::Esc, none);
        assert_eq!(idle.screen, Screen::Chat);
        assert!(cmds.is_empty(), "{cmds:?}");
        let (left, cmds) = press(idle, KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(left.screen, Screen::Sessions);
        assert!(cmds.is_empty() && left.chat.session_id == "sid", "{cmds:?}");
        let (quit, _) = press(app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(quit.quit);
    }

    // A tool still running when the turn settles or is aborted is drawn as stopped, never as
    // spinning on.
    #[test]
    fn a_tool_left_running_when_the_turn_ends_is_drawn_as_stopped() {
        let mut app = open_chat();
        for id in ["t1", "t2"] {
            app.chat.on_event(&Event::ToolStart {
                id: id.into(),
                name: "bash".into(),
                args: json!({"command": "sleep 100"}),
            });
        }
        if let Some(Item::Tool { state, .. }) = app.chat.items.last_mut() {
            *state = ToolState::Ok;
        }
        let (app, _) = super::super::app::update(
            app,
            super::super::app::Input::Bg(super::super::app::Bg::ChatSettled {
                result: Err("aborted".into()),
                refresh: None,
            }),
        );
        let states: Vec<ToolState> = app
            .chat
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Tool { state, .. } => Some(*state),
                _ => None,
            })
            .collect();
        assert_eq!(states, [ToolState::Stopped, ToolState::Ok]);
        let ascii = Theme {
            color: false,
            unicode: false,
            rgb: false,
        };
        assert_eq!(tool_tail(ascii, ToolState::Stopped, None)[0], "stopped");
        assert_eq!(ascii.stopped(), "#");
        // The pi event does the same.
        let mut c = Chat::opening(1, 0);
        c.on_event(&Event::ToolStart {
            id: "t".into(),
            name: "read".into(),
            args: json!({"path": "a"}),
        });
        c.on_event(&Event::AgentSettled { aborted: true });
        assert!(matches!(
            c.items[..],
            [Item::Tool {
                state: ToolState::Stopped,
                ..
            }]
        ));
    }

    // S-47: a run cut by its budget says which cap, in the chat; a run that was not cut says nothing.
    #[test]
    fn a_run_cut_by_its_budget_says_which_cap_in_the_chat() {
        use clio_brain::control::Stop;
        let settle = |stopped| {
            let summary = clio_brain::pi::rpc::RunSummary {
                stopped,
                ..Default::default()
            };
            let (app, _) = super::super::app::update(
                open_chat(),
                super::super::app::Input::Bg(super::super::app::Bg::ChatSettled {
                    result: Ok(summary),
                    refresh: None,
                }),
            );
            app
        };
        let cut = settle(Some(Stop::Steps));
        assert!(
            cut.chat
                .items
                .iter()
                .any(|i| matches!(i, Item::Error(m) if m.contains("step limit"))),
            "the chat shows the cap that cut the run"
        );
        let clean = settle(None);
        assert!(!clean.chat.items.iter().any(|i| matches!(i, Item::Error(_))));
    }

    // The person sees the level their request was judged at, and a run that was not assessed
    // (a bare client) says nothing.
    #[test]
    fn a_settled_run_says_the_level_it_was_judged_at() {
        use clio_brain::pi::level::Assessor;
        let assessed = Assessor::new(std::sync::Arc::new(clio_core::SystemClock))
            .assess("Borra la carpeta de pruebas");
        let settle = |assessment| {
            let summary = clio_brain::pi::rpc::RunSummary {
                assessment,
                ..Default::default()
            };
            let (app, _) = super::super::app::update(
                open_chat(),
                super::super::app::Input::Bg(super::super::app::Bg::ChatSettled {
                    result: Ok(summary),
                    refresh: None,
                }),
            );
            app
        };
        let judged = settle(Some(assessed));
        assert!(
            judged
                .chat
                .items
                .iter()
                .any(|i| matches!(i, Item::Note(m) if m.contains("level 2"))),
            "the chat shows the level"
        );
        assert!(
            !settle(None)
                .chat
                .items
                .iter()
                .any(|i| matches!(i, Item::Note(_)))
        );
    }

    // A chat still opening sends nothing yet: the first message waits for the session (and goes
    // when it is up), a second stays in the composer.
    #[test]
    fn a_chat_that_is_still_opening_does_not_send() {
        let mut app = open_chat();
        app.chat.opening = true;
        let (app, cmds) = press(typed(app, "hola"), KeyCode::Enter, KeyModifiers::NONE);
        assert!(cmds.is_empty() && !app.chat.running);
        assert_eq!(app.chat.queued.as_deref(), Some("hola"));
        assert!(
            app.chat.input.is_empty() && app.chat.items.last() == Some(&Item::User("hola".into()))
        );
        let (app, cmds) = press(typed(app, "y más"), KeyCode::Enter, KeyModifiers::NONE);
        assert!(cmds.is_empty() && !app.chat.running);
        assert!(app.flash.is_some() && app.chat.input == "y más");
    }

    // `x` on a session asks what deleting it removes, shows it, and deletes only on `y`.
    #[test]
    fn x_deletes_a_session_after_saying_what_goes_with_it() {
        use crate::cli::chat::{Row, RowKind};
        let mut app = App::new(
            "/p/proj",
            Theme {
                color: false,
                unicode: true,
                rgb: false,
            },
        );
        app.use_pi();
        app.screen = Screen::Sessions;
        app.sessions.set(vec![Row {
            kind: RowKind::Session,
            title: "t".into(),
            detail: String::new(),
            messages: 2,
            last_ms: None,
            reference: "/s/a.jsonl".into(),
            unavailable: None,
        }]);
        let none = KeyModifiers::NONE;
        // On "new session" there is nothing to delete.
        let (app, cmds) = press(app, KeyCode::Char('x'), none);
        assert!(cmds.is_empty() && app.flash.is_some());
        let (app, _) = press(app, KeyCode::Char('j'), none);
        let (app, cmds) = press(app, KeyCode::Char('x'), none);
        assert!(
            matches!(&cmds[..], [Cmd::PlanSessionDelete { reference }] if reference == "/s/a.jsonl")
        );
        let (app, _) = super::super::app::update(
            app,
            super::super::app::Input::Bg(super::super::app::Bg::SessionPlan(Ok((
                "/s/a.jsonl".into(),
                "t".into(),
                "the conversation and its working copy, with 2 file(s) changed NOT APPLIED to your folder".into(),
            )))),
        );
        assert!(
            app.confirm
                .as_ref()
                .unwrap()
                .workspace
                .contains("NOT APPLIED")
        );
        // Anything but y keeps it.
        let (app, cmds) = press(app, KeyCode::Char('n'), none);
        assert!(cmds.is_empty() && app.confirm.is_none());
        let (app, _) = press(app, KeyCode::Char('x'), none);
        let (app, _) = super::super::app::update(
            app,
            super::super::app::Input::Bg(super::super::app::Bg::SessionPlan(Ok((
                "/s/a.jsonl".into(),
                "t".into(),
                "the conversation only".into(),
            )))),
        );
        let (_, cmds) = press(app, KeyCode::Char('y'), none);
        assert!(
            matches!(&cmds[..], [Cmd::DeleteSession { reference }] if reference == "/s/a.jsonl")
        );
    }

    // The delete dialog names the conversation (title, age, messages: the drawer row's three
    // things), says it goes to the trash for 30 days, and keeps its key rule; after the delete
    // one note in the chat says where it went.
    #[test]
    fn the_delete_dialog_shows_the_conversation_and_says_it_goes_to_the_trash() {
        let title = "What weighs the most on the disk?";
        let asked = || {
            let mut app = super::super::tests_gallery::conversation(true);
            app.screen = Screen::Sessions;
            super::super::app::update(
                app,
                super::super::app::Input::Bg(super::super::app::Bg::SessionPlan(Ok((
                    "/s/2_b.jsonl".into(),
                    title.into(),
                    "the conversation only".into(),
                )))),
            )
            .0
        };
        let app = asked();
        let shown = super::super::tests::screen_text(&app, 100, 30);
        for want in [
            title,
            "26 min ago",
            "3 msgs",
            "trash for 30 days",
            "clio sessions restore",
        ] {
            assert!(shown.contains(want), "{want}\n{shown}");
        }
        assert!(!shown.contains("cannot be undone"), "{shown}");
        // Any other key cancels; y asks for the delete.
        let (cancelled, cmds) = press(asked(), KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(cmds.is_empty() && cancelled.confirm.is_none());
        let (_, cmds) = press(asked(), KeyCode::Char('y'), KeyModifiers::NONE);
        assert!(matches!(&cmds[..], [Cmd::DeleteSession { .. }]));
        // After it: the note, once, and the drawer's own line.
        let note = "deleted \"t\": it is in the trash for 30 days, `clio sessions restore abc` brings it back";
        let (app, _) = super::super::app::update(
            app,
            super::super::app::Input::Bg(super::super::app::Bg::SessionDeleted(
                Ok(note.into()),
                vec![],
            )),
        );
        let notes = app
            .chat
            .items
            .iter()
            .filter(|i| **i == Item::Note(note.into()))
            .count();
        assert_eq!(notes, 1);
        assert!(app.flash.as_deref().is_some_and(|f| f.contains("trash")));
    }

    // A row with no age and no message count has nothing to put after the title.
    #[test]
    fn the_delete_dialog_has_no_dangling_separator() {
        let mut app = super::super::tests_gallery::conversation(true);
        app.screen = Screen::Sessions;
        let (app, _) = super::super::app::update(
            app,
            super::super::app::Input::Bg(super::super::app::Bg::SessionPlan(Ok((
                "/s/not-listed.jsonl".into(),
                "Just a title".into(),
                "the conversation only".into(),
            )))),
        );
        let c = app.confirm.unwrap();
        assert_eq!(c.workspace, "\"Just a title\": the conversation only");
    }

    #[test]
    fn the_selector_opens_new_existing_and_explains_what_cannot_open() {
        use crate::cli::chat::{Row, RowKind};
        let row = |u: Option<&str>| Row {
            kind: RowKind::Session,
            title: "t".into(),
            detail: String::new(),
            messages: 2,
            last_ms: None,
            reference: "/s/a.jsonl".into(),
            unavailable: u.map(str::to_owned),
        };
        let mut app = App::new(
            "/p/proj",
            Theme {
                color: false,
                unicode: true,
                rgb: false,
            },
        );
        app.use_pi();
        app.sessions
            .set(vec![row(None), row(Some("not a Clio session"))]);
        let none = KeyModifiers::NONE;
        app.screen = Screen::Sessions;
        // "new session" opens nothing yet: its first message does.
        let (app, cmds) = press(app, KeyCode::Enter, none);
        assert!(cmds.is_empty() && app.chat.is_draft(), "{cmds:?}");
        assert_eq!(app.screen, Screen::Chat);
        let (app, cmds) = press(typed(app, "hola"), KeyCode::Enter, none);
        assert!(
            matches!(&cmds[..], [Cmd::ChatOpen { target: super::super::app::OpenTarget::New { project }, .. }] if project == "/p/proj")
        );
        assert_eq!(app.chat.queued.as_deref(), Some("hola"));
        let mut app = App::new(
            "/p/proj",
            Theme {
                color: false,
                unicode: true,
                rgb: false,
            },
        );
        app.use_pi();
        app.screen = Screen::Sessions;
        app.sessions
            .set(vec![row(None), row(Some("not a Clio session"))]);
        let (app, _) = press(app, KeyCode::Char('j'), none);
        let (app, cmds) = press(app, KeyCode::Enter, none);
        assert!(
            matches!(&cmds[..], [Cmd::ChatOpen { target: super::super::app::OpenTarget::Existing { reference }, .. }] if reference == "/s/a.jsonl")
        );
        let mut app = App {
            screen: Screen::Sessions,
            chat: Chat::default(),
            ..app
        };
        app.sessions.sel = 2;
        let (app, cmds) = press(app, KeyCode::Enter, none);
        assert!(cmds.is_empty() && app.notice.as_deref() == Some("not a Clio session"));
    }

    /// The Pi screen with two listed sessions, drawn once at `w` columns (a click needs a frame).
    fn pi_app(host: bool, w: u16) -> App {
        use crate::cli::chat::{Row, RowKind};
        let mut app = App::new(
            "/p/proj",
            Theme {
                color: false,
                unicode: true,
                rgb: false,
            },
        );
        app.use_pi();
        app.host = host;
        let row = |title: &str, reference: &str| Row {
            kind: RowKind::Session,
            title: title.into(),
            detail: String::new(),
            messages: 2,
            last_ms: None,
            reference: reference.into(),
            unavailable: None,
        };
        app.sessions.set(vec![
            row("session one", "/s/1_aaa.jsonl"),
            row("session two", "/s/2_bbb.jsonl"),
        ]);
        let _ = super::super::tests::screen_text(&app, w, 24);
        app
    }

    /// `pi_app` with the drawer open, drawn once.
    fn drawer_app(host: bool, w: u16) -> App {
        let (app, _) = press(pi_app(host, w), KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(app.screen, Screen::Sessions);
        let _ = super::super::tests::screen_text(&app, w, 24);
        // The key that opened it does not count for the quiet rule of a question.
        App {
            last_key_ms: None,
            ..app
        }
    }

    fn mouse(app: App, kind: crossterm::event::MouseEventKind, x: u16, y: u16) -> (App, Vec<Cmd>) {
        super::super::app::update(
            app,
            super::super::app::Input::Mouse(crossterm::event::MouseEvent {
                kind,
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            }),
        )
    }

    fn left() -> crossterm::event::MouseEventKind {
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left)
    }

    /// Where `text` is on the screen of `app` at `w` columns: (x, y).
    fn find(app: &App, w: u16, text: &str) -> (u16, u16) {
        let s = super::super::tests::screen_text(app, w, 24);
        s.lines()
            .enumerate()
            .find_map(|(y, l)| {
                l.find(text)
                    .map(|b| (l[..b].chars().count() as u16, y as u16))
            })
            .unwrap_or_else(|| panic!("{text:?} not on screen:\n{s}"))
    }

    // Review: every frame parsed and copied the whole conversation, ten times a second even when
    // nothing moved. Now an idle screen is not redrawn, a second frame over an unchanged
    // conversation parses no markdown, and a streaming answer is parsed again only when it grew.
    #[test]
    fn an_idle_screen_is_not_redrawn_and_an_unchanged_conversation_is_not_parsed_again() {
        use super::super::app::{Bg, Input, update};
        let mut a = pi_app(false, 100);
        a.chat = chat();
        for i in 0..400 {
            a.chat.items.push(Item::User(format!("pregunta {i}")));
            a.chat.items.push(Item::Assistant {
                text: format!("**respuesta** {i}\n\n- uno\n- dos\n\n```\ncódigo\n```"),
                live: false,
            });
        }
        let (a, _) = update(a, Input::Tick(60_000));
        assert!(!a.animating(), "an idle chat needs no frame from the clock");
        let draw = |a: &App| super::super::tests::screen_text(a, 100, 30);
        draw(&a);
        let parsed = a.chat.parsed.get();
        assert_eq!(parsed, 400);
        draw(&a);
        assert_eq!(a.chat.parsed.get(), parsed, "the second frame parsed again");

        // A streaming answer: parsed again only when its text changed.
        let mut a = a;
        a.chat.running = true;
        assert!(a.animating(), "the spinner and the seconds move");
        a.chat.on_event(&delta("hola"));
        draw(&a);
        draw(&a);
        assert_eq!(a.chat.parsed.get(), parsed + 1);
        a.chat.on_event(&delta(" mundo"));
        draw(&a);
        assert_eq!(a.chat.parsed.get(), parsed + 2);

        // The quiet second of a question counts down on its own; once it passed, nothing moves.
        a.chat.running = false;
        let (a, _) = update(
            a,
            Input::Bg(Bg::ChatAsks(vec![(
                "a1".into(),
                "«delete» rm -r x".into(),
                None,
            )])),
        );
        assert!(a.animating());
        let (a, _) = update(a, Input::Tick(61_000));
        assert!(!a.animating());
    }

    // Only the rows in the window are built, and they are exactly the rows of the whole
    // conversation there, at every scroll, the failure's lines at the end included.
    #[test]
    fn the_window_is_the_slice_of_the_whole_conversation_at_every_scroll() {
        let mut a = pi_app(false, 100);
        a.chat = chat();
        for i in 0..6 {
            a.chat
                .items
                .push(Item::User(format!("pregunta {i}\nen dos líneas")));
            a.chat.items.push(tool_item(i));
            a.chat.items.push(tool_item(i + 10));
            a.chat.items.push(Item::Assistant {
                text: format!("respuesta {i}\n\n- a\n- b"),
                live: false,
            });
            a.chat.items.push(Item::Note(format!("nota {i}")));
        }
        let t = a.theme;
        let tail = || vec![Line::from(""), Line::from("failed"), Line::from("fin")];
        let text = |v: &[Line]| v.iter().map(ToString::to_string).collect::<Vec<_>>();
        let (all, start, max) = conversation(&a, t, 60, tail(), 10_000);
        assert_eq!((start, max), (0, 0));
        for h in [1, 7, 20] {
            for scroll in 0..all.len() {
                a.chat.scroll = scroll;
                let (rows, start, max) = conversation(&a, t, 60, tail(), h);
                assert_eq!(max, all.len().saturating_sub(h));
                let end = (start + h).min(all.len());
                assert_eq!(
                    text(&rows),
                    text(&all[start..end]),
                    "h {h}, scroll {scroll}"
                );
            }
        }
    }

    fn tool_item(i: usize) -> Item {
        Item::Tool {
            id: format!("t{i}"),
            name: "bash".into(),
            summary: format!("ls {i}"),
            reason: String::new(),
            state: ToolState::Ok,
            ms: Some(5),
        }
    }

    // Review: with a box open (`/usage`, `/actions`…) the composer kept the accent border and the
    // terminal's cursor, as if typing went there.
    #[test]
    fn a_box_over_the_chat_takes_the_focus_from_the_composer() {
        let mut app = pi_app(false, 100);
        app.theme = Theme {
            color: true,
            unicode: true,
            rgb: false,
        };
        let (x, y) = find(&app, 100, "Type a message");
        let look = |app: &App| {
            let mut term =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 24)).unwrap();
            term.draw(|f| super::super::app::draw(f, app)).unwrap();
            let border = term.backend().buffer()[(x - 4, y)].style();
            (
                border,
                format!("{:?}", term.backend()).contains("cursor: true"),
            )
        };
        let (focused, cursor) = look(&app);
        assert!(cursor);
        app.modal = Some(super::super::commands::Modal::text(
            "Usage",
            vec!["x".into()],
        ));
        let (with_box, cursor) = look(&app);
        assert!(!cursor && with_box != focused, "{with_box:?}");
    }

    // `clio` opens on the chat alone: no sessions in sight, the composer with the keys and the
    // terminal's cursor in it; nothing is created until the first message, which goes out once
    // the session opens.
    #[test]
    fn clio_opens_on_the_chat_alone_with_the_keys_in_the_composer() {
        use super::super::app::{Bg, ChatInfo, Input, update};
        let app = pi_app(false, 100);
        assert_eq!(app.screen, Screen::Chat);
        assert!(app.chat.is_draft());
        assert!(
            !app.start_cmds()
                .iter()
                .any(|c| matches!(c, Cmd::ChatOpen { .. })),
            "opening Clio creates no session"
        );
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 24)).unwrap();
        term.draw(|f| super::super::app::draw(f, &app)).unwrap();
        let s = super::super::tests::screen_text(&app, 100, 24);
        assert!(
            !s.contains("session two") && !s.contains("╭ sessions"),
            "{s}"
        );
        assert!(
            s.contains("How can I help?") && s.contains("Type a message"),
            "{s}"
        );
        assert!(s.contains("Ctrl-S sessions"), "{s}");
        // The cursor sits where the placeholder starts, inside the composer's box.
        let (x, y) = find(&app, 100, "Type a message");
        let at = term.get_cursor_position().unwrap();
        assert_eq!((at.x, at.y), (x, y));
        let sent = |app: App| {
            let (app, cmds) = press(typed(app, "hola"), KeyCode::Enter, KeyModifiers::NONE);
            let seq = match &cmds[..] {
                [Cmd::ChatOpen { seq, .. }] => *seq,
                _ => panic!("{cmds:?}"),
            };
            (app, seq)
        };
        let (app, seq) = sent(pi_app(false, 100));
        let info = ChatInfo {
            id: "sid".into(),
            workspace: String::new(),
            history: vec![],
            refresh: None,
            presentation: true,
            notes: vec![],
        };
        let (app, cmds) = update(
            app,
            Input::Bg(Bg::ChatOpened {
                seq,
                result: Ok(Box::new(info)),
            }),
        );
        assert_eq!(
            cmds,
            vec![Cmd::ChatSend {
                text: "hola".into()
            }]
        );
        assert!(app.chat.running && app.chat.items == vec![Item::User("hola".into())]);
        // It could not open: the message is back in the field, and Enter tries again.
        let (app, seq) = sent(pi_app(false, 100));
        let (app, _) = update(
            app,
            Input::Bg(Bg::ChatOpened {
                seq,
                result: Err("no key".into()),
            }),
        );
        assert_eq!(app.chat.input, "hola");
        assert_eq!(app.chat.cursor, 4);
        assert!(app.chat.is_draft());
        assert!(matches!(&app.chat.items[..], [Item::Error(e)] if e == "no key"));
    }

    // Ctrl-S (or F2) opens the drawer with the keys and closes it again, as Esc does; Tab no
    // longer moves anything. A terminal narrower than `WIDE` gives the drawer its whole width.
    #[test]
    fn ctrl_s_toggles_the_drawer_and_a_narrow_terminal_gives_it_the_whole_width() {
        let shows =
            |app: &App, w, text| super::super::tests::screen_text(app, w, 24).contains(text);
        let app = pi_app(false, 100);
        let (app, _) = press(app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.screen, Screen::Chat);
        assert!(!shows(&app, 100, "session two"));
        for open in [
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE),
        ] {
            for close in [
                KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
                KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            ] {
                let (app, _) = press(pi_app(false, 100), open.code, open.modifiers);
                assert_eq!(app.screen, Screen::Sessions);
                assert!(shows(&app, 100, "session two") && shows(&app, 100, "Type a message"));
                // Narrow: the drawer only, the chat behind it.
                assert!(shows(&app, 70, "session two") && !shows(&app, 70, "Type a message"));
                let (app, _) = press(app, close.code, close.modifiers);
                assert_eq!(app.screen, Screen::Chat, "{close:?}");
                assert!(!shows(&app, 100, "session two"));
            }
        }
    }

    // Enter on a session in the drawer opens it, closes the drawer and gives the keys to the
    // composer; Enter on the session already open only closes it.
    #[test]
    fn enter_in_the_drawer_opens_the_session_and_closes_the_drawer() {
        let none = KeyModifiers::NONE;
        let (app, _) = press(drawer_app(false, 100), KeyCode::Char('j'), none);
        let (app, _) = press(app, KeyCode::Char('j'), none);
        let (app, cmds) = press(app, KeyCode::Enter, none);
        assert!(
            matches!(&cmds[..], [Cmd::ChatOpen { target: super::super::app::OpenTarget::Existing { reference }, .. }] if reference == "/s/2_bbb.jsonl"),
            "{cmds:?}"
        );
        assert_eq!(app.screen, Screen::Chat);
        // "new session" while the chat is a draft: nothing to open, the drawer closes.
        let (app, cmds) = press(drawer_app(false, 100), KeyCode::Enter, none);
        assert!(cmds.is_empty() && app.screen == Screen::Chat && app.chat.is_draft());
    }

    #[test]
    fn a_click_on_a_row_opens_it_a_click_outside_closes_the_drawer_and_the_wheel_scrolls() {
        let app = drawer_app(false, 100);
        let (x, y) = find(&app, 100, "session two");
        let (app, cmds) = mouse(app, left(), x, y);
        assert!(
            matches!(&cmds[..], [Cmd::ChatOpen { target: super::super::app::OpenTarget::Existing { reference }, .. }] if reference == "/s/2_bbb.jsonl"),
            "{cmds:?}"
        );
        assert_eq!(app.screen, Screen::Chat);
        // The wheel over the drawer moves its selection; a click outside it closes it.
        let mut app = drawer_app(false, 100);
        app.sessions.sel = 0;
        let (app, _) = mouse(app, crossterm::event::MouseEventKind::ScrollDown, x, y);
        assert_eq!(app.sessions.sel, 1);
        let (app, cmds) = mouse(app, left(), 70, 10);
        assert!(cmds.is_empty());
        assert_eq!(app.screen, Screen::Chat);
        // Closed, the left of the screen is the chat: a click there opens nothing.
        let (app, cmds) = mouse(app, left(), x, y);
        assert!(cmds.is_empty() && app.screen == Screen::Chat);
        // The wheel over the chat scrolls the conversation, within what was drawn.
        let mut app = pi_app(false, 100);
        app.chat = Chat::opening(1, 0);
        app.chat.opened("sid".into(), String::new(), &[], None, 0);
        for i in 0..60 {
            app.chat.items.push(Item::Note(format!("nota {i}")));
        }
        let _ = super::super::tests::screen_text(&app, 100, 24);
        let (app, _) = mouse(app, crossterm::event::MouseEventKind::ScrollUp, 50, 8);
        assert_eq!(app.chat.scroll, 3);
        assert!(super::super::tests::screen_text(&app, 100, 24).contains("more below"));
        let (app, _) = mouse(app, crossterm::event::MouseEventKind::ScrollDown, 50, 8);
        assert_eq!(app.chat.scroll, 0);
        // While a turn runs, a click on another session does not leave it (as Esc never did).
        let mut app = drawer_app(false, 100);
        app.chat = Chat::opening(1, 0);
        app.chat.opened("sid".into(), String::new(), &[], None, 0);
        app.chat.running = true;
        let (app, cmds) = mouse(app, left(), x, y);
        assert!(cmds.is_empty() && app.flash.is_some(), "{cmds:?}");
        assert!(
            super::super::tests::screen_text(&app, 100, 24).contains("the agent is working"),
            "the drawer says why"
        );
    }

    // A set-aside action waits for keys only: no click, on the dialog or anywhere, answers it,
    // not even when `s` was already typed.
    #[test]
    fn a_click_never_answers_the_question_of_a_set_aside_action() {
        use super::super::app::{Action, Confirm};
        let mut app = drawer_app(true, 100);
        let (x, y) = find(&app, 100, "session two");
        let mut c = Confirm::new(Action::Approve, "a1".into(), 0, "«delete» rm -r x".into());
        c.armed = true;
        app.confirm = Some(c);
        let (dx, dy) = find(&app, 100, "yes, run it");
        for (x, y) in [(x, y), (dx, dy), (70, 10)] {
            for kind in [
                left(),
                crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
                crossterm::event::MouseEventKind::ScrollDown,
            ] {
                let (next, cmds) = mouse(app, kind, x, y);
                app = next;
                assert!(cmds.is_empty(), "{cmds:?}");
                assert!(app.confirm.as_ref().is_some_and(|c| c.armed));
                assert_eq!(app.screen, Screen::Sessions);
            }
        }
        // The keys still answer it.
        let (app, cmds) = press(app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            cmds,
            vec![Cmd::ChatAnswer {
                id: "a1".into(),
                yes: true
            }]
        );
        assert!(app.confirm.is_none());
    }

    // The composer edits where its cursor is: arrows, Home/End and Ctrl-A/E, words, Backspace and
    // Delete, Ctrl-W, Ctrl-U, Alt-Enter, ↑/↓ between its rows, a paste; ↑ on its first row scrolls
    // the conversation instead.
    #[test]
    fn the_composer_edits_at_the_cursor_with_the_usual_keys() {
        let none = KeyModifiers::NONE;
        let ctrl = KeyModifiers::CONTROL;
        let mut app = typed(pi_app(false, 100), "hola mundo");
        for (code, mods) in [
            (KeyCode::Left, ctrl),
            (KeyCode::Left, none),
            (KeyCode::Right, none),
        ] {
            app = press(app, code, mods).0;
        }
        app = typed(app, "ñ");
        assert_eq!(app.chat.input, "hola ñmundo");
        app = press(app, KeyCode::Char('a'), ctrl).0;
        app = press(app, KeyCode::Delete, none).0;
        assert_eq!(app.chat.input, "ola ñmundo");
        app = press(app, KeyCode::Char('e'), ctrl).0;
        app = press(app, KeyCode::Backspace, none).0;
        app = press(app, KeyCode::Char('w'), ctrl).0;
        assert_eq!(app.chat.input, "ola ");
        app = press(app, KeyCode::Enter, KeyModifiers::ALT).0;
        app = typed(app, "segunda");
        app = press(app, KeyCode::Home, none).0;
        app = press(app, KeyCode::Up, none).0;
        assert_eq!(app.chat.cursor, 0, "↑ goes to the row above");
        app = press(app, KeyCode::Down, none).0;
        app = press(app, KeyCode::End, none).0;
        assert_eq!(app.chat.cursor, app.chat.input.len());
        app = press(app, KeyCode::Char('u'), ctrl).0;
        assert_eq!(app.chat.input, "ola \n");
        super::paste(&mut app, "pegado\u{1b}[2J");
        assert_eq!(
            (app.chat.input.as_str(), app.chat.cursor),
            ("ola \npegado", 11)
        );
        // A typed bidi mark never gets in.
        app = press(app, KeyCode::Char('\u{202e}'), none).0;
        assert_eq!(app.chat.input, "ola \npegado");
        // On the first row ↑ scrolls the conversation, and Ctrl-End comes back to the newest.
        let mut app = pi_app(false, 100);
        app.chat = Chat::opening(1, 0);
        app.chat.opened("sid".into(), String::new(), &[], None, 0);
        for i in 0..60 {
            app.chat.items.push(Item::Note(format!("nota {i}")));
        }
        let _ = super::super::tests::screen_text(&app, 100, 24);
        let app = press(app, KeyCode::Up, none).0;
        assert_eq!(app.chat.scroll, 1);
        let app = press(app, KeyCode::Home, ctrl).0;
        assert_eq!(app.chat.scroll, app.chat.scroll_max.get());
        let app = press(app, KeyCode::End, ctrl).0;
        assert_eq!(app.chat.scroll, 0);
    }

    // What the model says is drawn as markdown, and an escape sequence in it, in a link, a
    // heading or a code block, never reaches the screen.
    #[test]
    fn the_answer_is_drawn_as_markdown_and_clean() {
        let mut app = pi_app(false, 100);
        app.chat = Chat::opening(1, 0);
        app.chat.opened("sid".into(), String::new(), &[], None, 0);
        app.chat.items.push(Item::Assistant {
            text: "# Tí\u{1b}]0;x\u{7}tulo\n\n- **uno** y `dos`\n- [tres](http://e.com/\u{1b}[2J)\n\n```\nco\u{1b}[31mde &#27;[2J\n```".into(),
            live: false,
        });
        let s = super::super::tests::screen_text(&app, 100, 24);
        super::super::tests::assert_clean(&s);
        for want in ["Título", "• uno y dos", "• tres (http://e.com/)", "│ code"] {
            assert!(s.contains(want), "{want}\n{s}");
        }
        assert!(
            !s.contains("**") && !s.contains('`') && !s.contains("# "),
            "{s}"
        );
    }

    #[test]
    fn a_new_session_says_where_it_works() {
        let text = |host| super::super::tests::screen_text(&pi_app(host, 100), 100, 24);
        let host = text(true);
        assert!(
            host.contains("on this server") && !host.contains("in a copy"),
            "{host}"
        );
        assert!(text(false).contains("in a copy"));
    }

    // Second audit, finding 3: a model that prints one huge word with no spaces must not freeze
    // the chat. The whole history is wrapped on every frame, so the cost has to be linear.
    #[test]
    fn wrapping_one_huge_word_takes_linear_time() {
        // Multi-byte characters are the slow case (a character count is not vectorised).
        let word = "漢".repeat(4_000_000);
        let t = std::time::Instant::now();
        let rows = wrap(&word, 80);
        let took = t.elapsed();
        // Two columns each: 40 to a row of 80.
        assert_eq!(rows.len(), 4_000_000usize.div_ceil(40));
        assert!(rows.iter().all(|r| text::width(r) <= 80));
        assert_eq!(rows.concat(), word, "nothing lost, every cut on a boundary");
        assert!(took < std::time::Duration::from_secs(2), "took {took:?}");
    }

    #[test]
    fn wrapping_never_exceeds_the_width_and_keeps_every_word() {
        let t =
            "uno dos tres cuatro cinco seis siete ocho nueve diez supercalifragilisticoespialidoso";
        let rows = wrap(t, 12);
        assert!(rows.iter().all(|r| text::width(r) <= 12), "{rows:?}");
        assert_eq!(rows.join("").replace(' ', ""), t.replace(' ', ""));
        assert_eq!(wrap("a\n\nb", 10), vec!["a", "", "b"]);
    }
}
