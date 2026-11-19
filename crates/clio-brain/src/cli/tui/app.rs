//! The state of the TUI and the one function that changes it: `update(app, input) -> (app, cmds)`.
//! It never touches the terminal, the disk or a thread: anything that must happen outside is
//! returned as a `Cmd` for the runtime (`mod.rs`) to carry out, and its result comes back as an
//! `Input::Bg`. That is what lets every screen be tested without a terminal.
//!
//! The frame is always the same: a top bar and a rule under it, the screen, and one footer line
//! with the keys on the left and the use of the run and its context on the right. The chat of
//! the Pi harness draws its own column under the top bar (`chat::draw_screen`).

use clio_brain::agent::Event;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap};

use super::chat::Chat;
use super::commands::{Choice, Custom, Modal, Run};
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
use crate::cli::chat::local::OPEN_ELSEWHERE;
use crate::cli::chat::{Opened, Refresh, Row};
use crate::cli::doctor::Report;
use crate::cli::runs::{RunInfo, RunSummary};
use crate::cli::usage::{self, DEFAULT_WINDOW, RunLimits, RunUsage, Snapshot};

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
    New {
        project: String,
    },
    Existing {
        reference: String,
    },
    /// The last conversation of this mode and place (`SessionSource::last`), as the drawer
    /// would open it.
    Last {
        project: String,
    },
}

/// Why the last conversation was not reopened when there was none: no note, a plain draft.
pub const NOTHING_TO_REOPEN: &str = "no conversation to reopen";

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
    /// Esc during an automatic compaction: Pi's `abort` cancels it.
    CompactAbort,
    /// The person's answer to an action the session set aside (host mode).
    ChatAnswer {
        id: String,
        yes: bool,
    },
    /// The person's answer to ONE box that listed several items: each is answered in turn, in
    /// the order shown.
    ChatAnswers {
        ids: Vec<String>,
        yes: bool,
    },
    /// `/grant` (`""`: show the permission window) or `/grant off` (close it now).
    Grant {
        arg: String,
        session: String,
    },
    /// The person's answer to one of Pi's dialogs (a question of `ask_person`); `None` skips it.
    ChatUi {
        id: String,
        value: Option<String>,
    },
    /// The person's answer to a secret the model asked for: the value goes to the vault, `None`
    /// declines it.
    ChatSecret {
        id: String,
        name: String,
        value: Option<super::ask::SecretBox>,
    },
    /// Leave the chat: stop its Pi and its toolhost.
    ChatClose,
    /// The chat's slash commands (`commands.rs`): read the person's own commands again.
    LoadCommands,
    /// What a command picks from (models, connectors, levels).
    Choices(Run),
    SetModel(String),
    SetConnector(String),
    SetEffort(String),
    /// `/mouse on|off`: the terminal at once, `ui.mouse` in `config.json`.
    SetMouse(bool),
    SessionName(String),
    /// Pi's compaction, with the person's instructions (may be empty).
    Compact(String),
    /// `/compact N`: the context cap, in tokens, saved in `config.json`.
    SetCap(u64),
    /// What this session set aside and still waits.
    LoadPending {
        session: String,
    },
    /// What this session has waiting for the person, at the end of a turn: under an open
    /// permission window it is approved by it, else ONE box lists it; an ended window's summary
    /// is queued. `Bg::Approvals` says what was found.
    CheckApprovals {
        session: String,
    },
    /// The action log of one run (a host session).
    LoadActions {
        run: String,
    },
    /// Undo what a chat's session changed (asked first).
    UndoSession {
        id: String,
    },
    /// The page of a session's file that ends before line `before`.
    LoadHistory {
        seq: u64,
        file: String,
        before: usize,
    },
    /// A review of the session `session` (`/review`, or the judge's when `auto`: the judge is
    /// asked first). `what`: the description; `work`: the last turn that changed something.
    Review {
        session: String,
        what: Option<String>,
        work: Option<crate::cli::chat::TurnWork>,
        auto: bool,
    },
    /// The review of skill draft `id` (`name`) the model wrote in `session` (S-68).
    SkillReview {
        session: String,
        id: String,
        name: String,
    },
    /// `/skills [off|on|undo|remove <name>]`: the person's (a removal was confirmed already).
    Skills(String),
    /// `/services [...]`: the person's (a removal was confirmed already).
    Services(String),
    /// `/round`: the last rounds.
    Round,
    /// `/mcp [...]`: the person's (a removal or a confirmation was confirmed already).
    Mcp(String),
    /// `/mcp confirm <name>`: its tools are read, then asked about (`Action::ConfirmMcp`).
    McpConfirm(String),
    /// `/review stop`.
    ReviewStop,
    /// The automatic turn that gives a review's findings to the model (as data), and the
    /// reviewer's mark.
    ReviewTurn {
        findings: String,
        marked: Option<bool>,
        /// Clio's request that goes with them (`preprompt::REVIEW_ASK`).
        ask: &'static str,
    },
    /// Debug was switched; the agent reads the switch at every call.
    Debug(bool),
    /// Clear the terminal and draw everything again.
    Redraw,
}

