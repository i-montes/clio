//! The live run: a timeline of what the agent asks for and what the gateway decides. Use and
//! context are in the status bar (and, on a wide terminal, in a column at the right); `Tab`
//! switches to the debug view while debug is on.

use std::cell::Cell;

use clio_brain::agent::Event;
use clio_gateway::Outcome;
use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::app::{App, Cmd, Screen, clean, section};
use super::debug_view::{self, Block, Kind};
use super::keys::{Act, lookup};
use super::sanitize;
use super::{meter, theme::Theme};
use crate::cli::runs::{RunSummary, ending_label};
use crate::cli::usage::{self, DEFAULT_WINDOW, RunLimits, RunUsage, Snapshot};

#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    Tool {
        step: u32,
        name: String,
        target: String,
        outcome: Outcome,
        rule: String,
        ran: bool,
        chars: usize,
    },
    Protocol {
        step: u32,
        reason: String,
    },
    Final {
        step: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LiveView {
    #[default]
    Timeline,
    Debug,
}

#[derive(Default)]
pub struct Live {
    pub id: Option<String>,
    pub instruction: String,
    pub workspace: String,
    pub files: usize,
    pub started_ms: u64,
    pub step: u32,
    pub level: Option<String>,
    pub tainted: bool,
    pub entries: Vec<Entry>,
    pub cancelling: bool,
    pub outcome: Option<Result<RunSummary, String>>,
    /// Calls, context and limits of this run.
    pub usage: RunUsage,
    pub view: LiveView,
    /// Debug view: one block per model call.
    pub blocks: Vec<Block>,
    /// Lines scrolled up from the newest.
    pub scroll: usize,
    /// Rows and columns the current view has, set when drawn.
    pub view_h: Cell<u16>,
    pub view_w: Cell<u16>,
}

impl Live {
    pub fn starting(now_ms: u64, instruction: &str) -> Self {
        Self {
            started_ms: now_ms,
            instruction: instruction.to_owned(),
            ..Self::default()
        }
    }

    pub fn begin(&mut self, id: String, files: usize, workspace: String, usage: RunUsage) {
        self.id = Some(id);
        self.files = files;
        self.workspace = workspace;
        self.usage = usage;
    }

    fn block(&mut self) -> &mut Block {
        if self.blocks.is_empty() {
            self.blocks.push(Block::default());
        }
        let last = self.blocks.len() - 1;
        &mut self.blocks[last]
    }

    pub fn on_event(&mut self, e: &Event) {
        self.usage.record(e);
        match e {
            Event::Started { level } => self.level = Some(format!("{level:?}")),
            Event::Step { n } => self.step = *n,
            Event::ProtocolError { reason } => {
                self.entries.push(Entry::Protocol {
                    step: self.step,
                    reason: reason.clone(),
                });
                self.block()
                    .note(Kind::Warn, &format!("error de protocolo: {reason}"));
            }
            Event::Final => self.entries.push(Entry::Final { step: self.step }),
            Event::Tool {
                name,
                target,
                outcome,
                rule,
                ran,
                result_chars,
                tainted,
            } => {
                self.tainted = *tainted;
                self.entries.push(Entry::Tool {
                    step: self.step,
                    name: name.clone(),
                    target: target.clone(),
                    outcome: *outcome,
                    rule: rule.clone(),
                    ran: *ran,
                    chars: *result_chars,
                });
            }
            Event::Context(c) => self.blocks.push(Block::context(c)),
            Event::Usage(u) => self.block().usage(u),
            Event::Exchange(x) => self.block().exchange(x),
            Event::Decision(d) => self.block().decision(d),
        }
    }

    pub fn finish(&mut self, r: Result<RunSummary, String>) {
        if let Ok(s) = &r {
            self.step = s.steps;
            self.tainted = s.tainted;
        }
        self.outcome = Some(r);
    }

    fn elapsed_secs(&self, now_ms: u64) -> u64 {
        match &self.outcome {
            Some(Ok(s)) => u64::try_from(s.elapsed_ms).unwrap_or(0) / 1000,
            _ => now_ms.saturating_sub(self.started_ms) / 1000,
        }
    }

    /// What the status bar and the views show for this run.
    pub fn snapshot(&self, now_ms: u64, fallback: (u64, bool, RunLimits)) -> Snapshot {
        let (i, o, c) = self.usage.totals();
        let (window, window_known, limits) = if self.usage.window > 0 {
            (
                self.usage.window,
                self.usage.window_known,
                self.usage.limits,
            )
        } else {
            fallback
        };
        Snapshot {
            tokens_in: i,
            tokens_out: o,
            cost_micro: c,
            steps: self.step,
            secs: self.elapsed_secs(now_ms),
            limits,
            ctx: self.usage.context.as_ref().map(|c| c.total()),
            window: if window > 0 { window } else { DEFAULT_WINDOW },
            window_known,
            tainted: self.tainted,
        }
    }

    fn shown_view(&self, debug: bool) -> LiveView {
        if debug { self.view } else { LiveView::Timeline }
    }

    fn total_lines(&self, debug: bool) -> usize {
        match self.shown_view(debug) {
            LiveView::Timeline => self.entries.len(),
            LiveView::Debug => {
                debug_view::total_lines(&self.blocks, usize::from(self.view_w.get()))
            }
        }
    }

    fn max_scroll(&self, debug: bool) -> usize {
        self.total_lines(debug)
            .saturating_sub(usize::from(self.view_h.get()))
    }
}

pub fn key(app: &mut App, k: &KeyEvent) -> Vec<Cmd> {
    let Some(act) = lookup(app.ctx(), k) else {
        return vec![];
    };
    let (debug, done) = (app.debug, app.live.outcome.is_some());
    let max = app.live.max_scroll(debug);
    let page = usize::from(app.live.view_h.get().max(2) - 1);
    match act {
        Act::Cancel => {
            if app.live.cancelling {
                // The second time: leave now. The run stays incomplete; the original was never touched.
                app.quit = true;
                return vec![];
            }
            app.live.cancelling = true;
            app.flash =
                Some("cancelando: termina el paso en curso (otra vez para salir ya)".into());
            return vec![Cmd::Cancel];
        }
        Act::Open if done => {
            return match (&app.live.outcome, &app.live.id) {
                (Some(Ok(_)), Some(id)) => vec![Cmd::LoadDetail {
                    id: id.clone(),
                    diff: false,
                }],
                _ => app.go_home(),
            };
        }
        Act::Back => return app.go_home(),
        Act::Help => app.open_help(),
        Act::Context => app.open_view(Screen::Context),
        Act::Usage => app.open_view(Screen::Usage),
        Act::Tab => {
            app.live.scroll = 0;
            app.live.view = match app.live.view {
                LiveView::Timeline if !debug => {
                    app.flash = Some("el modo debug está apagado: actívalo con Ctrl-D".into());
                    LiveView::Timeline
                }
                LiveView::Timeline => LiveView::Debug,
                LiveView::Debug => LiveView::Timeline,
            };
        }
        Act::Up => app.live.scroll = (app.live.scroll + 1).min(max),
        Act::Down => app.live.scroll = app.live.scroll.saturating_sub(1),
        Act::PageUp => app.live.scroll = (app.live.scroll + page).min(max),
        Act::PageDown => app.live.scroll = app.live.scroll.saturating_sub(page),
        Act::Top => app.live.scroll = max,
        Act::Bottom => app.live.scroll = 0,
        _ => {}
    }
    vec![]
}

/// The widest layout puts context and use in a column at the right.
/// Terminal columns from which the live screen has two columns (the screen has a column of air on each side).
pub const WIDE: u16 = 100;
const SIDE_W: u16 = 40;

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let l = &app.live;
    let finished = l.outcome.is_some();
    let res_h: u16 = if finished {
        8.min(area.height.saturating_sub(6))
    } else {
        0
    };
    let [head, _, main, res] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(res_h),
    ])
    .areas(area);

    let head_span = if let Some(r) = &l.outcome {
        match r {
            Ok(s) if s.succeeded() => Span::styled(format!("{} terminó", t.check()), t.ok()),
            Ok(s) => Span::styled(
                format!("{} {}", t.cross(), ending_label(&s.ending)),
                t.fail(),
            ),
            Err(_) => Span::styled(format!("{} no pudo trabajar", t.cross()), t.fail()),
        }
    } else if l.cancelling {
        Span::styled(
            format!("{} cancelando… (termina el paso en curso)", t.bang()),
            t.warn(),
        )
    } else if l.id.is_none() {
        Span::styled(
            format!("{} copiando el proyecto…", t.spinner(app.now_ms)),
            t.accent(),
        )
    } else {
        Span::styled(
            format!("{} trabajando en la copia…", t.spinner(app.now_ms)),
            t.accent(),
        )
    };
    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                head_span,
                Span::styled(
                    format!(
                        "   {}{}",
                        l.id.as_deref()
                            .map(super::home::short_id)
                            .map(|s| clean(&s))
                            .unwrap_or_default(),
                        l.level
                            .as_ref()
                            .map(|v| format!("   nivel {v}"))
                            .unwrap_or_default()
                    ),
                    t.dim(),
                ),
            ]),
            Line::from(Span::styled(
                debug_view::fit(&clean(&l.instruction), usize::from(head.width), t),
                t.dim(),
            )),
        ]),
        head,
    );

    if area.width + 2 >= WIDE {
        let [left, sep, right] = Layout::horizontal([
            Constraint::Min(30),
            Constraint::Length(3),
            Constraint::Length(SIDE_W),
        ])
        .areas(main);
        draw_main(f, left, app);
        let bar = vec![
            Line::from(Span::styled(format!(" {}", t.rule_v()), t.dim()));
            usize::from(sep.height)
        ];
        f.render_widget(Paragraph::new(bar), sep);
        meter::draw_side(f, right, app);
    } else {
        draw_main(f, main, app);
    }

    if let Some(r) = &l.outcome {
        let mut lines: Vec<Line> = vec![section(t, "resultado", res.width)];
        match r {
            Ok(s) => {
                lines.push(Line::from(format!(
                    "{} archivo(s) cambiados {dot} {} {dot} {} pasos {dot} {}",
                    s.files(),
                    ending_label(&s.ending),
                    s.steps,
                    usage::ms_label(u64::try_from(s.elapsed_ms).unwrap_or(0)),
                    dot = t.dot()
                )));
                let msg = s
                    .final_message
                    .clone()
                    .unwrap_or_else(|| s.pending.join("; "));
                for m in sanitize::multiline(&msg).lines() {
                    lines.push(Line::from(m.to_owned()));
                }
            }
            Err(e) => lines.push(Line::from(Span::styled(clean(e), t.fail()))),
        }
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), res);
    }
}

