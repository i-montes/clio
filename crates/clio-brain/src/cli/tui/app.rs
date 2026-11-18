//! The state of the TUI and the one function that changes it: `update(app, input) -> (app, cmds)`.
//! It never touches the terminal, the disk or a thread: anything that must happen outside is
//! returned as a `Cmd` for the runtime (`mod.rs`) to carry out, and its result comes back as an
//! `Input::Bg`. That is what lets every screen be tested without a terminal.
//!
//! The frame is always the same: a top bar and a rule under it, the screen, and one footer line
//! with the keys on the left and the use of the run and its context on the right. The chat of
//! the Pi harness draws its own column under the top bar (`chat::draw_screen`).

use clio_brain::agent::Event;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap};

use super::chat::Chat;
use super::detail::Detail;
use super::doctor::DoctorView;
use super::home::{Header, Home};
use super::keys::{self, Act, Ctx};
use super::live::Live;
use super::memory::{MemoryData, MemoryView};
use super::new_run::{NewRun, Preview};
use super::sessions::Sessions;
use super::theme::Theme;
use super::{meter, status};
use super::{sanitize, text};
use crate::cli::chat::{Opened, Refresh, Row};
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
    Memory,
    /// The Pi harness: one screen, the sessions beside the chat; the keys go to the list
    /// (`Sessions`) or to the chat (`Chat`).
    Sessions,
    Chat,
}

/// What to open in a Pi chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenTarget {
    New { project: String },
    Existing { reference: String },
}

/// Something to do outside of `update`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    LoadHome,
    RunDoctor,
    /// Puts text on the terminal's clipboard (OSC 52): the terminal does it, nothing is run.
    Clipboard(String),
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
    LoadMemory,
    MemoryPromote {
        id: String,
    },
    MemoryArchive {
        id: String,
    },
    /// Works out what deleting a session removes, to ask before.
    PlanSessionDelete {
        reference: String,
    },
    DeleteSession {
        reference: String,
    },
    /// Useful (true) or wrong (false) for one memory a run recalled.
    MemoryFeedback {
        id: i64,
        useful: bool,
    },
    LoadSessions,
    /// Open a Pi session; `seq` says which request the answer belongs to.
    ChatOpen {
        seq: u64,
        target: OpenTarget,
    },
    ChatSend {
        text: String,
    },
    /// A message to the run in progress.
    ChatSteer {
        text: String,
    },
    ChatAbort,
    /// The person's answer to an action the session set aside (host mode).
    ChatAnswer {
        id: String,
        yes: bool,
    },
    /// Leave the chat: stop its Pi and its toolhost.
    ChatClose,
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
    Memory(Result<Box<MemoryData>, String>),
    Done(Result<String, String>),
    Sessions(Vec<Row>),
    /// What deleting a session removes: its file, its title, one line on what goes.
    SessionPlan(Result<(String, String, String), String>),
    /// A session deleted (or why not), and the list as it is now.
    SessionDeleted(Result<String, String>, Vec<Row>),
    ChatOpened {
        seq: u64,
        result: Result<Box<ChatInfo>, String>,
    },
    ChatEvent(clio_brain::pi::rpc::Event),
    /// Host mode: what the turn set aside, one question each: (id, «class» and what).
    ChatAsks(Vec<(String, String)>),
    /// What came of an answer to one of them.
    ChatAnswered(Result<String, String>),
    /// The run ended; `refresh` is Pi's own totals if it could still give them.
    ChatSettled {
        result: Result<clio_brain::pi::rpc::RunSummary, String>,
        refresh: Option<Refresh>,
    },
}

/// What the screen keeps of an opened session (the session itself stays with the runtime).
pub struct ChatInfo {
    pub id: String,
    pub workspace: String,
    pub history: Vec<serde_json::Value>,
    pub refresh: Option<Refresh>,
    /// Clio speaks first.
    pub presentation: bool,
}

impl From<&Opened> for ChatInfo {
    fn from(o: &Opened) -> Self {
        Self {
            id: o.id.clone(),
            workspace: o.workspace.clone(),
            history: o.history.clone(),
            refresh: o.refresh.clone(),
            presentation: o.presentation,
        }
    }
}

/// Files an apply dialog names before it says "and N more".
const CONFIRM_FILES: usize = 8;

