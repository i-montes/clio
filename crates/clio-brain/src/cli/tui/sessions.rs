//! The drawer of sessions (the Pi harness): "new session" and the sessions of every source,
//! local ones from Pi's own session folder and the remote placeholder. Hidden while the chat has
//! the keys (`Screen::Chat`); Ctrl-S opens it on the left, with the keys (`Screen::Sessions`), and
//! the chat stays in sight beside it.

use std::cell::Cell;

use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::app::{App, Cmd, OpenTarget, Screen, clean};
use super::chat::Chat;
use super::keys::{Act, Ctx, lookup};
use super::text;
use crate::cli::chat::{Row, RowKind};

#[derive(Default)]
pub struct Sessions {
    pub rows: Vec<Row>,
    /// 0 is "new session"; `n > 0` is `rows[n - 1]`.
    pub sel: usize,
    pub loading: bool,
    /// Where the last draw put the drawer (empty when hidden) and the chat beside it, and the
    /// drawer's rows: (y of the first, the first entry shown, entries shown, rows per entry).
    pub area: Cell<Rect>,
    pub chat_area: Cell<Rect>,
    pub list_at: Cell<(u16, usize, usize, u16)>,
}

/// Terminals narrower than this give the open drawer their whole width.
pub const WIDE: u16 = 80;
/// The drawer's width.
pub const DRAWER_W: u16 = 38;

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

/// A new session in the chat. Nothing is created yet: the run and Pi start with its first
/// message (`chat::send`), so opening Clio leaves nothing on disk.
pub(super) fn open_new(app: &mut App) -> Vec<Cmd> {
    if app.chat.is_draft() {
        app.screen = Screen::Chat;
        return vec![];
    }
    if app.busy() {
        return vec![];
    }
    app.chat = Chat::draft(app.now_ms);
    app.screen = Screen::Chat;
    vec![Cmd::ChatClose]
}

/// `clio` with no arguments: the last conversation of this mode and place, opened the way the
/// drawer opens one (its history from its file, then Pi); with none, or if it fails, a draft.
pub fn resume(app: &mut App) -> Vec<Cmd> {
    app.chat_seq += 1;
    app.chat = Chat::opening(app.chat_seq, app.now_ms);
    app.chat.resume = true;
    vec![Cmd::ChatOpen {
        seq: app.chat_seq,
        target: OpenTarget::Last {
            project: app.cwd.clone(),
        },
    }]
}

/// The row of the session open in the chat: found by its id in Pi's file name
/// (`<time>_<id>.jsonl`), or "new session" while the new one is a draft or not listed yet; none
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
    if app.busy() {
        return vec![];
    }
    let row = &app.sessions.rows[app.sessions.sel - 1];
    let (reference, title) = (row.reference.clone(), row.title.clone());
    app.chat_seq += 1;
    app.chat = Chat::opening(app.chat_seq, app.now_ms);
    app.chat.title = title;
    app.chat.file = Some(reference.clone());
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
        Act::Quit if !app.busy_to_quit() => app.quit = true,
        Act::Back | Act::Sessions => app.screen = Screen::Chat,
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
        // Its question would come over the chat's own (a set-aside action) while it works.
        Act::Delete if app.busy() => {}
        Act::Delete => match app.sessions.selected() {
            Some(_) if open_pos(app) == Some(app.sessions.sel) => {
                app.flash = Some("that session is open: open another before deleting it".into());
            }
            Some(row) => {
                return vec![Cmd::PlanSessionDelete {
                    reference: row.reference.clone(),
                }];
            }
            None => app.flash = Some("pick a session to delete it".into()),
        },
        Act::Help => app.open_help(),
        _ => {}
    }
    vec![]
}

/// `5 min ago`, `2 h ago`, `3 d ago`.
pub fn ago(now_ms: i64, then_ms: i64) -> String {
    let s = (now_ms - then_ms).max(0) / 1000;
    match s {
        0..=59 => "now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86_400),
    }
}

/// The second line of a row: how long ago and how many messages (the delete dialog shows it too).
pub fn age_and_count(r: &Row, now_ms: i64, dot: &str) -> String {
    let mut when = r.last_ms.map_or_else(String::new, |ms| ago(now_ms, ms));
    if r.messages > 0 {
        if !when.is_empty() {
            when.push_str(&format!(" {dot} "));
        }
        when.push_str(&format!("{} msgs", r.messages));
    }
    when
}

/// A click on a row opens it (as Enter), a click outside the open drawer closes it, and the wheel
/// scrolls the pane under the pointer. `step` never lets a click reach a question or a message.
pub fn mouse(app: &mut App, m: &MouseEvent) -> Vec<Cmd> {
    let at = Position::new(m.column, m.row);
    let side = app.screen == Screen::Sessions && app.sessions.area.get().contains(at);
    let last = app.sessions.len() - 1;
    // A click on a row of the slash menu runs it; elsewhere it closes the menu.
    if app.screen == Screen::Chat
        && app.chat.menu.is_some()
        && m.kind == MouseEventKind::Down(MouseButton::Left)
    {
        return super::commands::click(app, m.column, m.row).unwrap_or_else(|| {
            app.chat.menu = None;
            vec![]
        });
    }
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) if side => {
            let (y, first, shown, per) = app.sessions.list_at.get();
            if let Some(i) = m.row.checked_sub(y)
                && i % per < 2
                && usize::from(i / per) < shown
            {
                app.sessions.sel = (first + usize::from(i / per)).min(last);
                return open_selected(app);
            }
        }
        MouseEventKind::Down(MouseButton::Left) if app.screen == Screen::Sessions => {
            app.screen = Screen::Chat;
        }
        MouseEventKind::ScrollUp if side => app.sessions.sel = app.sessions.sel.saturating_sub(1),
        MouseEventKind::ScrollDown if side => app.sessions.sel = (app.sessions.sel + 1).min(last),
        MouseEventKind::ScrollUp if app.sessions.chat_area.get().contains(at) => {
            app.chat.scroll_by(3);
            return super::chat::earlier(app);
        }
        MouseEventKind::ScrollDown if app.sessions.chat_area.get().contains(at) => {
            app.chat.scroll_by(-3);
        }
        _ => {}
    }
    vec![]
}

