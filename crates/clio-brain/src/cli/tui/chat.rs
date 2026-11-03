//! The chat over a Pi session: the conversation, what the agent does as it does it (text as it
//! streams, one compact line per tool), the field to write in, and, with debug on (Ctrl-D), the
//! raw events Pi sends. Use and context are in the status bar. Everything Pi, the model or a tool
//! said is stored as it came and sanitized when drawn.

use std::cell::Cell;
use std::collections::VecDeque;

use clio_brain::pi::rpc::{Event, UsageMeter};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use serde_json::Value;

use super::app::{App, Cmd, Screen, clean, section};
use super::keys::{Act, lookup};
use super::sanitize;
use super::theme::Theme;
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
    const MARK: &str = "\n[… el resto se omitió]";
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
    pub running: bool,
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
    pub raw: VecDeque<String>,
    /// Bytes of text taken from Pi's events so far (see `MAX_HELD`).
    held: usize,
    /// Lines scrolled up from the newest.
    pub scroll: usize,
    pub view_h: Cell<u16>,
    pub view_w: Cell<u16>,
    streamed: bool,
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
            running: false,
            aborting: false,
            meter: UsageMeter::default(),
            turns: 0,
            started_ms: now_ms,
            tainted: false,
            egress_seen: 0,
            masked_seen: 0,
            raw: VecDeque::new(),
            held: 0,
            scroll: 0,
            view_h: Cell::new(0),
            view_w: Cell::new(0),
            streamed: false,
        }
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
        for m in history {
            self.add_history(m);
        }
        if let Some(r) = refresh {
            self.refresh(&r);
        }
    }

    fn add_history(&mut self, m: &Value) {
        let text = content_text(&m["content"]);
        match m["role"].as_str() {
            Some("user") if !text.is_empty() => self.items.push(Item::User(text)),
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
                let state = state_of(m["isError"] == true, &text);
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
    }

    /// The host struck values out of what Pi read (S-18): say how many are new, quietly. The
    /// person supervises here; `total` is the whole session's count so far.
    pub fn masked(&mut self, total: u32) {
        if total > self.masked_seen {
            let n = total - self.masked_seen;
            let (one, many) = ("valor", "valores");
            self.items.push(Item::Note(format!(
                "Clio tachó {n} {} con forma de llave en lo que leyó Pi: el modelo no los ve",
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
                "la jaula negó una conexión de Pi a {d}"
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
            Event::AgentSettled { .. } => self.running = false,
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
                            .map_or_else(|| "el modelo devolvió un error".into(), note),
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
                result,
                is_error,
                duration_ms,
                ..
            } => {
                let state = state_of(*is_error, &content_text(&result["content"]));
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
                .push(Item::Note(format!("compactando el contexto ({reason})…"))),
            Event::CompactionEnd {
                aborted,
                error,
                tokens_before,
                tokens_after,
                ..
            } => self.items.push(Item::Note(match (aborted, error) {
                (_, Some(e)) => format!("la compactación falló: {}", note(e)),
                (true, _) => "compactación cancelada".into(),
                _ => format!(
                    "contexto compactado{}",
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
                "reintento {attempt}/{max_attempts} en {} ({})",
                usage::ms_label(*delay_ms),
                note(error)
            ))),
            Event::AutoRetryEnd {
                success: false,
                final_error,
                ..
            } => self.items.push(Item::Error(format!(
                "los reintentos no bastaron: {}",
                note(final_error.as_deref().unwrap_or_default())
            ))),
            Event::ExtensionError { error, .. } => self.items.push(Item::Note(format!(
                "aviso de la extensión: {}",
                note(error)
            ))),
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

/// A tool that failed because the guard or the gateway refused it is "negado", not "falló".
fn state_of(is_error: bool, text: &str) -> ToolState {
    let t = text.to_lowercase();
    if !is_error {
        ToolState::Ok
    } else if t.contains("clio-guard:") || t.contains("denied") || t.contains("denegad") {
        ToolState::Denied
    } else {
        ToolState::Failed
    }
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
    let ctx = app.ctx();
    let Some(act) = lookup(ctx, k) else {
        return edit(app, k);
    };
    let page = usize::from(app.chat.view_h.get().max(2) - 1);
    let max = usize::MAX / 2;
    match act {
        Act::Quit => app.quit = true,
        Act::Back => {
            app.screen = Screen::Sessions;
            app.sessions.loading = true;
            return vec![Cmd::ChatClose, Cmd::LoadSessions, Cmd::LoadHome];
        }
        Act::Cancel if !app.chat.aborting => {
            app.chat.aborting = true;
            app.chat
                .items
                .push(Item::Note("abortando el turno…".into()));
            return vec![Cmd::ChatAbort];
        }
        Act::Help => app.open_help(),
        Act::Clear => app.chat.input.clear(),
        Act::Newline => app.chat.input.push('\n'),
        Act::Up => app.chat.scroll = (app.chat.scroll + 1).min(max),
        Act::Down => app.chat.scroll = app.chat.scroll.saturating_sub(1),
        Act::PageUp => app.chat.scroll = (app.chat.scroll + page).min(max),
        Act::PageDown => app.chat.scroll = app.chat.scroll.saturating_sub(page),
        Act::Open => return send(app),
        _ => {}
    }
    vec![]
}

fn send(app: &mut App) -> Vec<Cmd> {
    let c = &mut app.chat;
    let text = c.input.trim().to_owned();
    if text.is_empty() {
        return vec![];
    }
    if c.opening || c.failed.is_some() {
        app.flash = Some("la sesión todavía no está lista".into());
        return vec![];
    }
    c.input.clear();
    c.scroll = 0;
    c.items.push(Item::User(text.clone()));
    if c.running {
        return vec![Cmd::ChatSteer { text }];
    }
    c.running = true;
    c.aborting = false;
    vec![Cmd::ChatSend { text }]
}

/// Plain characters, Backspace.
fn edit(app: &mut App, k: &KeyEvent) -> Vec<Cmd> {
    match k.code {
        KeyCode::Backspace => {
            app.chat.input.pop();
        }
        KeyCode::Char(c)
            if !c.is_control()
                && !k
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            app.chat.input.push(c);
        }
        _ => {}
    }
    vec![]
}

/// Pasted text goes through the sanitizer: no escape sequences, and it never presses Enter.
pub fn paste(app: &mut App, text: &str) {
    app.chat.input.push_str(&sanitize::multiline(text));
}

/// Breaks `text` into rows of at most `w` characters, at spaces when it can.
pub fn wrap(text: &str, w: usize) -> Vec<String> {
    let w = w.max(8);
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        for word in para.split(' ') {
            let mut word = word;
            // Cut a word longer than the line in one pass: counting what is left on every turn
            // made a single huge word cost the square of its length, on every frame.
            while let Some((at, _)) = word.char_indices().nth(w) {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                out.push(word[..at].to_owned());
                word = &word[at..];
            }
            let need = cur.chars().count() + usize::from(!cur.is_empty()) + word.chars().count();
            if need > w && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
        }
        out.push(cur);
    }
    out
}

/// What a tool does, as a title ("Leer archivo"), like Aria's steps.
fn title(name: &str, reason: &str) -> String {
    if !reason.is_empty() {
        return clean(reason);
    }
    match name {
        "read" => "Leer archivo".into(),
        "write" => "Escribir archivo".into(),
        "edit" => "Editar archivo".into(),
        "bash" => "Ejecutar comando".into(),
        "ls" => "Listar carpeta".into(),
        "find" => "Buscar archivos".into(),
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

/// A kind of block; a blank line separates two blocks of different kinds, and the speaker's label
/// goes on top of the user's and the agent's blocks.
#[derive(PartialEq, Clone, Copy)]
enum Block {
    User,
    Agent,
    Tools,
    Notes,
}

fn conversation(app: &App, t: Theme, w: usize) -> Vec<Line<'static>> {
    let c = &app.chat;
    let mut lines: Vec<Line> = Vec::new();
    let body = w.saturating_sub(2);
    let mut last: Option<Block> = None;
    for it in &c.items {
        let kind = match it {
            Item::User(_) => Block::User,
            Item::Assistant { .. } => Block::Agent,
            Item::Tool { .. } => Block::Tools,
            Item::Note(_) | Item::Error(_) => Block::Notes,
        };
        if last != Some(kind) && last.is_some() {
            lines.push(Line::from(""));
        }
        last = Some(kind);
        let indented =
            |text: &str, style: ratatui::style::Style, lines: &mut Vec<Line<'static>>| {
                for row in wrap(&sanitize::multiline(text), body) {
                    lines.push(Line::from(vec![Span::raw("  "), Span::styled(row, style)]));
                }
            };
        match it {
            Item::User(text) => {
                // The accent marks you, as the composer's `›` does.
                for (i, row) in wrap(&sanitize::multiline(text), body)
                    .into_iter()
                    .enumerate()
                {
                    let lead = if i == 0 {
                        Span::styled(format!("{} ", t.sel()), t.accent())
                    } else {
                        Span::raw("  ")
                    };
                    lines.push(Line::from(vec![lead, Span::styled(row, t.accent())]));
                }
            }
            Item::Assistant { text, .. } => {
                indented(text, t.plain(), &mut lines);
            }
            Item::Tool {
                name,
                summary,
                reason,
                state,
                ms,
                ..
            } => {
                let (mark, word, style) = match state {
                    ToolState::Running => (t.spinner(app.now_ms), "", t.accent()),
                    ToolState::Ok => (t.check(), "", t.ok()),
                    ToolState::Failed => (t.cross(), " falló", t.fail()),
                    ToolState::Denied => (t.cross(), " negado", t.fail()),
                };
                let dur = ms.map_or_else(String::new, |m| format!("  {}", usage::ms_label(m)));
                let tail = format!("{word}{dur}");
                let label = title(name, reason);
                let summary = chip(name, summary);
                let room = w.saturating_sub(4 + label.chars().count() + 2 + tail.chars().count());
                let summary = super::debug_view::fit(&clean(&summary), room.max(8), t);
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(format!("{mark} "), style),
                    Span::styled(label, t.bold()),
                    Span::styled(format!("  {summary}"), t.dim()),
                    Span::styled(
                        tail,
                        if *state == ToolState::Ok || *state == ToolState::Running {
                            t.dim()
                        } else {
                            style
                        },
                    ),
                ]));
            }
            Item::Note(text) => lines.push(Line::from(Span::styled(
                format!("  {} {}", t.dot(), clean(text)),
                t.dim(),
            ))),
            Item::Error(text) => {
                for (i, row) in wrap(&sanitize::multiline(text), body.saturating_sub(2))
                    .into_iter()
                    .enumerate()
                {
                    let lead = if i == 0 {
                        Span::styled(format!("  {} ", t.cross()), t.fail())
                    } else {
                        Span::raw("    ")
                    };
                    lines.push(Line::from(vec![lead, Span::styled(row, t.fail())]));
                }
            }
        }
    }
    lines
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let c = &app.chat;
    let input_text = sanitize::multiline(&c.input);
    let input_rows: Vec<String> = wrap(&input_text, usize::from(area.width).saturating_sub(2));
    let input_h = u16::try_from(input_rows.len()).unwrap_or(4).clamp(1, 4);
    let dbg_h: u16 = if app.debug {
        (area.height / 3).clamp(4, 10)
    } else {
        0
    };
    let [head, conv, dbg, input] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(dbg_h),
        Constraint::Length(input_h + 1),
    ])
    .areas(area);

    let state = if let Some(e) = &c.failed {
        Span::styled(format!("{} {}", t.cross(), clean(e)), t.fail())
    } else if c.opening {
        Span::styled(
            format!(
                "{} copiando el proyecto y arrancando Pi…",
                t.spinner(app.now_ms)
            ),
            t.accent(),
        )
    } else if c.aborting && c.running {
        Span::styled(format!("{} abortando…", t.bang()), t.warn())
    } else if c.running {
        Span::styled(format!("{} trabajando…", t.spinner(app.now_ms)), t.accent())
    } else {
        Span::styled(format!("{} listo", t.check()), t.ok())
    };
    let who = if c.session_id.is_empty() {
        clean(&c.title)
    } else {
        super::home::short_id(&clean(&c.session_id))
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            state,
            Span::styled(format!("   {who}"), t.dim()),
            Span::styled(format!("   {}", clean(&c.workspace)), t.dim()),
        ])),
        head,
    );

    // The conversation, anchored to the bottom.
    let rows = usize::from(conv.height);
    c.view_h.set(conv.height);
    c.view_w.set(conv.width);
    let all = conversation(app, t, usize::from(conv.width));
    let end = all
        .len()
        .saturating_sub(c.scroll.min(all.len().saturating_sub(rows)));
    let start = end.saturating_sub(rows);
    let mut shown: Vec<Line> = all[start..end].to_vec();
    if shown.is_empty() && !c.opening && c.failed.is_none() {
        shown.push(Line::from(Span::styled(
            "escribe abajo y pulsa Enter",
            t.dim(),
        )));
    }
    f.render_widget(Paragraph::new(shown), conv);

    if app.debug {
        let mut lines = vec![section(t, "eventos de Pi (crudos)", dbg.width)];
        let room = usize::from(dbg.height).saturating_sub(1);
        let skip = c.raw.len().saturating_sub(room);
        lines.extend(c.raw.iter().skip(skip).map(|r| {
            Line::from(Span::styled(
                super::debug_view::fit(&clean(r), usize::from(dbg.width), t),
                t.dim(),
            ))
        }));
        f.render_widget(Paragraph::new(lines), dbg);
    }

    let mut lines = vec![section(t, "mensaje", input.width)];
    let skip = input_rows.len().saturating_sub(usize::from(input_h));
    let n = input_rows.len();
    for (i, row) in input_rows.into_iter().enumerate().skip(skip) {
        let lead = if i == 0 {
            Span::styled(format!("{} ", t.sel()), t.accent())
        } else {
            Span::raw("  ")
        };
        let mut spans = vec![lead, Span::raw(row)];
        if i + 1 == n {
            spans.push(Span::styled(t.cursor(), t.accent()));
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines), input);
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
            json!({"command":"ls","reason":" Revisar la estructura\nmás"}),
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
        assert_eq!(titles, ["Revisar la estructura", "Leer archivo"]);
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
        assert!(notes(&c)[0].contains("2 valores") && notes(&c)[0].contains("tachó"));
        c.masked(3);
        assert_eq!(notes(&c).len(), 2);
        assert!(
            notes(&c)[1].contains("1 valor ") || notes(&c)[1].contains("1 valor"),
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
        assert!(notes(&c)[0].contains("example.com:443") && notes(&c)[0].contains("jaula"));
        let more = vec![
            "example.com:443".to_owned(),
            "GET (no es CONNECT)".to_owned(),
        ];
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
            },
        );
        app.use_pi();
        app.screen = Screen::Chat;
        app.chat = Chat::opening(1, 0);
        app.chat.opened("sid".into(), String::new(), &[], None, 0);
        app
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
        // The run ends: Esc leaves to the selector and closes the session; Ctrl-C quits.
        let (mut app, _) = super::super::app::update(
            app,
            super::super::app::Input::Bg(super::super::app::Bg::ChatSettled {
                result: Ok(clio_brain::pi::rpc::RunSummary::default()),
                refresh: None,
            }),
        );
        assert!(!app.chat.running);
        let (left, cmds) = press(std::mem::replace(&mut app, open_chat()), KeyCode::Esc, none);
        assert_eq!(left.screen, Screen::Sessions);
        assert!(cmds.contains(&Cmd::ChatClose));
        let (quit, _) = press(app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(quit.quit);
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
                .any(|i| matches!(i, Item::Error(m) if m.contains("tope de pasos"))),
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
                .any(|i| matches!(i, Item::Note(m) if m.contains("nivel 2"))),
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

    #[test]
    fn a_chat_that_is_still_opening_does_not_send() {
        let mut app = open_chat();
        app.chat.opening = true;
        let (app, cmds) = press(typed(app, "hola"), KeyCode::Enter, KeyModifiers::NONE);
        assert!(cmds.is_empty() && !app.chat.running);
        assert!(app.flash.is_some() && app.chat.input == "hola");
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
            },
        );
        app.use_pi();
        app.sessions
            .set(vec![row(None), row(Some("no es de Clio"))]);
        let none = KeyModifiers::NONE;
        let (app, cmds) = press(app, KeyCode::Enter, none);
        assert!(
            matches!(&cmds[..], [Cmd::ChatOpen { target: super::super::app::OpenTarget::New { project }, .. }] if project == "/p/proj")
        );
        assert_eq!(app.screen, Screen::Chat);
        let mut app = App::new(
            "/p/proj",
            Theme {
                color: false,
                unicode: true,
            },
        );
        app.use_pi();
        app.sessions
            .set(vec![row(None), row(Some("no es de Clio"))]);
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
        assert!(cmds.is_empty() && app.notice.as_deref() == Some("no es de Clio"));
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
        assert_eq!(rows.len(), 4_000_000usize.div_ceil(80));
        assert!(rows.iter().all(|r| r.chars().count() <= 80));
        assert_eq!(rows.concat(), word, "nothing lost, every cut on a boundary");
        assert!(took < std::time::Duration::from_secs(2), "took {took:?}");
    }

    #[test]
    fn wrapping_never_exceeds_the_width_and_keeps_every_word() {
        let t =
            "uno dos tres cuatro cinco seis siete ocho nueve diez supercalifragilisticoespialidoso";
        let rows = wrap(t, 12);
        assert!(rows.iter().all(|r| r.chars().count() <= 12), "{rows:?}");
        assert_eq!(rows.join("").replace(' ', ""), t.replace(' ', ""));
        assert_eq!(wrap("a\n\nb", 10), vec!["a", "", "b"]);
    }
}