pub struct Planned {
    pub id: String,
    pub files: usize,
    pub workspace: String,
    /// One line per file about to be written: a mark (`M` changed, `+` new, `-` deleted) and the path.
    pub names: Vec<String>,
    /// Whether the agent read outside content; `None` when the run kept no record of it.
    pub tainted: Option<bool>,
}

// One value per frame or event, moved once: boxing `Bg` would only add an allocation per agent event.
#[allow(clippy::large_enum_variant)]
pub enum Input {
    Key(KeyEvent),
    /// Only with the Pi harness, which turns the mouse on.
    Mouse(MouseEvent),
    Paste(String),
    Tick(u64),
    Bg(Bg),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Apply,
    Undo,
    /// A memory: `Confirm::id` is its number and `Confirm::workspace` its text.
    Promote,
    Archive,
    /// A Pi session: `Confirm::id` is its file and `Confirm::workspace` what goes with it.
    DeleteSession,
    /// Host mode: run an action the session set aside. `Confirm::id` is its id and
    /// `Confirm::workspace` its class and what it does. Any other key rejects it.
    Approve,
}

/// A question that needs a `y`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirm {
    pub action: Action,
    pub id: String,
    pub files: usize,
    pub workspace: String,
    /// What an apply is about to write, one line each (empty for the other actions).
    pub names: Vec<String>,
    /// A warning to read before answering.
    pub note: Option<String>,
    /// An action set aside: `s` was typed, and Enter now runs it.
    pub armed: bool,
    /// An action set aside: the session it belongs to (only that session can answer it).
    pub session: String,
}

/// A set-aside question takes an answer only after this long with no key at all, so the tail of
/// what the person was typing when it opened never answers it.
pub const QUIET_MS: u64 = 1000;

