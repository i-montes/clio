//! The state of the TUI and the one function that changes it: `update(app, input) -> (app, cmds)`.
//! It never touches the terminal, the disk or a thread: anything that must happen outside is
//! returned as a `Cmd` for the runtime (`mod.rs`) to carry out, and its result comes back as an
//! `Input::Bg`. That is what lets every screen be tested without a terminal.

use clio_brain::agent::Event;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

use super::detail::Detail;
use super::doctor::DoctorView;
use super::home::{Header, Home};
use super::live::Live;
use super::new_run::{NewRun, Preview};
use super::sanitize;
use super::theme::Theme;
use crate::cli::doctor::Report;
use crate::cli::runs::{RunInfo, RunSummary};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Home,
    NewRun,
    Live,
    Detail,
    Doctor,
    Help,
}

/// Something to do outside of `update`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    LoadHome,
    RunDoctor,
    Check { dir: String, seq: u64 },
    Start { dir: String, instruction: String },
    Cancel,
    LoadDetail { id: String, diff: bool },
    PlanApply { id: String },
    Apply { id: String },
    Undo { id: String },
}

/// What a worker thread reports back.
pub enum Bg {
    Home(Box<(Header, Vec<RunInfo>)>),
    Doctor(Report),
    Checked {
        seq: u64,
        result: Result<Preview, String>,
    },
    Started {
        id: String,
        files: usize,
        max_steps: u32,
        max_seconds: u64,
    },
    Agent(Event),
    Finished(Result<Box<RunSummary>, String>),
    Detail {
        detail: Result<Box<Detail>, String>,
        diff: bool,
    },
    Planned(Result<Planned, String>),
    Done(Result<String, String>),
}

pub struct Planned {
    pub id: String,
    pub files: usize,
    pub workspace: String,
}

pub enum Input {
    Key(KeyEvent),
    Paste(String),
    Tick(u64),
    Bg(Bg),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Apply,
    Undo,
}

/// A question that needs a `y`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirm {
    pub action: Action,
    pub id: String,
    pub files: usize,
    pub workspace: String,
}

pub struct App {
    pub screen: Screen,
    /// Where `?` came from.
    pub before_help: Screen,
    pub quit: bool,
    pub now_ms: u64,
    pub theme: Theme,
    pub home: Home,
    pub new_run: NewRun,
    pub live: Live,
    pub detail: Option<Detail>,
    pub doctor: DoctorView,
    pub confirm: Option<Confirm>,
    /// A short line of feedback at the bottom.
    pub flash: Option<String>,
    /// An error or a longer message in a box, closed by any key.
    pub notice: Option<String>,
    pub check_seq: u64,
}

impl App {
    pub fn new(cwd: &str, theme: Theme) -> Self {
        Self {
            screen: Screen::Home,
            before_help: Screen::Home,
            quit: false,
            now_ms: 0,
            theme,
            home: Home::default(),
            new_run: NewRun::new(cwd),
            live: Live::default(),
            detail: None,
            doctor: DoctorView::default(),
            confirm: None,
            flash: None,
            notice: None,
            check_seq: 0,
        }
    }

    /// The commands to run when the TUI opens.
    pub fn start_cmds() -> Vec<Cmd> {
        vec![Cmd::LoadHome, Cmd::RunDoctor]
    }

    pub fn go_home(&mut self) -> Vec<Cmd> {
        self.screen = Screen::Home;
        vec![Cmd::LoadHome]
    }
}

pub fn is_ctrl_c(k: &KeyEvent) -> bool {
    k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c')
}

pub fn update(mut app: App, input: Input) -> (App, Vec<Cmd>) {
    let cmds = step(&mut app, input);
    (app, cmds)
}

fn step(app: &mut App, input: Input) -> Vec<Cmd> {
    match input {
        Input::Tick(ms) => {
            app.now_ms = ms;
            vec![]
        }
        Input::Paste(text) => {
            if app.screen == Screen::NewRun && app.confirm.is_none() && app.notice.is_none() {
                super::new_run::paste(app, &text)
            } else {
                vec![]
            }
        }
        Input::Bg(bg) => background(app, bg),
        Input::Key(k) if k.kind == KeyEventKind::Release => vec![],
        Input::Key(k) => {
            app.flash = None;
            if app.notice.take().is_some() {
                return vec![];
            }
            if app.confirm.is_some() {
                return answer(app, &k);
            }
            match app.screen {
                Screen::Home => super::home::key(app, &k),
                Screen::NewRun => super::new_run::key(app, &k),
                Screen::Live => super::live::key(app, &k),
                Screen::Detail => super::detail::key(app, &k),
                Screen::Doctor => super::doctor::key(app, &k),
                Screen::Help => {
                    app.screen = app.before_help;
                    vec![]
                }
            }
        }
    }
}