/// The drawer: a box with "new session" and the sessions, two rows each (the title, and when
/// and how long), the selection across the width, the open one marked; at its foot the keys, or
/// the last flash.
pub fn draw_drawer(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let s = &app.sessions;
    let block = Block::new()
        .borders(Borders::ALL)
        .border_set(t.border())
        .border_style(t.accent_line())
        .title(Span::styled(" sessions ", t.bold()));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 3 || inner.width < 8 {
        return;
    }
    let w = usize::from(inner.width);
    let list_h = inner.height - 2;
    // Two rows an entry, and a blank one between entries when they all fit with it.
    let all = u16::try_from(s.len()).unwrap_or(u16::MAX);
    let per: u16 = if all.saturating_mul(3).saturating_sub(1) <= list_h {
        3
    } else {
        2
    };
    let fits = usize::from(list_h.saturating_add(per - 2) / per).max(1);
    let skip = (s.sel + 1).saturating_sub(fits);
    let shown = fits.min(s.len() - skip);
    s.list_at.set((inner.y, skip, shown, per));
    let now = crate::cli::now_ms();
    let open = open_pos(app);
    let mut lines: Vec<Line> = Vec::new();
    for pos in (0..s.len()).skip(skip).take(shown) {
        if pos > skip && per == 3 {
            lines.push(Line::from(""));
        }
        let (title, title_style, under, under_style) = if pos == 0 {
            let place = if app.host {
                "on this server".to_owned()
            } else {
                format!("in a copy of {}", clean(&folder_of_cwd(&app.cwd)))
            };
            ("+ new session".to_owned(), t.bold(), place, t.dim())
        } else {
            let r = &s.rows[pos - 1];
            match (&r.kind, &r.unavailable) {
                (RowKind::Remote, why) => (
                    clean(&r.title),
                    t.warn(),
                    why.as_deref().map_or_else(String::new, clean),
                    t.warn(),
                ),
                (_, Some(why)) => (clean(&r.title), t.plain(), clean(why), t.warn()),
                _ => (
                    clean(&r.title),
                    t.plain(),
                    age_and_count(r, now, t.dot()),
                    t.dim(),
                ),
            }
        };
        let mark = if open == Some(pos) {
            Span::styled(format!("{} ", t.open_mark()), t.accent_line())
        } else {
            Span::raw("  ")
        };
        let room = w.saturating_sub(4);
        let row1 = text::fit(&title, room, t.ellipsis());
        let row2 = text::fit(&under, room, t.ellipsis());
        let selected = pos == s.sel;
        let pad = |r: &str| " ".repeat(room.saturating_sub(text::width(r)) + 1);
        if selected {
            let sel = t.selection();
            let mark = Span::styled(mark.content.clone(), sel);
            lines.push(Line::from(vec![
                Span::styled(" ", sel),
                mark,
                Span::styled(format!("{row1}{}", pad(&row1)), sel),
            ]));
            lines.push(Line::from(Span::styled(
                format!("   {row2}{}", pad(&row2)),
                sel,
            )));
        } else {
            lines.push(Line::from(vec![
                Span::raw(" "),
                mark,
                Span::styled(row1, title_style),
            ]));
            lines.push(Line::from(vec![
                Span::raw("   "),
                Span::styled(row2, under_style),
            ]));
        }
    }
    if s.loading && s.rows.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("   loading…", t.dim())));
    }
    f.render_widget(
        Paragraph::new(lines),
        Rect {
            height: list_h,
            ..inner
        },
    );
    // The foot: a flash (why something cannot be done now) or the keys.
    let foot = match &app.flash {
        Some(m) => Line::from(Span::styled(
            text::fit(&format!(" {}", clean(m)), w, t.ellipsis()),
            t.accent_line(),
        )),
        None => {
            let d = format!(" {} ", t.dot());
            Line::from(vec![
                Span::styled(" Enter", t.key()),
                Span::styled(format!(" open{d}"), t.dim()),
                Span::styled("n", t.key()),
                Span::styled(format!(" new{d}"), t.dim()),
                Span::styled("x", t.key()),
                Span::styled(" delete", t.dim()),
            ])
        }
    };
    f.render_widget(
        Paragraph::new(foot),
        Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
    );
}

fn folder_of_cwd(cwd: &str) -> String {
    std::path::Path::new(cwd)
        .file_name()
        .map_or_else(|| cwd.to_owned(), |n| n.to_string_lossy().into_owned())
}