/// This session's actions waiting, the line and the box of each; the lines of those an answer
/// began and never finished; and how many of other sessions wait.
pub type Waiting = (Vec<(String, super::commands::Pick)>, Vec<String>, usize);

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
    /// `/undo` ran: what it said, one line each. The box shows them; the chat keeps one note.
    SessionUndone(Result<Vec<String>, String>),
    ChatOpened {
        seq: u64,
        result: Result<Box<ChatInfo>, String>,
    },
    /// A page of the history of the chat `seq`, read from its session file.
    History {
        seq: u64,
        file: String,
        page: Result<clio_brain::pi::sessions::History, String>,
    },
    ChatEvent(clio_brain::pi::rpc::Event),
    /// Host mode: what the turn set aside, one question each: (id, «class» and what, and the
    /// change itself when it is one to the person's blocks).
    ChatAsks(Vec<(String, String, Option<super::ask::BlockAsk>)>),
    /// What came of an answer to one of them, and whether the person said yes.
    ChatAnswered(Result<String, String>, bool),
    /// The secrets this turn asked the person for: (id, (name, destination, why)) each, and
    /// whether the session is suspect.
    ChatSecrets(Vec<(String, (String, String, String))>, bool),
    /// The person's commands (`<config>/commands/*.md`) and the files left out.
    Commands(Vec<Custom>, Vec<String>),
    /// A command's choices and the picker's footer.
    Choices(Run, Result<(Vec<Choice>, Option<String>), String>),
    /// The session's new title.
    Named(Result<String, String>),
    /// What a compaction did, about how many tokens it left, and Pi's totals after it.
    Compacted(Result<String, String>, Option<u64>, Option<Refresh>),
    /// The context cap `/compact N` saved.
    CapSet(Result<u64, String>),
    /// This session's actions waiting, and how many of other sessions wait too.
    Pending(Result<Waiting, String>),
    /// What `Cmd::CheckApprovals` found: how many wait.
    Approvals {
        session: String,
        waiting: usize,
    },
    /// The permission window as read now: when it ends (wall clock, ms), the hours a yes is
    /// worth, whether the session is `suspect` (its boxes open no window) and whether the judge
    /// is on (off: no window opens).
    Grant {
        session: String,
        ends: Option<i64>,
        hours: u64,
        suspect: bool,
        judge: bool,
    },
    /// What the permission window approved: a quiet note (or why it could not).
    Granted(Result<String, String>),
    /// What the person is told of the permission window (opened by their yes, `/grant`): a
    /// quiet note, and what waits is looked at again (a window closed now is summed up).
    GrantSaid(Result<String, String>),
    /// A permission window ended: its record waits in the session for the turn that sums it up.
    GrantSummary {
        session: String,
    },
    /// Lines to show in a box with that title.
    Modal(Result<Modal, String>),
    /// A question that needs a `y`, built from what was read (`/mcp confirm`).
    Confirm(Result<Box<Confirm>, String>),
    /// What the turn about to settle changed (it changed something).
    TurnWork(Box<crate::cli::chat::TurnWork>),
    /// Skills, after a turn: the drafts to review (id, name) and notes for the person.
    SkillNews(Vec<(String, String)>, Vec<String>),
    /// Acts on what Clio created, done when a turn ended (free, or under the permission window):
    /// their notes; once the chat is idle, a turn tells the model what came of them.
    Acted(Vec<String>),
    /// A review of `session` started (`auto`: the judge asked for it, `why` its reason).
    ReviewStarted {
        session: String,
        auto: bool,
        why: String,
    },
    /// No review: the judge said no or could not be asked, or it was stopped first (`why`).
    ReviewSkipped {
        session: String,
        why: String,
    },
    Reviewed {
        session: String,
        result: Result<Box<crate::cli::chat::Reviewed>, String>,
    },
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
    /// Quiet notes for the chat, from opening it.
    pub notes: Vec<String>,
}

