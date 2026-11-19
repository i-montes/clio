//! The local source: Pi on this machine, and the whole lifecycle of what surrounds it, in one
//! place. Opening a session does, in this order, and undoes it in the reverse order on any
//! failure, on `shutdown`, on `Drop` (panic included) and when the interface dies:
//!
//! 1. the working copy: a NEW session is a `runs::prepare` run (the project copied to
//!    `runs/<id>/sandbox` with its manifest, so `clio diff|apply|undo <id>` work on it); a listed
//!    one must be a session whose folder is exactly that copy. In host mode there is no copy: the
//!    run folder holds `meta.json` (`host: true`), the folder is the person's home, and the
//!    toolhost acts on the real machine;
//! 2. `clio toolhost` (this same executable) on that copy, with `HOME` set to Clio's own Pi home
//!    (the jail self-test plants its decoy there, never in the person's home), a 0700 socket
//!    folder under `$XDG_RUNTIME_DIR/clio`, and the API key taught to its output filter;
//! 3. Pi through `pi::launch::start`, hardened, with the `clio-guard` extension, exactly the
//!    tools the guard registers, and the guard's own variables (prompt file and version, socket).
//!
//! Pi and the toolhost both exit when their stdin closes, so nothing is left behind even if this
//! process is killed with `SIGKILL`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use clio_core::SystemClock;
use clio_toolhost::pending::Pending;
use clio_vault::SecretValue;

use clio_brain::agent::AgentConfig;
use clio_brain::connector;
use clio_brain::control::{Budget, DayBudget};
use clio_brain::memory_link::{EmbedModel, MemoryLink};
use clio_brain::pi::PiError;
use clio_brain::pi::budget::RunGuard;
use clio_brain::pi::install;
use clio_brain::pi::launch::{Launch, PiProcess, Provider, SessionTarget, start, valid_id};
use clio_brain::pi::level::{self, Assessor, run_assessed_with};
use clio_brain::pi::preprompt::{APPROVALS_ASK, PrePrompt};
use clio_brain::pi::rpc::{Cancel, Event, RunSummary};
use clio_brain::pi::session::{
    GUARD_TOOLS, HOST_GUARD_TOOLS, HostOpts, Toolhost, ToolhostOpts, ensure_guard, guard_env,
};
use clio_brain::pi::sessions::{SessionInfo, list_sessions};
use clio_gateway::Source;
use serde_json::{Value, json};

use super::{Opened, Refresh, Row, RowKind, Session, SessionSource, Target};
use crate::cli::Res;
use crate::cli::home::{Config, Dirs, Limits, Mode};
use crate::cli::jev;
use crate::cli::pending;
use crate::cli::runs::{self, RunOpts};
use crate::cli::usage;