impl Confirm {
    pub fn new(action: Action, id: String, files: usize, workspace: String) -> Self {
        Self {
            action,
            id,
            files,
            workspace,
            names: vec![],
            note: None,
            armed: false,
            session: String::new(),
        }
    }
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
    pub memory: MemoryView,
    /// The Pi harness is on: the selector replaces the list of runs as the first screen.
    pub pi: bool,
    /// A new session works on this machine (host mode), not on a copy.
    pub host: bool,
    pub sessions: Sessions,
    pub chat: Chat,
    pub chat_seq: u64,
    pub confirm: Option<Confirm>,
    /// Host mode: the questions waiting behind the one on screen.
    pub asks: std::collections::VecDeque<Confirm>,
    /// Host mode: answers to set-aside actions still running (an approved one can take a while).
    pub answering: usize,
    /// When the last key (or paste) came, for the quiet rule of a set-aside question.
    pub last_key_ms: Option<u64>,
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
            memory: MemoryView::default(),
            pi: false,
            host: false,
            sessions: Sessions::default(),
            chat: Chat::default(),
            chat_seq: 0,
            confirm: None,
            asks: std::collections::VecDeque::new(),
            answering: 0,
            last_key_ms: None,
            flash: None,
            notice: None,
            check_seq: 0,
            view_scroll: 0,
            view_max: std::cell::Cell::new(0),
        }
    }

    /// The commands to run when the TUI opens.
    pub fn start_cmds(&self) -> Vec<Cmd> {
        let mut v = vec![Cmd::LoadHome, Cmd::RunDoctor];
        if self.pi {
            v.push(Cmd::LoadSessions);
        }
        v
    }

    /// Turns the Pi harness on: the first screen is the sessions beside a new chat, ready to type.
    pub fn use_pi(&mut self) {
        self.pi = true;
        self.screen = Screen::Chat;
        self.chat = Chat::draft(self.now_ms);
        self.sessions.loading = true;
    }

    pub fn go_home(&mut self) -> Vec<Cmd> {
        if self.pi {
            self.screen = Screen::Sessions;
            return vec![Cmd::LoadHome, Cmd::LoadSessions];
        }
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
            Screen::Memory if self.memory.filtering => Ctx::MemoryFilter,
            Screen::Memory => Ctx::Memory,
            Screen::Sessions => Ctx::Sessions,
            Screen::Chat if self.chat.running => Ctx::ChatRun,
            Screen::Chat => Ctx::Chat,
        }
    }

    /// The window, the limits and the name of the model Clio is set up with, for what a run did
    /// not record.
    pub(super) fn fallback(&self) -> (u64, bool, RunLimits) {
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
            Screen::Memory | Screen::Sessions => Screen::Home,
            Screen::Context | Screen::Usage => self.back,
            Screen::Help if matches!(self.before_help, Screen::Context | Screen::Usage) => {
                self.back
            }
            Screen::Help if matches!(self.before_help, Screen::Memory | Screen::Sessions) => {
                Screen::Home
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

    /// No key for `QUIET_MS`: a set-aside question may take an answer.
    pub fn quiet(&self) -> bool {
        self.last_key_ms
            .is_none_or(|t| self.now_ms.saturating_sub(t) >= QUIET_MS)
    }

    /// Whether the chat cannot be left now (quit, another session, a new one, a delete); when
    /// it cannot, the flash says why.
    pub fn busy(&mut self) -> bool {
        let why = if self.chat.running {
            "el agente está trabajando: Esc en el chat aborta el turno; luego cambia de sesión"
        } else if self.answering > 0 {
            "Clio todavía ejecuta lo que aprobaste: espera a que termine; luego cambia de sesión"
        } else if self.chat.opening {
            "la sesión se está abriendo: espera a que arranque; luego cambia de sesión"
        } else {
            return false;
        };
        self.flash = Some(why.into());
        true
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
            app.last_key_ms = Some(app.now_ms);
            if app.confirm.is_some() || app.notice.is_some() {
                vec![]
            } else if app.screen == Screen::NewRun {
                super::new_run::paste(app, &text)
            } else {
                if app.screen == Screen::Chat {
                    super::chat::paste(app, &text);
                }
                vec![]
            }
        }
        Input::Bg(bg) => background(app, bg),
        // A click never answers a question or closes a message: only keys do.
        Input::Mouse(m) => {
            use crossterm::event::MouseEventKind as M;
            // Moves and releases are not acts of the person (the terminal reports every move).
            if !matches!(m.kind, M::Down(_) | M::ScrollUp | M::ScrollDown)
                || app.confirm.is_some()
                || app.notice.is_some()
                || !matches!(app.screen, Screen::Sessions | Screen::Chat)
            {
                return vec![];
            }
            app.flash = None;
            super::sessions::mouse(app, &m)
        }
        Input::Key(k) if k.kind == KeyEventKind::Release => vec![],
        Input::Key(k) => {
            app.flash = None;
            // Every key counts for the quiet rule, even one pressed before the question opened.
            let quiet = app.quiet();
            app.last_key_ms = Some(app.now_ms);
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
                let cmds = answer(app, &k, quiet);
                next_ask(app);
                return cmds;
            }
            match app.screen {
                Screen::Home => super::home::key(app, &k),
                Screen::NewRun => super::new_run::key(app, &k),
                Screen::Live => super::live::key(app, &k),
                Screen::Detail => super::detail::key(app, &k),
                Screen::Doctor => super::doctor::key(app, &k),
                Screen::Memory => super::memory::key(app, &k),
                Screen::Sessions => super::sessions::key(app, &k),
                Screen::Chat => super::chat::key(app, &k),
                Screen::Context | Screen::Usage | Screen::Help => meter::key(app, &k),
            }
        }
    }
}

/// The reply to a confirmation: only `y` (or `s`, for sí) goes ahead. An action set aside, which
/// is asked when a turn ends while the person may be typing, takes `s` and then Enter; `n` or Esc
/// rejects it, and any other key only undoes a typed `s`. It hears none of them unless `quiet`
/// (no key for `QUIET_MS`): a key sooner is ignored and undoes a typed `s`.
fn answer(app: &mut App, k: &KeyEvent, quiet: bool) -> Vec<Cmd> {
    let Some(mut c) = app.confirm.take() else {
        return vec![];
    };
    if c.action == Action::Approve && !quiet {
        c.armed = false;
        app.confirm = Some(c);
        return vec![];
    }
    // A plain letter: Ctrl-S (save, by habit) or Alt-Y are not an answer.
    let plain = (k.modifiers - KeyModifiers::SHIFT).is_empty();
    let yes_key = plain && matches!(k.code, KeyCode::Char('y' | 'Y' | 's' | 'S'));
    let yes = if c.action == Action::Approve {
        let no = k.code == KeyCode::Esc || (plain && matches!(k.code, KeyCode::Char('n' | 'N')));
        let enter = plain && k.code == KeyCode::Enter;
        if !(no || enter && c.armed) {
            c.armed = yes_key;
            app.confirm = Some(c);
            return vec![];
        }
        !no
    } else {
        yes_key
    };
    if !yes && c.action != Action::Approve {
        app.flash = Some("cancelado: no se cambió nada".into());
        return vec![];
    }
    match c.action {
        Action::Apply => vec![Cmd::Apply { id: c.id }],
        Action::Undo => vec![Cmd::Undo { id: c.id }],
        Action::Promote => vec![Cmd::MemoryPromote { id: c.id }],
        Action::Archive => vec![Cmd::MemoryArchive { id: c.id }],
        Action::DeleteSession => vec![Cmd::DeleteSession { reference: c.id }],
        Action::Approve => {
            app.answering += 1;
            vec![Cmd::ChatAnswer { id: c.id, yes }]
        }
    }
}