fn draw_main(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let l = &app.live;
    if l.shown_view(app.debug) == LiveView::Debug {
        debug_view::draw(f, area, app);
        return;
    }
    let rows = usize::from(area.height).saturating_sub(1).max(1);
    l.view_h.set(u16::try_from(rows).unwrap_or(u16::MAX));
    let end = l
        .entries
        .len()
        .saturating_sub(l.scroll.min(l.entries.len().saturating_sub(rows)));
    let start = end.saturating_sub(rows);
    let mut lines = vec![section(t, "línea de tiempo", area.width)];
    if l.entries.is_empty() {
        lines.push(Line::from(Span::styled(
            "esperando la primera acción del modelo…",
            t.dim(),
        )));
    } else {
        lines.extend(
            l.entries[start..end]
                .iter()
                .map(|e| entry_line(t, e, usize::from(area.width))),
        );
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn entry_line<'a>(t: Theme, e: &Entry, width: usize) -> Line<'a> {
    match e {
        Entry::Tool {
            step,
            name,
            target,
            outcome,
            rule,
            ran,
            chars,
        } => {
            let (mark, word, style) = match (outcome, ran) {
                (Outcome::Deny, _) => (t.cross(), "negado", t.fail()),
                (_, false) => (t.bang(), "pedido, sin ejecutar", t.warn()),
                (Outcome::Ask, true) => (t.bang(), "pedido y aprobado", t.warn()),
                _ => (t.check(), "permitido", t.ok()),
            };
            let size = if *ran {
                format!("  {chars} car.")
            } else {
                String::new()
            };
            let rule = debug_view::fit(&clean(rule), 16, t);
            let tail = format!(" ({rule}){size}");
            let fixed = 5 + 11 + 2 + 2 + word.chars().count() + tail.chars().count();
            let target = debug_view::fit(&clean(target), width.saturating_sub(fixed).max(8), t);
            Line::from(vec![
                Span::styled(format!("{step:>3}  "), t.dim()),
                Span::styled(format!("{:<11}", clean(name)), t.bold()),
                Span::raw(format!("{target}  ")),
                Span::styled(format!("{mark} {word}"), style),
                Span::styled(tail, t.dim()),
            ])
        }
        Entry::Protocol { step, reason } => Line::from(vec![
            Span::styled(format!("{step:>3}  "), t.dim()),
            Span::styled(format!("{} respuesta inválida ", t.bang()), t.warn()),
            Span::styled(clean(reason), t.dim()),
        ]),
        Entry::Final { step } => Line::from(vec![
            Span::styled(format!("{step:>3}  "), t.dim()),
            Span::styled(format!("{} el modelo dice que terminó", t.check()), t.ok()),
        ]),
    }
}
