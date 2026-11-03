//! The session selector (the first screen with the Pi harness): "nueva sesión" and the sessions
//! of every source, local ones from Pi's own session folder and the remote placeholder. No boxes:
//! the selection is a marker and the accent color.

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::app::{App, Cmd, OpenTarget, Screen, clean, plural, section};
use super::chat::Chat;
use super::keys::{Act, Ctx, lookup};
use crate::cli::chat::{Row, RowKind};

#[derive(Default)]
pub struct Sessions {
    pub rows: Vec<Row>,
    /// 0 is "nueva sesión"; `n > 0` is `rows[n - 1]`.
    pub sel: usize,
    pub loading: bool,
}

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

fn open_new(app: &mut App) -> Vec<Cmd> {
    app.chat_seq += 1;
    app.chat = Chat::opening(app.chat_seq, app.now_ms);
    app.screen = Screen::Chat;
    vec![Cmd::ChatOpen {
        seq: app.chat_seq,
        target: OpenTarget::New {
            project: app.cwd.clone(),
        },
    }]
}

fn open_selected(app: &mut App) -> Vec<Cmd> {
    let Some(row) = app.sessions.selected() else {
        return open_new(app);
    };
    if let Some(why) = &row.unavailable {
        app.notice = Some(why.clone());
        return vec![];
    }
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
        Act::Quit => app.quit = true,
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

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let s = &app.sessions;
    let mut lines: Vec<Line> = super::home::state_line(app, t, usize::from(area.width));
    lines.push(Line::from(""));
    lines.push(section(t, "sesiones de Pi", area.width));
    let rows = usize::from(area.height).saturating_sub(lines.len()).max(1);
    let skip = (s.sel + 1).saturating_sub(rows);
    let now = crate::cli::now_ms();
    for pos in (0..s.len()).skip(skip).take(rows) {
        let selected = pos == s.sel;
        let base = if selected { t.selected() } else { t.plain() };
        let dim = if selected { base } else { t.dim() };
        let mark = Span::styled(
            if selected {
                format!("{} ", t.sel())
            } else {
                "  ".into()
            },
            base,
        );
        if pos == 0 {
            lines.push(Line::from(vec![
                mark,
                Span::styled("nueva sesión", base),
                Span::styled(
                    format!("   en una copia de {}", clean(&folder_of_cwd(&app.cwd))),
                    dim,
                ),
            ]));
            continue;
        }
        let r = &s.rows[pos - 1];
        let title_w = usize::from(area.width)
            .saturating_sub(2 + 14 + 10 + 18)
            .max(12);
        let title: String = clean(&r.title).chars().take(title_w).collect();
        let (style, tail) = match (&r.kind, &r.unavailable) {
            (RowKind::Remote, _) => (t.warn(), clean(&r.detail)),
            (_, Some(_)) => (t.warn(), "no es de Clio".to_owned()),
            _ => (dim, r.last_ms.map_or_else(String::new, |ms| ago(now, ms))),
        };
        let count = if r.kind == RowKind::Remote {
            String::new()
        } else {
            plural(r.messages as usize, "msg", "msgs")
        };
        lines.push(Line::from(vec![
            mark,
            Span::styled(
                format!("{title:<title_w$}  "),
                if selected { base } else { style },
            ),
            Span::styled(format!("{count:<9} "), dim),
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
