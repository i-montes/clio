//! The doctor screen: every check with its mark and, for each warning or failure, one line on
//! what to do.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use super::app::{App, Cmd, Screen, clean, is_ctrl_c, plural};
use crate::cli::doctor::Report;
use crate::cli::ui::Level;

#[derive(Default)]
pub struct DoctorView {
    pub report: Option<Report>,
    pub loading: bool,
    pub scroll: u16,
}

impl DoctorView {
    pub fn set(&mut self, r: Report) {
        self.report = Some(r);
        self.loading = false;
    }
}

pub fn key(app: &mut App, k: &KeyEvent) -> Vec<Cmd> {
    match k.code {
        KeyCode::Esc | KeyCode::Char('q') => return app.go_home(),
        _ if is_ctrl_c(k) => return app.go_home(),
        KeyCode::Char('r') => {
            app.doctor.loading = true;
            return vec![Cmd::RunDoctor];
        }
        KeyCode::Down | KeyCode::Char('j') => app.doctor.scroll = app.doctor.scroll.saturating_add(1),
        KeyCode::Up | KeyCode::Char('k') => app.doctor.scroll = app.doctor.scroll.saturating_sub(1),
        KeyCode::PageDown => app.doctor.scroll = app.doctor.scroll.saturating_add(10),
        KeyCode::PageUp => app.doctor.scroll = app.doctor.scroll.saturating_sub(10),
        KeyCode::Char('?') => {
            app.before_help = Screen::Doctor;
            app.screen = Screen::Help;
        }
        _ => {}
    }
    vec![]
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let mut lines: Vec<Line> = Vec::new();
    let mut title = " Doctor ".to_owned();
    match &app.doctor.report {
        None => lines.push(Line::from(Span::styled("comprobando…", t.dim()))),
        Some(r) => {
            title = format!(
                " Doctor: {} ok · {} · {} ",
                r.count(Level::Pass),
                plural(r.count(Level::Warn), "aviso", "avisos"),
                plural(r.count(Level::Fail), "fallo", "fallos")
            );
            for c in &r.checks {
                let (mark, style) = match c.level {
                    Level::Pass => ("PASS", t.ok()),
                    Level::Warn => ("WARN", t.warn()),
                    Level::Fail => ("FAIL", t.fail()),
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{mark}  "), style),
                    Span::raw(clean(&c.msg)),
                ]));
                if let (Some(h), true) = (&c.hint, c.level != Level::Pass) {
                    lines.push(Line::from(vec![
                        Span::raw("      → "),
                        Span::styled(clean(h), t.dim()),
                    ]));
                }
            }
        }
    }
    if app.doctor.loading && app.doctor.report.is_some() {
        title.push_str("(actualizando…) ");
    }
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((app.doctor.scroll, 0))
            .block(
                Block::new()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(Span::styled(title, t.bold())),
            ),
        area,
    );
}
