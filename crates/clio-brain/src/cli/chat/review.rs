//! The reviewer: a second Pi that checks a piece of work and answers with a verdict
//! (`docs/specs/2026-11-18-clio-creates-design.md`, stage 1). [`run`] is the only way to start
//! one. It starts like the session's own Pi (the same jail, connector and launcher rules, the
//! guard), with:
//!
//! - its own Pi folders and session, its own `clio toolhost` and run folder
//!   (`<run>/reviews/<n>/`): its session file never shows in the drawer, its reads never mark
//!   the conversation, and its commands and the chat's never wait for each other (the tool host
//!   runs one call at a time);
//! - the prompt `clio-reviewer@3` and a fixed list of tools (read, bash, find, ls), so its own
//!   prompt prefix caches; no memory, no person to ask (`clio toolhost --review`: what would wait
//!   for the person is refused), no background commands;
//! - a clean context: only the review request, never the conversation, the identity or memory;
//! - the session's marks, connector and model, the same day's budget, its own cost cap
//!   (`review.max_cost_micro_usd`), and the action log under its own run id (`<run>.review-<n>`),
//!   which `/actions` shows with the session's.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clio_brain::control::{Budget, DayBudget, Stop};
use clio_brain::pi::budget::RunGuard;
use clio_brain::pi::install;
use clio_brain::pi::launch::{Launch, SessionTarget, start};
use clio_brain::pi::review::{self, Answer};
use clio_brain::pi::rpc::{Cancel, Event, Usage};
use clio_brain::pi::session::{
    HostOpts, REVIEW_GUARD_TOOLS, Toolhost, ToolhostOpts, ensure_guard, review_guard_env,
};
use clio_brain::pi::{PiPaths, text};
use clio_core::SystemClock;
use clio_jev::{Clean, Questions};
use serde_json::{Value, json};

use super::local::{provider_of, task_budget};
use super::{ReviewRequest, Reviewed, TurnWork};
use crate::cli::home::{Config, Dirs, ModelCfg, REVIEW_MODEL_CLAUDE};
use crate::cli::{jev, usage};

/// The judge's closed question after a turn that changed something.
pub const NEEDS_REVIEW: &str = "needs_review";
pub const NEEDS_REVIEW_TEXT: &str = "The work listed in `done` was carried out on this machine to \
     answer `person_request`. It needs an independent check by someone else: it changes code, \
     configuration, a service or data, a mistake in it could go unnoticed, and it is not trivial \
     to see at a glance that it is right.";

/// Where the session that asks for a review stands.
pub struct Parent<'a> {
    pub dirs: &'a Dirs,
    /// Its run id and folder.
    pub run: &'a str,
    pub run_dir: &'a Path,
    /// The folder it works on: its copy, or the home in host mode.
    pub work: &'a Path,
    pub host: bool,
    /// Its mark: `Some(suspect)` when it is marked.
    pub marked: Option<bool>,
    pub day: Arc<Mutex<DayBudget>>,
    /// The session's connector and its model now (after `/model`): the review runs on that
    /// connector, and on that model when the connector does not offer the reviewer's.
    pub model: &'a ModelCfg,
}

/// The reviewer's model: `review.model`, else Opus with the `claude` connector, else the
/// session's; the session's when the connector does not offer it (and why).
fn model_for(d: &Dirs, cfg: &Config, session: &str) -> (String, Option<String>) {
    let want = cfg.review.model.clone().unwrap_or_else(|| {
        if cfg.model.provider == "claude" {
            REVIEW_MODEL_CLAUDE.to_owned()
        } else {
            session.to_owned()
        }
    });
    let offered = usage::pi_models(d, &cfg.model.provider)
        .unwrap_or_else(|| usage::catalog_models(d, &cfg.model.provider));
    if want == session || offered.is_empty() || offered.contains(&want) {
        (want, None)
    } else {
        (
            session.to_owned(),
            Some(format!(
                "{} is not offered by this connector: the review ran on the session's model",
                text::line(&want, 80)
            )),
        )
    }
}

/// A new, empty folder for the review: `<run>/reviews/<n>`.
fn new_folder(run_dir: &Path) -> Result<(u32, PathBuf), String> {
    let base = run_dir.join("reviews");
    crate::cli::mkdir_private(&base)?;
    for n in 1..10_000 {
        let dir = base.join(n.to_string());
        match std::fs::create_dir(&dir) {
            Ok(()) => {
                crate::cli::mkdir_private(&dir)?;
                return Ok((n, dir));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("{}: {e}", dir.display())),
        }
    }
    Err("too many reviews in this run".into())
}

/// A skill's request may carry the whole skill (SKILL.md and every file, at most 128 KiB).
const MAX_SKILL_SUBJECT: usize = 160_000;

/// What a skill draft's reviewer is asked (S-68): the skill whole, every file's content inline
/// (what is judged is what is registered, whether it opens the folder or not), and its check, run
/// from `folder`, the staged copy.
pub fn skill_request(d: &clio_toolhost::skills::Draft, folder: &Path) -> ReviewRequest {
    if clio_toolhost::services::is_service_draft(&d.id) {
        return service_request(d, folder);
    }
    let mut subject = format!(
        "A skill for Clio{}: instructions an agent will follow, and scripts it may run. A copy of it is in {} (it may be read-only: copy it under /tmp if its check must write). Every file of it, whole:\n\n--- SKILL.md ---\n{}\n",
        if d.update {
            " (a new version of one already registered)"
        } else {
            ""
        },
        folder.display(),
        d.skill_md
    );
    for f in &d.files {
        subject.push_str(&format!("--- {} ---\n{}\n", f.path, f.content));
    }
    subject.push_str(
        "\nCheck too that its description says what it does and when to use it, that its steps can be followed by an agent that never saw this conversation, and that it does nothing beyond what it says.",
    );
    ReviewRequest {
        what: format!("the skill «{}», before it is registered", d.name),
        subject,
        paths: vec![folder.display().to_string()],
        check: format!(
            "In {}, run the commands of the `## How to verify` section of its SKILL.md, as written; they must succeed. A check that would succeed whatever the skill does is no check: then design and run your own. The verdict is `fails` if the skill does not do what it says.",
            folder.display()
        ),
    }
}

