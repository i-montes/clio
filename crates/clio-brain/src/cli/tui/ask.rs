//! Clio asks the person (`ask_person`, through Pi's own extension dialogs): a box titled
//! `Clio asks` with the question, its answers as a list (↑/↓ and Enter, or the digit), and
//! `Other…`, which opens a one-line field. Esc skips the question; it never aborts the turn.
//!
//! It is not the approval box (`Visto bueno`): another title and border, and nothing here can
//! approve an action set aside or take a secret. The question and the answers are the model's
//! text: cleaned, capped and drawn as plain text. Like an approval, it takes its first key only
//! after the quiet second (`App::quiet`), so what the person was typing never answers it.

use std::cell::Cell;
use std::hash::{DefaultHasher, Hash, Hasher};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};

use super::app::{self, App, Cmd};
use super::{sanitize, text};

/// The last row of every question the guard asks: the person types their own answer.
pub const OTHER: &str = "Other…";
/// How the guard joins an answer and its description in one option string.
const SEP: &str = " — ";
pub const MAX_QUESTION: usize = 300;
const MAX_LABEL: usize = 60;
const MAX_DESC: usize = 120;
const MAX_OPTIONS: usize = 4;
const MAX_TYPED: usize = 2000;
/// A dialog with no time of its own closes after this long (the guard always gives one).
const DEFAULT_WAIT_MS: u64 = 10 * 60 * 1000;

/// One question of the guard's, on screen or waiting its turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// The Pi dialog it answers now (the `select`, then the `input` after `Other…`).
    pub id: String,
    /// Only the session that asked can answer.
    pub session: String,
    pub text: String,
    /// Label and description, cleaned and capped.
    pub options: Vec<(String, String)>,
    /// The option strings as Pi sent them: an answer is one of these, exactly.
    raw: Vec<String>,
    /// The guard offered `Other…` (it always does).
    pub other: bool,
    pub sel: usize,
    /// A key was taken: the quiet second is over for this question.
    pub ready: bool,
    /// `Other…`: what the person typed so far.
    pub typing: Option<String>,
    /// `Other…` went to Pi; its `input` dialog has not come yet. An answer decided meanwhile
    /// waits in `decided` (`Some(None)`: skipped).
    pub awaiting: bool,
    decided: Option<Option<String>>,
    pub shown_ms: u64,
    /// Pi gives up on the dialog then (it answers the model by itself): the box goes too.
    pub deadline_ms: u64,
    /// Where the last draw put the option rows, how many, and what they said: a click picks
    /// a row only if the row drawn there is still that row.
    pub rows_at: Cell<(Rect, usize, u64)>,
}

/// Model text for the box: one line, cleaned, at most `max` characters.
fn clip(s: &str, max: usize) -> String {
    sanitize::line(s).trim().chars().take(max).collect()
}

impl Question {
    /// From the guard's `select`: `label — description` strings, `Other…` last.
    pub fn from_select(
        id: &str,
        session: &str,
        title: &str,
        options: &[String],
        timeout_ms: Option<u64>,
        now_ms: u64,
    ) -> Option<Self> {
        let raw: Vec<String> = options
            .iter()
            .filter(|o| *o != OTHER)
            .take(MAX_OPTIONS)
            .cloned()
            .collect();
        let shown: Vec<(String, String)> = raw
            .iter()
            .map(|o| {
                let (l, d) = o.split_once(SEP).unwrap_or((o, ""));
                (clip(l, MAX_LABEL), clip(d, MAX_DESC))
            })
            .collect();
        if shown.is_empty() || shown.iter().any(|(l, _)| l.is_empty()) {
            return None;
        }
        Some(Self {
            id: id.to_owned(),
            session: session.to_owned(),
            text: clip(title, MAX_QUESTION),
            options: shown,
            raw,
            other: options.iter().any(|o| o == OTHER),
            sel: 0,
            ready: false,
            typing: None,
            awaiting: false,
            decided: None,
            shown_ms: now_ms,
            deadline_ms: now_ms.saturating_add(timeout_ms.unwrap_or(DEFAULT_WAIT_MS)),
            rows_at: Cell::new((Rect::default(), 0, 0)),
        })
    }

    /// From an `input` that came on its own: the field only.
    pub fn from_input(
        id: &str,
        session: &str,
        title: &str,
        timeout_ms: Option<u64>,
        now_ms: u64,
    ) -> Self {
        Self {
            id: id.to_owned(),
            session: session.to_owned(),
            text: clip(title, MAX_QUESTION),
            options: vec![],
            raw: vec![],
            other: false,
            sel: 0,
            ready: false,
            typing: Some(String::new()),
            awaiting: false,
            decided: None,
            shown_ms: now_ms,
            deadline_ms: now_ms.saturating_add(timeout_ms.unwrap_or(DEFAULT_WAIT_MS)),
            rows_at: Cell::new((Rect::default(), 0, 0)),
        }
    }

