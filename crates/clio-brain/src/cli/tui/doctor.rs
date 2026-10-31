//! The doctor screen: every check with its mark and, for each warning or failure, one line on
//! what to do. Each check appears once (Clio's own and the host script's do not repeat).

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::app::{App, Cmd, clean, plural};
use super::keys::{Act, Ctx, lookup};
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
    let page = 8;
    match lookup(Ctx::Doctor, k) {
        Some(Act::Back) => return app.go_home(),
        Some(Act::Refresh) => {
            app.doctor.loading = true;
            return vec![Cmd::RunDoctor];
        }
        Some(Act::Down) => app.doctor.scroll = app.doctor.scroll.saturating_add(1),
        Some(Act::Up) => app.doctor.scroll = app.doctor.scroll.saturating_sub(1),
        Some(Act::PageDown) => app.doctor.scroll = app.doctor.scroll.saturating_add(page),
        Some(Act::PageUp) => app.doctor.scroll = app.doctor.scroll.saturating_sub(page),
        Some(Act::Top) => app.doctor.scroll = 0,
        Some(Act::Bottom) => app.doctor.scroll = u16::MAX,
        Some(Act::Help) => app.open_help(),
        _ => {}
    }
    vec![]
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let mut lines: Vec<Line> = Vec::new();
    match &app.doctor.report {
        None => lines.push(Line::from(Span::styled("comprobando…", t.dim()))),
        Some(r) => {
            lines.push(Line::from(Span::styled(
                format!(
                    "doctor: {} ok {} {} {} {}{}",
                    r.count(Level::Pass),
                    t.dot(),
                    plural(r.count(Level::Warn), "aviso", "avisos"),
                    t.dot(),
                    plural(r.count(Level::Fail), "fallo", "fallos"),
                    if app.doctor.loading {
                        "   (actualizando…)"
                    } else {
                        ""
                    }
                ),
                t.bold(),
            )));
            lines.push(Line::from(""));
            for c in &r.checks {
                let (mark, style) = match c.level {
                    Level::Pass => (t.check(), t.ok()),
                    Level::Warn => (t.bang(), t.warn()),
                    Level::Fail => (t.cross(), t.fail()),
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{mark}  "), style),
                    Span::raw(clean(&c.msg)),
                ]));
                if let (Some(h), true) = (&c.hint, c.level != Level::Pass) {
                    lines.push(Line::from(vec![
                        Span::styled(format!("   {} ", t.arrow()), t.dim()),
                        Span::styled(clean(h), t.dim()),
                    ]));
                }
            }
        }
    }
    // Wrapped lines count as several: keep the scroll inside what exists.
    let max = u16::try_from(lines.len().saturating_sub(usize::from(area.height))).unwrap_or(0);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((app.doctor.scroll.min(max), 0)),
        area,
    );
}
