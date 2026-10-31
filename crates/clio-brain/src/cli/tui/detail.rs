//! A run in detail: summary, the agent's final message, the changed files and, for the chosen
//! one, its colored diff. Applying and undoing go through `cli::apply` (the same checks as the
//! plain commands); this screen only asks.

use std::cell::Cell;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use super::app::{Action, App, Cmd, Confirm, Screen, clean};
use super::sanitize;
use crate::cli::home::Dirs;
use crate::cli::runs::{self, RunInfo, Status, ending_label};
use crate::cli::work;

pub struct Row {
    pub kind: char,
    pub rel: String,
    pub diff: String,
}

pub struct Detail {
    pub info: RunInfo,
    pub rows: Vec<Row>,
    pub sel: usize,
    pub in_diff: bool,
    pub scroll: usize,
    /// Height of the diff area at the last draw, to bound scrolling.
    pub view_h: Cell<u16>,
}

impl Detail {
    /// Reads the run and the diff of every changed file.
    /// shortcut: all diffs are computed up front; a run with thousands of changed files is slow to open.
    pub fn load(dirs: &Dirs, id: &str) -> Result<Self, String> {
        let run = runs::resolve(dirs, id)?;
        let info = RunInfo::load(&run);
        let mut rows = Vec::new();
        if let (Ok(meta), Ok(manifest)) = (run.meta(), run.manifest()) {
            let ws = std::path::Path::new(&meta.workspace);
            for c in work::changes(&run.sandbox(), &manifest)? {
                rows.push(Row {
                    kind: runs::letter(c.kind),
                    diff: runs::diff_text(ws, &run, &manifest, &c),
                    rel: c.rel,
                });
            }
        }
        Ok(Self {
            info,
            rows,
            sel: 0,
            in_diff: false,
            scroll: 0,
            view_h: Cell::new(10),
        })
    }

    pub fn keep_position(&mut self, old: &Self) {
        self.sel = old.sel.min(self.rows.len().saturating_sub(1));
        self.in_diff = old.in_diff && !self.rows.is_empty();
        self.scroll = old.scroll;
    }

    fn total_lines(&self) -> usize {
        self.rows.get(self.sel).map_or(0, |r| r.diff.lines().count())
    }

    fn max_scroll(&self) -> usize {
        self.total_lines().saturating_sub(usize::from(self.view_h.get()))
    }
}

pub fn key(app: &mut App, k: &KeyEvent) -> Vec<Cmd> {
    let Some(d) = app.detail.as_mut() else {
        return app.go_home();
    };
    let id = d.info.id.clone();
    let page = usize::from(d.view_h.get().max(2) - 1);
    match (k.code, d.in_diff) {
        (KeyCode::Char('a'), _) => return vec![Cmd::PlanApply { id }],
        (KeyCode::Char('u'), _) => {
            if d.info.status == Status::Applied {
                app.confirm = Some(Confirm {
                    action: Action::Undo,
                    id,
                    files: d.info.files(),
                    workspace: d.info.workspace.clone(),
                });
            } else {
                app.notice = Some("Solo se puede deshacer un run que ya se aplicó.".into());
            }
        }
        (KeyCode::Char('?'), _) => {
            app.before_help = Screen::Detail;
            app.screen = Screen::Help;
        }
        (KeyCode::Esc | KeyCode::Char('q') | KeyCode::Left | KeyCode::Char('h'), true) => {
            d.in_diff = false;
        }
        (KeyCode::Esc | KeyCode::Char('q'), false) => return app.go_home(),
        (KeyCode::Enter | KeyCode::Right | KeyCode::Char('l'), false) => {
            if !d.rows.is_empty() {
                d.in_diff = true;
                d.scroll = 0;
            }
        }
        (KeyCode::Down | KeyCode::Char('j'), false) => {
            d.sel = (d.sel + 1).min(d.rows.len().saturating_sub(1));
        }
        (KeyCode::Up | KeyCode::Char('k'), false) => d.sel = d.sel.saturating_sub(1),
        (KeyCode::Down | KeyCode::Char('j'), true) => d.scroll = (d.scroll + 1).min(d.max_scroll()),
        (KeyCode::Up | KeyCode::Char('k'), true) => d.scroll = d.scroll.saturating_sub(1),
        (KeyCode::PageDown | KeyCode::Char(' '), true) => {
            d.scroll = (d.scroll + page).min(d.max_scroll());
        }
        (KeyCode::PageUp | KeyCode::Char('b'), true) => d.scroll = d.scroll.saturating_sub(page),
        (KeyCode::Home | KeyCode::Char('g'), true) => d.scroll = 0,
        (KeyCode::End | KeyCode::Char('G'), true) => d.scroll = d.max_scroll(),
        (KeyCode::Char(']'), true) if d.sel + 1 < d.rows.len() => {
            d.sel += 1;
            d.scroll = 0;
        }
        (KeyCode::Char('['), true) if d.sel > 0 => {
            d.sel -= 1;
            d.scroll = 0;
        }
        _ => {}
    }
    vec![]
}

