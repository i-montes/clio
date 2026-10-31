//! The live run: a timeline of what the agent asks for and what the gateway decides, the steps
//! and time against their limits, and the state of the session (tainted or not).

use clio_brain::agent::Event;
use clio_gateway::Outcome;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use super::app::{App, Cmd, clean, is_ctrl_c};
use super::sanitize;
use crate::cli::runs::{RunSummary, ending_label};

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

#[derive(Default)]
pub struct Live {
    pub id: Option<String>,
    pub instruction: String,
    pub files: usize,
    pub max_steps: u32,
    pub max_seconds: u64,
    pub started_ms: u64,
    pub step: u32,
    pub level: Option<String>,
    pub tainted: bool,
    pub entries: Vec<Entry>,
    pub cancelling: bool,
    pub outcome: Option<Result<RunSummary, String>>,
}

impl Live {
    pub fn starting(now_ms: u64, instruction: &str) -> Self {
        Self {
            started_ms: now_ms,
            instruction: instruction.to_owned(),
            ..Self::default()
        }
    }

    pub fn begin(&mut self, id: String, files: usize, max_steps: u32, max_seconds: u64) {
        self.id = Some(id);
        self.files = files;
        self.max_steps = max_steps;
        self.max_seconds = max_seconds;
    }

    pub fn on_event(&mut self, e: &Event) {
        match e {
            Event::Started { level } => self.level = Some(format!("{level:?}")),
            Event::Step { n } => self.step = *n,
            Event::ProtocolError { reason } => self.entries.push(Entry::Protocol {
                step: self.step,
                reason: reason.clone(),
            }),
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
        }
    }

    pub fn finish(&mut self, r: Result<RunSummary, String>) {
        if let Ok(s) = &r {
            self.step = s.steps;
            self.tainted = s.tainted;
        }
        self.outcome = Some(r);
    }
}

