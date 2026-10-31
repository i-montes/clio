//! The state of the TUI and the one function that changes it: `update(app, input) -> (app, cmds)`.
//! It never touches the terminal, the disk or a thread: anything that must happen outside is
//! returned as a `Cmd` for the runtime (`mod.rs`) to carry out, and its result comes back as an
//! `Input::Bg`. That is what lets every screen be tested without a terminal.
//!
//! The frame is always the same: one header line, the screen, a line of feedback, one line of key
//! hints and the status bar (use of the run and layer of context) on the very last line.

use clio_brain::agent::Event;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

use super::detail::Detail;
use super::doctor::DoctorView;
use super::home::{Header, Home};
use super::keys::{self, Act, Ctx};
use super::live::Live;
use super::new_run::{NewRun, Preview};
use super::sanitize;
use super::theme::Theme;
use super::{meter, status};
use crate::cli::doctor::Report;
use crate::cli::runs::{RunInfo, RunSummary};
use crate::cli::usage::{DEFAULT_WINDOW, RunLimits, RunUsage, Snapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Home,
    NewRun,
    Live,
    Detail,
    Doctor,
    Help,
    Context,
    Usage,
}

/// Something to do outside of `update`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    LoadHome,
    RunDoctor,
    Check {
        dir: String,
        seq: u64,
    },
    Start {
        dir: String,
        instruction: String,
    },
    Cancel,
    LoadDetail {
        id: String,
        diff: bool,
    },
    PlanApply {
        id: String,
    },
    Apply {
        id: String,
    },
    Undo {
        id: String,
    },
    /// Debug was switched; the agent reads the switch at every call.
    Debug(bool),
    /// Clear the terminal and draw everything again.
    Redraw,
}