/// Once no question is on screen, the next set-aside one; one of a session no longer open cannot
/// be answered here and waits in `clio pending`.
fn next_ask(app: &mut App) {
    while app.confirm.is_none()
        && let Some(c) = app.asks.pop_front()
    {
        if c.session == app.chat.session_id {
            app.confirm = Some(c);
        } else {
            app.flash = Some(format!(
                "{} es de otra sesión: espera en `clio pending`",
                c.id
            ));
        }
    }
}

/// A question that comes back late never takes the place of the one on screen.
fn dropped(app: &mut App, what: &str) -> bool {
    if app.confirm.is_none() {
        return false;
    }
    app.flash = Some(format!(
        "{what} llegó con otra pregunta abierta: pídelo otra vez"
    ));
    true
}

/// The run on screen: the selected one on the list, or the one open in the detail.
fn looking_at(app: &App, id: &str) -> bool {
    match app.screen {
        Screen::Home => app.home.selected().is_some_and(|r| r.id == id),
        Screen::Detail => app.detail.as_ref().is_some_and(|d| d.info.id == id),
        _ => false,
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
            if dropped(app, &format!("el plan de {}", p.id)) {
                return vec![];
            }
            // The plan takes a while. If the person is no longer looking at that run, a dialog
            // popping up now would turn the next key they type into an answer.
            if !looking_at(app, &p.id) {
                app.flash = Some(format!(
                    "el plan de {} quedó listo, pero cambiaste de pantalla: pídelo otra vez",
                    p.id
                ));
                return vec![];
            }
            let mut c = Confirm::new(Action::Apply, p.id, p.files, p.workspace);
            c.names = p.names;
            c.note = match p.tainted {
                Some(true) => {
                    Some("El agente leyó contenido de fuera: revisa el diff con cuidado.".into())
                }
                None => Some("Sin registro de lo que leyó el agente: revisa el diff.".into()),
                Some(false) => None,
            };
            app.confirm = Some(c);
            vec![]
        }
        Bg::Planned(Err(e)) => {
            app.notice = Some(e);
            vec![]
        }
        Bg::Memory(r) => {
            app.memory.loading = false;
            match r {
                Ok(d) => {
                    app.memory.error = None;
                    app.memory.data = Some(*d);
                    let n = app.memory.visible().len();
                    app.memory.sel = app.memory.sel.min(n.saturating_sub(1));
                }
                Err(e) => app.memory.error = Some(e),
            }
            vec![]
        }
        Bg::Done(Ok(msg)) => {
            app.flash = Some(msg);
            let mut cmds = vec![Cmd::LoadHome];
            if app.screen == Screen::Memory {
                cmds.push(Cmd::LoadMemory);
            }
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
        Bg::Sessions(rows) => {
            app.sessions.set(rows);
            vec![]
        }
        Bg::SessionPlan(r) => {
            match r {
                // A turn, an open or an approval that started meanwhile: no delete now.
                Ok(_) if dropped(app, "lo que se borraría") || app.busy() => {}
                Ok((file, title, what)) if app.screen == Screen::Sessions => {
                    app.confirm = Some(Confirm::new(
                        Action::DeleteSession,
                        file,
                        0,
                        format!("«{title}»: {what}"),
                    ));
                }
                Ok(_) => {}
                Err(e) => app.flash = Some(e),
            }
            vec![]
        }
        Bg::SessionDeleted(r, rows) => {
            app.sessions.set(rows);
            app.flash = Some(match r {
                Ok(m) | Err(m) => m,
            });
            vec![]
        }
        Bg::ChatOpened { seq, result } => {
            // An answer for a chat the person already left: the runtime has closed it.
            if seq != app.chat.seq {
                return vec![];
            }
            let queued = app.chat.queued.take();
            match result {
                Ok(info) => {
                    app.chat.opened(
                        info.id,
                        info.workspace,
                        &info.history,
                        info.refresh,
                        app.now_ms,
                    );
                    // A new session's first message: the presentation, if any, is asked in
                    // front of it (`PrePrompt`), so Clio introduces herself and answers.
                    if let Some(text) = queued {
                        app.chat.start_turn(app.now_ms);
                        return vec![Cmd::ChatSend { text }];
                    }
                    // Clio introduces herself: a turn with no words of the person, so nothing
                    // of hers goes into the chat.
                    if info.presentation {
                        app.chat.start_turn(app.now_ms);
                        return vec![Cmd::ChatSend {
                            text: String::new(),
                        }];
                    }
                }
                Err(e) => {
                    use super::chat::Item;
                    app.chat.opening = false;
                    match queued {
                        // Still a new session: what the person wrote is back in the field, in
                        // front of anything typed meanwhile, and Enter tries again.
                        Some(text) => {
                            app.chat.items.retain(|i| *i != Item::User(text.clone()));
                            app.chat.items.push(Item::Error(e));
                            let typed = std::mem::take(&mut app.chat.input);
                            app.chat.set_input(if typed.is_empty() {
                                text
                            } else {
                                format!("{text}\n{typed}")
                            });
                        }
                        None => app.chat.failed = Some(e),
                    }
                }
            }
            vec![]
        }
        Bg::ChatEvent(e) => {
            if matches!(e, clio_brain::pi::rpc::Event::AgentStart) && !app.chat.running {
                app.chat.turn_ms = app.now_ms;
            }
            app.chat.on_event(&e);
            vec![]
        }
        // They come before the turn settles, so from the session open now.
        Bg::ChatAsks(asks) => {
            for (id, what) in asks {
                let mut c = Confirm::new(Action::Approve, id, 0, what);
                c.session.clone_from(&app.chat.session_id);
                app.asks.push_back(c);
            }
            next_ask(app);
            vec![]
        }
        Bg::ChatAnswered(r) => {
            app.answering = app.answering.saturating_sub(1);
            app.chat.items.push(match r {
                Ok(m) => super::chat::Item::Note(m),
                Err(e) => super::chat::Item::Error(e),
            });
            vec![]
        }
        Bg::ChatSettled { result, refresh } => {
            app.chat.running = false;
            app.chat.aborting = false;
            match result {
                Err(e) => app.chat.items.push(super::chat::Item::Error(e)),
                Ok(summary) => {
                    if let Some(a) = &summary.assessment {
                        app.chat.items.push(super::chat::Item::Note(format!(
                            "pedido juzgado como {}",
                            a.notice()
                        )));
                    }
                    if let Some(notice) = summary.stop_notice() {
                        app.chat.items.push(super::chat::Item::Error(notice));
                    }
                }
            }
            if let Some(r) = refresh {
                app.chat.refresh(&r);
            }
            // The list beside the chat: a new session shows up, a title changes.
            if app.pi {
                return vec![Cmd::LoadSessions];
            }
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
    let used = text::width(title) + 3;
    let tail = t.rule().repeat(usize::from(width).saturating_sub(used));
    Line::from(vec![
        Span::styled(format!("{} ", t.rule()), t.faint()),
        Span::styled(title.to_owned(), t.bold()),
        Span::styled(format!(" {tail}"), t.faint()),
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
        Screen::Memory => "memoria",
        Screen::Sessions | Screen::Chat => "chat",
    }
}

fn folder_name(path: &str) -> String {
    let p = std::path::Path::new(path);
    p.file_name()
        .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned())
}

/// The chat's title: the session's, as the list has it, or "nueva sesión" for a draft.
fn chat_title(app: &App) -> String {
    let c = &app.chat;
    if c.is_draft() {
        return "nueva sesión".into();
    }
    super::sessions::open_pos(app)
        .and_then(|p| p.checked_sub(1))
        .and_then(|i| app.sessions.rows.get(i))
        .map(|r| r.title.clone())
        .filter(|t| !t.is_empty())
        .or_else(|| (!c.title.is_empty()).then(|| c.title.clone()))
        .unwrap_or_else(|| super::home::short_id(&c.session_id))
}

/// What is wrong, for the top bar: only what is wrong, and nothing about what is fine.
fn warnings(app: &App) -> Vec<String> {
    let mut v = Vec::new();
    if let Some(h) = &app.home.header {
        if h.config_error.is_some() {
            v.push("la configuración no carga".to_owned());
        } else if h.key == super::home::KeyState::Missing {
            v.push("falta la llave del modelo".to_owned());
        }
        if h.vault.starts_with("creada") {
            v.push("vault detenida".to_owned());
        }
    }
    if let Some((_, _, fail)) = app.home.doctor
        && fail > 0
    {
        v.push(format!("doctor: {}", plural(fail, "fallo", "fallos")));
    }
    if app.debug {
        v.push("debug".to_owned());
    }
    v
}

/// `clio · título` on the left; on the right what is wrong, in the warning color, and the model.
fn draw_top(f: &mut Frame, area: Rect, app: &App) {
    let t = app.theme;
    let w = usize::from(area.width);
    let pi_chat = app.pi && matches!(app.screen, Screen::Chat | Screen::Sessions);
    let title = if pi_chat {
        chat_title(app)
    } else {
        let folder = match app.screen {
            Screen::Live if !app.live.workspace.is_empty() => {
                Some(folder_name(&app.live.workspace))
            }
            Screen::Detail => app.detail.as_ref().map(|d| folder_name(&d.info.workspace)),
            Screen::Home | Screen::NewRun | Screen::Live => Some(folder_name(&app.cwd)),
            _ => None,
        };
        match folder {
            Some(f) => format!("{} {} {f}", screen_name(app), t.dot()),
            None => screen_name(app).to_owned(),
        }
    };
    let sep = format!(" {} ", t.dot());
    let warns = warnings(app).join(&sep);
    let model = app
        .home
        .header
        .as_ref()
        .filter(|h| h.config_error.is_none())
        .map_or_else(String::new, |h| clean(&h.model));
    let lead = 1 + 4 + text::width(&sep);
    // The model gives way to what is wrong, and that is cut before it pushes "clio" out.
    let both = text::width(&warns) + 3 + text::width(&model);
    let mut right: Vec<Span> = Vec::new();
    if warns.is_empty() || both + 5 <= w {
        if !warns.is_empty() {
            right.push(Span::styled(warns, t.warn()));
            right.push(Span::raw("   "));
        }
        right.push(Span::styled(model, t.dim()));
    } else {
        right.push(Span::styled(
            text::fit(&warns, w.saturating_sub(6), t.ellipsis()),
            t.warn(),
        ));
    }
    let right_w: usize = right.iter().map(|s| text::width(&s.content)).sum();
    // The title gives way to the right side.
    let room = w.saturating_sub(lead + right_w + 3);
    let title = if room >= 6 {
        text::fit(&clean(&title), room, t.ellipsis())
    } else {
        String::new()
    };
    let mut left = vec![Span::raw(" "), Span::styled("clio", t.accent())];
    if !title.is_empty() {
        left.push(Span::styled(sep, t.dim()));
        left.push(Span::styled(title, t.dim()));
    }
    let left_w: usize = left.iter().map(|s| text::width(&s.content)).sum();
    if left_w + right_w < w {
        left.push(Span::raw(" ".repeat(w - left_w - right_w - 1)));
        left.extend(right);
    }
    f.render_widget(Paragraph::new(Line::from(left)), area);
}

/// One footer line: the keys of this screen on the left (at most `max_hints`, the help one
/// always) and on the right the first of `rights` that leaves room for them (or the last one).
pub fn footer(
    f: &mut Frame,
    area: Rect,
    app: &App,
    max_hints: usize,
    rights: Vec<Vec<Span<'static>>>,
) {
    let t = app.theme;
    let sep = format!(" {} ", t.dot());
    let width = |h: &keys::Bind| text::width(h.shown) + 1 + text::width(h.what);
    // The keys to leave and to get help are always shown, last; the others give way to them.
    let (pinned, rest): (Vec<&keys::Bind>, Vec<&keys::Bind>) = keys::hints(app.ctx())
        .into_iter()
        .partition(|h| matches!(h.act, Act::Help | Act::Quit));
    let rest = &rest[..rest.len().min(max_hints.saturating_sub(pinned.len()))];
    let sep_w = text::width(&sep);
    let all: usize = rest.iter().chain(&pinned).map(|h| width(h) + sep_w).sum();
    let span_w = |v: &[Span]| v.iter().map(|s| text::width(&s.content)).sum::<usize>();
    let n = rights.len();
    let right = rights
        .into_iter()
        .enumerate()
        .find(|(i, r)| all + span_w(r) + 2 <= usize::from(area.width) + sep_w || i + 1 == n)
        .map(|(_, r)| r)
        .unwrap_or_default();
    let right_w = span_w(&right);
    let room = usize::from(area.width).saturating_sub(right_w + 2);
    let mut used = 0;
    let mut kept: Vec<&keys::Bind> = Vec::new();
    for h in pinned.iter().rev() {
        let need = width(h) + sep_w;
        if used + need > room + sep_w {
            break;
        }
        used += need;
        kept.insert(0, *h);
    }
    let mut shown: Vec<&keys::Bind> = Vec::new();
    for h in rest {
        let need = width(h) + sep_w;
        if used + need > room + sep_w {
            break;
        }
        used += need;
        shown.push(h);
    }
    shown.extend(kept);
    let mut spans: Vec<Span> = Vec::new();
    for (i, h) in shown.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(sep.clone(), t.dim()));
        }
        spans.push(Span::styled(h.shown, t.key()));
        spans.push(Span::styled(format!(" {}", h.what), t.dim()));
    }
    let left_w: usize = spans.iter().map(|s| text::width(&s.content)).sum();
    spans.push(Span::raw(
        " ".repeat(usize::from(area.width).saturating_sub(left_w + right_w)),
    ));
    spans.extend(right);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The use of the run on screen (or the chat's), at most `w` columns.
fn usage_line(app: &App, w: usize) -> Vec<Span<'static>> {
    let t = app.theme;
    let line = match app.run_data() {
        Some((snap, _)) => status::line(t, &snap, w),
        None if matches!(app.screen, Screen::NewRun | Screen::Doctor) => Line::default(),
        None => Line::from(Span::styled("sin uso todavía", t.dim())),
    };
    line.spans
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
    let [top, rule, rest] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(3),
    ])
    .areas(area);
    draw_top(f, top, app);
    f.render_widget(
        Paragraph::new(Span::styled(
            t.rule().repeat(usize::from(rule.width)),
            t.faint(),
        )),
        rule,
    );
    if app.pi && matches!(app.screen, Screen::Sessions | Screen::Chat) {
        super::chat::draw_screen(f, rest, app);
    } else {
        let [_, body, flash, foot] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(rest);
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
            Screen::Memory => super::memory::draw(f, body, app),
            Screen::Sessions | Screen::Chat => {}
            Screen::Help => meter::draw_help(f, body, app),
        }
        if let Some(msg) = &app.flash {
            f.render_widget(
                Paragraph::new(Span::styled(format!(" {}", clean(msg)), t.accent_line())),
                flash,
            );
        }
        let foot = Rect::new(foot.x + 1, foot.y, foot.width.saturating_sub(2), 1);
        // The help opened from the chat shows the chat's use.
        let right = if app.pi
            && matches!(app.before_help, Screen::Chat | Screen::Sessions)
            && app.screen == Screen::Help
        {
            super::chat::meter(app)
        } else {
            let w = usize::from(foot.width);
            vec![
                usage_line(app, w * 3 / 5),
                usage_line(app, w / 2),
                usage_line(app, w * 2 / 5),
                usage_line(app, 0),
            ]
        };
        footer(f, foot, app, 6, right);
    }
    if let Some(c) = &app.confirm {
        confirm_dialog(f, area, app, c);
    } else if let Some(n) = &app.notice {
        let lines: Vec<Line> = sanitize::multiline(n)
            .lines()
            .map(|l| Line::from(l.to_owned()))
            .collect();
        let hint = Line::from(Span::styled("cualquier tecla cierra", t.dim()));
        dialog(f, area, t, " Aviso ", lines, hint, t.fail());
    }
}