/// The reply to a confirmation: only `y` (or `s`, for sí) goes ahead.
fn answer(app: &mut App, k: &KeyEvent) -> Vec<Cmd> {
    let Some(c) = app.confirm.take() else {
        return vec![];
    };
    if !matches!(k.code, KeyCode::Char('y' | 'Y' | 's' | 'S')) {
        app.flash = Some("cancelado: no se cambió nada".into());
        return vec![];
    }
    match c.action {
        Action::Apply => vec![Cmd::Apply { id: c.id }],
        Action::Undo => vec![Cmd::Undo { id: c.id }],
    }
}

fn background(app: &mut App, bg: Bg) -> Vec<Cmd> {
    match bg {
        Bg::Home(b) => {
            let (header, runs) = *b;
            app.home.header = Some(header);
            app.home.runs = runs;
            app.home.sel = app.home.sel.min(app.home.runs.len().saturating_sub(1));
            vec![]
        }
        Bg::Doctor(r) => {
            app.home.doctor = Some(super::home::counts(&r));
            app.doctor.set(r);
            vec![]
        }
        Bg::Checked { seq, result } => {
            if seq == app.check_seq {
                app.new_run.check = Some(result);
            }
            vec![]
        }
        Bg::Started {
            id,
            files,
            max_steps,
            max_seconds,
        } => {
            app.live.begin(id, files, max_steps, max_seconds);
            vec![]
        }
        Bg::Agent(e) => {
            app.live.on_event(&e);
            vec![]
        }
        Bg::Finished(r) => {
            app.live.finish(r.map(|b| *b));
            vec![Cmd::LoadHome]
        }
        Bg::Detail { detail, diff } => match detail {
            Ok(d) => {
                let same = app.screen == Screen::Detail
                    && app.detail.as_ref().is_some_and(|o| o.info.id == d.info.id);
                let mut d = *d;
                if same && let Some(old) = &app.detail {
                    d.keep_position(old);
                } else {
                    d.in_diff = diff && !d.rows.is_empty();
                }
                app.detail = Some(d);
                app.screen = Screen::Detail;
                vec![]
            }
            Err(e) => {
                app.notice = Some(e);
                vec![]
            }
        },
        Bg::Planned(Ok(p)) => {
            app.confirm = Some(Confirm {
                action: Action::Apply,
                id: p.id,
                files: p.files,
                workspace: p.workspace,
            });
            vec![]
        }
        Bg::Planned(Err(e)) => {
            app.notice = Some(e);
            vec![]
        }
        Bg::Done(Ok(msg)) => {
            app.flash = Some(msg);
            let mut cmds = vec![Cmd::LoadHome];
            if let (Screen::Detail, Some(d)) = (app.screen, &app.detail) {
                cmds.push(Cmd::LoadDetail {
                    id: d.info.id.clone(),
                    diff: false,
                });
            }
            cmds
        }
        Bg::Done(Err(e)) => {
            app.notice = Some(e);
            vec![]
        }
    }
}

// ---------------------------------------------------------------- drawing

/// Text that is not ours, ready to be drawn (one line).
pub fn clean(s: &str) -> String {
    sanitize::line(s)
}

pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// One footer entry per key: `key` highlighted, then what it does.
pub fn keys_line(theme: Theme, keys: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (k, what) in keys {
        spans.push(Span::styled((*k).to_owned(), theme.key()));
        spans.push(Span::raw(format!(" {what}   ")));
    }
    Line::from(spans)
}