fn text(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn path_var() -> String {
    std::env::var("PATH").unwrap_or_default()
}

/// Can Pi run here at all: Node 22.19+ and the pinned install?
pub fn ready(dirs: &Dirs) -> Res<()> {
    install::find_node(&path_var()).map_err(text)?;
    install::locate(&crate::cli::pi::paths(dirs)).map_err(text)?;
    Ok(())
}

pub struct LocalSource {
    dirs: Arc<Dirs>,
    /// A conversation: Clio introduces herself to a person the memory does not know.
    presentation: bool,
    /// `--host` or `--copy`, over the configuration, for a new session.
    mode: Option<Mode>,
}

impl LocalSource {
    pub fn new(dirs: Arc<Dirs>) -> Self {
        Self {
            dirs,
            presentation: true,
            mode: None,
        }
    }

    #[must_use]
    pub fn with_mode(mut self, mode: Option<Mode>) -> Self {
        self.mode = mode;
        self
    }

    /// One instruction (`clio run`), not a conversation: nobody is there to answer.
    #[must_use]
    pub fn one_shot(mut self) -> Self {
        self.presentation = false;
        self
    }

    /// The only folder a session of Clio may work in: the copy of its run.
    fn copy_of(&self, id: &str) -> PathBuf {
        self.dirs.runs.join(id).join("sandbox")
    }

    /// The id comes from the header of a file Pi can write: it must be one plain name, and the
    /// copy it names must sit inside `runs/` (`Path::join` with an absolute id, or one with `..`,
    /// would point anywhere).
    fn is_ours(&self, s: &SessionInfo) -> bool {
        self.is_copy(s) || self.is_host(s)
    }

    /// A host-mode session: its run says so, and its folder is the person's home.
    fn is_host(&self, s: &SessionInfo) -> bool {
        let home = self.dirs.home.canonicalize().ok();
        valid_id(&s.id)
            && runs::read_json::<runs::Meta>(&self.dirs.runs.join(&s.id).join("meta.json"))
                .is_ok_and(|m| m.host && Path::new(&m.workspace).canonicalize().ok() == home)
            && home.is_some()
            && s.cwd_path.as_ref().and_then(|c| c.canonicalize().ok()) == home
    }

    fn is_copy(&self, s: &SessionInfo) -> bool {
        if !valid_id(&s.id) {
            return false;
        }
        let (Ok(runs), Ok(want)) = (
            self.dirs.runs.canonicalize(),
            self.copy_of(&s.id).canonicalize(),
        ) else {
            return false;
        };
        match &s.cwd_path {
            Some(have) => want.starts_with(&runs) && have.canonicalize().is_ok_and(|h| h == want),
            None => false,
        }
    }
}

impl SessionSource for LocalSource {
    fn list(&self) -> Vec<Row> {
        let list = list_sessions(&crate::cli::pi::paths(&self.dirs).sessions_dir());
        list.sessions
            .iter()
            .map(|s| Row {
                kind: RowKind::Session,
                title: s.title.clone(),
                detail: s.cwd.clone(),
                messages: s.messages,
                last_ms: Some(s.last_activity_ms),
                reference: s.path.display().to_string(),
                unavailable: (!self.is_ours(s)).then(|| {
                    "not a working copy of Clio's (another tool made it): it does not open here"
                        .to_owned()
                }),
            })
            .collect()
    }

    fn last(&self, project: &Path) -> Option<String> {
        let mode = match self.mode {
            Some(m) => m,
            None => Config::load(&self.dirs).ok()?.mode,
        };
        let project = project.canonicalize().ok();
        let of_project = |s: &SessionInfo| {
            let meta = runs::read_json::<runs::Meta>(&self.dirs.runs.join(&s.id).join("meta.json"));
            project.is_some()
                && meta
                    .ok()
                    .and_then(|m| Path::new(&m.workspace).canonicalize().ok())
                    == project
        };
        list_sessions(&crate::cli::pi::paths(&self.dirs).sessions_dir())
            .sessions
            .into_iter()
            .find(|s| match mode {
                Mode::Host => self.is_host(s),
                Mode::Copy => self.is_copy(s) && of_project(s),
            })
            .map(|s| s.path.display().to_string())
    }

    fn open(&self, target: &Target) -> Result<Opened, String> {
        let d = &*self.dirs;
        let cfg = Config::load(d)?;
        let paths = crate::cli::pi::paths(d);
        let node = install::find_node(&path_var()).map_err(text)?;
        let install = install::locate(&paths).map_err(text)?;
        // `Config::load` guarantees `cfg.model.provider` is a connector.
        let key = crate::cli::vault::model_key(
            d,
            connector::by_provider(&cfg.model.provider).expect("a connector"),
        )?;
        // 1. the working copy
        let (id, session, host) = match target {
            Target::New { .. } if self.mode.unwrap_or(cfg.mode) == Mode::Host => {
                clean_old_runs(d);
                let id = runs::prepare_host(d, "Pi session")?;
                (id.clone(), SessionTarget::New { id: Some(id) }, true)
            }
            Target::New { project } => {
                clean_old_runs(d);
                let p = runs::prepare(
                    d,
                    &RunOpts {
                        workspace: project.clone(),
                        max_steps: None,
                        allow_broad: false,
                        instruction: "Pi session".into(),
                        adapter_key: false,
                    },
                )?;
                (p.id.clone(), SessionTarget::New { id: Some(p.id) }, false)
            }
            Target::Existing { reference } => {
                // Only a listed session, and only one whose folder is its own copy.
                let list = list_sessions(&paths.sessions_dir());
                let s = list
                    .sessions
                    .iter()
                    .find(|s| s.path.display().to_string() == *reference)
                    .ok_or("that session is no longer in Pi's sessions folder")?;
                if !self.is_ours(s) {
                    return Err(
                        "the session is not a working copy of Clio's: it does not open".into(),
                    );
                }
                (
                    s.id.clone(),
                    SessionTarget::Open(reference.clone()),
                    self.is_host(s),
                )
            }
        };
        let run_dir = d.runs.join(&id);
        // One `clio` per conversation: two would run two Pi on one file and share one run folder.
        let lock = hold(&run_dir)?;
        // A file Pi can write: if it is not as Clio left it, what it says is outside content.
        let session_file = match target {
            Target::Existing { reference } => Some(PathBuf::from(reference)),
            Target::New { .. } => None,
        };
        let changed = session_file
            .as_deref()
            .is_some_and(|f| changed_since_close(d, &run_dir, f));
        if !changed {
            seal(d, &run_dir, session_file.as_deref());
        }
        let work = if host {
            d.home.clone()
        } else {
            self.copy_of(&id)
        };
        let (state, control, rules) = (d.data.join("host"), d.control(), d.host_rules());
        // A chat can ask the person for a secret in both modes: its requests wait there too.
        if host || self.presentation {
            // 0700 even if an earlier version left it open (S-23).
            crate::cli::mkdir_private(&state)?;
        }
        let person = crate::cli::vault::person_socket(d)?;
        // Skills (S-68): a chat's, with the shipped one installed when it never was there.
        let skills = if self.presentation {
            crate::cli::skills::ensure(d)?;
            Some(crate::cli::skills::root(d))
        } else {
            None
        };
        // The project of the session, for the memory: decided here from the person's own folder
        // (the copy has no history), never by the model.
        let project = runs::read_json::<runs::Meta>(&run_dir.join("meta.json"))
            .ok()
            .and_then(|m| clio_brain::project::settle(&d.data, Path::new(&m.workspace)))
            .map(|(name, _, _)| name);
        let mut service_env = jev::toolhost_env(d, &cfg.jev);
        if let Some(p) = &project {
            service_env.push(("CLIO_MEMORY_PROJECT".into(), p.clone()));
        }
        // 2. the toolhost
        let exe = std::env::current_exe().map_err(text)?;
        // The service has Pi's HOME, so it cannot find the vector model by itself.
        let models = MemoryLink::model_dirs(&d.home);
        let memory = Some((d.data.as_path(), models.as_slice()));
        let toolhost = Toolhost::start(
            &exe,
            d.need_runtime()?,
            &paths,
            &run_dir,
            &work,
            &key,
            ToolhostOpts {
                memory,
                // Jev, if the person turned it on (the service that handles the model's proposals
                // asks it what to do with each; the key stays in the vault), and the project.
                extra_env: &service_env,
                host: host.then(|| HostOpts {
                    home: &d.home,
                    state: &state,
                    control: &control,
                    rules: rules.as_deref(),
                    run: &id,
                }),
                // Only with a person there to answer (not `clio run`).
                secrets: self.presentation.then_some((
                    state.as_path(),
                    id.as_str(),
                    person.as_path(),
                )),
                review: false,
                skills: skills.as_deref(),
                show: None,
            },
        )?;
        // 3. Pi
        let launch = Launch {
            guard: Some(ensure_guard(&install)?),
            guard_tools: GUARD_TOOLS
                .iter()
                .chain(HOST_GUARD_TOOLS.iter().filter(|_| host))
                .map(|t| (*t).to_owned())
                .collect(),
            extra_env: guard_env(&paths, &toolhost, host, self.presentation)?,
            provider: provider_of(d, &cfg),
            api_key: Some(key),
            cwd: work.clone(),
            name: None,
            install,
            node,
            paths,
            session,
            helper: exe,
            skills: skills.is_some().then(|| crate::cli::skills::live(d)),
        };
        let pi = start(&launch).map_err(text)?;
        // A conversation: the person answers the guard's questions (`ask_person`).
        pi.client.set_interactive(self.presentation);
        let history = pi.client.get_messages().map_err(text)?;
        // Clio's identity ships with her: written once, on a memory that has none. A memory that
        // cannot be opened for writing now is left as it is (the next session tries again).
        if let Ok(link) = MemoryLink::open_write(&d.data, &EmbedModel::none()) {
            let _ = clio_brain::primordial::seed(&link);
        }
        // What goes in front of every message: the time, the memories, the tools (Jev optional).
        let entries = pi
            .client
            .request("get_entries", json!({}))
            .unwrap_or(Value::Null);
        let pre = jev::preprompt(
            d,
            &cfg.jev,
            &entries,
            &toolhost,
            &pi.session_id,
            &run_dir,
            project.as_deref(),
            // Only a new session: an old one is the middle of a conversation.
            self.presentation && matches!(target, Target::New { .. }),
        );
        pre.set_summaries(cfg.memory.session_summary);
        // A session that was marked before (the service leaves the mark in the run folder) is
        // marked again in the new service: the mark does not go away by closing the chat.
        let taint_file = run_dir.join("toolhost/tainted.json");
        if taint_file.is_file() {
            // A session the judge flagged stays `suspect`: it asks before reaching out.
            let suspect =
                std::fs::read_to_string(&taint_file).is_ok_and(|t| t.contains("\"suspect\""));
            jev::taint_session(&toolhost.socket, &pi.session_id, suspect);
        }
        let mut notes = Vec::new();
        if changed {
            jev::taint_session(&toolhost.socket, &pi.session_id, false);
            notes.push(CHANGED_SINCE_CLOSE.to_owned());
        }
        // A service paused or asking for attention since the last chat (S-69): a quiet note.
        notes.extend(crate::cli::services::take_notices(d));
        let session_file =
            session_file.or_else(|| pi.state.session_file.clone().map(PathBuf::from));
        // What Pi offers this connector, kept for `/model` before a session's first message.
        if let Some(c) = connector::by_provider(&cfg.model.provider)
            && let Ok(v) = pi.client.request("get_available_models", json!({}))
        {
            usage::save_pi_models(d, c.provider, &usage::pi_models_of(&v, c.pi_provider));
        }
        let s = ChatSession {
            pi,
            toolhost,
            dirs: self.dirs.clone(),
            run_dir: run_dir.clone(),
            session_file,
            lock: Mutex::new(Some(lock)),
            down: Mutex::new(false),
            budget: if host {
                host_budget(task_budget(&cfg.limits))
            } else {
                task_budget(&cfg.limits)
            },
            // Copy mode has no daily cap, like the builtin loop; host mode has one per day,
            // kept between sessions.
            day: Arc::new(Mutex::new(if host {
                DayBudget::kept(
                    &state.join("day.json"),
                    cfg.limits.day_cost_micro_usd,
                    cfg.limits.day_steps,
                )
            } else {
                DayBudget::new(u64::MAX)
            })),
            prompts: AtomicU64::new(0),
            assessor: Assessor::new(Arc::new(SystemClock)),
            pre,
            taint_file,
            summarized: AtomicBool::new(false),
            busy: Mutex::new(()),
            id: id.clone(),
            work: work.clone(),
            host_mode: host,
            model: Mutex::new(cfg.model.clone()),
            turn: Mutex::default(),
            findings: Mutex::default(),
            skills,
            skill_fails: Mutex::default(),
            skill_notes: Mutex::default(),
            skill_handed: Mutex::default(),
            // Host mode, or a chat (copy mode): what waits for the person (a secret request).
            host: (host || self.presentation).then(|| HostSide {
                dirs: self.dirs.clone(),
                run: id.clone(),
                asked: Mutex::default(),
                approved: Mutex::default(),
                notes: Mutex::default(),
            }),
        };
        let refresh = s.refresh().ok();
        let presentation = s.pre.presenting();
        Ok(Opened {
            host,
            presentation,
            id: s.pi.session_id.clone(),
            workspace: work.display().to_string(),
            history,
            refresh,
            notes,
            session: Arc::new(s),
        })
    }
}

pub(super) fn provider_of(d: &Dirs, cfg: &Config) -> Provider {
    let m = &cfg.model;
    // `Config::load` guarantees `m.provider` is a connector.
    let mut p = connector::by_provider(&m.provider)
        .expect("a connector")
        .pi_launch_provider(&m.model);
    if let Some(u) = &m.base_url {
        p.base_url.clone_from(u);
    }
    p.thinking.clone_from(&m.thinking);
    p.context_window = usage::window_for(d, &m.provider, &m.model).0;
    // Micro-dollars per million tokens to dollars per million tokens.
    p.dollars_per_mtok = (
        m.price_in_micro_per_mtok as f64 / 1e6,
        m.price_out_micro_per_mtok as f64 / 1e6,
    );
    p
}

/// An open session: Pi, and the toolhost beside it. Field order is the order they are dropped:
/// Pi first, then the toolhost.
/// The caps of one prompt: the configured limits, and the anti-loop and retries of the builtin loop.
pub(super) fn task_budget(limits: &Limits) -> Budget {
    let mut b = AgentConfig::new("pi", "", Path::new("")).budget;
    b.max_steps = limits.max_steps;
    b.max_ms = i64::try_from(limits.max_seconds.saturating_mul(1000)).unwrap_or(i64::MAX);
    b.max_cost_micro = limits.max_cost_micro_usd;
    b
}

/// Host mode: no cap of steps or time per prompt (the day's budget holds instead); the cost per
/// prompt and the anti-loop stay.
fn host_budget(mut b: Budget) -> Budget {
    b.max_steps = u32::MAX;
    b.max_ms = i64::MAX;
    b
}

/// Old run folders go when a new session opens; what could not go is written to
/// `cleanup.log` (the screen may be the TUI's), and the session opens anyway.
fn clean_old_runs(d: &Dirs) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let failed = runs::clean_old(d, crate::cli::now_ms());
    if failed.is_empty() {
        return;
    }
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(d.data.join("cleanup.log"))
        .and_then(|mut f| f.write_all(format!("{}\n", failed.join("\n")).as_bytes()));
}