fn confirm_dialog(f: &mut Frame, area: Rect, app: &App, c: &Confirm) {
    let t = app.theme;
    let verb = match c.action {
        Action::Apply => format!("Aplicar {} archivo(s) en", c.files),
        Action::Undo => format!("Deshacer lo aplicado ({} archivo(s)) en", c.files),
        Action::Promote => format!(
            "Aprobar el recuerdo m{} (pasa a activo y se recordará)",
            c.id
        ),
        Action::Archive => format!("Archivar el recuerdo m{} (ya no se recordará)", c.id),
        Action::DeleteSession => "Borrar la sesión (no se puede deshacer); se va:".to_owned(),
        Action::Approve => format!("Clio apartó {}. ¿Lo ejecuto?", c.id),
    };
    let mut lines = vec![Line::from(Span::styled(verb, t.bold())), Line::from("")];
    // The class of a set-aside action (`«borrar» …`) in the warning color.
    let what = clean(&c.workspace);
    match what.split_once('»') {
        Some((class, rest)) if c.action == Action::Approve && class.starts_with('«') => {
            lines.push(Line::from(vec![
                Span::styled(format!("{class}»"), t.warn()),
                Span::raw(rest.to_owned()),
            ]));
        }
        _ => lines.push(Line::from(what)),
    }
    for n in c.names.iter().take(CONFIRM_FILES) {
        lines.push(Line::from(format!("  {}", clean(n))));
    }
    if c.names.len() > CONFIRM_FILES {
        lines.push(Line::from(Span::styled(
            format!("  … y {} más", c.names.len() - CONFIRM_FILES),
            t.dim(),
        )));
    }
    if let Some(n) = &c.note {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(clean(n), t.warn())));
    }
    if c.action == Action::Approve && !app.quiet() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "espera un segundo sin teclear para responder…",
            t.dim(),
        )));
    }
    let d = format!("   {}   ", t.dot());
    let hint = if c.action == Action::Approve {
        Line::from(vec![
            Span::styled(if c.armed { "Enter" } else { "s y Enter" }, t.key()),
            Span::styled(" sí, ejecutarlo", t.dim()),
            Span::styled(d, t.dim()),
            Span::styled("n o Esc", t.key()),
            Span::styled(" no, rechazarlo", t.dim()),
        ])
    } else {
        Line::from(vec![
            Span::styled("y", t.key()),
            Span::styled(" sí, continuar", t.dim()),
            Span::styled(d, t.dim()),
            Span::styled("cualquier otra tecla", t.key()),
            Span::styled(" cancela", t.dim()),
        ])
    };
    let border = if matches!(c.action, Action::Approve | Action::DeleteSession) {
        t.warn()
    } else {
        t.accent_line()
    };
    dialog(f, area, t, " Confirmar ", lines, hint, border);
}