pub fn key(app: &mut App, k: &KeyEvent) -> Vec<Cmd> {
    if app.live.outcome.is_some() {
        return match k.code {
            KeyCode::Enter => match (&app.live.outcome, &app.live.id) {
                (Some(Ok(_)), Some(id)) => vec![Cmd::LoadDetail {
                    id: id.clone(),
                    diff: false,
                }],
                _ => app.go_home(),
            },
            KeyCode::Esc | KeyCode::Char('q') => app.go_home(),
            _ if is_ctrl_c(k) => app.go_home(),
            _ => vec![],
        };
    }
    if matches!(k.code, KeyCode::Esc) || is_ctrl_c(k) {
        if app.live.cancelling {
            // The second time: leave now. The run stays incomplete; the original was never touched.
            app.quit = true;
            return vec![];
        }
        app.live.cancelling = true;
        app.flash = Some("cancelando: termina el paso en curso (otra vez para salir ya)".into());
        return vec![Cmd::Cancel];
    }
    vec![]
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn bar(done: u64, total: u64, width: usize) -> String {
    let total = total.max(1);
    let filled = usize::try_from((done.min(total) * width as u64) / total).unwrap_or(width);
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let l = &app.live;
    let finished = l.outcome.is_some();
    let result_h: u16 = if finished { 8 } else { 0 };
    let [top, mid, res] = Layout::vertical([
        Constraint::Length(6),
        Constraint::Min(3),
        Constraint::Length(result_h),
    ])
    .areas(area);

    let elapsed_ms = match &l.outcome {
        Some(Ok(s)) => u64::try_from(s.elapsed_ms).unwrap_or(0),
        _ => app.now_ms.saturating_sub(l.started_ms),
    };
    let secs = elapsed_ms / 1000;
    let head = if let Some(r) = &l.outcome {
        match r {
            Ok(s) if s.succeeded() => Span::styled("✓ terminó", t.ok()),
            Ok(s) => Span::styled(format!("✗ {}", ending_label(&s.ending)), t.fail()),
            Err(_) => Span::styled("✗ no pudo trabajar", t.fail()),
        }
    } else if l.cancelling {
        Span::styled("◌ cancelando… (termina el paso en curso)", t.warn())
    } else if l.id.is_none() {
        Span::styled(
            format!(
                "{} copiando el proyecto…",
                SPINNER[(app.now_ms / 100 % 10) as usize]
            ),
            t.accent(),
        )
    } else {
        Span::styled(
            format!(
                "{} trabajando en la copia…",
                SPINNER[(app.now_ms / 100 % 10) as usize]
            ),
            t.accent(),
        )
    };
    let width = 24;
    let session = if l.tainted {
        Span::styled(
            "contaminada: leyó archivos, la pasarela es más estricta",
            t.warn(),
        )
    } else {
        Span::styled("limpia", t.ok())
    };
    let lines = vec![
        Line::from(vec![
            head,
            Span::raw("   "),
            Span::styled(l.id.as_deref().map(clean).unwrap_or_default(), t.dim()),
            Span::styled(
                l.level
                    .as_ref()
                    .map(|v| format!("   nivel {v}"))
                    .unwrap_or_default(),
                t.dim(),
            ),
        ]),
        Line::from(vec![
            Span::styled("Pasos  ", t.dim()),
            Span::raw(bar(u64::from(l.step), u64::from(l.max_steps), width)),
            Span::raw(format!(" {}/{}", l.step, l.max_steps)),
        ]),
        Line::from(vec![
            Span::styled("Tiempo ", t.dim()),
            Span::raw(bar(secs, l.max_seconds, width)),
            Span::raw(format!(" {secs}s/{}s", l.max_seconds)),
        ]),
        Line::from(vec![Span::styled("Sesión ", t.dim()), session]),
    ];
    f.render_widget(
        Paragraph::new(lines).block(
            Block::new()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(Span::styled(
                    format!(
                        " {} ",
                        clean(&l.instruction).chars().take(60).collect::<String>()
                    ),
                    t.bold(),
                )),
        ),
        top,
    );

    let rows = usize::from(mid.height.saturating_sub(2));
    let skip = l.entries.len().saturating_sub(rows);
    let timeline: Vec<Line> = if l.entries.is_empty() {
        vec![Line::from(Span::styled(
            "  esperando la primera acción del modelo…",
            t.dim(),
        ))]
    } else {
        l.entries
            .iter()
            .skip(skip)
            .map(|e| entry_line(app, e))
            .collect()
    };
    f.render_widget(
        Paragraph::new(timeline).block(
            Block::new()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(Span::styled(" Línea de tiempo ", t.bold())),
        ),
        mid,
    );

    if let Some(r) = &l.outcome {
        let mut lines: Vec<Line> = Vec::new();
        match r {
            Ok(s) => {
                lines.push(Line::from(format!(
                    "{} archivo(s) cambiados · {} · {} pasos · {:.1}s",
                    s.files(),
                    ending_label(&s.ending),
                    s.steps,
                    s.elapsed_ms as f64 / 1000.0
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
        f.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }).block(
                Block::new()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(Span::styled(" Resultado ", t.bold())),
            ),
            res,
        );
    }
}

fn entry_line<'a>(app: &App, e: &Entry) -> Line<'a> {
    let t = app.theme;
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
            let (word, style) = match (outcome, ran) {
                (Outcome::Deny, _) => ("negado", t.fail()),
                (_, false) => ("pedido, sin ejecutar", t.warn()),
                (Outcome::Ask, true) => ("pedido y aprobado", t.warn()),
                _ => ("permitido", t.ok()),
            };
            let size = if *ran {
                format!("  {chars} car.")
            } else {
                String::new()
            };
            Line::from(vec![
                Span::styled(format!(" {step:>3}  "), t.dim()),
                Span::styled(format!("{:<11}", clean(name)), t.bold()),
                Span::raw(format!("{}  ", clean(target))),
                Span::styled(word, style),
                Span::styled(format!(" ({}){size}", clean(rule)), t.dim()),
            ])
        }
        Entry::Protocol { step, reason } => Line::from(vec![
            Span::styled(format!(" {step:>3}  "), t.dim()),
            Span::styled("respuesta inválida ", t.warn()),
            Span::styled(clean(reason), t.dim()),
        ]),
        Entry::Final { step } => Line::from(vec![
            Span::styled(format!(" {step:>3}  "), t.dim()),
            Span::styled("el modelo dice que terminó", t.ok()),
        ]),
    }
}