struct ChatSession {
    pi: PiProcess,
    toolhost: Toolhost,
    dirs: Arc<Dirs>,
    run_dir: PathBuf,
    /// Pi's file of the session, sealed when it closes.
    session_file: Option<PathBuf>,
    /// The run folder's lock, held until the session is shut down (`hold`).
    lock: Mutex<Option<std::fs::File>>,
    down: Mutex<bool>,
    /// S-47: each prompt is a task with these caps; the day's spend is shared by all of them.
    budget: Budget,
    day: Arc<Mutex<DayBudget>>,
    prompts: AtomicU64,
    /// The level of each prompt, decided from the person's words alone (S-07, ADR 0008).
    assessor: Assessor,
    /// The time, memories and tool suggestions put in front of each message (Jev optional).
    pre: PrePrompt,
    /// Where the service leaves the mark when it saw (or was told of) what is not the person's.
    taint_file: PathBuf,
    /// The closing summary was asked for (or skipped): once per session.
    summarized: AtomicBool,
    /// Held while a turn runs: the closing turn never runs beside one (both would read Pi's events).
    busy: Mutex<()>,
    host: Option<HostSide>,
    /// The run, the folder it works on, its mode, connector and model (`/model` changes it):
    /// what a review starts from.
    id: String,
    work: PathBuf,
    host_mode: bool,
    model: Mutex<crate::cli::home::ModelCfg>,
    /// The last turn, for what it changed (`last_work`).
    turn: Mutex<super::review::Turn>,
    /// A review's findings for the next message, and the reviewer's mark.
    findings: Mutex<Option<(String, Option<bool>, &'static str)>>,
    /// Clio's skills folder: a chat's (S-68).
    skills: Option<PathBuf>,
    /// Failed reviews in a row of each skill's drafts, by name.
    skill_fails: Mutex<std::collections::HashMap<String, u32>>,
    /// When the notes of a skill's review last went to the model, by name: once an hour.
    skill_notes: Mutex<std::collections::HashMap<String, i64>>,
    /// The drafts this opening of the session handed out for review.
    skill_handed: Mutex<HashSet<String>>,
}

/// Host mode, or a chat: what the session set aside or asked for, and what the person answered.
struct HostSide {
    dirs: Arc<Dirs>,
    run: String,
    /// The pending actions already put to the person: each is asked once.
    asked: Mutex<HashSet<String>>,
    /// What the actions the person approved did, for the model's next message.
    approved: Mutex<Vec<String>>,
    /// What the person answered to the secrets the model asked for (never a value), for the
    /// model's next message.
    notes: Mutex<Vec<String>>,
}

/// A draft id as the tool host makes them: 8 hex characters.
fn valid_draft_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// What a skill draft's review leads to (S-68).
#[derive(Debug, PartialEq, Eq)]
enum SkillNext {
    Register,
    /// It waits for the person, and why.
    Wait(String),
    Reject,
}

/// The verdict decides, not the findings (`clio-reviewer@3` defines it): `passes` registers it,
/// unless the session (or the reviewer) is marked: then it waits; `could_not_check` always waits;
/// `fails` is rejected.
fn skill_next(a: &clio_brain::pi::review::Answer, marked: bool, suspect: bool) -> SkillNext {
    use clio_brain::pi::review::Verdict;
    match a.verdict {
        Verdict::CouldNotCheck => SkillNext::Wait(format!(
            "its check could not be run here: {}",
            clio_brain::pi::text::line(&a.ran, 300)
        )),
        Verdict::Passes => match (marked, suspect) {
            (_, true) => {
                SkillNext::Wait("this session read content that tried to give orders".to_owned())
            }
            (true, false) => SkillNext::Wait("this session read content from outside".to_owned()),
            (false, false) => SkillNext::Register,
        },
        Verdict::Fails => SkillNext::Reject,
    }
}

/// Not by itself, whatever the review says (S-68): a draft of the skill Clio ships with, or of a
/// name the person removed (and has not brought back).
fn kept_for_person(root: &Path, name: &str) -> Option<String> {
    use crate::cli::skills;
    if skills::is_shipped(name) {
        Some("it changes a skill Clio ships with".to_owned())
    } else if skills::record(root, name).is_none() && skills::was_removed(root, name) {
        Some("you removed a skill of this name".to_owned())
    } else {
        None
    }
}

/// `next`, held for the person when `kept` says why.
fn held(next: SkillNext, kept: Option<String>) -> SkillNext {
    match (next, kept) {
        (SkillNext::Register, Some(why)) => SkillNext::Wait(why),
        (SkillNext::Wait(w), Some(why)) => SkillNext::Wait(format!("{w}; and {why}")),
        (next, _) => next,
    }
}

/// The session's marks for a draft's registration: the draft's own (as the session was when it
/// was written), the mark file's, and the reviewer's (`Some(suspect)`): marked if any is.
fn marks(
    d: &clio_toolhost::skills::Draft,
    file: Option<bool>,
    reviewer: Option<bool>,
) -> (bool, bool) {
    (
        d.marked || file.is_some() || reviewer.is_some(),
        d.suspect || file == Some(true) || reviewer == Some(true),
    )
}

/// A review's notes on `name` go to the model at most once an hour: a version written in answer
/// to notes is reviewed and registered, and its own notes are the person's only.
fn notes_due(sent: &mut std::collections::HashMap<String, i64>, name: &str, now: i64) -> bool {
    if sent.get(name).is_some_and(|at| now - at < HOUR_MS) {
        return false;
    }
    sent.insert(name.to_owned(), now);
    true
}

const HOUR_MS: i64 = 3_600_000;

/// `taken/<id>.done`: the draft's outcome was decided; it is not reviewed again.
fn done_marker(th: &Path, id: &str) -> PathBuf {
    clio_toolhost::skills::taken_dir(th).join(format!("{id}.done"))
}

/// Drafts of `session` taken for review whose outcome was never decided (a quit, `/new` or a
/// crash during their review): reviewed again, in the session that wrote them.
fn unfinished(th: &Path, session: &str) -> Vec<(String, String)> {
    clio_toolhost::skills::drafts_in(&clio_toolhost::skills::taken_dir(th))
        .into_iter()
        .filter(|d| d.session == session && valid_draft_id(&d.id))
        .filter(|d| !done_marker(th, &d.id).exists())
        .map(|d| (d.id, d.name))
        .collect()
}

/// Failed reviews of a skill's drafts, in a row, after which the draft is dropped.
const SKILL_ATTEMPTS: u32 = 3;

/// A review of `name`'s draft ended: which failure in a row it is, and whether the draft is
/// dropped (the third). Only `failed` counts; any other verdict clears the count.
fn skill_count(
    fails: &mut std::collections::HashMap<String, u32>,
    name: &str,
    failed: bool,
) -> (u32, bool) {
    if !failed {
        fails.remove(name);
        return (0, false);
    }
    let attempt = fails.entry(name.to_owned()).or_insert(0);
    *attempt += 1;
    let attempt = *attempt;
    let dropped = attempt >= SKILL_ATTEMPTS;
    if dropped {
        fails.remove(name);
    }
    (attempt, dropped)
}

impl ChatSession {
    /// The mark the session's tool host left in the run folder: `Some(suspect)` when marked.
    fn mark_file(&self) -> Option<bool> {
        std::fs::read_to_string(&self.taint_file)
            .ok()
            .map(|t| t.contains("\"suspect\""))
    }

