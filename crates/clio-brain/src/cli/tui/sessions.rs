//! The sessions beside the chat (the screen of the Pi harness): "nueva sesión" and the sessions
//! of every source, local ones from Pi's own session folder and the remote placeholder. No boxes:
//! the selection is a marker and the accent color. `Screen::Sessions` is this screen with the keys
//! on the list, `Screen::Chat` with the keys on the chat.

use std::cell::Cell;

use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::app::{App, Cmd, OpenTarget, Screen, clean, section};
use super::chat::Chat;
use super::keys::{Act, Ctx, lookup};
use crate::cli::chat::{Row, RowKind};

#[derive(Default)]
pub struct Sessions {
    pub rows: Vec<Row>,
    /// 0 is "nueva sesión"; `n > 0` is `rows[n - 1]`.
    pub sel: usize,
    pub loading: bool,
    /// Where the last draw put the list (empty when hidden) and the chat, and the list's first
    /// row: (y, row, rows shown).
    pub area: Cell<Rect>,
    pub chat_area: Cell<Rect>,
    pub list_at: Cell<(u16, usize, usize)>,
}

/// Terminals at least this wide show the list beside the chat; narrower ones only while the list
/// has the keys (Tab).
pub const WIDE: u16 = 80;
/// The list's width: "nueva sesión  en este servidor" fits.
const SIDE_W: u16 = 32;

impl Sessions {
    pub fn len(&self) -> usize {
        self.rows.len() + 1
    }

    pub fn set(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        self.loading = false;
        self.sel = self.sel.min(self.len() - 1);
    }

    pub fn selected(&self) -> Option<&Row> {
        self.sel.checked_sub(1).and_then(|i| self.rows.get(i))
    }
}

/// As leaving the chat always was: a turn that runs is aborted (Esc in the chat) first.
fn running(app: &mut App) -> bool {
    if app.chat.running {
        app.flash = Some(
            "el agente está trabajando: Esc en el chat aborta el turno; luego cambia de sesión"
                .into(),
        );
    }
    app.chat.running
}

/// A new session in the chat. Nothing is created yet: the run and Pi start with its first
/// message (`chat::send`), so opening Clio leaves nothing on disk.
fn open_new(app: &mut App) -> Vec<Cmd> {
    if app.chat.is_draft() {
        app.screen = Screen::Chat;
        return vec![];
    }
    if running(app) {
        return vec![];
    }
    app.chat = Chat::draft(app.now_ms);
    app.screen = Screen::Chat;
    vec![Cmd::ChatClose]
}

/// The row of the session open in the chat: found by its id in Pi's file name
/// (`<time>_<id>.jsonl`), or "nueva sesión" while the new one is a draft or not listed yet; none
/// while one is still opening.
pub fn open_pos(app: &App) -> Option<usize> {
    let c = &app.chat;
    if c.session_id.is_empty() {
        return c.is_draft().then_some(0);
    }
    let tail = format!("_{}.jsonl", c.session_id);
    Some(
        app.sessions
            .rows
            .iter()
            .position(|r| r.reference.ends_with(&tail))
            .map_or(0, |i| i + 1),
    )
}

fn open_selected(app: &mut App) -> Vec<Cmd> {
    let Some(row) = app.sessions.selected() else {
        return open_new(app);
    };
    if let Some(why) = &row.unavailable {
        app.notice = Some(why.clone());
        return vec![];
    }
    if open_pos(app) == Some(app.sessions.sel) {
        app.screen = Screen::Chat;
        return vec![];
    }
    if running(app) {
        return vec![];
    }
    let row = &app.sessions.rows[app.sessions.sel - 1];
    let (reference, title) = (row.reference.clone(), row.title.clone());
    app.chat_seq += 1;
    app.chat = Chat::opening(app.chat_seq, app.now_ms);
    app.chat.title = title;
    app.screen = Screen::Chat;
    vec![Cmd::ChatOpen {
        seq: app.chat_seq,
        target: OpenTarget::Existing { reference },
    }]
}

pub fn key(app: &mut App, k: &KeyEvent) -> Vec<Cmd> {
    let last = app.sessions.len() - 1;
    let Some(act) = lookup(Ctx::Sessions, k) else {
        return vec![];
    };
    match act {
        Act::Quit if !running(app) => app.quit = true,
        Act::Back => app.screen = Screen::Chat,
        Act::Down => app.sessions.sel = (app.sessions.sel + 1).min(last),
        Act::Up => app.sessions.sel = app.sessions.sel.saturating_sub(1),
        Act::Top => app.sessions.sel = 0,
        Act::Bottom => app.sessions.sel = last,
        Act::New => return open_new(app),
        Act::Open => return open_selected(app),
        Act::Refresh => {
            app.sessions.loading = true;
            return vec![Cmd::LoadSessions];
        }
        Act::Doctor => {
            app.screen = Screen::Doctor;
            app.doctor.loading = true;
            return vec![Cmd::RunDoctor];
        }
        Act::Memory => return super::memory::opened(app),
        Act::Delete => match app.sessions.selected() {
            Some(_) if app.chat.opening || open_pos(app) == Some(app.sessions.sel) => {
                app.flash = Some("esa sesión está abierta: abre otra antes de borrarla".into());
            }
            Some(row) => {
                return vec![Cmd::PlanSessionDelete {
                    reference: row.reference.clone(),
                }];
            }
            None => app.flash = Some("elige una sesión para borrarla".into()),
        },
        Act::Help => app.open_help(),
        _ => {}
    }
    vec![]
}