    /// The rows of the list: the answers, and `Other…`.
    fn rows(&self) -> usize {
        self.options.len() + usize::from(self.other)
    }

    fn rows_id(&self) -> u64 {
        let mut h = DefaultHasher::new();
        (&self.id, &self.options, self.other, self.typing.is_some()).hash(&mut h);
        h.finish()
    }
}

/// What waits for the person besides an approval: a question of the guard's.
pub enum Prompt {
    Question(Question),
}

impl Prompt {
    pub fn session(&self) -> &str {
        match self {
            Self::Question(q) => &q.session,
        }
    }

    pub fn shown_ms(&self) -> u64 {
        match self {
            Self::Question(q) => q.shown_ms,
        }
    }

    /// The quiet second is over for it.
    pub fn ready(&self) -> bool {
        match self {
            Self::Question(q) => q.ready,
        }
    }

    pub fn show(&mut self, now_ms: u64) {
        match self {
            Self::Question(q) => q.shown_ms = now_ms,
        }
    }

    /// Pi has already given up on it.
    pub fn expired(&self, now_ms: u64) -> bool {
        match self {
            Self::Question(q) => now_ms >= q.deadline_ms,
        }
    }
}

/// An extension dialog came from Pi: a question for the person, or the field that follows
/// `Other…`. Anything else is not drawn here.
pub fn on_request(
    app: &mut App,
    id: &str,
    method: &str,
    title: &str,
    options: &[String],
    timeout_ms: Option<u64>,
) -> Vec<Cmd> {
    let (now, session) = (app.now_ms, app.chat.session_id.clone());
    match method {
        "select" => {
            if let Some(q) = Question::from_select(id, &session, title, options, timeout_ms, now) {
                app.offer(Prompt::Question(q));
            }
            vec![]
        }
        "input" => {
            if let Some(Prompt::Question(q)) = &mut app.prompt
                && q.awaiting
            {
                q.id = id.to_owned();
                q.awaiting = false;
                q.deadline_ms = now.saturating_add(timeout_ms.unwrap_or(DEFAULT_WAIT_MS));
                if let Some(answer) = q.decided.take() {
                    return close(app, answer);
                }
                return vec![];
            }
            app.offer(Prompt::Question(Question::from_input(
                id, &session, title, timeout_ms, now,
            )));
            vec![]
        }
        _ => vec![],
    }
}

/// The box goes; Pi gets the answer (`None`: skipped).
fn close(app: &mut App, value: Option<String>) -> Vec<Cmd> {
    match app.prompt.take() {
        Some(Prompt::Question(q)) => vec![Cmd::ChatUi { id: q.id, value }],
        None => vec![],
    }
}

/// The person picked row `i`: an answer goes to Pi; `Other…` opens the field.
fn choose(app: &mut App, i: usize) -> Vec<Cmd> {
    let Some(Prompt::Question(q)) = &mut app.prompt else {
        return vec![];
    };
    if i < q.raw.len() {
        let value = q.raw[i].clone();
        return close(app, Some(value));
    }
    if q.other && i == q.raw.len() {
        q.typing = Some(String::new());
        q.awaiting = true;
        return vec![Cmd::ChatUi {
            id: q.id.clone(),
            value: Some(OTHER.into()),
        }];
    }
    vec![]
}

/// A key while the question is on screen.
pub fn key(app: &mut App, k: &KeyEvent, quiet: bool) -> Vec<Cmd> {
    let Some(Prompt::Question(q)) = &mut app.prompt else {
        return vec![];
    };
    // Before the quiet second nothing counts: what the person was typing never answers it.
    if !q.ready && !quiet {
        return vec![];
    }
    q.ready = true;
    let plain = (k.modifiers - KeyModifiers::SHIFT).is_empty();
    if let Some(typed) = &mut q.typing {
        let answer = match k.code {
            KeyCode::Esc => Some(None),
            KeyCode::Enter if plain && !typed.trim().is_empty() => {
                Some(Some(typed.trim().to_owned()))
            }
            KeyCode::Backspace => {
                typed.pop();
                None
            }
            KeyCode::Char(c)
                if !k
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && !c.is_control()
                    && !clio_brain::pi::text::is_deceptive(c)
                    && typed.chars().count() < MAX_TYPED =>
            {
                typed.push(c);
                None
            }
            _ => None,
        };
        return match answer {
            Some(a) if q.awaiting => {
                q.decided = Some(a);
                vec![]
            }
            Some(a) => close(app, a),
            None => vec![],
        };
    }
    let n = q.rows();
    match k.code {
        KeyCode::Esc => close(app, None),
        KeyCode::Up => {
            q.sel = q.sel.saturating_sub(1);
            vec![]
        }
        KeyCode::Down => {
            q.sel = (q.sel + 1).min(n.saturating_sub(1));
            vec![]
        }
        KeyCode::Enter if plain => {
            let i = q.sel;
            choose(app, i)
        }
        KeyCode::Char(d @ '1'..='9') if plain => {
            let i = usize::from(d as u8 - b'1');
            if i < n { choose(app, i) } else { vec![] }
        }
        _ => vec![],
    }
}