    /// What a skill draft's review leads to (S-68): `passes` registers it in a clean session (its
    /// findings, if any, go to the model as notes) and sets it aside for the person in a marked
    /// one; `could_not_check` always waits for the person; `fails` gives the findings back to the
    /// model, and the third failure in a row drops it. Registering happens here, in the person's
    /// `clio`.
    fn after_skill_review(
        &self,
        root: &Path,
        h: &HostSide,
        d: &clio_toolhost::skills::Draft,
        rv: &super::Reviewed,
    ) -> Result<super::SkillOutcome, String> {
        if clio_toolhost::services::is_service_draft(&d.id) {
            return self.after_service_review(h, d, rv);
        }
        let a = &rv.answer;
        let name = d.name.clone();
        let (marked, suspect) = marks(d, self.mark_file(), rv.marked);
        let next = held(skill_next(a, marked, suspect), kept_for_person(root, &name));
        let (attempt, dropped) = skill_count(
            &mut locked(&self.skill_fails),
            &name,
            next == SkillNext::Reject,
        );
        let wait = match next {
            SkillNext::Register => None,
            SkillNext::Wait(why) => Some(why),
            SkillNext::Reject => {
                let next = if dropped {
                    // The name is refused by `skill_write` for the next hour.
                    let ban =
                        clio_toolhost::skills::dropped_file(&self.run_dir.join("toolhost"), &name);
                    ban.parent()
                        .map_or(Ok(()), crate::cli::mkdir_private)
                        .and_then(|()| {
                            crate::cli::write_private(
                                &ban,
                                crate::cli::now_ms().to_string().as_bytes(),
                            )
                        })?;
                    "It is dropped, and that name is refused for the next hour: tell the person what failed instead."
                } else {
                    "Fix what the findings say and call skill_write again with the whole skill."
                };
                return Ok(super::SkillOutcome::Rejected {
                    to_model: format!(
                        "Your draft {} of the skill «{name}» was not registered: its review failed (attempt {attempt} of {SKILL_ATTEMPTS}). {next}\n\n{}",
                        d.id,
                        a.for_model()
                    ),
                    name,
                    attempt,
                    dropped,
                });
            }
        };
        let verdict = a.verdict.word();
        let Some(reason) = wait else {
            let n = crate::cli::skills::register(
                root,
                &crate::cli::skills::New {
                    name: &d.name,
                    description: &d.description,
                    skill_md: &d.skill_md,
                    files: &d.files,
                    by: "review",
                    verdict,
                },
            )
            .inspect_err(|e| {
                locked(&h.notes).push(format!(
                    "the skill «{name}» passed its review but could not be registered: {e}"
                ));
            })?;
            locked(&h.notes).push(format!(
                "Clio registered your skill «{name}» (version {n}) after its review passed; it is available from the next session."
            ));
            let notes = (a.goes_to_model()
                && notes_due(&mut locked(&self.skill_notes), &name, crate::cli::now_ms()))
            .then(|| a.for_model());
            return Ok(super::SkillOutcome::Registered {
                name,
                description: d.description.clone(),
                n,
                notes,
            });
        };
        let args = json!({
            "name": d.name,
            "description": d.description,
            "skill_md": d.skill_md,
            "files": d.files,
            "sha256": d.sha256,
            "draft": d.id,
            "update": d.update,
            "verdict": verdict,
        });
        let p = clio_toolhost::pending::set_aside_skill(
            &pending::state(&self.dirs),
            &self.id,
            &self.pi.session_id,
            args,
            &reason,
        )?;
        locked(&h.asked).insert(p.id.clone());
        locked(&h.notes).push(format!(
            "your skill «{name}» was not registered by itself: {reason}. It waits for the person's approval ({}); do not write it again meanwhile.",
            p.id
        ));
        Ok(super::SkillOutcome::Waits {
            ask: Box::new(crate::cli::tui::ask::BlockAsk::new(&p, suspect)),
            id: p.id,
        })
    }

    /// What a service draft's review leads to (S-69): `fails`, or a secret the vault says may not
    /// go to one of its hosts, gives the findings back to the model (the third in a row drops it)
    /// and the person never sees it; any other verdict sets it aside for the person, always, with
    /// the verdict and the review's notes in the box.
    /// Nothing registers a service but the person's approval.
    fn after_service_review(
        &self,
        h: &HostSide,
        d: &clio_toolhost::skills::Draft,
        rv: &super::Reviewed,
    ) -> Result<super::SkillOutcome, String> {
        let a = &rv.answer;
        let name = d.name.clone();
        let (marked, suspect) = marks(d, self.mark_file(), rv.marked);
        // A secret the vault would not let go to one of its hosts: refused now, when it can tell.
        let mismatch = serde_json::from_str::<clio_toolhost::services::Manifest>(&d.skill_md)
            .ok()
            .and_then(|m| crate::cli::services::secret_hosts_refused(&self.dirs, &m));
        let failed = a.verdict == clio_brain::pi::review::Verdict::Fails || mismatch.is_some();
        let (attempt, dropped) = skill_count(&mut locked(&self.skill_fails), &name, failed);
        if failed {
            let next = if dropped {
                let ban =
                    clio_toolhost::skills::dropped_file(&self.run_dir.join("toolhost"), &name);
                ban.parent()
                    .map_or(Ok(()), crate::cli::mkdir_private)
                    .and_then(|()| {
                        crate::cli::write_private(&ban, crate::cli::now_ms().to_string().as_bytes())
                    })?;
                "It is dropped, and that name is refused for the next hour: tell the person what failed instead."
            } else {
                "Fix what the findings say and call service_write again with the whole service."
            };
            return Ok(super::SkillOutcome::ServiceRejected {
                to_model: format!(
                    "Your draft {} of the service «{name}» was not offered to the person: {} (attempt {attempt} of {SKILL_ATTEMPTS}). {next}\n\n{}",
                    d.id,
                    mismatch.as_deref().unwrap_or("its review failed"),
                    a.for_model()
                ),
                name,
                attempt,
                dropped,
            });
        }
        let verdict = a.verdict.word();
        let reason = if marked {
            "a service always waits for your approval; and this session read content from outside"
        } else {
            "a service always waits for your approval"
        };
        let args = json!({
            "name": d.name,
            "description": d.description,
            "skill_md": d.skill_md,
            "files": d.files,
            "sha256": d.sha256,
            "draft": d.id,
            "update": d.update,
            "verdict": verdict,
            "notes": a.for_model(),
        });
        let p = clio_toolhost::pending::set_aside_service(
            &pending::state(&self.dirs),
            &self.id,
            &self.pi.session_id,
            args,
            reason,
        )?;
        locked(&h.asked).insert(p.id.clone());
        locked(&h.notes).push(format!(
            "your service «{name}» passed to the person: it waits for their approval ({}); do not write it again meanwhile.",
            p.id
        ));
        Ok(super::SkillOutcome::Waits {
            ask: Box::new(crate::cli::tui::ask::BlockAsk::new(&p, suspect)),
            id: p.id,
        })
    }
}

/// A turn with no words of the person, no findings and no presentation is one the approvals
/// started: it goes with Clio's request (`Some(true)`), or is no turn at all when there is
/// nothing to give (`None`: the outcomes went with another message). Any other: `Some(false)`.
fn approvals_turn(text: &str, findings: bool, presenting: bool, data: &str) -> Option<bool> {
    if !text.is_empty() || findings || presenting {
        return Some(false);
    }
    (!data.is_empty()).then_some(true)
}

fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A session with fewer messages of the person than this closes without a summary.
const SUMMARY_AFTER_PROMPTS: u64 = 4;
/// The closing turn is short: a few steps and this long at most.
const SUMMARY_STEPS: u32 = 4;
const SUMMARY_MS: i64 = 30_000;

impl Session for ChatSession {
    fn run_prompt(
        &self,
        text: &str,
        cancel: &Cancel,
        on_event: &mut dyn FnMut(&Event),
    ) -> Result<RunSummary, PiError> {
        let _turn = self
            .busy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A review's findings: data from another agent, with Clio's request to answer them. A
        // reviewer that read what is not the person's marks this session the same way, and the
        // judge reads them first; a mark that cannot be made means they are not given.
        let findings = match locked(&self.findings).take() {
            Some((f, marked, ask)) => Some((
                jev::findings_for_model(&self.toolhost.socket, &self.pi.session_id, &f, marked)
                    .map_err(PiError::Io)?,
                ask,
            )),
            None => None,
        };
        let n = self.prompts.fetch_add(1, Ordering::Relaxed);
        let mut guard = RunGuard::new(
            Arc::new(SystemClock),
            &format!("{}-{n}", self.pi.session_id),
            self.budget,
            self.day.clone(),
        );
        let (approved, notes) = self
            .host
            .as_ref()
            .map(|h| {
                (
                    std::mem::take(&mut *locked(&h.approved)),
                    std::mem::take(&mut *locked(&h.notes)),
                )
            })
            .unwrap_or_default();
        if !approved.is_empty() {
            // What an approved action printed is outside text, read by a service of its own:
            // the session is marked here, as its own toolhost would.
            jev::taint_session(&self.toolhost.socket, &self.pi.session_id, false);
        }
        let data = approved
            .into_iter()
            .chain(notes)
            .collect::<Vec<_>>()
            .join("\n\n");
        match approvals_turn(text, findings.is_some(), self.pre.presenting(), &data) {
            Some(true) => self.pre.reviewed(APPROVALS_ASK),
            Some(false) => {}
            None => {
                return Ok(RunSummary {
                    handled: true,
                    ..RunSummary::default()
                });
            }
        }
        let mut given = Vec::new();
        if !data.is_empty() {
            given.push((Source::ToolResult, data.as_str()));
        }
        if let Some((f, ask)) = &findings {
            self.pre.reviewed(ask);
            given.push((Source::SubagentSummary, f.as_str()));
        }
        let mut turn = super::review::Turn {
            request: text.to_owned(),
            start_ms: crate::cli::now_ms(),
            ..Default::default()
        };
        let mut watch = |e: &Event| {
            if let Event::ToolStart { id, name, args } = e {
                turn.calls.push((id.clone(), name.clone(), args.clone()));
            }
            on_event(e);
        };
        let r = run_assessed_with(
            &self.pi.client,
            &self.assessor,
            &self.pre,
            text,
            &given,
            cancel,
            &mut guard,
            &mut watch,
        );
        turn.end_ms = crate::cli::now_ms();
        if let Ok(s) = &r {
            turn.answer.clone_from(&s.text);
        }
        *locked(&self.turn) = turn;
        r
    }

    fn last_work(&self) -> Option<super::TurnWork> {
        let turn = locked(&self.turn).clone();
        let state = self.dirs.data.join("host");
        super::review::work_of(
            &turn,
            &self.pi.session_id,
            self.host_mode
                .then_some((state.as_path(), self.id.as_str())),
            &self.run_dir.join("toolhost/effects.jsonl"),
        )
    }

    fn needs_review(&self, work: &super::TurnWork) -> super::review::Judged {
        let j = super::review::needs_review(&self.dirs, work);
        super::review::record(&self.run_dir, &j);
        j
    }