/// What a service draft's reviewer is asked (S-69): as a skill's, with its manifest as the main
/// file and its `verify` as the check. Its approval is always the person's: the review only
/// decides whether the person is asked at all.
pub fn service_request(d: &clio_toolhost::skills::Draft, folder: &Path) -> ReviewRequest {
    let mut subject = format!(
        "A service for Clio{}: a program that will run by itself on a schedule, with no model, as the person, with the machine read-only except its state folder ($CLIO_STATE_DIR) and the paths in `writes`, and the network only to the hosts in `network`. A copy of it is in {} (it may be read-only: copy it under /tmp if its check must write). Every file of it, whole:\n\n--- {} ---\n{}\n",
        if d.update {
            " (a new version of one already registered)"
        } else {
            ""
        },
        folder.display(),
        clio_toolhost::services::MANIFEST,
        d.skill_md
    );
    for f in &d.files {
        subject.push_str(&format!("--- {} ---\n{}\n", f.path, f.content));
    }
    subject.push_str(
        "\nCheck too that its description says what it does, that running it twice in a row is safe, that it exits 0 when all is well, 10 when the person must look at something and anything else on a failure, that it keeps its state only in $CLIO_STATE_DIR, that every host it reaches is in `network` and every path it writes is in `writes`, that no secret is in a file (secrets come as the variables in `secrets`), and that it does nothing beyond what it says.",
    );
    ReviewRequest {
        what: format!(
            "the service «{}», before the person is asked to approve it",
            d.name
        ),
        subject,
        paths: vec![folder.display().to_string()],
        check: format!(
            "In {}, run the `verify` command of its {} as written; it must succeed without the network. A check that would succeed whatever the service does is no check: then design and run your own. The verdict is `fails` if the service does not do what it says.",
            folder.display(),
            clio_toolhost::services::MANIFEST
        ),
    }
}

/// A draft's main file: a service's manifest, or a skill's SKILL.md.
fn main_of(d: &clio_toolhost::skills::Draft) -> &'static str {
    if clio_toolhost::services::is_service_draft(&d.id) {
        clio_toolhost::services::MANIFEST
    } else {
        "SKILL.md"
    }
}

/// `<review>/stage/<name>/`: the skill's files for its reviewer, written here by the person's
/// `clio`. Every level is made new (0700) and every file new, never through a link; nothing that
/// already exists is reused. Any failure is an error (the review then could not check).
fn stage(dir: &Path, d: &clio_toolhost::skills::Draft) -> Result<PathBuf, String> {
    let stage = dir.join("stage");
    crate::cli::skills::new_dir(&stage)?;
    let folder = stage.join(&d.name);
    crate::cli::skills::write_tree_named(&folder, main_of(d), &d.skill_md, &d.files)?;
    Ok(folder)
}