/// A centered box over the screen: a question or a message that needs a key. The text is wrapped
/// to the box, with a column and a row of air inside the border, and the keys on the last row.
fn dialog(
    f: &mut Frame,
    area: Rect,
    t: Theme,
    title: &str,
    lines: Vec<Line<'static>>,
    hint: Line<'static>,
    border: ratatui::style::Style,
) {
    let w = area.width.saturating_sub(6).min(72);
    let inner = usize::from(w.saturating_sub(6)).max(1);
    // A row of air above and below, when the terminal has the rows for it.
    let air = area.height >= 24;
    let mut rows: Vec<Line> = Vec::new();
    if air {
        rows.push(Line::from(""));
    }
    for l in lines.into_iter().chain([Line::from(""), hint]) {
        let segs: Vec<(String, ratatui::style::Style)> = l
            .spans
            .into_iter()
            .map(|s| (s.content.into_owned(), s.style))
            .collect();
        let none = (Vec::new(), 0);
        rows.extend(super::markdown::wrap_spans(
            &segs,
            inner,
            none.clone(),
            none,
        ));
    }
    if air {
        rows.push(Line::from(""));
    }
    let h = (u16::try_from(rows.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2))
    .min(area.height);
    let r = centered(area, w, h);
    // A column of space around it, so the text under it does not touch its border.
    f.render_widget(
        Clear,
        Rect::new(r.x.saturating_sub(1), r.y, r.width + 2, r.height).intersection(area),
    );
    f.render_widget(Clear, r);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_set(t.border())
        .border_style(border)
        .padding(Padding::horizontal(2))
        .title(Span::styled(title.to_owned(), t.bold()));
    f.render_widget(Paragraph::new(rows).block(block), r);
}

/// Counts by label, for the home header and the live panel.
pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
