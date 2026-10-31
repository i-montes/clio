//! The home screen: the state of Clio at a glance and the list of recent runs.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph};

use super::app::{App, Action, Cmd, Confirm, Screen, clean, is_ctrl_c, plural};
use crate::cli::runs::{RunInfo, Status};
use crate::cli::ui::Level;

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
    pub provider: String,
    pub model: String,
    pub key: KeyState,
    pub vault: String,
    /// The configuration did not load: the message.
    pub config_error: Option<String>,
}

#[derive(Default)]
pub struct Home {
    pub header: Option<Header>,
    pub runs: Vec<RunInfo>,
    pub sel: usize,
    /// (ok, warnings, failures) once the doctor has run.
    pub doctor: Option<(usize, usize, usize)>,
}

impl Home {
    pub fn selected(&self) -> Option<&RunInfo> {
        self.runs.get(self.sel)
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

pub fn key(app: &mut App, k: &KeyEvent) -> Vec<Cmd> {
    let n = app.home.runs.len();
    let sel = &mut app.home.sel;
    match k.code {
        KeyCode::Char('q') | KeyCode::Esc => app.quit = true,
        _ if is_ctrl_c(k) => app.quit = true,
        KeyCode::Down | KeyCode::Char('j') => *sel = (*sel + 1).min(n.saturating_sub(1)),
        KeyCode::Up | KeyCode::Char('k') => *sel = sel.saturating_sub(1),
        KeyCode::Home | KeyCode::Char('g') => *sel = 0,
        KeyCode::End | KeyCode::Char('G') => *sel = n.saturating_sub(1),
        KeyCode::Char('n') => {
            app.screen = Screen::NewRun;
            return super::new_run::opened(app);
        }
        KeyCode::Char('r') => {
            app.flash = Some("lista actualizada".into());
            return vec![Cmd::LoadHome];
        }
        KeyCode::Char('D') => {
            app.screen = Screen::Doctor;
            app.doctor.loading = true;
            return vec![Cmd::RunDoctor];
        }
        KeyCode::Char('?') => {
            app.before_help = Screen::Home;
            app.screen = Screen::Help;
        }
        KeyCode::Enter | KeyCode::Char('d' | 'a' | 'u')
            if !k.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            return on_selected(app, k.code);
        }
        _ => {}
    }
    vec![]
}

fn on_selected(app: &mut App, code: KeyCode) -> Vec<Cmd> {
    let Some(run) = app.home.selected() else {
        app.flash = Some("no hay ningún run elegido; crea uno con n".into());
        return vec![];
    };
    let id = run.id.clone();
    match code {
        KeyCode::Char('u') => {
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
        KeyCode::Char('a') => vec![Cmd::PlanApply { id }],
        KeyCode::Char('d') => vec![Cmd::LoadDetail { id, diff: true }],
        _ => vec![Cmd::LoadDetail { id, diff: false }],
    }
}

fn dot(t: super::theme::Theme, ok: bool) -> Span<'static> {
    if ok {
        Span::styled("●", t.ok())
    } else {
        Span::styled("●", t.warn())
    }
}

pub fn draw(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let [head, list] = Layout::vertical([Constraint::Length(4), Constraint::Min(3)]).areas(area);

    let mut lines = Vec::new();
    match &app.home.header {
        None => lines.push(Line::from(Span::styled("cargando…", t.dim()))),
        Some(h) => {
            if let Some(e) = &h.config_error {
                lines.push(Line::from(vec![
                    dot(t, false),
                    Span::raw(" configuración: "),
                    Span::styled(clean(e), t.warn()),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled("Modelo ", t.dim()),
                    Span::styled(format!("{} / {}", clean(&h.provider), clean(&h.model)), t.bold()),
                    Span::raw("    "),
                    Span::styled("Clave ", t.dim()),
                    match h.key {
                        KeyState::Present => Span::styled("presente", t.ok()),
                        KeyState::OpenPermissions => {
                            Span::styled("presente, pero otros pueden leerla", t.fail())
                        }
                        KeyState::Missing => Span::styled("falta", t.warn()),
                    },
                ]));
            }
            let doctor = match app.home.doctor {
                None => Span::styled("comprobando…", t.dim()),
                Some((ok, warn, fail)) => Span::styled(
                    format!("{ok} ok · {} · {}", plural(warn, "aviso", "avisos"), plural(fail, "fallo", "fallos")),
                    if fail > 0 {
                        t.fail()
                    } else if warn > 0 {
                        t.warn()
                    } else {
                        t.ok()
                    },
                ),
            };
            lines.push(Line::from(vec![
                Span::styled("Vault ", t.dim()),
                Span::raw(clean(&h.vault)),
                Span::raw("    "),
                Span::styled("Doctor ", t.dim()),
                doctor,
            ]));
        }
    }
    f.render_widget(
        Paragraph::new(lines).block(
            Block::new()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(Span::styled(" Estado ", t.bold())),
        ),
        head,
    );

    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Span::styled(" Runs recientes ", t.bold()));
    if app.home.runs.is_empty() {
        f.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from("  Todavía no hay runs."),
                Line::from(vec![
                    Span::raw("  Pulsa "),
                    Span::styled("n", t.key()),
                    Span::raw(" para pedirle algo a Clio sobre un proyecto."),
                ]),
            ])
            .block(block),
            list,
        );
        return;
    }
    let inner_w = usize::from(list.width.saturating_sub(2));
    let instr_w = inner_w.saturating_sub(2 + 15 + 21 + 9);
    let items: Vec<ListItem> = app
        .home
        .runs
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let selected = i == app.home.sel;
            let st = if selected { t.selected() } else { t.status(r.status) };
            let base = if selected { t.selected() } else { t.plain() };
            let instr: String = clean(&r.instruction).chars().take(instr_w).collect();
            ListItem::new(Line::from(vec![
                Span::styled(if selected { "▶ " } else { "  " }, base),
                Span::styled(format!("{:<15}", short_id(&r.id)), base),
                Span::styled(format!("{:<21}", r.status.label()), st),
                Span::styled(format!("{:>3} arch ", r.files()), base),
                Span::styled(instr, base),
            ]))
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(app.home.sel));
    f.render_stateful_widget(List::new(items).block(block), list, &mut state);
}

/// For the doctor summary on the header.
pub fn counts(r: &crate::cli::doctor::Report) -> (usize, usize, usize) {
    (r.count(Level::Pass), r.count(Level::Warn), r.count(Level::Fail))
}