/// After the review: the staged copy must still be the draft, byte for byte (each file read
/// without following a link); otherwise what was reviewed is not what would be registered, and
/// the review could not check.
fn settle(answer: Answer, folder: &Path, d: &clio_toolhost::skills::Draft) -> Answer {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let read = |rel: &str| -> Result<String, String> {
        let p = folder.join(rel);
        let mut t = String::new();
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(&p)
            .and_then(|mut f| f.read_to_string(&mut t))
            .map_err(|e| format!("{}: {e}", p.display()))?;
        Ok(t)
    };
    let staged = read(main_of(d)).and_then(|md| {
        let files = d
            .files
            .iter()
            .map(|f| {
                Ok(clio_toolhost::skills::SkillFile {
                    path: f.path.clone(),
                    content: read(&f.path)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(clio_toolhost::skills::digest(&md, &files))
    });
    match staged {
        Ok(h) if h == d.sha256 => answer,
        Ok(_) => Answer::could_not_check(
            "the staged copy of the skill changed during the review: what was reviewed is not the draft",
            None,
        ),
        Err(e) => Answer::could_not_check(
            &format!("the staged copy of the skill could not be read back after the review: {e}"),
            None,
        ),
    }
}

/// What a review checks: a turn's work (or what `/review` names), or a skill draft (S-68).
#[derive(Clone, Copy)]
enum Subject<'a> {
    Work(&'a ReviewRequest),
    Skill(&'a clio_toolhost::skills::Draft),
}

/// Runs one review to its end (or its limit, or `cancel`). Never fails: what kept it from
/// running is a `could_not_check` that says why.
pub fn run(p: &Parent<'_>, req: &ReviewRequest, cancel: &Cancel) -> Reviewed {
    review_of(p, Subject::Work(req), cancel)
}

/// The review of a skill draft (S-68): staged in its own folder (`<run>/reviews/<n>/stage/`,
/// Clio's data: no jail sees it, and in host mode only the reviewer's commands, read-only), its
/// whole text in the request, and the staged copy checked against the draft after.
pub fn run_skill(p: &Parent<'_>, d: &clio_toolhost::skills::Draft, cancel: &Cancel) -> Reviewed {
    review_of(p, Subject::Skill(d), cancel)
}

fn review_of(p: &Parent<'_>, what: Subject<'_>, cancel: &Cancel) -> Reviewed {
    let began = Instant::now();
    let mut out = Reviewed {
        answer: Answer::could_not_check("the review did not start", None),
        model: p.model.model.clone(),
        fallback: None,
        secs: 0,
        commands: 0,
        marked: None,
        full: None,
        usage: Usage::default(),
        skill: None,
    };
    let mut req = match what {
        Subject::Work(r) => r.clone(),
        Subject::Skill(d) => skill_request(d, Path::new("(not staged)")),
    };
    let dir = match started(p, what, &mut req, cancel, &mut out) {
        Ok(dir) => Some(dir),
        Err((dir, e)) => {
            out.answer = Answer::could_not_check(&format!("the review did not run: {e}"), None);
            dir
        }
    };
    out.secs = began.elapsed().as_secs();
    if let Some(dir) = dir {
        keep(&dir, &req, &out);
    }
    out
}

/// What `run` does once it has a folder; on failure, the folder (if any) and why. A skill's
/// request (`req`) is made once it is staged.
fn started(
    p: &Parent<'_>,
    what: Subject<'_>,
    req: &mut ReviewRequest,
    cancel: &Cancel,
    out: &mut Reviewed,
) -> Result<PathBuf, (Option<PathBuf>, String)> {
    if cancel.is_cancelled() {
        return Err((None, "it was stopped".into()));
    }
    let d = p.dirs;
    // In host mode a reviewer's commands run only under bwrap (S-67): without it, nothing runs.
    if p.host {
        clio_node::HostJail::with(clio_node::Launcher::Bwrap).map_err(|e| {
            (
                None,
                format!("a review's commands run only under bwrap, and bwrap cannot run here: {e}"),
            )
        })?;
    }
    let cfg = session_cfg(d, p.model).map_err(|e| (None, e))?;
    let (model, fallback) = model_for(d, &cfg, &p.model.model);
    out.model.clone_from(&model);
    out.fallback = fallback;
    let (n, dir) = new_folder(p.run_dir).map_err(|e| (None, e))?;
    let fail = |e: String| (Some(dir.clone()), e);
    let staged = match what {
        Subject::Skill(d) => {
            let folder = stage(&dir, d).map_err(|e| {
                fail(format!(
                    "the skill could not be staged for its reviewer: {e}"
                ))
            })?;
            *req = skill_request(d, &folder);
            Some(folder)
        }
        Subject::Work(_) => None,
    };
    // Copy mode: the reviewer works in the staged copy. Host mode: in the home, and its commands
    // see the stage read-only (a window into Clio's masked data folder).
    let work = match &staged {
        Some(f) if !p.host => f.clone(),
        _ => p.work.to_path_buf(),
    };
    let show = staged.as_deref().and_then(Path::parent).filter(|_| p.host);
    let id = format!("{}.review-{n}", p.run);
    let main = crate::cli::pi::paths(d);
    let node = install::find_node(&std::env::var("PATH").unwrap_or_default())
        .map_err(|e| fail(e.to_string()))?;
    let install = install::locate(&main).map_err(|e| fail(e.to_string()))?;
    let connector = clio_brain::connector::by_provider(&cfg.model.provider)
        .ok_or_else(|| fail("no connector".into()))?;
    let key = crate::cli::vault::model_key(d, connector).map_err(fail)?;
    // Its own Pi folders: its session file is not the conversation's, nor its `models.json`.
    let paths = PiPaths { root: dir.clone() };
    let exe = std::env::current_exe().map_err(|e| fail(e.to_string()))?;
    let (state, control, rules) = (d.data.join("host"), d.control(), d.host_rules());
    let service_env = jev::toolhost_env(d, &cfg.jev);
    let toolhost = Toolhost::start(
        &exe,
        d.need_runtime().map_err(fail)?,
        &paths,
        &dir,
        &work,
        &key,
        ToolhostOpts {
            memory: None,
            extra_env: &service_env,
            host: p.host.then(|| HostOpts {
                home: &d.home,
                state: &state,
                control: &control,
                rules: rules.as_deref(),
                run: &id,
            }),
            secrets: None,
            review: true,
            skills: None,
            show,
            mcp: None,
        },
    )
    .map_err(fail)?;
    let mut rc = cfg.clone();
    rc.model.model.clone_from(&model);
    rc.model.thinking.clone_from(&cfg.review.thinking);
    let launch = Launch {
        guard: Some(ensure_guard(&install).map_err(fail)?),
        guard_tools: REVIEW_GUARD_TOOLS.map(String::from).to_vec(),
        extra_env: review_guard_env(&paths, &toolhost, p.host).map_err(fail)?,
        provider: provider_of(d, &rc),
        api_key: Some(key),
        cwd: work.clone(),
        name: None,
        install,
        node,
        paths,
        session: SessionTarget::New {
            id: Some(id.clone()),
        },
        helper: exe,
        skills: None,
    };
    let pi = start(&launch).map_err(|e| fail(e.to_string()))?;
    pi.client.set_interactive(false);
    // The session's marks: a reviewer of a marked session asks before what a marked one asks.
    if let Some(suspect) = p.marked
        && !jev::taint_session(&toolhost.socket, &pi.session_id, suspect)
    {
        return Err(fail(
            "it could not be marked like the session that asked for it".into(),
        ));
    }
    let mut guard = RunGuard::new(
        Arc::new(SystemClock),
        &id,
        review_budget(&cfg),
        p.day.clone(),
    );
    let ask = match what {
        Subject::Work(_) => review::request(&req.what, &req.subject, &req.check),
        Subject::Skill(_) => {
            review::request_capped(&req.what, &req.subject, &req.check, MAX_SKILL_SUBJECT)
        }
    };
    // Its commands, for the note (they are in the action log in host mode).
    let mut commands = 0;
    let ran = pi
        .client
        .run_prompt_limited(&ask, cancel, &mut guard, &mut |e| {
            if matches!(e, Event::ToolStart { name, .. } if name == "bash") {
                commands += 1;
            }
        });
    out.commands = commands;
    let (answer, raw) = match ran {
        Err(e) => (
            Answer::could_not_check(&format!("the review did not run: {e}"), None),
            String::new(),
        ),
        Ok(s) => {
            out.usage = s.usage;
            let a = if cancel.is_cancelled() || s.aborted {
                Answer::could_not_check("the review was stopped", Some(&s.text))
            } else if let Some(stop) = s.stopped {
                Answer::could_not_check(
                    &format!("the review reached its limit: {}", limit(stop)),
                    Some(&s.text),
                )
            } else if s.text.trim().is_empty() && !s.errors.is_empty() {
                Answer::could_not_check(
                    &format!("the reviewer's model failed: {}", s.errors.join("; ")),
                    None,
                )
            } else {
                review::parse(&s.text)
            };
            (a, s.text)
        }
    };
    out.answer = answer;
    drop(pi);
    toolhost.stop();
    if let (Subject::Skill(d), Some(f)) = (what, &staged) {
        out.answer = settle(
            std::mem::replace(&mut out.answer, Answer::could_not_check("", None)),
            f,
            d,
        );
    }
    // The reviewer's own mark, as its tool host left it.
    out.marked = std::fs::read_to_string(dir.join("toolhost/tainted.json"))
        .ok()
        .map(|t| t.contains("\"suspect\""));
    let full = dir.join("answer.txt");
    if crate::cli::write_private(&full, text::data(&raw).as_bytes()).is_ok() {
        out.full = Some(full.display().to_string());
    }
    Ok(dir)
}

/// The config a review starts from: the file's, with the session's connector and model (the
/// file may name another since `/connector` or `/model`).
fn session_cfg(d: &Dirs, model: &ModelCfg) -> Result<Config, String> {
    let mut cfg = Config::load(d)?;
    cfg.model = model.clone();
    Ok(cfg)
}

/// A review's caps: its own steps, time and cost (`review.*`), the task's anti-loop and retries.
fn review_budget(cfg: &Config) -> Budget {
    let mut b = task_budget(&cfg.limits);
    b.max_steps = cfg.review.max_steps;
    b.max_ms = i64::try_from(cfg.review.max_seconds.saturating_mul(1000)).unwrap_or(i64::MAX);
    b.max_cost_micro = cfg.review.max_cost_micro_usd;
    b
}

/// Which limit ended a review, with the key that sets it.
fn limit(stop: Stop) -> String {
    let key = match stop {
        Stop::Steps => " (review.max_steps)",
        Stop::Time => " (review.max_seconds)",
        Stop::Cost => " (review.max_cost_micro_usd)",
        _ => "",
    };
    format!("{}{key}", stop.reason())
}

/// `review.json` in the review's folder: what was asked and what came of it.
fn keep(dir: &Path, req: &ReviewRequest, r: &Reviewed) {
    let a = &r.answer;
    let v = json!({
        "what": req.what,
        "paths": req.paths,
        "check": req.check,
        "verdict": a.verdict.word(),
        "ran": a.ran,
        "findings": a.findings.iter().map(|f| json!({"what": f.what, "evidence": f.evidence})).collect::<Vec<_>>(),
        "dropped": a.dropped,
        "raw": a.raw,
        "model": r.model,
        "fallback": r.fallback,
        "secs": r.secs,
        "commands": r.commands,
        "marked": r.marked,
        "usage": {"input": r.usage.input, "output": r.usage.output, "cache_read": r.usage.cache_read, "cache_write": r.usage.cache_write, "cost": r.usage.cost},
    });
    let _ = crate::cli::write_private(
        &dir.join("review.json"),
        serde_json::to_string_pretty(&v)
            .unwrap_or_default()
            .as_bytes(),
    );
}

/// What came of the judge's question about a turn: a review, or why not.
#[derive(Debug, Clone, PartialEq)]
pub enum Judged {
    /// `review.auto` is off.
    AutoOff,
    /// The judge is off.
    JudgeOff,
    /// No vault to scrub what the judge would read.
    NoVault(String),
    /// The config, the question or the judge's call failed, or the judge did not answer.
    Failed(String),
    /// The judge's score, under `review.threshold`.
    Below { score: f64, threshold: f64 },
    /// Asked, and yes.
    Yes { score: f64 },
}

impl Judged {
    pub fn yes(&self) -> bool {
        matches!(self, Self::Yes { .. })
    }

    /// In words, for the debug note and `judged.jsonl`.
    pub fn reason(&self) -> String {
        match self {
            Self::AutoOff => "review.auto is off".into(),
            Self::JudgeOff => "the judge is off".into(),
            Self::NoVault(e) => format!("no vault to scrub what the judge reads: {e}"),
            Self::Failed(e) => format!("the judge could not be asked: {e}"),
            Self::Below { score, threshold } => {
                format!("the judge said {score:.2}, under the threshold {threshold:.2}")
            }
            Self::Yes { score } => format!("the judge said {score:.2}: a review"),
        }
    }

    fn word(&self) -> &'static str {
        match self {
            Self::AutoOff => "auto_off",
            Self::JudgeOff => "judge_off",
            Self::NoVault(_) => "no_vault",
            Self::Failed(_) => "failed",
            Self::Below { .. } => "below",
            Self::Yes { .. } => "yes",
        }
    }
}

/// One line per decision in `<run>/reviews/judged.jsonl` (private, like the run's other files).
/// Best effort: a line that cannot be written is said on stderr.
pub fn record(run_dir: &Path, j: &Judged) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = run_dir.join("reviews");
    let score = match j {
        Judged::Below { score, .. } | Judged::Yes { score } => Some(*score),
        _ => None,
    };
    let line = json!({
        "at_ms": crate::cli::now_ms(),
        "decision": j.word(),
        "score": score,
        "reason": j.reason(),
    });
    let wrote = crate::cli::mkdir_private(&dir).and_then(|()| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(dir.join("judged.jsonl"))
            .and_then(|mut f| writeln!(f, "{line}"))
            .map_err(|e| e.to_string())
    });
    if let Err(e) = wrote {
        eprintln!("clio: the judge's decision on a review was not recorded: {e}");
    }
}

/// What the judge reads about a turn's work: each string scrubbed whole, then cut (S-63), so
/// a secret across a cut is never half left.
fn judge_state(
    work: &TurnWork,
    scrub: &dyn Fn(&str) -> Result<String, String>,
) -> Result<Value, String> {
    let done = work
        .commands
        .iter()
        .map(|c| ("ran", c))
        .chain(work.files.iter().map(|f| ("wrote", f)))
        .take(40)
        .map(|(k, v)| Ok(format!("{k}: {}", text::line(&scrub(v)?, 300))))
        .collect::<Result<Vec<String>, String>>()?;
    let request = text::line(&scrub(&work.request)?, 2000);
    Ok(json!({ "person_request": request, "done": done }))
}

/// The judge's closed question about a turn's work: a review only with `Judged::Yes` (the
/// judge's score at or over `review.threshold`).
pub fn needs_review(d: &Dirs, work: &TurnWork) -> Judged {
    let cfg = match Config::load(d) {
        Ok(c) => c,
        Err(e) => return Judged::Failed(e),
    };
    let judge = jev::client(d, &cfg.jev);
    let ask = judge.as_ref().map(|j| {
        move |state: &Clean, qs: &Questions| {
            j.ask(state, qs)
                .map(|a| a.noul(NEEDS_REVIEW))
                .map_err(|e| e.to_string())
        }
    });
    let vault = crate::cli::vault::gateway_socket(d)
        .map(|s| clio_vault::VaultClient::new(&s).with_timeout(Duration::from_secs(2)));
    let scrub = vault
        .as_ref()
        .map(|v| move |t: &str| v.scrub(t).map_err(|e| e.to_string()))
        .map_err(Clone::clone);
    decide(
        &cfg.review,
        work,
        ask.as_ref().map(|f| f as &Ask<'_>),
        scrub
            .as_ref()
            .map(|f| f as &Scrub<'_>)
            .map_err(Clone::clone),
    )
}

type Ask<'a> = dyn Fn(&Clean, &Questions) -> Result<Option<f64>, String> + 'a;
type Scrub<'a> = dyn Fn(&str) -> Result<String, String> + 'a;

/// `needs_review` once the judge (`None`: off) and the vault's scrubber (or why there is none)
/// are known.
fn decide(
    cfg: &crate::cli::home::ReviewCfg,
    work: &TurnWork,
    judge: Option<&Ask<'_>>,
    scrub: Result<&Scrub<'_>, String>,
) -> Judged {
    if !cfg.auto {
        return Judged::AutoOff;
    }
    let Some(judge) = judge else {
        return Judged::JudgeOff;
    };
    let scrub = match scrub {
        Ok(s) => s,
        Err(e) => return Judged::NoVault(e),
    };
    let state = match judge_state(work, scrub).and_then(|s| Clean::try_new(s, scrub)) {
        Ok(s) => s,
        Err(e) => return Judged::NoVault(e),
    };
    let qs = match Questions::new().noul(NEEDS_REVIEW, NEEDS_REVIEW_TEXT) {
        Ok(q) => q,
        Err(e) => return Judged::Failed(e),
    };
    match judge(&state, &qs) {
        Err(e) => Judged::Failed(e),
        Ok(None) => Judged::Failed("it did not answer the question".into()),
        Ok(Some(score)) if score >= cfg.threshold => Judged::Yes { score },
        Ok(Some(score)) => Judged::Below {
            score,
            threshold: cfg.threshold,
        },
    }
}

/// One turn as the session saw it: its tool calls, by id.
#[derive(Debug, Clone, Default)]
pub struct Turn {
    pub request: String,
    pub start_ms: i64,
    pub end_ms: i64,
    /// (call id, Pi's tool, its arguments).
    pub calls: Vec<(String, String, Value)>,
    pub answer: String,
}

/// What `turn` changed, as the action log records it: in host mode the log of the person's
/// machine (`<state>/actions.jsonl`: this run and session, done or stopped), in copy mode the
/// session's effect log (`effects.jsonl`: the write and command calls of this turn that the tool
/// host applied). `None`: nothing (a read-only turn).
pub fn work_of(
    turn: &Turn,
    session: &str,
    host: Option<(&Path, &str)>,
    effects: &Path,
) -> Option<TurnWork> {
    let (mut commands, mut files) = (Vec::new(), Vec::new());
    let lines = |p: &Path| {
        std::fs::read_to_string(p)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .collect::<Vec<_>>()
    };
    match host {
        Some((state, run)) => {
            for l in lines(&state.join("actions.jsonl")) {
                let at = l["at_ms"].as_i64().unwrap_or(0);
                if l["run"] != run
                    || l["session"] != session
                    || at < turn.start_ms
                    || at > turn.end_ms
                    || !matches!(l["outcome"].as_str(), Some("done" | "stopped"))
                {
                    continue;
                }
                let what = text::line(l["what"].as_str().unwrap_or(""), 300);
                match l["tool"].as_str() {
                    Some("exec") => commands.push(what),
                    Some("fs_write" | "fs_mkdir") => files.push(what),
                    _ => {}
                }
            }
        }
        None => {
            let applied: std::collections::HashSet<String> = lines(effects)
                .iter()
                .filter_map(|l| l["id"].as_str())
                .filter_map(|id| id.strip_prefix(session)?.strip_prefix(':'))
                .filter_map(|rest| rest.split(':').next().map(str::to_owned))
                .collect();
            for (id, tool, args) in &turn.calls {
                if !applied.contains(id) {
                    continue;
                }
                match tool.as_str() {
                    "bash" => {
                        commands.push(text::line(args["command"].as_str().unwrap_or(""), 300))
                    }
                    "write" | "edit" => {
                        files.push(text::line(args["path"].as_str().unwrap_or(""), 300))
                    }
                    _ => {}
                }
            }
        }
    }
    files.dedup();
    (!commands.is_empty() || !files.is_empty()).then(|| TurnWork {
        request: turn.request.clone(),
        commands,
        files,
        answer: text::block(&turn.answer, 4000),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn turn() -> Turn {
        Turn {
            request: "fix the backup script".into(),
            start_ms: 100,
            end_ms: 200,
            calls: vec![
                ("c1".into(), "read".into(), json!({"path": "a.sh"})),
                ("c2".into(), "write".into(), json!({"path": "a.sh"})),
                ("c3".into(), "bash".into(), json!({"command": "sh a.sh"})),
                ("c4".into(), "bash".into(), json!({"command": "never ran"})),
            ],
            answer: "done".into(),
        }
    }

    #[test]
    fn copy_mode_counts_only_the_writes_and_commands_the_tool_host_applied() {
        let t = tempfile::tempdir().unwrap();
        let effects = t.path().join("effects.jsonl");
        std::fs::write(
            &effects,
            "{\"id\":\"s1:c1:0\",\"hash\":\"h\",\"result\":{}}\n{\"id\":\"s1:c2:1\",\"hash\":\"h\",\"result\":{}}\n{\"id\":\"s1:c3:2\",\"hash\":\"h\",\"result\":{\"exit_code\":0}}\n{\"id\":\"other:c4:3\",\"hash\":\"h\",\"result\":{}}\n",
        )
        .unwrap();
        let w = work_of(&turn(), "s1", None, &effects).unwrap();
        assert_eq!(w.files, vec!["a.sh"]);
        assert_eq!(w.commands, vec!["sh a.sh"]);
        // A turn that only read changed nothing.
        let mut read_only = turn();
        read_only.calls.truncate(1);
        assert_eq!(work_of(&read_only, "s1", None, &effects), None);
    }

    #[test]
    fn host_mode_reads_the_action_log_of_this_run_session_and_turn() {
        let t = tempfile::tempdir().unwrap();
        let line = |at: i64, run: &str, tool: &str, what: &str, outcome: &str| {
            json!({"at_ms": at, "run": run, "session": "s1", "tool": tool, "what": what, "outcome": outcome}).to_string()
        };
        let log = [
            line(150, "r1", "exec", "systemctl --user restart x", "run"),
            line(151, "r1", "exec", "systemctl --user restart x", "done"),
            line(152, "r1", "fs_write", "/home/p/a.conf", "done"),
            line(153, "r1", "exec", "rm -rf /", "deny"),
            line(154, "r1", "fs_write", "/home/p/b", "aside"),
            line(155, "r1.review-1", "exec", "cat a", "done"),
            line(250, "r1", "exec", "later", "done"),
        ]
        .join("\n");
        std::fs::write(t.path().join("actions.jsonl"), log).unwrap();
        let w = work_of(&turn(), "s1", Some((t.path(), "r1")), Path::new("/none")).unwrap();
        assert_eq!(w.commands, vec!["systemctl --user restart x"]);
        assert_eq!(w.files, vec!["/home/p/a.conf"]);
        assert_eq!(w.request, "fix the backup script");
    }

    fn dirs_in(root: &Path, cfg: &Config) -> Dirs {
        let data = root.join("data");
        let d = Dirs {
            config: root.join("config"),
            runs: data.join("runs"),
            data,
            runtime: Some(root.join("run/clio")),
            home: root.to_path_buf(),
        };
        std::fs::create_dir_all(&d.config).unwrap();
        std::fs::create_dir_all(&d.data).unwrap();
        std::fs::write(d.config_file(), serde_json::to_string(cfg).unwrap()).unwrap();
        d
    }

    fn work() -> TurnWork {
        TurnWork {
            request: "fix it".into(),
            commands: vec!["make test".into()],
            files: vec!["src/a.rs".into()],
            answer: "fixed".into(),
        }
    }

    // `review.auto` off is the only thing between a judge and vault that say yes and a review;
    // every other way to no review says which it is.
    #[test]
    fn without_the_judge_or_with_auto_off_no_automatic_review() {
        let t = tempfile::tempdir().unwrap();
        let cfg = Config::default_with(vec![]);
        assert!(!cfg.jev.enabled && cfg.review.auto);
        assert_eq!(
            needs_review(&dirs_in(t.path(), &cfg), &work()),
            Judged::JudgeOff
        );
        let yes = |_: &Clean, _: &Questions| Ok(Some(0.95));
        let plain = |t: &str| Ok(t.to_owned());
        let mut r = cfg.review.clone();
        assert_eq!(
            decide(&r, &work(), Some(&yes), Ok(&plain)),
            Judged::Yes { score: 0.95 }
        );
        r.auto = false;
        assert_eq!(decide(&r, &work(), Some(&yes), Ok(&plain)), Judged::AutoOff);
        r.auto = true;
        assert_eq!(decide(&r, &work(), None, Ok(&plain)), Judged::JudgeOff);
        assert_eq!(
            decide(&r, &work(), Some(&yes), Err("no socket".into())),
            Judged::NoVault("no socket".into())
        );
        let low = |_: &Clean, _: &Questions| Ok(Some(0.5));
        let j = decide(&r, &work(), Some(&low), Ok(&plain));
        assert_eq!(
            j,
            Judged::Below {
                score: 0.5,
                threshold: 0.8
            }
        );
        assert!(!j.yes() && j.reason().contains("0.50") && j.reason().contains("0.80"));
        let silent = |_: &Clean, _: &Questions| Err("timeout".into());
        assert!(matches!(
            decide(&r, &work(), Some(&silent), Ok(&plain)),
            Judged::Failed(e) if e == "timeout"
        ));
    }

    // S-63: what the judge reads is scrubbed whole and cut after, so a secret across the cut
    // leaves nothing of itself.
    #[test]
    fn the_judges_input_is_scrubbed_before_it_is_cut() {
        let secret = "sk-ant-api03-ZZZZZZZZZZZZZZZZZZZZ";
        let mut w = work();
        w.commands = vec![format!("{} {secret}", "x".repeat(290))];
        w.request = format!("{} {secret}", "y".repeat(1995));
        let scrub = |t: &str| Ok(t.replace(secret, "[REDACTED]"));
        let state = judge_state(&w, &scrub).unwrap().to_string();
        assert!(!state.contains("sk-ant"), "{state}");
        assert!(state.contains("ran: xxx"), "{state}");
        let fails = |_: &str| Err("vault down".to_owned());
        assert_eq!(judge_state(&w, &fails).unwrap_err(), "vault down");
    }

    // Every decision is one private line in the run's `reviews/judged.jsonl`.
    #[test]
    fn each_decision_of_the_judge_is_kept_in_the_run() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        record(t.path(), &Judged::AutoOff);
        record(t.path(), &Judged::Yes { score: 0.9 });
        let file = t.path().join("reviews/judged.jsonl");
        let lines: Vec<Value> = std::fs::read_to_string(&file)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines[0]["decision"], "auto_off");
        assert_eq!(lines[0]["score"], Value::Null);
        assert_eq!(lines[1]["decision"], "yes");
        assert_eq!(lines[1]["score"], 0.9);
        assert!(lines[1]["reason"].as_str().unwrap().contains("0.90"));
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(&t.path().join("reviews")), 0o700);
    }

    // A review has its own cost cap (Opus on a paid connector costs more than a task's), and a
    // review cut by a limit says which, with its key.
    #[test]
    fn a_review_has_its_own_budget_and_says_which_limit_ended_it() {
        let mut cfg = Config::default_with(vec![]);
        cfg.limits.max_cost_micro_usd = 100_000;
        cfg.review.max_steps = 12;
        cfg.review.max_seconds = 90;
        let b = review_budget(&cfg);
        assert_eq!(
            (b.max_cost_micro, b.max_steps, b.max_ms),
            (1_000_000, 12, 90_000)
        );
        cfg.review.max_cost_micro_usd = 250_000;
        assert_eq!(review_budget(&cfg).max_cost_micro, 250_000);
        assert!(limit(Stop::Cost).ends_with("(review.max_cost_micro_usd)"));
        assert!(limit(Stop::Time).ends_with("(review.max_seconds)"));
        assert!(limit(Stop::Steps).ends_with("(review.max_steps)"));
        assert_eq!(limit(Stop::DayCost), Stop::DayCost.reason());
    }

    // After `/connector` (saved for the next session) and `/model` (the session switched), a
    // review runs on the session's connector and falls back to its model now.
    #[test]
    fn a_review_runs_on_the_sessions_connector_and_model() {
        let t = tempfile::tempdir().unwrap();
        let mut file = Config::default_with(vec![]);
        file.model.provider = "opencode-go".into();
        file.model.model = "beam-free".into();
        let d = dirs_in(t.path(), &file);
        let mut session = file.model.clone();
        session.provider = "claude".into();
        session.model = "claude-sonnet-5-5".into();
        usage::save_pi_models(
            &d,
            "claude",
            &["claude-sonnet-5-5".into(), REVIEW_MODEL_CLAUDE.into()],
        );
        let cfg = session_cfg(&d, &session).unwrap();
        assert_eq!(cfg.model.provider, "claude");
        assert_eq!(
            model_for(&d, &cfg, &session.model),
            (REVIEW_MODEL_CLAUDE.into(), None)
        );
        // The session's connector does not offer the reviewer's model: the session's model now.
        usage::save_pi_models(
            &d,
            "claude",
            &["claude-sonnet-5-5".into(), "claude-haiku".into()],
        );
        session.model = "claude-haiku".into();
        let cfg = session_cfg(&d, &session).unwrap();
        assert_eq!(model_for(&d, &cfg, &session.model).0, "claude-haiku");
    }

    #[test]
    fn the_reviewer_runs_on_opus_with_claude_and_on_the_sessions_model_when_it_is_not_offered() {
        let t = tempfile::tempdir().unwrap();
        let mut cfg = Config::default_with(vec![]);
        cfg.model.provider = "claude".into();
        cfg.model.model = "claude-sonnet-5-5".into();
        let d = dirs_in(t.path(), &cfg);
        usage::save_pi_models(
            &d,
            "claude",
            &["claude-sonnet-5-5".into(), REVIEW_MODEL_CLAUDE.into()],
        );
        assert_eq!(
            model_for(&d, &cfg, "claude-sonnet-5-5"),
            (REVIEW_MODEL_CLAUDE.into(), None)
        );
        usage::save_pi_models(&d, "claude", &["claude-sonnet-5-5".into()]);
        let (m, why) = model_for(&d, &cfg, "claude-sonnet-5-5");
        assert_eq!(m, "claude-sonnet-5-5");
        assert!(why.unwrap().contains("not offered"));
        // Another connector: the session's model unless `review.model` says otherwise.
        cfg.model.provider = "opencode-go".into();
        assert_eq!(model_for(&d, &cfg, "beam-free"), ("beam-free".into(), None));
    }

    fn skill(files: &[(&str, &str)]) -> clio_toolhost::skills::Draft {
        let md = clio_toolhost::skills::compose(
            "demo",
            "Says hello. Use when greeting.",
            "1. Run it.",
            "bash scripts/a.sh",
        );
        let files: Vec<_> = files
            .iter()
            .map(|(p, c)| clio_toolhost::skills::SkillFile {
                path: (*p).into(),
                content: (*c).into(),
            })
            .collect();
        clio_toolhost::skills::Draft {
            id: "a1".into(),
            session: "s".into(),
            name: "demo".into(),
            description: "Says hello. Use when greeting.".into(),
            reason: "r".into(),
            verify: "bash scripts/a.sh".into(),
            sha256: clio_toolhost::skills::digest(&md, &files),
            skill_md: md,
            files,
            created_ms: 1,
            update: false,
            marked: false,
            suspect: false,
        }
    }

    // S-68: the stage is made new, level by level, in the review's own folder: a path planted
    // there (a link to a decoy `live/`, under a parent that cannot be written) is never followed
    // or reused, and nothing is written outside the stage.
    #[test]
    fn a_planted_stage_is_refused_and_nothing_is_written_through_it() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let d = skill(&[("scripts/a.sh", "#!/bin/bash\necho hi\n")]);
        let decoy = t.path().join("decoy-live");
        std::fs::create_dir(&decoy).unwrap();
        let review = t.path().join("reviews/1");
        std::fs::create_dir_all(review.join("stage")).unwrap();
        std::os::unix::fs::symlink(&decoy, review.join("stage/demo")).unwrap();
        std::fs::set_permissions(review.join("stage"), std::fs::Permissions::from_mode(0o500))
            .unwrap();
        let e = stage(&review, &d).unwrap_err();
        assert!(e.contains("stage"), "{e}");
        std::fs::set_permissions(review.join("stage"), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        assert_eq!(std::fs::read_dir(&decoy).unwrap().count(), 0);
        // Only the link alone: refused too.
        std::fs::remove_dir_all(review.join("stage")).unwrap();
        std::fs::create_dir(review.join("stage")).unwrap();
        std::os::unix::fs::symlink(&decoy, review.join("stage/demo")).unwrap();
        assert!(stage(&review, &d).is_err());
        assert_eq!(std::fs::read_dir(&decoy).unwrap().count(), 0);
        // A clean folder: the stage, private, with the draft's bytes.
        let clean = t.path().join("reviews/2");
        std::fs::create_dir(&clean).unwrap();
        let f = stage(&clean, &d).unwrap();
        assert_eq!(f, clean.join("stage/demo"));
        let mode = std::fs::metadata(clean.join("stage"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
        assert_eq!(
            std::fs::read_to_string(f.join("scripts/a.sh")).unwrap(),
            d.files[0].content
        );
    }

    // S-68: a staging that fails ends the review as `could_not_check`, before anything starts.
    #[test]
    fn a_skill_that_cannot_be_staged_could_not_be_checked() {
        let t = tempfile::tempdir().unwrap();
        let d = dirs_in(t.path(), &Config::default_with(vec![]));
        let run_dir = d.runs.join("r1");
        std::fs::create_dir_all(&run_dir).unwrap();
        let model = Config::default_with(vec![]).model;
        let p = Parent {
            dirs: &d,
            run: "r1",
            run_dir: &run_dir,
            work: t.path(),
            host: true,
            marked: None,
            day: Arc::new(Mutex::new(DayBudget::new(u64::MAX))),
            model: &model,
        };
        // A file where one of its folders must go: it cannot be written.
        let bad = skill(&[("a", "x"), ("a/b", "y")]);
        let rv = run_skill(&p, &bad, &Cancel::new());
        assert_eq!(rv.answer.verdict, review::Verdict::CouldNotCheck);
        assert!(
            rv.answer.ran.contains("could not be staged"),
            "{}",
            rv.answer.ran
        );
    }

    // S-68: what was reviewed must still be the draft when the review ends: a staged file
    // changed meanwhile turns any verdict into `could_not_check`; one gone or made a link too.
    #[test]
    fn a_staged_file_changed_during_the_review_could_not_be_checked() {
        let t = tempfile::tempdir().unwrap();
        let d = skill(&[("scripts/a.sh", "#!/bin/bash\necho hi\n")]);
        let passes = review::parse(
            "```review\nverdict: passes\nran: bash scripts/a.sh\nfindings: none\n```",
        );
        let review = t.path().join("r");
        std::fs::create_dir(&review).unwrap();
        let f = stage(&review, &d).unwrap();
        assert_eq!(settle(passes.clone(), &f, &d), passes);
        std::fs::write(f.join("scripts/a.sh"), "#!/bin/bash\nrm -rf ~\n").unwrap();
        let a = settle(passes.clone(), &f, &d);
        assert_eq!(a.verdict, review::Verdict::CouldNotCheck);
        assert!(a.ran.contains("changed during the review"), "{}", a.ran);
        std::fs::remove_file(f.join("scripts/a.sh")).unwrap();
        std::os::unix::fs::symlink(t.path().join("elsewhere"), f.join("scripts/a.sh")).unwrap();
        assert_eq!(
            settle(passes, &f, &d).verdict,
            review::Verdict::CouldNotCheck
        );
    }

    // S-68: the reviewer gets every file whole in the request, and is told a check that cannot
    // fail is none.
    #[test]
    fn a_skills_request_carries_every_file_whole_and_what_a_check_is() {
        let body = format!("#!/bin/bash\n{}\necho end-of-script\n", "x".repeat(30_000));
        let d = skill(&[("scripts/a.sh", &body), ("ref.md", "the reference\n")]);
        let r = skill_request(&d, Path::new("/s/demo"));
        assert!(r.subject.contains("--- SKILL.md ---\n---\nname: demo"));
        assert!(r.subject.contains(&format!("--- scripts/a.sh ---\n{body}")));
        assert!(r.subject.contains("--- ref.md ---\nthe reference"));
        assert!(
            r.check
                .contains("would succeed whatever the skill does is no check")
        );
        assert!(
            r.check
                .contains("`fails` if the skill does not do what it says")
        );
        let asked = review::request_capped(&r.what, &r.subject, &r.check, MAX_SKILL_SUBJECT);
        assert!(asked.contains("echo end-of-script"), "cut");
    }

    #[test]
    fn the_review_request_of_a_turn_lists_what_was_done() {
        let w = work();
        let r = ReviewRequest::of_work(None, Some(&w));
        assert!(r.what.contains("fix it"));
        assert!(r.subject.contains("- ran: make test") && r.subject.contains("- wrote: src/a.rs"));
        assert_eq!(r.paths, vec!["src/a.rs"]);
        let r = ReviewRequest::of_work(Some("the parser"), Some(&w));
        assert_eq!(r.what, "the parser");
    }
}