    fn review(
        &self,
        req: &super::ReviewRequest,
        cancel: &Cancel,
    ) -> Result<super::Reviewed, String> {
        let marked = std::fs::read_to_string(&self.taint_file)
            .ok()
            .map(|t| t.contains("\"suspect\""));
        let model = locked(&self.model).clone();
        let parent = super::review::Parent {
            dirs: &self.dirs,
            run: &self.id,
            run_dir: &self.run_dir,
            work: &self.work,
            host: self.host_mode,
            marked,
            day: self.day.clone(),
            model: &model,
        };
        Ok(super::review::run(&parent, req, cancel))
    }

    fn queue_findings(&self, findings: &str, marked: Option<bool>, ask: &'static str) {
        *locked(&self.findings) = Some((findings.to_owned(), marked, ask));
    }

    fn skill_news(&self) -> (Vec<(String, String)>, Vec<String>) {
        let Some(root) = &self.skills else {
            return (vec![], vec![]);
        };
        let th = self.run_dir.join("toolhost");
        let notes = crate::cli::skills::apply_reports(root, &th);
        let dir = clio_toolhost::skills::drafts_dir(&th);
        let taken = clio_toolhost::skills::taken_dir(&th);
        let mut handed = locked(&self.skill_handed);
        // Taken before and never answered: once per opening of the session.
        let mut out: Vec<(String, String)> = unfinished(&th, &self.pi.session_id)
            .into_iter()
            .filter(|(id, _)| handed.insert(id.clone()))
            .collect();
        for d in clio_toolhost::skills::drafts(&th) {
            if d.session != self.pi.session_id || !valid_draft_id(&d.id) {
                continue;
            }
            // Taken once: a draft is reviewed once, unless its review never ended.
            let from = dir.join(format!("{}.json", d.id));
            if crate::cli::mkdir_private(&taken).is_ok()
                && std::fs::rename(&from, taken.join(format!("{}.json", d.id))).is_ok()
            {
                handed.insert(d.id.clone());
                out.push((d.id, d.name));
            }
        }
        (out, notes)
    }

    fn review_skill(&self, id: &str, cancel: &Cancel) -> Result<super::Reviewed, String> {
        let root = self
            .skills
            .as_deref()
            .ok_or("skills are not available in this session")?;
        let h = self.host.as_ref().ok_or("nobody to ask in this session")?;
        if !valid_draft_id(id) {
            return Err(format!("{id:?} is not a draft"));
        }
        let th = self.run_dir.join("toolhost");
        let file = clio_toolhost::skills::taken_dir(&th).join(format!("{id}.json"));
        // Decided (or not reviewable): it is not reviewed again.
        let done = || crate::cli::write_private(&done_marker(&th, id), b"");
        let d: clio_toolhost::skills::Draft = match std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
        {
            Some(d) => d,
            None => {
                done()?;
                return Err(format!("draft {id} is gone or damaged"));
            }
        };
        // What is reviewed, shown and registered is exactly what the judge read.
        if d.id != id
            || d.session != self.pi.session_id
            || clio_toolhost::skills::digest(&d.skill_md, &d.files) != d.sha256
        {
            done()?;
            return Err(format!("draft {id} changed after it was stored: dropped"));
        }
        // A service's draft (its id says so) must be a service's, a skill's a skill's.
        let stored = if clio_toolhost::services::is_service_draft(&d.id) {
            clio_toolhost::services::check_stored(&d.name, &d.skill_md, &d.files).map(drop)
        } else {
            clio_toolhost::skills::check_stored(&d.name, &d.skill_md, &d.files)
        };
        if let Err(e) = stored {
            done()?;
            return Err(e);
        }
        // The reviewer is marked like the session was when the draft was written, or is now.
        let (m, suspect) = marks(&d, self.mark_file(), None);
        let model = locked(&self.model).clone();
        let parent = super::review::Parent {
            dirs: &self.dirs,
            run: &self.id,
            run_dir: &self.run_dir,
            work: &self.work,
            host: self.host_mode,
            marked: m.then_some(suspect),
            day: self.day.clone(),
            model: &model,
        };
        let mut rv = super::review::run_skill(&parent, &d, cancel);
        if cancel.is_cancelled() {
            // Not decided: reviewed again when this session is opened again.
            return Err(format!(
                "the review of «{}» was stopped; it runs again when this session is opened again",
                d.name
            ));
        }
        let outcome = self.after_skill_review(root, h, &d, &rv);
        done()?;
        rv.skill = Some(outcome?);
        Ok(rv)
    }

    fn new_pending(&self) -> Vec<Pending> {
        let Some(h) = &self.host else {
            return Vec::new();
        };
        let mut asked = locked(&h.asked);
        pending::waiting(&h.dirs)
            .into_iter()
            .filter(|p| p.run == h.run && p.session == self.pi.session_id)
            .filter(|p| asked.insert(p.id.clone()))
            .collect()
    }

    fn waiting(&self) -> Vec<Pending> {
        let Some(h) = &self.host else {
            return Vec::new();
        };
        let mut asked = locked(&h.asked);
        pending::waiting(&h.dirs)
            .into_iter()
            .filter(|p| p.run == h.run && p.session == self.pi.session_id)
            .inspect(|p| {
                asked.insert(p.id.clone());
            })
            .collect()
    }

    fn answer(&self, id: &str, yes: bool) -> Result<String, String> {
        // One already put to the person, or one of this session still waiting (`/pending`).
        let h = self
            .host
            .as_ref()
            .filter(|h| {
                locked(&h.asked).contains(id)
                    || pending::waiting(&h.dirs)
                        .iter()
                        .any(|p| p.id == id && p.run == h.run && p.session == self.pi.session_id)
            })
            .ok_or_else(|| format!("{id} is not of this session"))?;
        if pending::waiting(&h.dirs)
            .iter()
            .any(|p| p.id == id && clio_toolhost::pending::secret_of(p).is_some())
        {
            return Err(format!("{id} asks for a secret: answer it in its own box"));
        }
        // A block change or a skill: the model hears what came of it with the next message, as
        // of a secret.
        let block = pending::waiting(&h.dirs)
            .iter()
            .find(|p| p.id == id)
            .and_then(|p| {
                clio_toolhost::pending::block_of(p)
                    .map(|b| format!("nothing was written to «{}»", b.name))
                    .or_else(|| {
                        clio_toolhost::pending::skill_of(p)
                            .map(|s| format!("the skill «{}» is not registered", s.0))
                    })
                    .or_else(|| {
                        clio_toolhost::pending::service_of(p)
                            .map(|s| format!("the service «{}» is not registered", s.0))
                    })
            });
        if let Some(refused) = block {
            let note = if yes {
                match pending::approve(&h.dirs, id) {
                    Ok(p) => match &p.state {
                        clio_toolhost::pending::State::Done { output_tail, .. } => {
                            format!("approved {id}: {output_tail}")
                        }
                        _ => format!("approved {id}"),
                    },
                    Err(e) => {
                        locked(&h.notes).push(format!(
                            "the person approved {id}, but nothing was written: {e}"
                        ));
                        return Err(format!("{id}: {e}"));
                    }
                }
            } else {
                pending::reject(&h.dirs, id)?;
                format!("rejected {id}: {refused}")
            };
            locked(&h.notes).push(format!("the person {note}"));
            return Ok(note);
        }
        if !yes {
            pending::reject(&h.dirs, id)?;
            let note = format!("rejected {id}: it is not run");
            locked(&h.notes).push(format!("the person {note}"));
            return Ok(note);
        }
        // Said yes, but it could not be done: an outcome too, told as such.
        let p = pending::approve(&h.dirs, id).inspect_err(|e| {
            locked(&h.notes).push(format!(
                "the person approved {id}, but it could not be done: {e}"
            ));
        })?;
        let what = pending::outcome(&p);
        // What it printed is outside text: the judge reads it before the model does.
        let (shown, flagged) = jev::judged_for_model(
            &self.toolhost.socket,
            &self.pi.session_id,
            &what,
            clio_toolhost::pending::shown(&p),
        )
        .unwrap_or_else(|| (what.clone(), false));
        locked(&h.approved).push(format!("the person approved {id}: {shown}"));
        Ok(if flagged {
            format!(
                "approved {id}: {what}\n(Clio's judge flagged this output: the agent was not shown it)"
            )
        } else {
            format!("approved {id}: {what}")
        })
    }

    fn answer_secret(&self, id: &str, value: Option<&SecretValue>) -> Result<String, String> {
        let h = self
            .host
            .as_ref()
            .filter(|h| locked(&h.asked).contains(id))
            .ok_or_else(|| format!("{id} is not of this session"))?;
        let note = pending::answer_secret(&h.dirs, id, value)?;
        locked(&h.notes).push(note.clone());
        Ok(note)
    }

    fn suspect(&self) -> bool {
        self.mark_file() == Some(true)
    }

    fn steer(&self, text: &str) -> Result<(), PiError> {
        self.pi.client.steer(text)
    }

    fn ui_respond(&self, id: &str, value: Option<&str>) {
        // Best effort: a Pi that is gone has nobody waiting for the answer.
        let _ = self.pi.client.ui_respond(id, value);
    }

