//! The home screen: the state of Clio in one dim line and the list of recent runs, which `/`
//! filters. No boxes: the selection is a marker and the accent color.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::app::{Action, App, Cmd, Confirm, Screen, clean, plural, section};
use super::keys::{Act, lookup};
use super::theme::Theme;
use crate::cli::runs::{RunInfo, Status};
use crate::cli::ui::Level;
use crate::cli::usage::{RunLimits, RunUsage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Present,
    /// The key file can be read by others.
    OpenPermissions,
    Missing,
}

/// What the header shows. The key itself is never read into memory here, only its state.
#[derive(Debug, Clone)]
pub struct Header {
    pub model: String,
    pub key: KeyState,
    pub vault: String,
    /// The configuration did not load: the message.
    pub config_error: Option<String>,
    /// The limits of a run and the model's window, for what an older run did not record.
    pub limits: RunLimits,
    pub window: u64,
    pub window_known: bool,
}

#[derive(Default)]
pub struct Home {
    pub header: Option<Header>,
    pub runs: Vec<RunInfo>,
    /// Position in the visible (filtered) list.
    pub sel: usize,
    /// (ok, warnings, failures) once the doctor has run.
    pub doctor: Option<(usize, usize, usize)>,
    /// The usage saved for the newest run.
    pub last_usage: RunUsage,
    pub filter: String,
    /// Typing in the filter.
    pub filtering: bool,
}

impl Home {
    /// Indices into `runs` that match the filter (all of them when it is empty).
    pub fn visible(&self) -> Vec<usize> {
        let f = self.filter.trim().to_lowercase();
        self.runs
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                f.is_empty()
                    || [
                        &r.id,
                        &r.instruction,
                        &r.workspace,
                        &r.status.label().to_owned(),
                    ]
                    .iter()
                    .any(|s| s.to_lowercase().contains(&f))
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn selected(&self) -> Option<&RunInfo> {
        self.visible().get(self.sel).and_then(|i| self.runs.get(*i))
    }
}

/// `20261031-181646-5a67` shown as `1031-1816-5a67`: enough to tell runs apart on one line.
pub fn short_id(id: &str) -> String {
    let b = id.as_bytes();
    if b.len() >= 20 && b[8] == b'-' && b[15] == b'-' {
        format!("{}-{}-{}", &id[4..8], &id[9..13], &id[16..])
    } else {
        id.chars().take(14).collect()
    }
}

fn filter_key(app: &mut App, k: &KeyEvent) -> Vec<Cmd> {
    let n = app.home.visible().len();
    match lookup(super::keys::Ctx::Filter, k) {
        Some(Act::Open) => app.home.filtering = false,
        Some(Act::Back) => {
            app.home.filtering = false;
            app.home.filter.clear();
            app.home.sel = 0;
        }
        Some(Act::Down) => app.home.sel = (app.home.sel + 1).min(n.saturating_sub(1)),
        Some(Act::Up) => app.home.sel = app.home.sel.saturating_sub(1),
        _ => match k.code {
            KeyCode::Backspace => {
                app.home.filter.pop();
                app.home.sel = 0;
            }
            KeyCode::Char(c)
                if !c.is_control()
                    && !k
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                app.home.filter.push(c);
                app.home.sel = 0;
            }
            _ => {}
        },
    }
    vec![]
}

pub fn key(app: &mut App, k: &KeyEvent) -> Vec<Cmd> {
    if app.home.filtering {
        return filter_key(app, k);
    }
    let n = app.home.visible().len();
    let Some(act) = lookup(super::keys::Ctx::Home, k) else {
        return vec![];
    };
    match act {
        Act::Quit => app.quit = true,
        Act::Back => {
            app.home.filter.clear();
            app.home.sel = 0;
        }
        Act::Down => app.home.sel = (app.home.sel + 1).min(n.saturating_sub(1)),
        Act::Up => app.home.sel = app.home.sel.saturating_sub(1),
        Act::Top => app.home.sel = 0,
        Act::Bottom => app.home.sel = n.saturating_sub(1),
        Act::Filter => app.home.filtering = true,
        Act::New => {
            app.screen = Screen::NewRun;
            return super::new_run::opened(app);
        }
        Act::Refresh => {
            app.flash = Some("lista actualizada".into());
            return vec![Cmd::LoadHome];
        }
        Act::Doctor => {
            app.screen = Screen::Doctor;
            app.doctor.loading = true;
            return vec![Cmd::RunDoctor];
        }
        Act::Help => app.open_help(),
        Act::Context => app.open_view(Screen::Context),
        Act::Usage => app.open_view(Screen::Usage),
        Act::Memory => return super::memory::opened(app),
        Act::Open | Act::Diff | Act::Apply | Act::Undo => return on_selected(app, act),
        _ => {}
    }
    vec![]
}