/// A click on an answer selects it (it never answers): only if the row drawn there is still
/// that row.
pub fn click(app: &mut App, m: &MouseEvent) {
    let Some(Prompt::Question(q)) = &mut app.prompt else {
        return;
    };
    if m.kind != MouseEventKind::Down(MouseButton::Left) || q.typing.is_some() {
        return;
    }
    let (area, count, drawn) = q.rows_at.get();
    if !area.contains(Position::new(m.column, m.row)) || drawn != q.rows_id() {
        return;
    }
    let i = usize::from(m.row - area.y);
    if i < count {
        q.sel = i;
    }
}

/// The box: the question, the answers (the first is the recommended one), `Other…`, the field
/// when it is open, and the keys.
pub fn draw(f: &mut Frame, area: Rect, app: &App, q: &Question) {
    let t = app.theme;
    let w = app::dialog_inner(area);
    let mut rows: Vec<Line<'static>> = Vec::new();
    rows.extend(app::wrap_line(
        Line::from(Span::styled(q.text.clone(), t.bold())),
        w,
    ));
    rows.push(Line::from(""));
    let first = rows.len();
    for (i, (label, desc)) in q.options.iter().enumerate() {
        let mut head = format!("{}  {label}", i + 1);
        if i == 0 {
            head.push_str(" (recommended)");
        }
        let line = if desc.is_empty() {
            head
        } else {
            format!("{head}  {} {desc}", t.dot())
        };
        rows.push(row(t, &line, i == q.sel && q.typing.is_none(), w));
    }
    if q.other {
        let i = q.options.len();
        rows.push(row(
            t,
            &format!("{}  {OTHER}", i + 1),
            i == q.sel && q.typing.is_none(),
            w,
        ));
    }
    if let Some(typed) = &q.typing {
        if !q.options.is_empty() {
            rows.push(Line::from(""));
        }
        let shown = text::fit(
            &format!("{typed}{}", t.cursor()),
            w.saturating_sub(4),
            t.ellipsis(),
        );
        rows.push(Line::from(vec![
            Span::styled(format!("  {} ", t.prompt()), t.accent_line()),
            Span::raw(shown),
        ]));
    }
    let waiting = !q.ready && !app.quiet();
    if waiting {
        rows.push(Line::from(""));
        rows.push(Line::from(Span::styled(
            "wait a second without typing to answer…",
            t.dim(),
        )));
    }
    rows.push(Line::from(""));
    let d = format!("   {}   ", t.dot());
    let hint = if q.typing.is_some() {
        Line::from(vec![
            Span::styled("Enter", t.key()),
            Span::styled(" send", t.dim()),
            Span::styled(d, t.dim()),
            Span::styled("Esc", t.key()),
            Span::styled(" skip the question", t.dim()),
        ])
    } else {
        Line::from(vec![
            Span::styled(format!("{}{} Enter", t.up(), t.down()), t.key()),
            Span::styled(format!(" or 1-{}", q.rows()), t.key()),
            Span::styled(" choose", t.dim()),
            Span::styled(d, t.dim()),
            Span::styled("Esc", t.key()),
            Span::styled(" skip the question", t.dim()),
        ])
    };
    rows.extend(app::wrap_line(hint, w));
    let at = app::dialog_box(f, area, t, " Clio asks ", rows, t.accent_line());
    if q.rows() > 0 {
        let y =
            at.y.saturating_add(u16::try_from(first).unwrap_or(u16::MAX));
        let h = u16::try_from(q.rows()).unwrap_or(0);
        // Only what the box really shows can be clicked.
        let area = Rect::new(at.x, y, at.width, h).intersection(at);
        q.rows_at.set((area, usize::from(area.height), q.rows_id()));
    }
}

/// One answer, on one row: the selected one across the width.
fn row(t: super::theme::Theme, s: &str, selected: bool, w: usize) -> Line<'static> {
    let mark = if selected { t.sel() } else { " " };
    let s = text::fit(&format!("{mark} {s}"), w, t.ellipsis());
    if selected {
        let fill = " ".repeat(w.saturating_sub(text::width(&s)));
        Line::from(Span::styled(format!("{s}{fill}"), t.selection()))
    } else {
        Line::from(Span::raw(s))
    }
}