/// What a worker thread reports back.
pub enum Bg {
    /// The header, the runs (newest first) and the usage saved for the newest one.
    Home(Box<(Header, Vec<RunInfo>, RunUsage)>),
    Doctor(Report),
    Checked {
        seq: u64,
        result: Result<Preview, String>,
    },
    Started {
        id: String,
        files: usize,
        workspace: String,
        /// Empty, with the limits and the window this run goes by.
        usage: RunUsage,
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
    /// Where the context and usage views came from: they show that run's data.
    pub back: Screen,
    pub quit: bool,
    pub now_ms: u64,
    pub theme: Theme,
    pub cwd: String,
    pub debug: bool,
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
    /// Scroll of the context, usage and help views, and how far the last draw let it go.
    pub view_scroll: usize,
    pub view_max: std::cell::Cell<usize>,
}

impl App {
    pub fn new(cwd: &str, theme: Theme) -> Self {
        Self {
            screen: Screen::Home,
            before_help: Screen::Home,
            back: Screen::Home,
            quit: false,
            now_ms: 0,
            theme,
            cwd: cwd.to_owned(),
            debug: false,
            home: Home::default(),
            new_run: NewRun::new(cwd),
            live: Live::default(),
            detail: None,
            doctor: DoctorView::default(),
            confirm: None,
            flash: None,
            notice: None,
            check_seq: 0,
            view_scroll: 0,
            view_max: std::cell::Cell::new(0),
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

    /// The screen as the key map sees it.
    pub fn ctx(&self) -> Ctx {
        self.ctx_of(self.screen)
    }

    pub fn ctx_of(&self, s: Screen) -> Ctx {
        match s {
            Screen::Home if self.home.filtering => Ctx::Filter,
            Screen::Home => Ctx::Home,
            Screen::NewRun => Ctx::NewRun,
            Screen::Live if self.live.outcome.is_some() => Ctx::LiveDone,
            Screen::Live => Ctx::LiveRun,
            Screen::Detail if self.detail.as_ref().is_some_and(|d| d.in_diff) => Ctx::DetailDiff,
            Screen::Detail => Ctx::DetailList,
            Screen::Doctor => Ctx::Doctor,
            Screen::Context => Ctx::Context,
            Screen::Usage => Ctx::Usage,
            Screen::Help => Ctx::Help,
        }
    }

    /// The window, the limits and the name of the model Clio is set up with, for what a run did
    /// not record.
    fn fallback(&self) -> (u64, bool, RunLimits) {
        self.home
            .header
            .as_ref()
            .map_or((DEFAULT_WINDOW, false, RunLimits::default()), |h| {
                (h.window, h.window_known, h.limits)
            })
    }

    /// The screen whose run the views show: for the overlays, the one they were opened from.
    pub fn data_screen(&self) -> Screen {
        match self.screen {
            Screen::Context | Screen::Usage => self.back,
            Screen::Help if matches!(self.before_help, Screen::Context | Screen::Usage) => {
                self.back
            }
            Screen::Help => self.before_help,
            s => s,
        }
    }

    /// The numbers of the run on screen, and its per-call usage. `None` where no run is shown.
    pub fn run_data(&self) -> Option<(Snapshot, &RunUsage)> {
        let fb = self.fallback();
        match self.data_screen() {
            Screen::Live if self.live.id.is_some() || self.live.outcome.is_some() => {
                Some((self.live.snapshot(self.now_ms, fb), &self.live.usage))
            }
            Screen::Detail => {
                let d = self.detail.as_ref()?;
                let s = d.info.summary.as_ref()?;
                Some((Snapshot::of_summary(s, &d.usage, fb), &d.usage))
            }
            Screen::Home => {
                let s = self.home.runs.first()?.summary.as_ref()?;
                Some((
                    Snapshot::of_summary(s, &self.home.last_usage, fb),
                    &self.home.last_usage,
                ))
            }
            _ => None,
        }
    }

    /// Opens the context or the usage view of the run on this screen.
    pub fn open_view(&mut self, view: Screen) {
        if self.run_data().is_none() {
            self.flash = Some(if self.screen == Screen::Home {
                "todavía no hay runs".into()
            } else {
                "este run no tiene datos de uso".into()
            });
            return;
        }
        if !matches!(self.screen, Screen::Context | Screen::Usage) {
            self.back = self.screen;
        }
        self.screen = view;
        self.view_scroll = 0;
    }

    pub fn open_help(&mut self) {
        self.before_help = self.screen;
        self.screen = Screen::Help;
        self.view_scroll = 0;
    }
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
            // The keys that work everywhere, even over a message.
            match keys::lookup(app.ctx(), &k) {
                Some(Act::Debug) => {
                    app.debug = !app.debug;
                    app.flash = Some(if app.debug {
                        "debug activado (Tab en vivo; se escribe debug.log en la carpeta del run)"
                            .into()
                    } else {
                        "debug desactivado".into()
                    });
                    return vec![Cmd::Debug(app.debug)];
                }
                Some(Act::Redraw) => return vec![Cmd::Redraw],
                _ => {}
            }
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
                Screen::Context | Screen::Usage | Screen::Help => meter::key(app, &k),
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
            let (header, runs, last) = *b;
            app.home.header = Some(header);
            app.home.runs = runs;
            app.home.last_usage = last;
            let n = app.home.visible().len();
            app.home.sel = app.home.sel.min(n.saturating_sub(1));
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
            workspace,
            usage,
        } => {
            app.live.begin(id, files, workspace, usage);
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

/// A thin labeled line: `─ título ──────────`.
pub fn section(t: Theme, title: &str, width: u16) -> Line<'static> {
    let used = title.chars().count() + 3;
    let tail = t.rule().repeat(usize::from(width).saturating_sub(used));
    Line::from(vec![
        Span::styled(format!("{} ", t.rule()), t.dim()),
        Span::styled(title.to_owned(), t.bold()),
        Span::styled(format!(" {tail}"), t.dim()),
    ])
}

pub const MIN_W: u16 = 60;
pub const MIN_H: u16 = 16;

fn screen_name(app: &App) -> &'static str {
    match app.screen {
        Screen::Home => "inicio",
        Screen::NewRun => "nuevo trabajo",
        Screen::Live => "en vivo",
        Screen::Detail => "detalle",
        Screen::Doctor => "doctor",
        Screen::Help => "ayuda",
        Screen::Context => "contexto",
        Screen::Usage => "uso",
    }
}

fn folder_name(path: &str) -> String {
    let p = std::path::Path::new(path);
    p.file_name()
        .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned())
}

/// `clio · carpeta · modelo`, and on the right the screen (and `debug` when it is on).
fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let folder = match app.screen {
        Screen::Live if !app.live.workspace.is_empty() => folder_name(&app.live.workspace),
        Screen::Detail => app
            .detail
            .as_ref()
            .map_or_else(|| folder_name(&app.cwd), |d| folder_name(&d.info.workspace)),
        _ => folder_name(&app.cwd),
    };
    let sep = format!(" {} ", t.dot());
    let mut left = vec![
        Span::styled("clio", t.bold()),
        Span::styled(format!("{sep}{}", clean(&folder)), t.plain()),
    ];
    if let Some(h) = &app.home.header
        && h.config_error.is_none()
    {
        left.push(Span::styled(format!("{sep}{}", clean(&h.model)), t.dim()));
    }
    let mut right = vec![Span::styled(screen_name(app), t.dim())];
    if app.debug {
        right.insert(0, Span::styled("debug ", t.warn()));
    }
    let used: usize = left.iter().map(|s| s.content.chars().count()).sum();
    let r: usize = right.iter().map(|s| s.content.chars().count()).sum();
    let pad = usize::from(area.width).saturating_sub(used + r + 1);
    left.push(Span::raw(" ".repeat(pad)));
    left.extend(right);
    f.render_widget(Paragraph::new(Line::from(left)), area);
}