fn on_selected(app: &mut App, act: Act) -> Vec<Cmd> {
    let Some(run) = app.home.selected() else {
        app.flash = Some(if app.home.runs.is_empty() {
            "no hay ningún run; crea uno con n".into()
        } else {
            "ningún run coincide con el filtro".into()
        });
        return vec![];
    };
    let id = run.id.clone();
    match act {
        Act::Undo => {
            if run.status != Status::Applied {
                app.notice = Some("Solo se puede deshacer un run que ya se aplicó.".into());
                return vec![];
            }
            app.confirm = Some(Confirm {
                action: Action::Undo,
                files: run.files(),
                workspace: run.workspace.clone(),
                id,
            });
            vec![]
        }
        Act::Apply => vec![Cmd::PlanApply { id }],
        Act::Diff => vec![Cmd::LoadDetail { id, diff: true }],
        _ => vec![Cmd::LoadDetail { id, diff: false }],
    }
}

/// The one dim line of state: key, vault, doctor. Only a problem gets a color.
fn state_line(app: &App, t: Theme, width: usize) -> Vec<Line<'static>> {
    let sep = || Span::styled(format!(" {} ", t.dot()), t.dim());
    let Some(h) = &app.home.header else {
        return vec![Line::from(Span::styled("cargando…", t.dim()))];
    };
    if let Some(e) = &h.config_error {
        return vec![Line::from(vec![
            Span::styled(format!("{} configuración: ", t.bang()), t.warn()),
            Span::styled(clean(e), t.warn()),
        ])];
    }
    let key = match h.key {
        KeyState::Present => Span::styled("clave presente", t.dim()),
        KeyState::OpenPermissions => {
            Span::styled("clave presente, pero otros pueden leerla", t.fail())
        }
        KeyState::Missing => Span::styled("falta la clave del modelo", t.warn()),
    };
    let doctor = match app.home.doctor {
        None => Span::styled("doctor: comprobando…", t.dim()),
        Some((ok, warn, fail)) => Span::styled(
            format!(
                "doctor: {ok} ok {} {} {} {}",
                t.dot(),
                plural(warn, "aviso", "avisos"),
                t.dot(),
                plural(fail, "fallo", "fallos")
            ),
            if fail > 0 {
                t.fail()
            } else if warn > 0 {
                t.warn()
            } else {
                t.dim()
            },
        ),
    };
    let mut first = vec![
        key,
        sep(),
        Span::styled(format!("vault {}", clean(&h.vault)), t.dim()),
    ];
    let w: usize = first
        .iter()
        .chain(std::iter::once(&doctor))
        .map(|s| s.content.chars().count())
        .sum::<usize>()
        + 3;
    // One line when it fits; otherwise the doctor goes on the next one.
    if w <= width {
        first.push(sep());
        first.push(doctor);
        vec![Line::from(first)]
    } else {
        vec![Line::from(first), Line::from(doctor)]
    }
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let mut lines: Vec<Line> = state_line(app, t, usize::from(area.width));
    lines.push(Line::from(""));
    lines.push(section(t, "recientes", area.width));
    let h = &app.home;
    if h.filtering || !h.filter.is_empty() {
        let n = h.visible().len();
        lines.push(Line::from(vec![
            Span::styled("/", t.accent()),
            Span::raw(clean(&h.filter)),
            Span::styled(if h.filtering { t.cursor() } else { "" }, t.accent()),
            Span::styled(format!("   {} de {}", n, h.runs.len()), t.dim()),
        ]));
    }
    if h.runs.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from("Todavía no hay runs."));
        lines.push(Line::from(vec![
            Span::raw("Pulsa "),
            Span::styled("n", t.key()),
            Span::raw(" para pedirle algo a Clio sobre un proyecto."),
        ]));
        f.render_widget(Paragraph::new(lines), area);
        return;
    }
    let visible = h.visible();
    if visible.is_empty() {
        lines.push(Line::from(Span::styled(
            "Ningún run coincide con el filtro.",
            t.dim(),
        )));
    }
    let used = lines.len();
    let rows = usize::from(area.height).saturating_sub(used).max(1);
    let skip = (h.sel + 1).saturating_sub(rows);
    let instr_w = usize::from(area.width).saturating_sub(2 + 15 + 21 + 9);
    for (pos, i) in visible.iter().enumerate().skip(skip).take(rows) {
        let r = &h.runs[*i];
        let selected = pos == h.sel;
        let base = if selected { t.selected() } else { t.plain() };
        let st = if selected {
            t.selected()
        } else {
            t.status(r.status)
        };
        let instr: String = clean(&r.instruction).chars().take(instr_w).collect();
        lines.push(Line::from(vec![
            Span::styled(
                if selected {
                    format!("{} ", t.sel())
                } else {
                    "  ".into()
                },
                base,
            ),
            Span::styled(
                format!("{:<15}", short_id(&r.id)),
                if selected { base } else { t.dim() },
            ),
            Span::styled(format!("{:<21}", r.status.label()), st),
            Span::styled(
                format!("{:>3} arch ", r.files()),
                if selected { base } else { t.dim() },
            ),
            Span::styled(instr, base),
        ]));
    }
    f.render_widget(Paragraph::new(lines), area);
}

/// For the doctor summary on the header.
pub fn counts(r: &crate::cli::doctor::Report) -> (usize, usize, usize) {
    (
        r.count(Level::Pass),
        r.count(Level::Warn),
        r.count(Level::Fail),
    )
}
