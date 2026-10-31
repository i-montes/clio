//! A run in detail: summary, the agent's final message, the changed files and, for the chosen
//! one, its colored diff. Applying and undoing go through `cli::apply` (the same checks as the
//! plain commands); this screen only asks.

use std::cell::Cell;

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::app::{Action, App, Cmd, Confirm, Screen, clean, section};
use super::keys::{Act, lookup};
use super::sanitize;
use crate::cli::home::Dirs;
use crate::cli::runs::{self, RunInfo, Status, ending_label};
use crate::cli::usage::{self, RunUsage};
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
    /// Which of the memories this run recalled `+` and `-` judge.
    pub mem_sel: usize,
    /// What the run used and its context, from `usage.json` (empty for an older run).
    pub usage: RunUsage,
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
            mem_sel: 0,
            usage: RunUsage::load(&run.path),
            view_h: Cell::new(10),
        })
    }

    pub fn keep_position(&mut self, old: &Self) {
        self.sel = old.sel.min(self.rows.len().saturating_sub(1));
        self.in_diff = old.in_diff && !self.rows.is_empty();
        self.scroll = old.scroll;
        self.mem_sel = old.mem_sel.min(self.recalled().len().saturating_sub(1));
    }

    /// The ids of the memories the run brought into its context.
    pub fn recalled(&self) -> &[i64] {
        self.info
            .summary
            .as_ref()
            .and_then(|s| s.memory.as_ref())
            .map_or(&[], |m| m.recalled.as_slice())
    }

    fn total_lines(&self) -> usize {
        self.rows
            .get(self.sel)
            .map_or(0, |r| r.diff.lines().count())
    }

    fn max_scroll(&self) -> usize {
        self.total_lines()
            .saturating_sub(usize::from(self.view_h.get()))
    }
}

pub fn key(app: &mut App, k: &KeyEvent) -> Vec<Cmd> {
    let ctx = app.ctx();
    let Some(act) = lookup(ctx, k) else {
        return vec![];
    };
    match act {
        Act::Help => {
            app.open_help();
            return vec![];
        }
        Act::Context => {
            app.open_view(Screen::Context);
            return vec![];
        }
        Act::Usage => {
            app.open_view(Screen::Usage);
            return vec![];
        }
        _ => {}
    }
    let Some(d) = app.detail.as_mut() else {
        return app.go_home();
    };
    let id = d.info.id.clone();
    let page = usize::from(d.view_h.get().max(2) - 1);
    if d.in_diff {
        match act {
            Act::Apply => return vec![Cmd::PlanApply { id }],
            Act::Undo => ask_undo(app),
            Act::Back => d.in_diff = false,
            Act::Down => d.scroll = (d.scroll + 1).min(d.max_scroll()),
            Act::Up => d.scroll = d.scroll.saturating_sub(1),
            Act::PageDown => d.scroll = (d.scroll + page).min(d.max_scroll()),
            Act::PageUp => d.scroll = d.scroll.saturating_sub(page),
            Act::Top => d.scroll = 0,
            Act::Bottom => d.scroll = d.max_scroll(),
            Act::NextFile if d.sel + 1 < d.rows.len() => {
                d.sel += 1;
                d.scroll = 0;
            }
            Act::PrevFile if d.sel > 0 => {
                d.sel -= 1;
                d.scroll = 0;
            }
            _ => {}
        }
    } else {
        match act {
            Act::Apply => return vec![Cmd::PlanApply { id }],
            Act::Undo => ask_undo(app),
            Act::Back => return app.go_home(),
            Act::Tab => {
                let n = d.recalled().len();
                if n == 0 {
                    app.flash = Some("este run no usó ningún recuerdo".into());
                } else {
                    d.mem_sel = (d.mem_sel + 1) % n;
                }
            }
            Act::Useful | Act::Wrong => match d.recalled().get(d.mem_sel) {
                Some(m) => {
                    return vec![Cmd::MemoryFeedback {
                        id: *m,
                        useful: act == Act::Useful,
                    }];
                }
                None => app.flash = Some("este run no usó ningún recuerdo".into()),
            },
            Act::Diff if !d.rows.is_empty() => {
                d.in_diff = true;
                d.scroll = 0;
            }
            Act::Down => d.sel = (d.sel + 1).min(d.rows.len().saturating_sub(1)),
            Act::Up => d.sel = d.sel.saturating_sub(1),
            Act::Top => d.sel = 0,
            Act::Bottom => d.sel = d.rows.len().saturating_sub(1),
            _ => {}
        }
    }
    vec![]
}