fn draw_hints(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let mut spans: Vec<Span> = Vec::new();
    let mut used = 0usize;
    for h in keys::hints(app.ctx()) {
        let w = h.shown.chars().count() + 1 + h.what.chars().count() + 3;
        if used + w > usize::from(area.width) {
            break;
        }
        used += w;
        spans.push(Span::styled(h.shown, t.key()));
        spans.push(Span::styled(format!(" {}   ", h.what), t.dim()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_status(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let line = match app.run_data() {
        Some((snap, _)) => status::line(t, &snap, usize::from(area.width)),
        None => Line::from(Span::styled(
            if app.screen == Screen::NewRun || app.screen == Screen::Doctor {
                ""
            } else {
                "sin uso todavía"
            },
            t.dim(),
        )),
    };
    f.render_widget(Paragraph::new(line), area);
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    if area.width < MIN_W || area.height < MIN_H {
        f.render_widget(
            Paragraph::new(format!(
                "Terminal muy pequeña: se necesita al menos {MIN_W}x{MIN_H} y esta es de {}x{}. Agrándala.",
                area.width, area.height
            ))
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let t = app.theme;
    let [header, _, body, flash, hints, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_header(f, header, app);
    // One column of air on each side.
    let body = Rect::new(
        body.x + 1,
        body.y,
        body.width.saturating_sub(2),
        body.height,
    );
    match app.screen {
        Screen::Home => super::home::draw(f, body, app),
        Screen::NewRun => super::new_run::draw(f, body, app),
        Screen::Live => super::live::draw(f, body, app),
        Screen::Detail => super::detail::draw(f, body, app),
        Screen::Doctor => super::doctor::draw(f, body, app),
        Screen::Context => meter::draw_context(f, body, app),
        Screen::Usage => meter::draw_usage(f, body, app),
        Screen::Help => meter::draw_help(f, body, app),
    }
    if let Some(msg) = &app.flash {
        f.render_widget(
            Paragraph::new(Span::styled(format!(" {}", clean(msg)), t.ok())),
            flash,
        );
    }
    draw_hints(
        f,
        Rect::new(hints.x + 1, hints.y, hints.width.saturating_sub(1), 1),
        app,
    );
    draw_status(
        f,
        Rect::new(status.x + 1, status.y, status.width.saturating_sub(1), 1),
        app,
    );
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
        lines.push(Line::from(Span::styled(
            "(cualquier tecla cierra)",
            t.dim(),
        )));
        dialog(f, area, t, " Aviso ", lines, t.fail());
    }
}

/// The only boxes of the interface: a question or a message that needs an answer.
fn dialog(
    f: &mut Frame,
    area: Rect,
    t: Theme,
    title: &str,
    lines: Vec<Line<'static>>,
    border: ratatui::style::Style,
) {
    let w = area.width.saturating_sub(6).clamp(30, 76);
    let h = (u16::try_from(lines.len()).unwrap_or(u16::MAX) + 3).min(area.height);
    let r = centered(area, w, h.max(5));
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::new()
                .borders(Borders::ALL)
                .border_type(if t.unicode {
                    BorderType::Rounded
                } else {
                    BorderType::Plain
                })
                .border_style(border)
                .title(Span::styled(title.to_owned(), t.bold())),
        ),
        r,
    );
}

/// Counts by label, for the home header and the live panel.
pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