/// `hace 5 min`, `hace 2 h`, `hace 3 d`.
pub fn ago(now_ms: i64, then_ms: i64) -> String {
    let s = (now_ms - then_ms).max(0) / 1000;
    match s {
        0..=59 => "ahora".into(),
        60..=3599 => format!("hace {} min", s / 60),
        3600..=86_399 => format!("hace {} h", s / 3600),
        _ => format!("hace {} d", s / 86_400),
    }
}

/// A click on a row opens it (as Enter), a click on the chat gives it the keys, and the wheel
/// scrolls the pane under the pointer. `step` never lets a click reach a question or a message.
pub fn mouse(app: &mut App, m: &MouseEvent) -> Vec<Cmd> {
    let at = Position::new(m.column, m.row);
    let (side, main) = (
        app.sessions.area.get().contains(at),
        app.sessions.chat_area.get().contains(at),
    );
    let last = app.sessions.len() - 1;
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) if side => {
            let (y, first, shown) = app.sessions.list_at.get();
            if let Some(i) = m.row.checked_sub(y).map(usize::from).filter(|i| *i < shown) {
                app.sessions.sel = (first + i).min(last);
                return open_selected(app);
            }
        }
        MouseEventKind::Down(MouseButton::Left) if main => app.screen = Screen::Chat,
        MouseEventKind::ScrollUp if side => app.sessions.sel = app.sessions.sel.saturating_sub(1),
        MouseEventKind::ScrollDown if side => app.sessions.sel = (app.sessions.sel + 1).min(last),
        MouseEventKind::ScrollUp if main => app.chat.scroll = app.chat.scroll.saturating_add(3),
        MouseEventKind::ScrollDown if main => app.chat.scroll = app.chat.scroll.saturating_sub(3),
        _ => {}
    }
    vec![]
}

/// The screen of the Pi harness: the list on the left and the chat. On a narrow terminal the list
/// shows only while it has the keys.
pub fn draw_both(f: &mut Frame, area: Rect, app: &App) {
    let side_w = if f.area().width >= WIDE || app.screen == Screen::Sessions {
        SIDE_W
    } else {
        0
    };
    let [side, _, main] = Layout::horizontal([
        Constraint::Length(side_w),
        Constraint::Length(side_w.min(2)),
        Constraint::Min(0),
    ])
    .areas(area);
    app.sessions.area.set(side);
    app.sessions.chat_area.set(main);
    if side_w > 0 {
        draw(f, side, app);
    }
    super::chat::draw(f, main, app);
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let s = &app.sessions;
    let mut lines: Vec<Line> = super::home::state_line(app, t, usize::from(area.width));
    lines.push(Line::from(""));
    lines.push(section(t, "sesiones de Pi", area.width));
    let rows = usize::from(area.height).saturating_sub(lines.len()).max(1);
    let skip = (s.sel + 1).saturating_sub(rows);
    s.list_at.set((
        area.y + u16::try_from(lines.len()).unwrap_or(u16::MAX),
        skip,
        rows.min(s.len() - skip),
    ));
    let now = crate::cli::now_ms();
    let open = open_pos(app);
    for pos in (0..s.len()).skip(skip).take(rows) {
        // The selection shows only while the list has the keys; the open one always.
        let selected = pos == s.sel && app.screen == Screen::Sessions;
        let base = if selected {
            t.selected()
        } else if open == Some(pos) {
            t.bold()
        } else {
            t.plain()
        };
        let dim = if selected { base } else { t.dim() };
        let mark = if selected {
            Span::styled(format!("{} ", t.sel()), base)
        } else if open == Some(pos) {
            Span::styled(format!("{} ", t.rule_v()), t.accent())
        } else {
            Span::raw("  ")
        };
        if pos == 0 {
            let place = if app.host {
                "en este servidor".to_owned()
            } else {
                format!("en una copia de {}", clean(&folder_of_cwd(&app.cwd)))
            };
            lines.push(Line::from(vec![
                mark,
                Span::styled("nueva sesión", base),
                Span::styled(format!("  {place}"), dim),
            ]));
            continue;
        }
        let r = &s.rows[pos - 1];
        let (style, tail) = match (&r.kind, &r.unavailable) {
            // Its title says it; the why shows below when it is selected.
            (RowKind::Remote, _) => (t.warn(), String::new()),
            (_, Some(_)) => (t.warn(), "no es de Clio".to_owned()),
            _ => (base, r.last_ms.map_or_else(String::new, |ms| ago(now, ms))),
        };
        let title_w = usize::from(area.width)
            .saturating_sub(4 + tail.chars().count())
            .max(8);
        let title: String = clean(&r.title).chars().take(title_w).collect();
        lines.push(Line::from(vec![
            mark,
            Span::styled(
                format!("{title:<title_w$}  "),
                if selected { base } else { style },
            ),
            Span::styled(tail, dim),
        ]));
    }
    if let Some(r) = s.selected()
        && let Some(why) = &r.unavailable
        && usize::from(area.height) > lines.len() + 1
    {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(clean(why), t.warn())));
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn folder_of_cwd(cwd: &str) -> String {
    std::path::Path::new(cwd)
        .file_name()
        .map_or_else(|| cwd.to_owned(), |n| n.to_string_lossy().into_owned())
}