const MIN_W: u16 = 40;
const MIN_H: u16 = 10;

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    if area.width < MIN_W || area.height < MIN_H {
        f.render_widget(
            Paragraph::new(format!("Terminal muy pequeña: se necesita al menos {MIN_W}x{MIN_H}."))
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let t = app.theme;
    let [title, body, flash, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(2),
    ])
    .areas(area);
    let name = match app.screen {
        Screen::Home => "Inicio",
        Screen::NewRun => "Nuevo trabajo",
        Screen::Live => "En vivo",
        Screen::Detail => "Detalle del run",
        Screen::Doctor => "Doctor",
        Screen::Help => "Ayuda",
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" clio ", t.selected()),
            Span::styled(format!(" {name}"), t.bold()),
            Span::styled("  (el original nunca se toca sin tu permiso)", t.dim()),
        ])),
        title,
    );
    let keys: Vec<(&str, &str)> = match app.screen {
        Screen::Home => vec![
            ("n", "nuevo"),
            ("Enter", "abrir"),
            ("d", "diff"),
            ("a", "aplicar"),
            ("u", "deshacer"),
            ("r", "refrescar"),
            ("D", "doctor"),
            ("?", "ayuda"),
            ("q", "salir"),
        ],
        Screen::NewRun => vec![
            ("Tab", "cambiar campo"),
            ("Enter", "lanzar"),
            ("Alt+Enter", "línea nueva"),
            ("Ctrl+U", "borrar campo"),
            ("Esc", "volver"),
        ],
        Screen::Live => {
            if app.live.outcome.is_some() {
                vec![("Enter", "ver el run"), ("Esc", "inicio")]
            } else {
                vec![("Esc / Ctrl+C", "cancelar (termina el paso en curso)")]
            }
        }
        Screen::Detail => vec![
            ("j/k", "mover"),
            ("Enter", "ver diff"),
            ("a", "aplicar"),
            ("u", "deshacer"),
            ("Esc", "volver"),
            ("?", "ayuda"),
        ],
        Screen::Doctor => vec![("j/k", "desplazar"), ("r", "repetir"), ("Esc", "volver")],
        Screen::Help => vec![("cualquier tecla", "volver")],
    };
    f.render_widget(
        Paragraph::new(keys_line(t, &keys)).wrap(Wrap { trim: true }),
        footer,
    );
    if let Some(msg) = &app.flash {
        f.render_widget(Paragraph::new(Span::styled(clean(msg), t.ok())), flash);
    }
    match app.screen {
        Screen::Home => super::home::draw(f, body, app),
        Screen::NewRun => super::new_run::draw(f, body, app),
        Screen::Live => super::live::draw(f, body, app),
        Screen::Detail => super::detail::draw(f, body, app),
        Screen::Doctor => super::doctor::draw(f, body, app),
        Screen::Help => draw_help(f, body, t),
    }
    if let Some(c) = &app.confirm {
        let verb = match c.action {
            Action::Apply => format!("Aplicar {} archivo(s) en", c.files),
            Action::Undo => format!("Deshacer lo aplicado ({} archivo(s)) en", c.files),
        };
        let lines = vec![
            Line::from(Span::styled(verb, t.bold())),
            Line::from(clean(&c.workspace)),
            Line::from(""),
            Line::from(vec![
                Span::styled("y", t.key()),
                Span::raw(" sí, continuar      "),
                Span::styled("cualquier otra tecla", t.key()),
                Span::raw(" cancela"),
            ]),
        ];
        dialog(f, area, t, " Confirmar ", lines, t.warn());
    } else if let Some(n) = &app.notice {
        let mut lines: Vec<Line> = sanitize::multiline(n)
            .lines()
            .map(|l| Line::from(l.to_owned()))
            .collect();
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("(cualquier tecla cierra)", t.dim())));
        dialog(f, area, t, " Aviso ", lines, t.fail());
    }
}

fn dialog(
    f: &mut Frame,
    area: Rect,
    t: Theme,
    title: &str,
    lines: Vec<Line<'static>>,
    border: ratatui::style::Style,
) {
    let w = area.width.saturating_sub(6).clamp(30, 76);
    let h = (u16::try_from(lines.len()).unwrap_or(u16::MAX) + 4).min(area.height);
    let r = centered(area, w, h.max(5));
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::new()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(border)
                .title(Span::styled(title.to_owned(), t.bold())),
        ),
        r,
    );
}

fn draw_help(f: &mut Frame, area: Rect, t: Theme) {
    let row = |k: &str, d: &str| {
        Line::from(vec![
            Span::styled(format!("  {k:<14}"), t.key()),
            Span::raw(d.to_owned()),
        ])
    };
    let head = |s: &str| Line::from(Span::styled(s.to_owned(), t.accent()));
    let lines = vec![
        head("Clio trabaja en una COPIA de tu proyecto"),
        Line::from("  Tú revisas el diff y decides qué vuelve al original; todo se puede deshacer."),
        Line::from(""),
        head("Inicio"),
        row("j / k, flechas", "elegir un run"),
        row("n", "nuevo trabajo"),
        row("Enter", "abrir el run (resumen, mensaje del agente y archivos)"),
        row("d / a / u", "ver el diff / aplicar / deshacer el run elegido"),
        row("r / D", "refrescar la lista / abrir el doctor"),
        row("q / Ctrl+C", "salir"),
        Line::from(""),
        head("Detalle"),
        row("j / k", "elegir archivo (o desplazar el diff)"),
        row("Enter / Esc", "abrir el diff / volver"),
        row("PgUp PgDn", "desplazar el diff de a una página"),
        row("a / u", "aplicar (pide y) / deshacer (pide y)"),
        Line::from(""),
        head("Nuevo trabajo y en vivo"),
        row("Tab", "cambiar entre carpeta e instrucción"),
        row("Enter", "lanzar (Alt+Enter o Ctrl+J: línea nueva)"),
        row("Esc / Ctrl+C", "cancelar en vivo: termina tras el paso en curso"),
    ];
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::new()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(Span::styled(" Teclas ", t.bold())),
        ),
        area,
    );
}

/// Counts by label, for the home header and the live panel.
pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