fn ask_undo(app: &mut App) {
    let Some(d) = &app.detail else {
        return;
    };
    if d.info.status == Status::Applied {
        app.confirm = Some(Confirm {
            action: Action::Undo,
            id: d.info.id.clone(),
            files: d.info.files(),
            workspace: d.info.workspace.clone(),
        });
    } else {
        app.notice = Some("Solo se puede deshacer un run que ya se aplicó.".into());
    }
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let Some(d) = &app.detail else {
        return;
    };
    if d.in_diff {
        draw_diff(f, area, app, d);
        return;
    }
    let t = app.theme;
    let info = &d.info;
    let mem_line = memory_line(app, d);
    let [sum_a, _, msg_a, _, files_a] = Layout::vertical([
        Constraint::Length(4 + u16::from(mem_line.is_some())),
        Constraint::Length(1),
        Constraint::Percentage(40),
        Constraint::Length(1),
        Constraint::Min(3),
    ])
    .areas(area);

    let mut lines = vec![
        Line::from(vec![
            Span::styled(info.status.label(), t.status(info.status)),
            Span::styled(format!(" {} {}", t.dot(), clean(&info.id)), t.dim()),
        ]),
        Line::from(vec![
            Span::styled("carpeta ", t.dim()),
            Span::raw(clean(&info.workspace)),
        ]),
        Line::from(vec![
            Span::styled("pedido  ", t.dim()),
            Span::raw(clean(&info.instruction)),
        ]),
    ];
    match &info.summary {
        Some(s) => lines.push(Line::from(Span::styled(
            format!(
                "nivel {} {dot} {} pasos {dot} {} {dot} {} {dot} {} denegadas",
                s.level,
                s.steps,
                usage::ms_label(u64::try_from(s.elapsed_ms).unwrap_or(0)),
                ending_label(&s.ending),
                s.denied,
                dot = t.dot()
            ),
            t.dim(),
        ))),
        None => lines.push(Line::from(Span::styled(
            "sin resumen: el run no llegó a terminar",
            t.fail(),
        ))),
    }
    lines.extend(mem_line);
    f.render_widget(Paragraph::new(lines), sum_a);

    let msg = info
        .summary
        .as_ref()
        .map(|s| {
            s.final_message
                .clone()
                .unwrap_or_else(|| format!("(sin mensaje final) {}", s.pending.join("; ")))
        })
        .unwrap_or_default();
    let mut msg_lines = vec![section(t, "mensaje del agente", area.width)];
    msg_lines.extend(
        sanitize::multiline(&msg)
            .lines()
            .map(|l| Line::from(l.to_owned())),
    );
    f.render_widget(Paragraph::new(msg_lines).wrap(Wrap { trim: false }), msg_a);

    let mut items: Vec<Line> = Vec::new();
    if d.rows.is_empty() {
        items.push(Line::from(Span::styled("sin cambios", t.dim())));
    } else {
        // The heading takes one line; the selected row stays in view.
        let rows_h = usize::from(files_a.height).saturating_sub(1).max(1);
        let skip = (d.sel + 1).saturating_sub(rows_h);
        for (i, r) in d.rows.iter().enumerate().skip(skip) {
            let sel = i == d.sel;
            let kind_style = match r.kind {
                'A' => t.add(),
                'D' => t.del(),
                _ => t.warn(),
            };
            let base = if sel { t.selected() } else { t.plain() };
            items.push(Line::from(vec![
                Span::styled(
                    if sel {
                        format!("{} ", t.sel())
                    } else {
                        "  ".into()
                    },
                    base,
                ),
                Span::styled(format!("{} ", r.kind), if sel { base } else { kind_style }),
                Span::styled(clean(&r.rel), base),
            ]));
        }
    }
    let mut all = vec![section(
        t,
        &format!("archivos cambiados ({})", d.rows.len()),
        area.width,
    )];
    all.extend(items);
    f.render_widget(Paragraph::new(all), files_a);
}

fn draw_diff(f: &mut Frame, area: Rect, app: &App, d: &Detail) {
    let t = app.theme;
    let Some(row) = d.rows.get(d.sel) else {
        return;
    };
    let [head, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    let h = body.height;
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
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{} {}", row.kind, clean(&row.rel)), t.bold()),
            Span::styled(
                format!(
                    "  {} {}/{} {} línea {}/{}",
                    t.dot(),
                    d.sel + 1,
                    d.rows.len(),
                    t.dot(),
                    (scroll + 1).min(total.max(1)),
                    total
                ),
                t.dim(),
            ),
        ])),
        head,
    );
    f.render_widget(Paragraph::new(lines), body);
}

/// `memoria  recordó m3 [m7] · guardó 1 (1 en cuarentena)`: what the memory did in this run. The
/// memory in brackets is the one `+` and `-` judge (Tab changes it).
fn memory_line(app: &App, d: &Detail) -> Option<Line<'static>> {
    let t = app.theme;
    let m = d.info.summary.as_ref()?.memory.as_ref()?;
    let recalled = if m.recalled.is_empty() {
        "no recordó nada".to_owned()
    } else {
        let ids: Vec<String> = m
            .recalled
            .iter()
            .enumerate()
            .map(|(i, id)| {
                if i == d.mem_sel {
                    format!("[m{id}]")
                } else {
                    format!("m{id}")
                }
            })
            .collect();
        format!("recordó {} (+ útil, - equivocado)", ids.join(" "))
    };
    let mut text = format!(
        "{recalled} {} guardó {} ({} en cuarentena)",
        t.dot(),
        m.saved.len(),
        m.pending
    );
    if !m.rejected.is_empty() {
        text += &format!(" {} {} rechazado(s)", t.dot(), m.rejected.len());
    }
    Some(Line::from(vec![
        Span::styled("memoria ", t.dim()),
        Span::raw(clean(&text)),
    ]))
}