    fn refresh(&self) -> Result<Refresh, PiError> {
        let stats = self.pi.client.get_session_stats()?;
        // The guard writes `clio-guard.taint` into the session when it read outside content.
        let entries = self.pi.client.request("get_entries", json!({}))?;
        let tainted = self.taint_file.is_file()
            || entries["entries"]
                .as_array()
                .is_some_and(|a| a.iter().any(is_taint));
        let all = entries["entries"].as_array().map_or(&[][..], Vec::as_slice);
        let masked = masked_total(all);
        let egress_denied = self
            .pi
            .jail
            .as_ref()
            .map(clio_brain::pi::jail::PiJail::egress_denied)
            .unwrap_or_default();
        Ok(Refresh {
            stats,
            tainted,
            egress_denied,
            masked,
            withheld: withheld(all),
            judge_silent: all.iter().any(|e| e["customType"] == "clio-guard.judge"),
            jobs: jev::jobs_running(&self.toolhost.socket, &self.pi.session_id),
        })
    }

    fn summarize(&self) {
        if self.summarized.swap(true, Ordering::SeqCst)
            || self.prompts.load(Ordering::Relaxed) < SUMMARY_AFTER_PROMPTS
            || self.down.lock().map_or(true, |d| *d)
        {
            return;
        }
        // A turn still running (the person left in the middle of one): no summary.
        let Ok(_turn) = self.busy.try_lock() else {
            return;
        };
        let Some(text) = self
            .pre
            .closing_text(&clio_brain::pi::envelope::stamp_now())
        else {
            return;
        };
        // The service first narrows the session to the summary: a closing turn that cannot be
        // narrowed is not run (the model would have every tool and nobody watching).
        if !jev::closing_to_host(
            &self.toolhost.socket,
            &self.pi.session_id,
            &self.toolhost.said_token,
        ) {
            return;
        }
        let mut budget = self.budget;
        budget.max_steps = budget.max_steps.min(SUMMARY_STEPS);
        budget.max_ms = budget.max_ms.min(SUMMARY_MS);
        let mut guard = RunGuard::new(
            Arc::new(SystemClock),
            &format!("{}-cierre", self.pi.session_id),
            budget,
            self.day.clone(),
        );
        // Best effort: a Pi still busy with a turn, or a model that does not answer in time, closes
        // the session without a summary.
        let _ = self
            .pi
            .client
            .run_prompt_limited(&text, &Cancel::new(), &mut guard, &mut |_| {});
    }

    fn command(&self, kind: &str, fields: Value) -> Result<Value, String> {
        // Never beside a turn: both would read Pi's events, and a turn's model must not change
        // under it.
        let Ok(_idle) = self.busy.try_lock() else {
            return Err("the agent is working: wait for the turn to end".into());
        };
        if self.down.lock().map_or(true, |d| *d) {
            return Err("the session is already closed".into());
        }
        let model = (kind == "set_model")
            .then(|| fields["modelId"].as_str().map(str::to_owned))
            .flatten();
        let r = if kind == "compact" {
            level::compact(
                &self.pi.client,
                &self.pre,
                fields["customInstructions"].as_str(),
            )
        } else {
            self.pi.client.request(kind, fields)
        };
        // The session's model now: a review falls back to it.
        if let (Ok(_), Some(m)) = (&r, model) {
            locked(&self.model).model = m;
        }
        r.map_err(text)
    }

    fn abort_compaction(&self) {
        // Best effort: a compaction that ended meanwhile has nothing to cancel.
        let _ = self.pi.client.abort();
    }

    fn shutdown(&self) {
        if let Ok(mut g) = self.down.lock() {
            if std::mem::replace(&mut *g, true) {
                return;
            }
            self.pi.client.kill();
            // Reaped, so nothing more reaches the file before it is sealed.
            self.pi.client.shutdown();
            self.toolhost.stop();
            seal(&self.dirs, &self.run_dir, self.session_file.as_deref());
            // Released here, not when the last handle goes: the same conversation may reopen now.
            locked(&self.lock).take();
        }
    }
}

impl Drop for ChatSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// What `open` says when another `clio` holds the conversation.
pub const OPEN_ELSEWHERE: &str = "that conversation is open in another terminal";

const CHANGED_SINCE_CLOSE: &str = "the session file is not as Clio left it when it closed: what it holds is treated as outside content";

/// Where Clio writes how she left a session's file (size and SHA-256), in its run folder.
const SEAL: &str = "session-seal.json";

/// Made the first time a seal is written: a run with no seal whose file changed after it predates
/// the seals only if the change does too.
const SEALS_SINCE: &str = "sessions-sealed";

/// Holds an advisory lock on the run folder for as long as the file lives; `OPEN_ELSEWHERE` when
/// another `clio` holds it.
fn hold(run_dir: &Path) -> Result<std::fs::File, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(run_dir.join("session.lock"))
        .map_err(text)?;
    // 0600 whatever the umask (S-23).
    crate::cli::set_mode(&run_dir.join("session.lock"), 0o600)?;
    rustix::fs::flock(&f, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .map_err(|_| OPEN_ELSEWHERE.to_owned())?;
    Ok(f)
}

/// Size and SHA-256 of `file`; a missing file is empty.
fn digest(file: &Path) -> (u64, String) {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let (mut h, mut size, mut buf) = (Sha256::new(), 0u64, vec![0u8; 1 << 16]);
    if let Ok(mut f) = std::fs::File::open(file) {
        while let Ok(n) = f.read(&mut buf) {
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            size += n as u64;
        }
    }
    (
        size,
        h.finalize().iter().map(|b| format!("{b:02x}")).collect(),
    )
}

/// Writes the seal of `file` as it is now (an empty one when there is no file yet). Written when
/// the session opens too, so a session Clio never closed does not match it.
fn seal(d: &Dirs, run_dir: &Path, file: Option<&Path>) {
    use std::os::unix::fs::OpenOptionsExt;
    let (size, sha256) = file.map_or_else(|| digest(Path::new("")), digest);
    let since = d.data.join(SEALS_SINCE);
    // Made once (its time is what counts), 0600 whatever the umask (S-23).
    if std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&since)
        .is_ok()
    {
        let _ = crate::cli::set_mode(&since, 0o600);
    }
    let _ = crate::cli::write_private(
        &run_dir.join(SEAL),
        json!({"size": size, "sha256": sha256})
            .to_string()
            .as_bytes(),
    );
}

/// The session's file is not as Clio sealed it, or, in a run with no seal, it changed after Clio
/// started sealing. A run from before the seals, untouched since, is not a mismatch.
fn changed_since_close(d: &Dirs, run_dir: &Path, file: &Path) -> bool {
    match std::fs::read(run_dir.join(SEAL)) {
        Ok(b) => {
            let (size, sha256) = digest(file);
            serde_json::from_slice::<Value>(&b).map_or(true, |v| {
                v["size"] != size || v["sha256"] != sha256.as_str()
            })
        }
        Err(_) => {
            let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
            matches!((mtime(file), mtime(&d.data.join(SEALS_SINCE))), (Some(f), Some(s)) if f > s)
        }
    }
}

/// How many values the host struck out of what Pi read, from the guard's `clio-guard.masked`
/// entries (one per read that had any).
fn masked_total(entries: &[Value]) -> u32 {
    entries
        .iter()
        .filter(|e| e["customType"] == "clio-guard.masked")
        .filter_map(|e| e["data"]["count"].as_u64())
        .fold(0u32, |a, n| {
            a.saturating_add(u32::try_from(n).unwrap_or(u32::MAX))
        })
}

/// What the judge withheld, from the guard's `clio-guard.withheld` entries.
fn withheld(entries: &[Value]) -> Vec<super::Withheld> {
    entries
        .iter()
        .filter(|e| e["customType"] == "clio-guard.withheld")
        .map(|e| {
            let d = &e["data"];
            let s = |k: &str| d[k].as_str().unwrap_or("").to_owned();
            super::Withheld {
                tool: s("tool"),
                source: s("source"),
                bytes: d["bytes"].as_u64().unwrap_or(0),
                file: d["file"].as_str().map(str::to_owned),
            }
        })
        .collect()
}