impl From<&Opened> for ChatInfo {
    fn from(o: &Opened) -> Self {
        Self {
            id: o.id.clone(),
            workspace: o.workspace.clone(),
            history: o.history.clone(),
            refresh: o.refresh.clone(),
            presentation: o.presentation,
            notes: o.notes.clone(),
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
    /// The terminal changed size: only a redraw.
    Resize,
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
    /// `/undo`: what the chat's session changed goes back. `Confirm::id` is the session.
    UndoSession,
    /// `/skills remove`: `Confirm::id` is the skill's name.
    RemoveSkill,
    /// `/services remove`: `Confirm::id` is the service's name.
    RemoveService,
    /// `/mcp remove`: `Confirm::id` is the server's name.
    RemoveMcp,
    /// `/mcp confirm`: `Confirm::id` is the server's name, `Confirm::names` its tools as it
    /// described them. `y` confirms them and turns it on.
    ConfirmMcp,
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
    /// When it came on screen (its quiet second starts then, not when it was queued).
    pub shown_ms: u64,
    /// An approval box that lists several items: the ids after `id`, answered with it.
    pub more: Vec<String>,
    /// A change to one of the person's blocks: drawn whole, scrolled with the arrows.
    pub block: Option<super::ask::BlockAsk>,
}

/// A set-aside question takes an answer only after this long with no key at all, so the tail of
/// what the person was typing when it opened never answers it.
pub const QUIET_MS: u64 = 1000;

impl Action {
    /// Under the approval timing rule: a quiet second, `y` arms it (only once a long box's end was
    /// shown), Enter runs it.
    pub fn timed(self) -> bool {
        matches!(self, Self::Approve | Self::ConfirmMcp)
    }
}

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
            shown_ms: 0,
            more: Vec::new(),
            block: None,
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
    /// A box of lines over the chat (`/usage`, `/pending`...), under any question.
    pub modal: Option<Modal>,
    /// Host mode: the questions waiting behind the one on screen.
    pub asks: std::collections::VecDeque<Confirm>,
    /// What Clio asks the person (`ask.rs`): never at the same time as an approval; each waits
    /// for the other to close.
    pub prompt: Option<super::ask::Prompt>,
    pub prompts: std::collections::VecDeque<super::ask::Prompt>,
    /// Host mode: answers to set-aside actions still running (an approved one can take a while).
    pub answering: usize,
    /// When the last key (or paste) came, for the quiet rule of a set-aside question.
    pub last_key_ms: Option<u64>,
    /// The chat: the last key was a Ctrl-C that did not quit, so the next one quits.
    pub quit_armed: bool,
    /// The chat takes the mouse (`ui.mouse`, `/mouse`); off, the terminal selects text.
    pub mouse: bool,
    /// When the judge's reviews of the last hour started (`review.max_per_hour`).
    pub reviews_at: Vec<u64>,
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
            modal: None,
            asks: std::collections::VecDeque::new(),
            prompt: None,
            prompts: std::collections::VecDeque::new(),
            answering: 0,
            last_key_ms: None,
            quit_armed: false,
            mouse: false,
            reviews_at: Vec::new(),
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

    /// Past this many tokens of context, Clio compacts between turns (`session.context_cap_tokens`).
    pub fn context_cap(&self) -> u64 {
        self.home
            .header
            .as_ref()
            .map_or(crate::cli::home::CONTEXT_CAP, |h| h.context_cap)
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
                "no runs yet".into()
            } else {
                "this run has no usage data".into()
            });
            return;
        }
        if !matches!(self.screen, Screen::Context | Screen::Usage) {
            self.back = self.screen;
        }
        self.screen = view;
        self.view_scroll = 0;
    }

    /// No key for `QUIET_MS`, and the question on screen shown for at least as long: a
    /// set-aside question may take an answer. A key typed for something else, even one well
    /// before the question appeared, never answers it.
    pub fn quiet(&self) -> bool {
        let since = |t: u64| self.now_ms.saturating_sub(t) >= QUIET_MS;
        self.last_key_ms.is_none_or(since)
            && self.confirm.as_ref().is_none_or(|c| since(c.shown_ms))
            && self.prompt.as_ref().is_none_or(|p| since(p.shown_ms()))
    }

    /// Set-aside item `id` has its box on screen or in line.
    pub fn asking(&self, id: &str) -> bool {
        let secret =
            |p: &super::ask::Prompt| matches!(p, super::ask::Prompt::Secret(s) if s.id == id);
        let mine = |c: &Confirm| c.id == id || c.more.iter().any(|m| m == id);
        self.confirm.as_ref().is_some_and(mine)
            || self.asks.iter().any(mine)
            || self.prompt.as_ref().is_some_and(secret)
            || self.prompts.iter().any(secret)
    }

    /// Something Clio asks: on screen if nothing else is, else in line.
    pub fn offer(&mut self, p: super::ask::Prompt) {
        self.prompts.push_back(p);
        next_ask(self);
    }

    /// Something on screen moves or counts by itself (a spinner, the seconds of a turn or a run,
    /// the quiet second of a question): only then does the clock alone redraw the screen.
    pub fn animating(&self) -> bool {
        let waiting = self
            .confirm
            .as_ref()
            .is_some_and(|c| c.action.timed() && !c.armed)
            || self.prompt.as_ref().is_some_and(|p| !p.ready());
        let waiting = waiting && !self.quiet();
        let chat = matches!(self.screen, Screen::Chat | Screen::Sessions)
            && (self.chat.opening || self.chat.running || self.chat.review.running.is_some());
        let live = (self.screen == Screen::Live
            || matches!(self.screen, Screen::Context | Screen::Usage) && self.back == Screen::Live)
            && self.live.outcome.is_none();
        waiting || chat || live
    }

    /// How long a frame may stay on screen with nothing coming in: the sessions drawer shows ages
    /// ("3 min ago") that go stale without a redraw. `None`: until something comes in.
    pub fn stale_after(&self) -> Option<std::time::Duration> {
        (self.screen == Screen::Sessions).then(|| std::time::Duration::from_secs(30))
    }

    /// Whether the chat cannot be left now (quit, another session, a new one, a delete); when
    /// it cannot, the flash says why.
    pub fn busy(&mut self) -> bool {
        self.busy_unless(false)
    }

    /// `busy`, but an automatic compaction does not keep the person from quitting: closing stops
    /// Pi as it always does, and a compaction in flight is simply not recorded (the file is
    /// append-only). A `/compact` the person asked for is still waited for.
    pub fn busy_to_quit(&mut self) -> bool {
        self.busy_unless(true)
    }

    fn busy_unless(&mut self, quitting: bool) -> bool {
        let why = if self.chat.running {
            "the agent is working: Esc in the chat aborts the turn; then switch session"
        } else if self.answering > 0 {
            "Clio is still running what you approved: wait for it to finish; then switch session"
        } else if self.chat.opening {
            "the session is opening: wait for it to start; then switch session"
        } else if self.chat.compacting && !(quitting && self.chat.auto_compact) {
            "Pi is compacting the context: wait for it to finish"
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
    let mut cmds = step(&mut app, input);
    // The judge's question about a turn and a review's findings wait for the chat to be idle:
    // whatever made it so, they go now.
    if app.pi {
        cmds.extend(super::review::next_skill(&mut app));
        cmds.extend(super::review::after_turn(&mut app));
        cmds.extend(super::review::deliver(&mut app));
        cmds.extend(super::approvals::deliver(&mut app));
        cmds.extend(super::approvals::check(&mut app));
    }
    (app, cmds)
}

fn step(app: &mut App, input: Input) -> Vec<Cmd> {
    match input {
        Input::Tick(ms) => {
            app.now_ms = ms;
            // Pi gave up on the question (it answered the model by itself): the box goes.
            if app.prompt.as_ref().is_some_and(|p| p.expired(ms)) {
                app.prompt = None;
                app.flash = Some("no answer in time: Clio goes on with her best judgment".into());
                next_ask(app);
            }
            vec![]
        }
        Input::Resize => vec![],
        Input::Paste(text) => {
            let quiet = app.quiet();
            app.last_key_ms = Some(app.now_ms);
            if app.confirm.is_none() && app.notice.is_none() {
                super::ask::paste(app, &text, quiet);
            }
            if app.confirm.is_some()
                || app.prompt.is_some()
                || app.notice.is_some()
                || app.modal.is_some()
            {
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
            if app.prompt.is_some() && app.confirm.is_none() && app.notice.is_none() {
                super::ask::click(app, &m);
                return vec![];
            }
            if !matches!(m.kind, M::Down(_) | M::ScrollUp | M::ScrollDown)
                || app.confirm.is_some()
                || app.notice.is_some()
                || app.modal.is_some()
                || !matches!(app.screen, Screen::Sessions | Screen::Chat)
            {
                return vec![];
            }
            app.flash = None;
            super::sessions::mouse(app, &m)
        }
        Input::Key(k) => {
            // A release, or a modifier alone, is not a key: it must not disarm or answer anything.
            let Some(k) = keys::person_key(k) else {
                return vec![];
            };
            // The chat: Ctrl-C twice always quits, whatever runs or is on screen (leaving stops
            // it, as closing does).
            let ctrl_c = app.pi && keys::Key::Ctrl('c').matches(&k);
            if ctrl_c && app.quit_armed {
                app.quit = true;
                return vec![];
            }
            let cmds = key(app, k);
            app.quit_armed = ctrl_c && !app.quit;
            if app.quit_armed {
                app.flash = Some(match app.flash.take() {
                    Some(f) => format!("{f} · Ctrl-C again quits"),
                    None => "Ctrl-C again quits".into(),
                });
            }
            cmds
        }
    }
}

/// A key the person pressed: what it does where they are.
fn key(app: &mut App, k: KeyEvent) -> Vec<Cmd> {
    app.flash = None;
    // Every key counts for the quiet rule, even one pressed before the question opened;
    // and the question itself must have been on screen for the quiet second.
    let quiet = app.quiet();
    app.last_key_ms = Some(app.now_ms);
    // The keys that work everywhere, even over a message.
    match keys::lookup(app.ctx(), &k) {
        Some(Act::Debug) => {
            app.debug = !app.debug;
            app.flash = Some(if app.debug {
                "debug on (Tab for live; debug.log is written in the run folder)".into()
            } else {
                "debug off".into()
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
    if app.prompt.is_some() {
        let cmds = super::ask::key(app, &k, quiet);
        next_ask(app);
        return cmds;
    }
    if app.modal.is_some() {
        return super::commands::modal_key(app, &k);
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

/// The one line `/undo` leaves in the chat, from the lines `apply::undo_lines` said: a copy
/// says `undone: N file(s) (…)`, a host session one line per file it gave back or left alone,
/// and `nothing is left to undo in …` when there was nothing.
fn undo_note(lines: &[String]) -> String {
    let first = lines.first().map_or("", |l| l.trim());
    if first.starts_with("nothing is left to undo") {
        return "nothing to undo".to_owned();
    }
    if let Some(done) = first.strip_prefix("undone: ") {
        return format!("undone: {done}");
    }
    fn said(l: &str) -> &str {
        l.trim().split(' ').next().unwrap_or("")
    }
    let back = lines
        .iter()
        .filter(|l| {
            matches!(said(l), "restored" | "deleted") && !l.trim().starts_with("deleted the folder")
        })
        .count();
    let left = lines
        .iter()
        .filter(|l| matches!(said(l), "left" | "could"))
        .count();
    if left == 0 {
        format!("undone: {back} file(s) went back as they were")
    } else {
        format!("undone: {back} file(s) went back as they were, {left} left alone")
    }
}

/// The reply to a confirmation: only `y` (or `s`, still accepted) goes ahead. An action set aside, which
/// is asked when a turn ends while the person may be typing, hears its first key only once it
/// has been on screen for `QUIET_MS` and no key came for as long (`quiet`): then `s` arms it and
/// `n` or Esc rejects it. Once armed, Enter
/// runs it at once; any other key disarms it, and the quiet second starts again.
fn answer(app: &mut App, k: &KeyEvent, quiet: bool) -> Vec<Cmd> {
    let Some(mut c) = app.confirm.take() else {
        return vec![];
    };
    // A plain letter: Ctrl-S (save, by habit) or Alt-Y are not an answer.
    let plain = (k.modifiers - KeyModifiers::SHIFT).is_empty();
    let yes_key = plain && matches!(k.code, KeyCode::Char('y' | 'Y' | 's' | 'S'));
    let yes = if c.action.timed() {
        let no = k.code == KeyCode::Esc || (plain && matches!(k.code, KeyCode::Char('n' | 'N')));
        let enter = plain && k.code == KeyCode::Enter;
        // A block change longer than the box: the arrows move it (and disarm it, as any key);
        // `y` arms it only once its last row was on screen.
        let seen = c.block.as_ref().is_none_or(|b| b.seen_end.get());
        if let Some(b) = c.block.as_mut() {
            let max = b.max.get();
            match k.code {
                KeyCode::Up => b.scroll = b.scroll.saturating_sub(1),
                KeyCode::Down => b.scroll = (b.scroll + 1).min(max),
                KeyCode::PageUp => b.scroll = b.scroll.saturating_sub(10),
                KeyCode::PageDown => b.scroll = (b.scroll + 10).min(max),
                _ => {}
            }
        }
        match (c.armed, quiet) {
            (true, _) if enter => true,
            (_, true) if no => false,
            (false, true) if yes_key && seen => {
                c.armed = true;
                app.confirm = Some(c);
                return vec![];
            }
            _ => {
                if yes_key && !seen {
                    app.flash =
                        Some("read the change to its end first (the arrows move it)".into());
                }
                c.armed = false;
                app.confirm = Some(c);
                return vec![];
            }
        }
    } else {
        yes_key
    };
    if !yes && c.action != Action::Approve {
        app.flash = Some("cancelled: nothing was changed".into());
        return vec![];
    }
    match c.action {
        Action::Apply => vec![Cmd::Apply { id: c.id }],
        Action::Undo => vec![Cmd::Undo { id: c.id }],
        Action::Promote => vec![Cmd::MemoryPromote { id: c.id }],
        Action::Archive => vec![Cmd::MemoryArchive { id: c.id }],
        Action::DeleteSession => vec![Cmd::DeleteSession { reference: c.id }],
        Action::UndoSession => vec![Cmd::UndoSession { id: c.id }],
        Action::RemoveSkill => vec![Cmd::Skills(format!("remove {} --yes", c.id))],
        Action::RemoveService => vec![Cmd::Services(format!("remove {} --yes", c.id))],
        Action::RemoveMcp => vec![Cmd::Mcp(format!("remove {} --yes", c.id))],
        // With the digest of the list shown: refused if it changed meanwhile.
        Action::ConfirmMcp => vec![Cmd::Mcp(format!("confirm {} {} --yes", c.id, c.workspace))],
        Action::Approve if c.more.is_empty() => {
            app.answering += 1;
            vec![Cmd::ChatAnswer { id: c.id, yes }]
        }
        Action::Approve => {
            let mut ids = vec![c.id];
            ids.extend(c.more);
            app.answering += ids.len();
            vec![Cmd::ChatAnswers { ids, yes }]
        }
    }
}

/// Once no question is on screen, the next set-aside one; one of a session no longer open cannot
/// be answered here and waits in `clio pending`.
pub(super) fn next_ask(app: &mut App) {
    // What Clio asks goes first: Pi waits for it, and only for so long.
    while app.confirm.is_none()
        && app.prompt.is_none()
        && let Some(mut p) = app.prompts.pop_front()
    {
        // Pi already gave up on it, or its session is no longer open: nobody waits for it.
        if p.session() == app.chat.session_id && !p.expired(app.now_ms) {
            p.show(app.now_ms);
            app.prompt = Some(p);
        }
    }
    while app.confirm.is_none()
        && app.prompt.is_none()
        && let Some(mut c) = app.asks.pop_front()
    {
        if c.session == app.chat.session_id {
            c.shown_ms = app.now_ms;
            app.confirm = Some(c);
        } else {
            app.flash = Some(format!(
                "{} belongs to another session: it waits in `clio pending`",
                c.id
            ));
        }
    }
}

/// Something keeps the chat from being idle: a turn, a compaction, an open, a message waiting,
/// an approval running, a question on screen or waiting. What Clio starts between turns (the
/// automatic compaction, a review, its findings) waits for none of them to be there.
pub fn chat_waits(app: &App) -> bool {
    let c = &app.chat;
    c.running
        || c.compacting
        || c.opening
        || c.queued.is_some()
        || c.session_id.is_empty()
        || app.answering > 0
        || app.confirm.is_some()
        || !app.asks.is_empty()
        || app.prompt.is_some()
        || !app.prompts.is_empty()
}

/// A turn ended: a context over the cap is compacted now, as `/compact` does, if nothing waits
/// (a question, an approval, a message, an action still running). Once per turn that ends.
fn over_cap(app: &mut App) -> Vec<Cmd> {
    let cap = app.context_cap();
    let waits = chat_waits(app);
    let c = &mut app.chat;
    let Some(tokens) = c.meter.context.and_then(|x| x.tokens) else {
        return vec![];
    };
    if tokens <= cap {
        c.over_cap_said = false;
    }
    if tokens <= cap || waits {
        return vec![];
    }
    c.compacting = true;
    c.auto_compact = true;
    c.items.push(super::chat::Item::Note(format!(
        "the context holds {} tokens, over the cap of {}: compacting it automatically…",
        usage::tokens(tokens),
        usage::tokens(cap)
    )));
    vec![Cmd::Compact(String::new())]
}

/// `clio` could not reopen the last conversation: a draft, as when there is none, with what was
/// typed meanwhile kept. With none, a message sent meanwhile starts the new one; a failure says why
/// in a note and gives the message back to the composer.
fn reopen_failed(app: &mut App, e: String, queued: Option<String>) -> Vec<Cmd> {
    let typed = std::mem::take(&mut app.chat.input);
    app.chat = Chat::draft(app.now_ms);
    app.chat.set_input(typed);
    if e == NOTHING_TO_REOPEN {
        return queued.map_or_else(Vec::new, |text| super::chat::send_text(app, text));
    }
    app.chat
        .items
        .push(super::chat::Item::Note(if e == OPEN_ELSEWHERE {
            "your last conversation is open in another terminal: this is a new one".to_owned()
        } else {
            format!("could not reopen the last conversation ({e}): this is a new one")
        }));
    if let Some(text) = queued {
        app.chat.unsend(text);
    }
    vec![]
}

/// A question that comes back late never takes the place of the one on screen.
fn dropped(app: &mut App, what: &str) -> bool {
    if app.confirm.is_none() && app.prompt.is_none() {
        return false;
    }
    app.flash = Some(format!(
        "{what} arrived while another question was open: ask again"
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
            if dropped(app, &format!("the plan for {}", p.id)) {
                return vec![];
            }
            // The plan takes a while. If the person is no longer looking at that run, a dialog
            // popping up now would turn the next key they type into an answer.
            if !looking_at(app, &p.id) {
                app.flash = Some(format!(
                    "the plan for {} was ready, but you changed screen: ask again",
                    p.id
                ));
                return vec![];
            }
            let mut c = Confirm::new(Action::Apply, p.id, p.files, p.workspace);
            c.names = p.names;
            c.note = match p.tainted {
                Some(true) => {
                    Some("The agent read outside content: check the diff carefully.".into())
                }
                None => Some("No record of what the agent read: check the diff.".into()),
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
                Ok(_) if dropped(app, "what would be deleted") || app.busy() => {}
                Ok((file, title, what)) if app.screen == Screen::Sessions => {
                    // The title whole (the dialog wraps it), then what the drawer row shows.
                    let when = app
                        .sessions
                        .rows
                        .iter()
                        .find(|r| r.reference == file)
                        .map(|r| {
                            super::sessions::age_and_count(r, crate::cli::now_ms(), app.theme.dot())
                        })
                        .unwrap_or_default();
                    let when = if when.is_empty() {
                        String::new()
                    } else {
                        format!(" {} {when}", app.theme.dot())
                    };
                    app.confirm = Some(Confirm::new(
                        Action::DeleteSession,
                        file,
                        0,
                        format!("\"{title}\"{when}: {what}"),
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
                Ok(m) => {
                    app.chat.items.push(super::chat::Item::Note(m));
                    "session deleted: it is in the trash".to_owned()
                }
                Err(m) => m,
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
                    // Read from the session's file already; Pi's copy only when it could not be.
                    let history = if app.chat.from_file {
                        &[][..]
                    } else {
                        &info.history[..]
                    };
                    app.chat
                        .opened(info.id, info.workspace, history, info.refresh, app.now_ms);
                    app.chat
                        .items
                        .extend(info.notes.into_iter().map(super::chat::Item::Note));
                    // What waits from before: asked about once the chat is idle, if it is time.
                    app.chat.approvals.check = true;
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
                    app.chat.opening = false;
                    if app.chat.resume {
                        return reopen_failed(app, e, queued);
                    }
                    if let Some(text) = queued {
                        app.chat.unsend(text);
                        // Still a new session: Enter tries again.
                        if app.chat.file.is_none() {
                            app.chat.items.push(super::chat::Item::Error(e));
                            return vec![];
                        }
                    }
                    app.chat.failed = Some(e);
                }
            }
            vec![]
        }
        Bg::History { seq, file, page } => {
            if seq != app.chat.seq {
                return vec![];
            }
            let c = &mut app.chat;
            let earlier = std::mem::replace(&mut c.loading_earlier, false);
            match page {
                Ok(h) => c.add_page(&h.entries, h.more.then_some(h.first)),
                Err(e) if earlier => {
                    app.flash = Some(format!("could not read the earlier messages: {e}"));
                }
                Err(e) => c.items.push(super::chat::Item::Note(format!(
                    "the session file could not be read ({e}): showing what Pi keeps of the conversation"
                ))),
            }
            if c.title.is_empty()
                && let Some(r) = app.sessions.rows.iter().find(|r| r.reference == file)
            {
                c.title.clone_from(&r.title);
            }
            c.file = Some(file);
            vec![]
        }
        Bg::ChatEvent(e) => {
            if matches!(e, clio_brain::pi::rpc::Event::AgentStart) && !app.chat.running {
                app.chat.turn_ms = app.now_ms;
            }
            app.chat.on_event(&e);
            if let clio_brain::pi::rpc::Event::ExtensionUiRequest {
                id,
                method,
                title,
                options,
                timeout_ms,
                ..
            } = &e
            {
                return super::ask::on_request(app, id, method, title, options, *timeout_ms);
            }
            vec![]
        }
        // They come before the turn settles, so from the session open now.
        // ONE box for everything that waits and is not in a box yet.
        Bg::ChatAsks(asks) => {
            let new: Vec<_> = asks
                .into_iter()
                .filter(|(id, ..)| !app.asking(id))
                .collect();
            if let Some(c) = super::approvals::one_box(app, new) {
                app.asks.push_back(c);
            }
            next_ask(app);
            vec![]
        }
        Bg::ChatSecrets(asks, suspect) => {
            for (id, what) in asks {
                if app.asking(&id) {
                    continue;
                }
                let session = app.chat.session_id.clone();
                let ask =
                    super::ask::SecretAsk::new(&id, &session, (&what.0, &what.1, &what.2), suspect);
                app.offer(super::ask::Prompt::Secret(ask));
            }
            vec![]
        }
        Bg::ChatAnswered(r, yes) => {
            app.answering = app.answering.saturating_sub(1);
            let a = &mut app.chat.approvals;
            a.waiting = a.waiting.saturating_sub(1);
            // Only something done starts a turn; a rejection or a failure rides with the next
            // message of the person.
            a.answered |= yes && r.is_ok();
            app.chat.items.push(match r {
                Ok(m) => super::chat::Item::Note(m),
                Err(e) => super::chat::Item::Error(e),
            });
            vec![]
        }
        Bg::Commands(customs, left) => {
            app.chat.customs = customs;
            if !left.is_empty() && app.chat.menu.is_some() {
                app.flash = Some(format!(
                    "commands left out: {} (the name uses a-z, 0-9 and -, must not clash with one of Clio's, and the file is at most 32 kB)",
                    clean(&left.join(", "))
                ));
            }
            vec![]
        }
        Bg::Choices(run, r) => {
            let Some(m) = app
                .chat
                .menu
                .as_mut()
                .filter(|m| m.pick == Some((run, None)))
            else {
                return vec![];
            };
            match r {
                Ok((items, note)) => {
                    m.sel = items.iter().position(|c| c.current).unwrap_or(0);
                    m.pick = Some((run, Some(items)));
                    m.note = note;
                }
                Err(e) => {
                    m.pick = None;
                    app.flash = Some(e);
                }
            }
            vec![]
        }
        Bg::Named(r) => match r {
            Ok(title) => {
                app.flash = Some(format!("title: {}", clean(&title)));
                app.chat.title = title;
                vec![Cmd::LoadSessions]
            }
            Err(e) => {
                app.notice = Some(e);
                vec![]
            }
        },
        Bg::Compacted(r, after, refresh) => {
            let c = &mut app.chat;
            c.compacting = false;
            let auto = std::mem::take(&mut c.auto_compact);
            let stopped = std::mem::take(&mut c.aborting) && auto;
            let ok = r.is_ok();
            c.items.push(match r {
                Ok(m) => super::chat::Item::Note(m),
                Err(_) if stopped => {
                    super::chat::Item::Note("the automatic compaction was stopped".into())
                }
                Err(e) => super::chat::Item::Error(format!("could not compact: {e}")),
            });
            if let Some(r) = refresh {
                c.refresh(&r);
            }
            // Not tried again before another turn ends.
            let cap = app.context_cap();
            if auto && ok && after.is_some_and(|n| n > cap) && !app.chat.over_cap_said {
                app.chat.over_cap_said = true;
                app.chat.items.push(super::chat::Item::Note(format!(
                    "the context is still over the cap of {} tokens after compacting: /compact N raises it to N thousand",
                    usage::tokens(cap)
                )));
            }
            // A message sent meanwhile goes out now, after what the compaction said.
            let c = &mut app.chat;
            let Some(text) = c.queued.take() else {
                return vec![];
            };
            let mine = super::chat::Item::User(text.clone());
            if let Some(i) = c.items.iter().rposition(|i| *i == mine) {
                c.items.remove(i);
                c.items.push(mine);
                c.rendered.take();
            }
            c.start_turn(app.now_ms);
            c.approvals.auto_turns = 0;
            vec![Cmd::ChatSend { text }]
        }
        Bg::CapSet(r) => {
            match r {
                Ok(cap) => {
                    if let Some(h) = &mut app.home.header {
                        h.context_cap = cap;
                    }
                    app.chat.items.push(super::chat::Item::Note(format!(
                        "context cap: {} tokens; past it Clio compacts between turns (saved in config.json)",
                        usage::tokens(cap)
                    )));
                }
                Err(e) => app.notice = Some(e),
            }
            vec![]
        }
        Bg::Approvals { session, waiting } => {
            super::approvals::checked(app, &session, waiting);
            vec![]
        }
        Bg::Grant {
            session,
            ends,
            hours,
            suspect,
            judge,
        } => {
            super::approvals::grant_state(app, &session, ends, hours, (suspect, judge));
            vec![]
        }
        // Not an answer of the person's: no turn of its own; it rides with the next message. The
        // count of what waits comes with the check (`Bg::Approvals`): an act approved inline was
        // never counted.
        Bg::Granted(r) => {
            app.chat.items.push(match r {
                Ok(m) => super::chat::Item::Note(m),
                Err(e) => super::chat::Item::Error(e),
            });
            vec![]
        }
        Bg::GrantSaid(r) => {
            app.chat.approvals.check = true;
            app.chat.items.push(match r {
                Ok(m) => super::chat::Item::Note(m),
                Err(e) => super::chat::Item::Error(e),
            });
            vec![]
        }
        Bg::GrantSummary { session } => {
            if session == app.chat.session_id {
                app.chat.approvals.summary = true;
            }
            vec![]
        }
        Bg::Pending(r) => {
            match r {
                Ok((rows, cut, others)) => {
                    // The count of the status line, from what was just read.
                    app.chat.approvals.waiting = rows.len();
                    let (mut lines, picks): (Vec<String>, _) = rows.into_iter().unzip();
                    if lines.is_empty() {
                        lines.push("nothing in this session waits for your approval".into());
                    }
                    // After the picks: none of them can be picked.
                    lines.extend(cut);
                    let mut m = Modal::text("Pending", lines);
                    m.picks = picks;
                    if others > 0 {
                        m.foot = Some(format!(
                            "and {others} from other sessions: clio pending in a terminal"
                        ));
                    }
                    app.modal = Some(m);
                }
                Err(e) => app.notice = Some(e),
            }
            vec![]
        }
        Bg::Modal(r) => {
            match r {
                Ok(m) => app.modal = Some(m),
                Err(e) => app.notice = Some(e),
            }
            vec![]
        }
        // In line with the other questions: never over one already open.
        Bg::Confirm(r) => {
            match r {
                Ok(mut c) => {
                    c.session.clone_from(&app.chat.session_id);
                    app.asks.push_back(*c);
                    next_ask(app);
                }
                Err(e) => app.notice = Some(e),
            }
            vec![]
        }
        Bg::SessionUndone(r) => {
            match r {
                Ok(lines) => {
                    app.chat
                        .items
                        .push(super::chat::Item::Note(undo_note(&lines)));
                    app.modal = Some(Modal::text("Undo", lines));
                }
                Err(e) => app.notice = Some(e),
            }
            vec![]
        }
        Bg::TurnWork(w) => {
            app.chat.review.work = Some(*w);
            app.chat.review.changed = true;
            vec![]
        }
        Bg::Acted(notes) => {
            app.chat
                .items
                .extend(notes.into_iter().map(super::chat::Item::Note));
            app.chat.approvals.acted = true;
            vec![]
        }
        Bg::SkillNews(drafts, notes) => {
            for n in notes {
                app.chat.items.push(super::chat::Item::Note(n));
            }
            app.chat.review.skills.extend(drafts);
            vec![]
        }
        Bg::ReviewStarted { session, auto, why } => {
            super::review::started(app, &session, auto, &why);
            vec![]
        }
        Bg::ReviewSkipped { session, why } => {
            super::review::skipped(app, &session, &why);
            vec![]
        }
        Bg::Reviewed { session, result } => {
            super::review::ended(app, &session, result);
            vec![]
        }
        Bg::ChatSettled { result, refresh } => {
            app.chat.running = false;
            // A question lives inside its turn: Pi answered it by itself if the turn ended. A
            // secret request waits for its answer.
            use super::ask::Prompt;
            if matches!(app.prompt, Some(Prompt::Question(_))) {
                app.prompt = None;
            }
            app.prompts.retain(|p| matches!(p, Prompt::Secret(_)));
            next_ask(app);
            app.chat.aborting = false;
            app.chat.stop_tools();
            match result {
                Err(e) => app.chat.items.push(super::chat::Item::Error(e)),
                Ok(summary) => {
                    if let Some(a) = &summary.assessment {
                        app.chat.items.push(super::chat::Item::Note(format!(
                            "request judged as {}",
                            a.notice()
                        )));
                    }
                    if let Some(notice) = summary.stop_notice() {
                        app.chat.items.push(super::chat::Item::Error(notice));
                    }
                }
            }
            let refreshed = refresh.is_some();
            if let Some(r) = refresh {
                app.chat.refresh(&r);
                if r.jobs > 0 {
                    let n = r.jobs;
                    let (s, verb) = if n == 1 { ("", "is") } else { ("s", "are") };
                    app.chat.items.push(super::chat::Item::Note(format!(
                        "{n} background command{s} {verb} still running; all stop when the session ends"
                    )));
                }
            }
            // The list beside the chat: a new session shows up, a title changes.
            if app.pi {
                app.chat.approvals.check = true;
                let mut cmds = vec![Cmd::LoadSessions];
                // Before the compaction: it would make the chat look busy.
                cmds.extend(super::review::turn_ended(app));
                if refreshed {
                    cmds.extend(over_cap(app));
                }
                return cmds;
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

/// A thin labeled line: `─ title ──────────`.
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
        Screen::Home => "home",
        Screen::NewRun => "new run",
        Screen::Live => "live",
        Screen::Detail => "detail",
        Screen::Doctor => "doctor",
        Screen::Help => "help",
        Screen::Context => "context",
        Screen::Usage => "usage",
        Screen::Memory => "memory",
        Screen::Sessions | Screen::Chat => "chat",
    }
}

fn folder_name(path: &str) -> String {
    let p = std::path::Path::new(path);
    p.file_name()
        .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned())
}

/// The chat's title: the session's, as the list has it, or "new session" for a draft.
fn chat_title(app: &App) -> String {
    let c = &app.chat;
    if c.is_draft() {
        return "new session".into();
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
            v.push("config does not load".to_owned());
        } else if h.key == super::home::KeyState::Missing {
            v.push("model key missing".to_owned());
        }
        if h.vault.starts_with("created") {
            v.push("vault stopped".to_owned());
        }
    }
    if let Some((_, _, fail)) = app.home.doctor
        && fail > 0
    {
        v.push(format!("doctor: {}", plural(fail, "failure", "failures")));
    }
    if app.debug {
        v.push("debug".to_owned());
    }
    v
}

/// `clio · title` on the left; on the right what is wrong, in the warning color, and the model.
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
    // With Pi, how hard the model thinks too: `claude-sonnet-5-5 · low`.
    let model = app
        .home
        .header
        .as_ref()
        .filter(|h| h.config_error.is_none())
        .map_or_else(String::new, |h| {
            match super::commands::level_of(&h.thinking) {
                Some(level) if app.pi => format!("{} {} {level}", clean(&h.model), t.dot()),
                _ => clean(&h.model),
            }
        });
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
        None => Line::from(Span::styled("no usage yet", t.dim())),
    };
    line.spans
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    if area.width < MIN_W || area.height < MIN_H {
        f.render_widget(
            Paragraph::new(format!(
                "Terminal too small: it needs at least {MIN_W}x{MIN_H} and this one is {}x{}. Make it larger.",
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
    if let Some(m) = &app.modal {
        // Under the top bar and its rule, which always stay.
        super::commands::draw_modal(f, rest, app, m);
    }
    if let Some(c) = &app.confirm {
        match &c.block {
            // Under the top bar and its rule, like the secret box: it can take the whole height.
            Some(b) => block_dialog(f, rest, app, c, b),
            None => confirm_dialog(f, area, app, c),
        }
    } else if let Some(p) = &app.prompt {
        // Under the top bar and its rule, like the boxes of the commands.
        match p {
            super::ask::Prompt::Question(q) => super::ask::draw(f, rest, app, q),
            super::ask::Prompt::Secret(s) => super::ask::draw_secret(f, rest, app, s),
        }
    } else if let Some(n) = &app.notice {
        let lines: Vec<Line> = sanitize::multiline(n)
            .lines()
            .map(|l| Line::from(l.to_owned()))
            .collect();
        let hint = Line::from(Span::styled("any key closes", t.dim()));
        dialog(f, area, t, " Notice ", lines, hint, t.fail());
    }
}

fn confirm_dialog(f: &mut Frame, area: Rect, app: &App, c: &Confirm) {
    let t = app.theme;
    let verb = match c.action {
        Action::Apply => format!("Apply {} file(s) in", c.files),
        Action::Undo => format!("Undo what was applied ({} file(s)) in", c.files),
        Action::Promote => format!(
            "Approve memory m{} (it becomes active and will be remembered)",
            c.id
        ),
        Action::Archive => format!("Archive memory m{} (it will no longer be remembered)", c.id),
        Action::DeleteSession => {
            "Delete the session? All of it goes to the trash for 30 days, and `clio sessions restore` brings it back whole:".to_owned()
        }
        Action::Approve => format!("Clio set aside {}. Run it?", c.id),
        Action::UndoSession => "Undo this session's file changes in".to_owned(),
        Action::RemoveSkill => format!(
            "Remove the skill «{}»? Pi stops loading it from the next session; its versions are kept.",
            clean(&c.id)
        ),
        Action::RemoveService => format!(
            "Remove the service «{}»? It stops running; its versions are kept.",
            clean(&c.id)
        ),
        Action::RemoveMcp => format!(
            "Remove the MCP server «{}»? Its files, data and record go to removed/.",
            clean(&c.id)
        ),
        Action::ConfirmMcp => format!(
            "Confirm the tools of the MCP server «{}» and turn it on? These descriptions are its own text:",
            clean(&c.id)
        ),
    };
    let mut lines = vec![Line::from(Span::styled(verb, t.bold())), Line::from("")];
    // A set-aside action (`«delete» …`): its class in the warning color, the command under it.
    let what = clean(&c.workspace);
    match what.split_once('»') {
        Some((class, rest)) if c.action == Action::Approve && class.starts_with('«') => {
            lines.push(Line::from(Span::styled(format!("{class}»"), t.warn())));
            lines.push(Line::from(Span::styled(rest.trim().to_owned(), t.code())));
        }
        _ => lines.push(Line::from(what)),
    }
    for n in c.names.iter().take(CONFIRM_FILES) {
        lines.push(Line::from(format!("  {}", clean(n))));
    }
    if c.names.len() > CONFIRM_FILES {
        lines.push(Line::from(Span::styled(
            format!("  … and {} more", c.names.len() - CONFIRM_FILES),
            t.dim(),
        )));
    }
    if let Some(n) = &c.note {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(clean(n), t.warn())));
    }
    if c.action == Action::Approve && !c.armed && !app.quiet() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "wait a second without typing to answer…",
            t.dim(),
        )));
    }
    let d = format!("   {}   ", t.dot());
    let hint = if c.action == Action::Approve {
        Line::from(vec![
            Span::styled(if c.armed { "Enter" } else { "y then Enter" }, t.key()),
            Span::styled(" yes, run it", t.dim()),
            Span::styled(d, t.dim()),
            Span::styled(if c.armed { "other key" } else { "n or Esc" }, t.key()),
            Span::styled(
                if c.armed {
                    " goes back"
                } else {
                    " no, reject it"
                },
                t.dim(),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled("y", t.key()),
            Span::styled(" yes, go on", t.dim()),
            Span::styled(d, t.dim()),
            Span::styled("any other key", t.key()),
            Span::styled(" cancels", t.dim()),
        ])
    };
    let border = if matches!(c.action, Action::Approve | Action::DeleteSession) {
        t.warn()
    } else {
        t.accent_line()
    };
    let title = if c.action == Action::Approve {
        " Approval "
    } else {
        " Confirm "
    };
    dialog(f, area, t, title, lines, hint, border);
}

/// The approval of a change to one of the person's blocks: every row of it, whole, wrapped; when
/// it is longer than the box, a window of it that the arrows move, and a line saying where it is.
/// Never cut with an ellipsis: the person approves what they can read (S-36).
fn block_dialog(f: &mut Frame, area: Rect, app: &App, c: &Confirm, b: &super::ask::BlockAsk) {
    use super::ask::Part;
    let t = app.theme;
    let w = dialog_inner(area);
    let mut body: Vec<Line<'static>> = if c.action == Action::Approve {
        let what = if c.more.is_empty() {
            format!("Clio set aside {}.", c.id)
        } else {
            format!("Clio set aside {} items.", c.more.len() + 1)
        };
        vec![Line::from(Span::styled(what, t.bold())), Line::from("")]
    } else {
        vec![]
    };
    for (part, text) in b.parts() {
        let (style, indent) = match part {
            Part::Head => (t.bold(), ""),
            Part::Label => (t.dim(), ""),
            Part::Text => (t.code(), "  "),
            Part::Warn => (t.warn(), ""),
        };
        if part == Part::Warn {
            body.push(Line::from(""));
        }
        for l in wrap_line(
            Line::from(Span::styled(text, style)),
            w.saturating_sub(indent.len()),
        ) {
            let mut spans = vec![Span::raw(indent)];
            spans.extend(l.spans);
            body.push(Line::from(spans));
        }
    }
    let mut foot: Vec<Line<'static>> = Vec::new();
    if !c.armed && !app.quiet() {
        foot.push(Line::from(""));
        foot.push(Line::from(Span::styled(
            "wait a second without typing to answer…",
            t.dim(),
        )));
    }
    let d = format!("   {}   ", t.dot());
    let hint = Line::from(vec![
        Span::styled(if c.armed { "Enter" } else { "y then Enter" }, t.key()),
        Span::styled(b.yes().to_owned(), t.dim()),
        Span::styled(d, t.dim()),
        Span::styled(if c.armed { "other key" } else { "n or Esc" }, t.key()),
        Span::styled(
            if c.armed {
                " goes back"
            } else if c.action == Action::Approve && !c.more.is_empty() {
                " no to all"
            } else if c.action == Action::Approve {
                " no, reject it"
            } else {
                " no, leave it as it is"
            },
            t.dim(),
        ),
    ]);
    foot.push(Line::from(""));
    foot.extend(wrap_line(hint, w));
    // The rows the box has for the change: the screen less its border, its air and the keys.
    let air = if area.height >= 24 { 2 } else { 0 };
    let room = usize::from(area.height)
        .saturating_sub(2 + air + foot.len())
        .max(1);
    let total = body.len();
    let rows = if total <= room {
        b.max.set(0);
        b.seen_end.set(true);
        body
    } else {
        // One row says where the window is.
        let shown = room.saturating_sub(1).max(1);
        let max = total - shown;
        b.max.set(max);
        let from = b.scroll.min(max);
        if from == max {
            b.seen_end.set(true);
        }
        let mut rows: Vec<Line<'static>> = body.into_iter().skip(from).take(shown).collect();
        rows.push(Line::from(Span::styled(
            format!(
                "{}{} rows {}-{} of {total}{}",
                t.up(),
                t.down(),
                from + 1,
                from + shown,
                if from < max { ", more below" } else { "" }
            ),
            t.dim(),
        )));
        rows
    };
    let all: Vec<Line<'static>> = rows.into_iter().chain(foot).collect();
    let title = if c.action == Action::Approve {
        " Approval "
    } else {
        " Confirm "
    };
    dialog_box(f, area, t, title, all, t.warn());
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
    let inner = dialog_inner(area);
    let mut rows: Vec<Line> = Vec::new();
    for l in lines.into_iter().chain([Line::from(""), hint]) {
        rows.extend(wrap_line(l, inner));
    }
    dialog_box(f, area, t, title, rows, border);
}

/// The columns of text inside a dialog on `area`.
pub(super) fn dialog_inner(area: Rect) -> usize {
    usize::from(area.width.saturating_sub(6).min(72).saturating_sub(6)).max(1)
}

/// One line wrapped to `w` columns, its styles kept.
pub(super) fn wrap_line(l: Line<'static>, w: usize) -> Vec<Line<'static>> {
    let segs: Vec<(String, ratatui::style::Style)> = l
        .spans
        .into_iter()
        .map(|s| (s.content.into_owned(), s.style))
        .collect();
    let none = (Vec::new(), 0);
    super::markdown::wrap_spans(&segs, w, none.clone(), none)
}

/// Draws rows already wrapped to `dialog_inner` in a centered box, a row of air above and below
/// when the terminal has the rows for it. Returns where the first row went (x, y, width).
pub(super) fn dialog_box(
    f: &mut Frame,
    area: Rect,
    t: Theme,
    title: &str,
    mut rows: Vec<Line<'static>>,
    border: ratatui::style::Style,
) -> Rect {
    let w = area.width.saturating_sub(6).min(72);
    let air = area.height >= 24;
    if air {
        rows.insert(0, Line::from(""));
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
    Rect::new(
        r.x + 3,
        r.y + 1 + u16::from(air),
        r.width.saturating_sub(6),
        r.height.saturating_sub(2 + 2 * u16::from(air)),
    )
}

/// Counts by label, for the home header and the live panel.
pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