fn block<'a>(app: &App, title: String) -> Block<'a> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Span::styled(format!(" {title} "), app.theme.bold()))
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let Some(d) = &app.detail else {
        return;
    };
    let t = app.theme;
    if d.in_diff {
        draw_diff(f, area, app, d);
        return;
    }
    let info = &d.info;
    let [sum_a, msg_a, files_a] = Layout::vertical([
        Constraint::Length(6),
        Constraint::Percentage(40),
        Constraint::Min(4),
    ])
    .areas(area);

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Estado ", t.dim()),
            Span::styled(info.status.label(), t.status(info.status)),
            Span::raw("   "),
            Span::styled(clean(&info.id), t.dim()),
        ]),
        Line::from(vec![Span::styled("Carpeta ", t.dim()), Span::raw(clean(&info.workspace))]),
        Line::from(vec![
            Span::styled("Pedido ", t.dim()),
            Span::raw(clean(&info.instruction)),
        ]),
    ];
    match &info.summary {
        Some(s) => lines.push(Line::from(format!(
            "nivel {} · {} pasos · {:.1}s · {} · {} denegadas",
            s.level,
            s.steps,
            s.elapsed_ms as f64 / 1000.0,
            ending_label(&s.ending),
            s.denied
        ))),
        None => lines.push(Line::from(Span::styled(
            "sin resumen: el run no llegó a terminar",
            t.fail(),
        ))),
    }
    f.render_widget(Paragraph::new(lines).block(block(app, "Resumen".into())), sum_a);

    let msg = info
        .summary
        .as_ref()
        .map(|s| s.final_message.clone().unwrap_or_else(|| format!("(sin mensaje final) {}", s.pending.join("; "))))
        .unwrap_or_default();
    let mut msg_lines: Vec<Line> = sanitize::multiline(&msg).lines().map(|l| Line::from(l.to_owned())).collect();
    if info.summary.as_ref().is_some_and(|s| s.tainted) {
        msg_lines.push(Line::from(""));
        msg_lines.push(Line::from(Span::styled(
            "Ojo: el agente leyó archivos que pudieron traer instrucciones ocultas; revisa el diff.",
            t.warn(),
        )));
    }
    f.render_widget(
        Paragraph::new(msg_lines)
            .wrap(Wrap { trim: false })
            .block(block(app, "Mensaje del agente".into())),
        msg_a,
    );

    let items: Vec<Line> = if d.rows.is_empty() {
        vec![Line::from(Span::styled("  sin cambios", t.dim()))]
    } else {
        d.rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let sel = i == d.sel;
                let kind_style = match r.kind {
                    'A' => t.add(),
                    'D' => t.del(),
                    _ => t.warn(),
                };
                let base = if sel { t.selected() } else { t.plain() };
                Line::from(vec![
                    Span::styled(if sel { "▶ " } else { "  " }, base),
                    Span::styled(format!("{} ", r.kind), if sel { t.selected() } else { kind_style }),
                    Span::styled(clean(&r.rel), base),
                ])
            })
            .collect()
    };
    let rows_h = usize::from(files_a.height.saturating_sub(2));
    let skip = (d.sel + 1).saturating_sub(rows_h);
    f.render_widget(
        Paragraph::new(items.into_iter().skip(skip).collect::<Vec<_>>())
            .block(block(app, format!("Archivos cambiados ({})", d.rows.len()))),
        files_a,
    );
}

fn draw_diff(f: &mut Frame, area: Rect, app: &App, d: &Detail) {
    let t = app.theme;
    let Some(row) = d.rows.get(d.sel) else {
        return;
    };
    let h = area.height.saturating_sub(2);
    d.view_h.set(h);
    let scroll = d.scroll.min(d.max_scroll());
    let text = sanitize::multiline(&row.diff);
    let lines: Vec<Line> = text
        .lines()
        .skip(scroll)
        .take(usize::from(h))
        .map(|l| {
            let style = if l.starts_with("+++") || l.starts_with("---") {
                t.bold()
            } else if l.starts_with('+') {
                t.add()
            } else if l.starts_with('-') {
                t.del()
            } else if l.starts_with("@@") {
                t.hunk()
            } else if l.starts_with('#') {
                t.warn()
            } else {
                t.plain()
            };
            Line::from(Span::styled(l.to_owned(), style))
        })
        .collect();
    let total = d.total_lines();
    let title = format!(
        "{} {} · {}/{} · línea {}/{}",
        row.kind,
        clean(&row.rel),
        d.sel + 1,
        d.rows.len(),
        (scroll + 1).min(total.max(1)),
        total
    );
    f.render_widget(Paragraph::new(lines).block(block(app, title)), area);
}