fn is_taint(e: &Value) -> bool {
    e["customType"] == "clio-guard.taint" && e["data"]["tainted"] == true
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // S-68: the verdict decides: a review that passes registers a skill by itself, with or without
    // findings, and only in a clean session; a check that could not run, or a marked session,
    // waits for the person; only `fails` is rejected.
    #[test]
    fn a_skill_registers_by_itself_only_when_it_passed_in_a_clean_session() {
        use clio_brain::pi::review::parse;
        let a = |verdict: &str, findings: &str| {
            parse(&format!(
                "```review\nverdict: {verdict}\nran: bash check.sh (exit 0)\nfindings:{findings}\n```"
            ))
        };
        let clean = a("passes", " none");
        let found = a(
            "passes",
            "\n- what: it breaks on spaces\n  evidence: ran it on 'a b'",
        );
        assert_eq!(skill_next(&clean, false, false), SkillNext::Register);
        assert!(
            matches!(skill_next(&clean, true, false), SkillNext::Wait(w) if w.contains("outside"))
        );
        assert!(
            matches!(skill_next(&clean, true, true), SkillNext::Wait(w) if w.contains("orders"))
        );
        assert_eq!(skill_next(&found, false, false), SkillNext::Register);
        assert!(matches!(
            skill_next(&found, true, false),
            SkillNext::Wait(_)
        ));
        assert_eq!(
            skill_next(
                &a("fails", "\n- what: the check fails\n  evidence: exit 1"),
                false,
                false
            ),
            SkillNext::Reject
        );
        assert_eq!(
            skill_next(&a("fails", " none"), false, false),
            SkillNext::Reject
        );
        for marked in [false, true] {
            assert!(matches!(
                skill_next(&a("could_not_check", " none"), marked, false),
                SkillNext::Wait(w) if w.starts_with("its check could not be run")
            ));
        }
        // Only `fails` counts toward the three: the third in a row drops the draft, and a pass
        // after two failures registers it and clears the count.
        let mut fails = std::collections::HashMap::new();
        let mut review = |a: &clio_brain::pi::review::Answer| {
            let next = skill_next(a, false, false);
            let failed = next == SkillNext::Reject;
            (next, skill_count(&mut fails, "x", failed))
        };
        let failed = a("fails", "\n- what: the check fails\n  evidence: exit 1");
        assert_eq!(review(&failed).1, (1, false));
        assert_eq!(review(&failed).1, (2, false));
        assert_eq!(review(&found), (SkillNext::Register, (0, false)));
        assert_eq!(review(&failed).1, (1, false));
        assert_eq!(review(&failed).1, (2, false));
        assert_eq!(review(&failed), (SkillNext::Reject, (3, true)));
        assert_eq!(review(&failed).1, (1, false), "a dropped draft starts over");
        // An answer that is not the fixed shape is `could_not_check`: it never registers.
        assert!(matches!(
            skill_next(&parse("looks fine to me"), false, false),
            SkillNext::Wait(_)
        ));
    }

    fn draft(name: &str, session: &str, id: &str) -> clio_toolhost::skills::Draft {
        clio_toolhost::skills::Draft {
            id: id.into(),
            session: session.into(),
            name: name.into(),
            description: "d".into(),
            reason: "r".into(),
            verify: "v".into(),
            skill_md: "md".into(),
            files: vec![],
            created_ms: 1,
            update: false,
            sha256: String::new(),
            marked: false,
            suspect: false,
        }
    }

    // S-68: the draft's own marks count: with the mark file gone, a draft written in a marked
    // session still waits for the person.
    #[test]
    fn a_marked_draft_waits_even_without_the_mark_file() {
        use clio_brain::pi::review::parse;
        let passes = parse("```review\nverdict: passes\nran: bash t.sh\nfindings: none\n```");
        let mut d = draft("x", "s", "a1");
        let next = |d: &clio_toolhost::skills::Draft, file, rv| {
            let (m, s) = marks(d, file, rv);
            skill_next(&passes, m, s)
        };
        assert_eq!(next(&d, None, None), SkillNext::Register);
        d.marked = true;
        assert!(matches!(next(&d, None, None), SkillNext::Wait(w) if w.contains("outside")));
        d.suspect = true;
        assert!(matches!(next(&d, None, None), SkillNext::Wait(w) if w.contains("orders")));
        // Any one of the three marks.
        let clean = draft("x", "s", "a1");
        assert!(matches!(
            next(&clean, Some(false), None),
            SkillNext::Wait(_)
        ));
        assert!(
            matches!(next(&clean, None, Some(true)), SkillNext::Wait(w) if w.contains("orders"))
        );
    }

    // S-68: the shipped skill's name, or one the person removed, never registers by itself.
    #[test]
    fn the_shipped_skill_and_a_removed_name_wait_for_the_person() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("skills");
        let bundled = kept_for_person(&root, crate::cli::skills::BUNDLED).unwrap();
        assert!(bundled.contains("ships with"), "{bundled}");
        assert_eq!(kept_for_person(&root, "pdf-tools"), None);
        std::fs::create_dir_all(root.join("removed/pdf-tools-1700000000000")).unwrap();
        let removed = kept_for_person(&root, "pdf-tools").unwrap();
        assert!(removed.contains("removed"), "{removed}");
        assert_eq!(kept_for_person(&root, "pdf"), None);
        assert_eq!(
            held(SkillNext::Register, Some(bundled.clone())),
            SkillNext::Wait(bundled.clone())
        );
        assert!(matches!(
            held(SkillNext::Wait("its check could not be run here".into()), Some(bundled)),
            SkillNext::Wait(w) if w.contains("could not") && w.contains("ships with")
        ));
        assert_eq!(held(SkillNext::Reject, Some("x".into())), SkillNext::Reject);
        assert_eq!(held(SkillNext::Register, None), SkillNext::Register);
    }

    // A turn with no words of the person is the approvals' only with what came of them to give;
    // with nothing, there is no turn. Words, findings or a presentation make it another turn.
    #[test]
    fn an_empty_turn_is_the_approvals_only_with_outcomes_to_give() {
        assert_eq!(
            approvals_turn("", false, false, "the person approved a1"),
            Some(true)
        );
        assert_eq!(approvals_turn("", false, false, ""), None);
        assert_eq!(approvals_turn("and now?", false, false, "x"), Some(false));
        assert_eq!(approvals_turn("", true, false, ""), Some(false));
        assert_eq!(approvals_turn("", false, true, ""), Some(false));
    }

    // S-68: a review's notes on a skill go to the model once an hour per name, so a version
    // written in answer to notes cannot start another round by itself.
    #[test]
    fn a_skills_notes_go_to_the_model_once_an_hour() {
        let mut sent = std::collections::HashMap::new();
        assert!(notes_due(&mut sent, "x", 1_000));
        assert!(!notes_due(&mut sent, "x", 2_000));
        assert!(notes_due(&mut sent, "y", 2_000));
        assert!(!notes_due(&mut sent, "x", 1_000 + HOUR_MS - 1));
        assert!(notes_due(&mut sent, "x", 1_000 + HOUR_MS));
    }

    // S-68: a draft taken for review whose outcome was never decided is reviewed again when its
    // session is opened again; one decided, or of another session, is not.
    #[test]
    fn a_taken_draft_without_its_outcome_is_queued_again() {
        let t = tempfile::tempdir().unwrap();
        let th = t.path().join("toolhost");
        let taken = clio_toolhost::skills::taken_dir(&th);
        std::fs::create_dir_all(&taken).unwrap();
        for d in [
            draft("open", "s", "a1"),
            draft("decided", "s", "a2"),
            draft("theirs", "other", "a3"),
        ] {
            std::fs::write(
                taken.join(format!("{}.json", d.id)),
                serde_json::to_string(&d).unwrap(),
            )
            .unwrap();
        }
        std::fs::write(done_marker(&th, "a2"), "").unwrap();
        assert_eq!(unfinished(&th, "s"), vec![("a1".into(), "open".into())]);
        assert!(unfinished(&t.path().join("none"), "s").is_empty());
    }

    // S-47: the Pi path gets the same caps as the builtin loop, from the same configuration.
    #[test]
    fn a_prompt_is_a_task_with_the_configured_caps_and_the_builtin_anti_loop() {
        let b = task_budget(&Limits {
            max_steps: 40,
            max_seconds: 600,
            max_cost_micro_usd: 100_000,
            day_cost_micro_usd: 0,
            day_steps: 0,
        });
        assert_eq!(
            (b.max_steps, b.max_ms, b.max_cost_micro, b.max_repeats),
            (40, 600_000, 100_000, 4)
        );
    }

    // `/effort` saves `model.thinking`: a new session starts Pi with it (`low` when unset).
    #[test]
    fn the_thinking_level_of_the_config_reaches_pis_arguments() {
        let t = tempfile::tempdir().unwrap();
        let (src, _) = local_source(t.path());
        let mut cfg: Config = serde_json::from_value(json!({
            "model": {"provider": "anthropic", "model": "claude-sonnet-5-5", "prompt": "x@1",
                "price_in_micro_per_mtok": 0, "price_out_micro_per_mtok": 0},
            "limits": {"max_steps": 1, "max_seconds": 1, "max_cost_micro_usd": 1},
            "workspace": {"max_files": 1, "max_bytes": 1}
        }))
        .unwrap();
        assert_eq!(provider_of(&src.dirs, &cfg).thinking, "low");
        cfg.model.thinking = "high".into();
        // `hardened_args` puts `provider.thinking` after `--thinking` (pinned in `pi::launch`).
        assert_eq!(provider_of(&src.dirs, &cfg).thinking, "high");
    }

    // S-16, S-22 (second audit, finding 1): a session file is written by Pi, which is jailed but
    // not trusted. Its header id must not turn into a path outside `runs/`.
    fn local_source(home: &Path) -> (LocalSource, PathBuf) {
        let data = home.join("data");
        let dirs = Dirs {
            config: home.join("config"),
            runs: data.join("runs"),
            data: data.clone(),
            runtime: None,
            home: home.to_path_buf(),
        };
        let sessions = crate::cli::pi::paths(&dirs).sessions_dir();
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::create_dir_all(&dirs.runs).unwrap();
        (LocalSource::new(Arc::new(dirs)), sessions)
    }

    fn plant(sessions: &Path, file: &str, id: &str, cwd: &Path) {
        let header = json!({"type":"session","version":3,"id":id,
            "timestamp":"2026-10-31T12:00:00.000Z","cwd":cwd});
        std::fs::write(sessions.join(file), format!("{header}\n")).unwrap();
    }

    fn availability(src: &LocalSource) -> Vec<bool> {
        src.list().iter().map(|r| r.unavailable.is_none()).collect()
    }

    #[test]
    fn a_session_whose_id_is_a_path_is_not_a_working_copy_of_clio() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().canonicalize().unwrap();
        let (src, sessions) = local_source(&home);
        // Control: a real run folder with a valid id is ours.
        let run = home.join("data/runs/20261031-aaaa");
        std::fs::create_dir_all(run.join("sandbox")).unwrap();
        plant(
            &sessions,
            "t_good.jsonl",
            "20261031-aaaa",
            &run.join("sandbox"),
        );
        assert_eq!(availability(&src), [true], "the control must be openable");
        // A folder of the person's that happens to end in `sandbox`.
        let proj = home.join("proj");
        std::fs::create_dir_all(proj.join("sandbox")).unwrap();
        // An absolute id replaces the whole path in `Path::join`.
        plant(
            &sessions,
            "t_abs.jsonl",
            proj.to_str().unwrap(),
            &proj.join("sandbox"),
        );
        // `..` climbs out of `runs/`.
        plant(
            &sessions,
            "t_dots.jsonl",
            "../../../proj",
            &proj.join("sandbox"),
        );
        let rows = src.list();
        let bad: Vec<_> = rows
            .iter()
            .filter(|r| {
                r.reference.ends_with("t_abs.jsonl") || r.reference.ends_with("t_dots.jsonl")
            })
            .collect();
        assert_eq!(bad.len(), 2);
        assert!(
            bad.iter().all(|r| r.unavailable.is_some()),
            "a forged id made a folder of the person's look like a working copy"
        );
    }

    // `clio` reopens the first session the drawer lists that is Clio's, of the mode a new session
    // would have and, in copy mode, of this project; with none, nothing.
    #[test]
    fn the_last_conversation_is_the_newest_of_this_mode_and_place() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().canonicalize().unwrap();
        let (_, sessions) = local_source(&home);
        let (proj, other) = (home.join("proj"), home.join("other"));
        let run = |id: &str, host: bool, ws: &Path| {
            let d = home.join("data/runs").join(id);
            std::fs::create_dir_all(d.join("sandbox")).unwrap();
            std::fs::create_dir_all(ws).unwrap();
            let meta = json!({"id": id, "workspace": ws, "instruction": "x", "created_ms": 0,
                "host": host});
            std::fs::write(d.join("meta.json"), meta.to_string()).unwrap();
            d.join("sandbox")
        };
        // Newest first.
        plant(&sessions, "a.jsonl", "20261101-ffff", &proj);
        plant(
            &sessions,
            "b.jsonl",
            "20261101-cccc",
            &run("20261101-cccc", false, &other),
        );
        plant(
            &sessions,
            "c.jsonl",
            "20261101-bbbb",
            &run("20261101-bbbb", false, &proj),
        );
        run("20261101-aaaa", true, &home);
        plant(&sessions, "d.jsonl", "20261101-aaaa", &home);
        run("20261031-0000", true, &home);
        plant(&sessions, "e.jsonl", "20261031-0000", &home);
        for (n, f) in ["e", "d", "c", "b", "a"].iter().enumerate() {
            let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000 * (n as u64 + 1));
            std::fs::File::options()
                .write(true)
                .open(sessions.join(format!("{f}.jsonl")))
                .unwrap()
                .set_modified(at)
                .unwrap();
        }
        let file = |f: &str| Some(sessions.join(f).display().to_string());
        let host = local_source(&home).0.with_mode(Some(Mode::Host));
        assert_eq!(host.last(&proj), file("d.jsonl"));
        let copy = local_source(&home).0.with_mode(Some(Mode::Copy));
        assert_eq!(copy.last(&proj), file("c.jsonl"));
        assert_eq!(copy.last(&other), file("b.jsonl"));
        assert_eq!(copy.last(&home.join("nowhere")), None);
        assert_eq!(copy.last(&home), None);
    }

    // A session file is Pi's to write: reopened, it must be as Clio left it when it closed. A run
    // from before the seals, untouched since, is not a mismatch; one touched after Clio started
    // sealing is.
    #[test]
    fn a_session_file_changed_after_it_was_closed_is_caught() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().canonicalize().unwrap();
        let (src, sessions) = local_source(&home);
        let d = &*src.dirs;
        let (run, file) = (d.runs.join("20261118-aaaa"), sessions.join("a.jsonl"));
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(&file, "{\"type\":\"session\"}\n").unwrap();
        // Old run, no seal anywhere: not a mismatch.
        assert!(!changed_since_close(d, &run, &file));
        // Closed cleanly, reopened untouched: no mark.
        seal(d, &run, Some(&file));
        assert!(!changed_since_close(d, &run, &file));
        // A forged turn appended after the close: marked.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap();
        std::io::Write::write_all(&mut f, b"{\"role\":\"user\"}\n").unwrap();
        assert!(changed_since_close(d, &run, &file));
        // Same size, other bytes: marked.
        seal(d, &run, Some(&file));
        let same = std::fs::read_to_string(&file)
            .unwrap()
            .replace("user", "USER");
        std::fs::write(&file, same).unwrap();
        assert!(changed_since_close(d, &run, &file));
        // A run with no seal whose file changed after Clio started sealing: marked.
        let old = d.runs.join("20261117-bbbb");
        std::fs::create_dir_all(&old).unwrap();
        let other = sessions.join("b.jsonl");
        std::fs::write(&other, "x").unwrap();
        let past = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000);
        std::fs::File::options()
            .write(true)
            .open(d.data.join(SEALS_SINCE))
            .unwrap()
            .set_modified(past + std::time::Duration::from_secs(1))
            .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&other)
            .unwrap()
            .set_modified(past)
            .unwrap();
        assert!(
            !changed_since_close(d, &old, &other),
            "older than the seals"
        );
        std::fs::write(&other, "x forged").unwrap();
        assert!(changed_since_close(d, &old, &other));
    }

    // S-23: what the seals and the lock leave behind is private whatever the umask: the marker in
    // the data folder, the seal and the lock in the run folder, after an open and after a close.
    #[test]
    fn the_seal_the_marker_and_the_lock_are_private() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().canonicalize().unwrap();
        let (src, sessions) = local_source(&home);
        let d = &*src.dirs;
        let run = d.runs.join("20261118-cccc");
        std::fs::create_dir_all(&run).unwrap();
        let file = sessions.join("c.jsonl");
        std::fs::write(&file, "{}\n").unwrap();
        let modes = || {
            [
                crate::cli::mode(&d.data.join(SEALS_SINCE)),
                crate::cli::mode(&run.join(SEAL)),
                crate::cli::mode(&run.join("session.lock")),
            ]
        };
        // Open: the lock, then the seal of the file as it is.
        let lock = hold(&run).unwrap();
        seal(d, &run, Some(&file));
        assert_eq!(modes(), [Some(0o600); 3]);
        // Close: sealed again, the lock released.
        seal(d, &run, Some(&file));
        drop(lock);
        assert_eq!(modes(), [Some(0o600); 3]);
    }

    // Two `clio` on one conversation: the second cannot hold it while the first does.
    #[test]
    fn a_conversation_is_held_by_one_opener() {
        let t = tempfile::tempdir().unwrap();
        let first = hold(t.path()).unwrap();
        assert_eq!(hold(t.path()).unwrap_err(), OPEN_ELSEWHERE);
        drop(first);
        assert!(hold(t.path()).is_ok());
    }

    #[test]
    fn a_huge_time_limit_does_not_overflow() {
        let b = task_budget(&Limits {
            max_steps: 1,
            max_seconds: u64::MAX,
            max_cost_micro_usd: 1,
            day_cost_micro_usd: 0,
            day_steps: 0,
        });
        assert_eq!(b.max_ms, i64::MAX);
    }

    #[test]
    fn the_masked_entries_of_the_guard_add_up() {
        let e = |t: &str, n: Value| json!({"type":"custom","customType":t,"data":{"count":n}});
        let entries = [
            e("clio-guard.masked", json!(2)),
            e("clio-guard.taint", json!(5)), // another entry: not counted
            e("clio-guard.masked", json!(1)),
            e("clio-guard.masked", json!("many")), // not a number: ignored
            json!({"type":"message"}),
        ];
        assert_eq!(masked_total(&entries), 3);
        assert_eq!(masked_total(&[]), 0);
    }

    #[test]
    fn only_the_guards_taint_entry_counts() {
        assert!(is_taint(
            &json!({"type":"custom","customType":"clio-guard.taint","data":{"tainted":true}})
        ));
        assert!(!is_taint(
            &json!({"type":"custom","customType":"clio-guard.tools","data":{"tainted":true}})
        ));
        assert!(!is_taint(
            &json!({"type":"custom","customType":"clio-guard.taint","data":{}})
        ));
    }
}
