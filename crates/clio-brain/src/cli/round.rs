//! The round (`docs/specs/2026-11-19-the-round-design.md`): Clio wakes by herself, looks at what
//! she was asked to watch, and leaves a notice only when something needs the person.
//!
//! - **When**: `clio services run-due` (the services timer, every minute) asks `run_due`: every
//!   `round.every_hours`, never in `round.quiet`, one at a time (`<data>/round/round.lock`), one
//!   catch-up after downtime (due is measured from the last start). It starts `clio round
//!   run-timer` in a transient user scope of its own, as a service is started, and does not wait.
//! - **What**: one unattended session of the agent in host mode (`LocalSource::round`): its own run
//!   folder and Pi session (never in the sessions drawer), the prompt `clio-round@1`, the host
//!   tools, nobody to ask. Its only message is built here: Clio's request (`ROUND_ASK`) and, as
//!   data the judge reads first, what to look at (`request`).
//! - **Its answer** is read strictly (`pi::round::parse`); what is told is filtered so a thing is
//!   not told twice (`pi::round::to_tell`). The notices are kept in `<data>/round/notices.json`
//!   and shown once when a chat opens, then given to that chat's model with the person's next
//!   message, as outside text.
//! - **Limits**: no round without the judge on, the vault open and the model's key (one notice
//!   says why, once); a judge flag ends it at once; its own caps (`round.*`) and the day's budget.
//!   It never answers a box or opens a window: nothing here can (`a_round_answers_nothing`).

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clio_brain::control::{Budget, Stop};
use clio_brain::pi::preprompt::ROUND_ASK;
use clio_brain::pi::round::{self as r, Given, Report};
use clio_brain::pi::rpc::{Cancel, Event};
use clio_toolhost::pending::Pending;
use serde::{Deserialize, Serialize};

use super::chat::{SessionSource, Target};
use super::home::{Config, Dirs};
use super::services::{self, Notice};
use super::tui::sanitize;
use super::{Res, io_err, mkdir_private, now_ms, write_private};

pub use clio_brain::pi::round::quiet_hours;

/// The instruction of a round's run folder: how a run is known to be a round's.
pub const INSTRUCTION: &str = "Clio's round";
/// Rounds kept in the record.
const KEPT: usize = 50;
/// Approvals that expire within this long are given to the round.
const EXPIRING_MS: i64 = 8 * 3_600_000;

pub fn root(d: &Dirs) -> PathBuf {
    d.data.join("round")
}

/// One round, as `clio round` and `/round` show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rec {
    pub start_ms: i64,
    pub ms: u64,
    /// `timer` or `person`.
    pub by: String,
    #[serde(default)]
    pub run: Option<String>,
    pub outcome: String,
    /// What the person was told, as the chat shows it.
    #[serde(default)]
    pub notices: Vec<String>,
    /// What it set aside for the person.
    #[serde(default)]
    pub aside: usize,
    /// Its answer when it could not report: data, cleaned and capped, sent nowhere.
    #[serde(default)]
    pub raw: Option<String>,
}

/// `<data>/round/state.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct St {
    #[serde(default)]
    last_start_ms: Option<i64>,
    /// Why rounds do not run now (told once).
    #[serde(default)]
    blocked: Option<String>,
    /// What the person was last told of each thing, by key.
    #[serde(default)]
    told: BTreeMap<String, Given>,
    #[serde(default)]
    rounds: Vec<Rec>,
}

fn locked_file(p: &Path) -> Res<fs::File> {
    fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(p)
        .map_err(|e| io_err(p, &e))
}

fn read_state(root: &Path) -> St {
    fs::read_to_string(root.join("state.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Reads, changes and writes the state under its lock (the timer and `clio round` may both).
fn with_state<T>(root: &Path, f: impl FnOnce(&mut St) -> T) -> Res<T> {
    mkdir_private(root)?;
    let l = locked_file(&root.join(".state.lock"))?;
    rustix::fs::flock(&l, rustix::fs::FlockOperation::LockExclusive)
        .map_err(|e| format!("{}: {e}", root.display()))?;
    let mut st = read_state(root);
    let out = f(&mut st);
    let keep = st.rounds.len().saturating_sub(KEPT);
    st.rounds.drain(..keep);
    let p = root.join("state.json");
    let tmp = p.with_extension("json.tmp");
    write_private(
        &tmp,
        &serde_json::to_vec_pretty(&st).map_err(|e| e.to_string())?,
    )?;
    fs::rename(&tmp, &p).map_err(|e| io_err(&p, &e))?;
    Ok(out)
}

/// The round's lock, held while one runs; `None` when another holds it.
fn try_lock(root: &Path) -> Res<Option<fs::File>> {
    mkdir_private(root)?;
    let f = locked_file(&root.join("round.lock"))?;
    Ok(
        rustix::fs::flock(&f, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .is_ok()
            .then_some(f),
    )
}

// ---- when ------------------------------------------------------------------------------------

fn is_due(cfg: &Config, st: &St, now: i64) -> bool {
    r::due(
        cfg.round.every_hours,
        quiet_hours(&cfg.round.quiet),
        st.last_start_ms,
        now,
        clio_toolhost::services::local_offset(now),
    )
}

/// What a round needs before it may run: the judge on, the vault open, the model's key. Why not.
fn ready(d: &Dirs, cfg: &Config) -> Res<()> {
    if !cfg.jev.enabled {
        return Err("the judge is off (jev.enabled): a round reads outside content with nobody watching, so it runs only with the judge on".into());
    }
    super::vault::open_now(d)?;
    let c = clio_brain::connector::by_provider(&cfg.model.provider).ok_or("no connector")?;
    super::vault::model_key(d, c).map(drop)
}

/// Rounds do not run, and why: told once (a new reason is told again), cleared by a round.
fn blocked(root: &Path, why: &str, now: i64) {
    let new = with_state(root, |st| {
        let new = st.blocked.as_deref() != Some(why);
        st.blocked = Some(why.to_owned());
        new
    });
    if new.unwrap_or(false) {
        push(root, now, "", &format!("rounds do not run: {why}"));
    }
}

/// `clio services run-due`: starts a round when one is due, detached (`start`), and never waits
/// for it. What it did, one line, or nothing when no round was due.
pub fn run_due(d: &Dirs, now: i64) -> Vec<String> {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return vec![format!("round: {e}")],
    };
    run_due_with(d, now, &|cfg| ready(d, cfg), &|| {
        start(clio_node::host::systemd_run().as_deref(), &exe)
    })
}

fn run_due_with(
    d: &Dirs,
    now: i64,
    ready: &dyn Fn(&Config) -> Res<()>,
    start: &dyn Fn() -> Res<()>,
) -> Vec<String> {
    let Ok(cfg) = Config::load(d) else {
        return vec![];
    };
    let root = root(d);
    if !is_due(&cfg, &read_state(&root), now) {
        return vec![];
    }
    match try_lock(&root) {
        Ok(Some(_)) => {}
        Ok(None) => return vec!["round: skipped, a round is still running".into()],
        Err(e) => return vec![format!("round: {e}")],
    }
    if let Err(why) = ready(&cfg) {
        blocked(&root, &why, now);
        return vec![format!("round: not run: {why}")];
    }
    vec![match start() {
        Ok(()) => "round: started in a process of its own".into(),
        Err(e) => format!("round: {e}"),
    }]
}

/// Starts `clio round run-timer` and does not wait: in a transient user scope of its own when
/// `systemd-run` is there (out of the timer's unit, which may then end without stopping it), else
/// as a process of its own.
fn start(systemd_run: Option<&Path>, exe: &Path) -> Res<()> {
    use std::os::unix::process::CommandExt;
    if let Some(sr) = systemd_run {
        return services::start_detached(sr, exe, &["round", "run-timer"]);
    }
    std::process::Command::new(exe)
        .args(["round", "run-timer"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .map(drop)
        .map_err(|e| format!("{}: {e}", exe.display()))
}

// ---- what it is given ------------------------------------------------------------------------

/// `HH:MM` local of `ms`.
fn hhmm(ms: i64) -> String {
    let t = (ms / 1000 + clio_toolhost::services::local_offset(ms)).rem_euclid(86_400);
    format!("{:02}:{:02}", t / 3600, t / 60 % 60)
}

/// The person's `vigilancia` block, each entry as it is now.
fn watch_list(d: &Dirs) -> Vec<String> {
    let model = clio_brain::memory_link::EmbedModel::none();
    clio_brain::memory_link::MemoryLink::open_read(&d.data, &model)
        .ok()
        .flatten()
        .and_then(|l| l.block_rows("vigilancia").ok())
        .unwrap_or_default()
        .into_iter()
        .map(|row| row.content)
        .collect()
}

/// Services and MCP servers that raised a notice since `since`, or are paused now: name, state
/// and the notice line, never their output beyond it.
fn attention(d: &Dirs, since: i64) -> Vec<String> {
    let mut out = Vec::new();
    for (word, root) in [
        ("service", services::root(d)),
        ("MCP server", super::mcp_servers::root(d)),
    ] {
        for rec in super::skills::records(&root) {
            if !rec.enabled
                && let Some(why) = rec.off.as_deref().filter(|w| w.starts_with("paused"))
            {
                out.push(format!("{word} «{}»: off ({why})", rec.name));
            }
        }
        let mut recent = services::recent_notices(&root, 50);
        recent.retain(|n| n.at_ms > since);
        recent.reverse();
        for n in recent {
            let state = super::skills::record(&root, &n.name)
                .map_or("gone", |x| if x.enabled { "on" } else { "off" });
            out.push(format!(
                "{word} «{}» ({state}), {}: {}",
                n.name,
                hhmm(n.at_ms),
                n.text
            ));
        }
    }
    out
}

/// What waits for the person and expires within `EXPIRING_MS`.
fn expiring(d: &Dirs, now: i64) -> Vec<String> {
    super::pending::waiting(d)
        .iter()
        .filter(|p| p.expires_ms - now <= EXPIRING_MS)
        .map(|p| super::pending::line(p, now))
        .collect()
}

/// The data of the round's message: only what Clio's own records hold, never a conversation.
fn request(d: &Dirs, st: &St, now: i64) -> String {
    let since = st.last_start_ms.unwrap_or(now - 24 * 3_600_000);
    let told: Vec<String> = st
        .told
        .iter()
        .map(|(k, g)| {
            format!(
                "{k}: {} — {}",
                serde_json::to_value(g.state)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default(),
                g.notice
            )
        })
        .collect();
    let section = |title: &str, lines: Vec<String>| {
        let body = if lines.is_empty() {
            "(none)".to_owned()
        } else {
            lines
                .iter()
                .map(|l| format!("- {}", sanitize::line(l)))
                .collect::<Vec<_>>()
                .join("\n")
        };
        format!("{title}\n{body}")
    };
    [
        section(
            "What the person asked you to watch (their block «vigilancia»):",
            watch_list(d),
        ),
        section(
            "Services and MCP servers that raised a notice or are paused since your last round:",
            attention(d, since),
        ),
        section(
            "What waits for the person's approval and expires within 8 hours:",
            expiring(d, now),
        ),
        section(
            "What earlier rounds told the person, still open (key: state — notice; reuse these keys):",
            told,
        ),
    ]
    .join("\n\n")
}

// ---- running one -----------------------------------------------------------------------------

/// A round's caps: `round.*`, with the task's anti-loop and retries; the day's budget is shared.
pub fn budget(cfg: &Config) -> Budget {
    let mut b = super::chat::local::task_budget(&cfg.limits);
    b.max_steps = cfg.round.max_steps;
    b.max_ms = i64::try_from(cfg.round.max_seconds.saturating_mul(1000)).unwrap_or(i64::MAX);
    b.max_cost_micro = cfg.round.max_cost_micro_usd;
    b
}

/// Which limit cut a round, with the key that sets it.
fn limit(stop: Stop) -> String {
    let key = match stop {
        Stop::Steps => " (round.max_steps)",
        Stop::Time => " (round.max_seconds)",
        Stop::Cost => " (round.max_cost_micro_usd)",
        Stop::DayCost => " (limits.day_cost_micro_usd)",
        Stop::DaySteps => " (limits.day_steps)",
        _ => "",
    };
    format!("{}{key}", stop.reason())
}

/// Keeps a notice for the next chat; `key` empty for Clio's own.
fn push(root: &Path, at: i64, key: &str, text: &str) {
    let n = Notice {
        at_ms: at,
        name: key.to_owned(),
        text: text.to_owned(),
        seen: false,
    };
    if let Err(e) = services::with_notices(root, |all| all.push(n)) {
        eprintln!("clio round: a notice was lost: {e}");
    }
}

fn line(n: &Notice) -> String {
    let at = hhmm(n.at_ms);
    sanitize::line(&if n.name.is_empty() {
        format!("the round at {at}: {}", n.text)
    } else {
        format!("the round at {at}: «{}» {}", n.name, n.text)
    })
}

/// The rounds' notices no chat has shown yet, as quiet notes, now marked seen.
pub fn take_notices(d: &Dirs) -> Vec<String> {
    let root = root(d);
    if !root.join("notices.json").is_file() {
        return vec![];
    }
    services::with_notices(&root, |all| {
        all.iter_mut()
            .filter(|n| !n.seen)
            .map(|n| {
                n.seen = true;
                line(n)
            })
            .collect()
    })
    .unwrap_or_default()
}

/// What the chat's model is given with the person's next message (inside the data markers).
pub fn for_model(lines: &[String]) -> String {
    format!(
        "Notices from Clio's rounds (she looked by herself while the person was away); the person saw them when this chat opened:\n{}",
        lines.join("\n")
    )
}

/// Whether run `run` is a round's (its folder says so).
pub fn is_round(d: &Dirs, run: &str) -> bool {
    clio_brain::pi::launch::valid_id(run)
        && super::runs::read_json::<super::runs::Meta>(&d.runs.join(run).join("meta.json"))
            .is_ok_and(|m| m.host && m.instruction == INSTRUCTION)
}

/// `p`, named as set aside by a round when it was (the box and `/pending` say so).
pub fn with_origin(d: &Dirs, mut p: Pending) -> Pending {
    let Ok(m) = super::runs::read_json::<super::runs::Meta>(&d.runs.join(&p.run).join("meta.json"))
    else {
        return p;
    };
    if !is_round(d, &p.run) {
        return p;
    }
    let origin = format!("set aside by the round at {}", hhmm(m.created_ms));
    p.reason = Some(match p.reason.take() {
        Some(r) => format!("{origin}; {r}"),
        None => origin,
    });
    p
}

/// What came of a session's run: the outcome and the notices for the person (Clio's own words,
/// or the round's entries that are told), the raw answer when it could not report.
fn settle(
    st: &mut St,
    ran: Result<clio_brain::pi::rpc::RunSummary, String>,
    flagged: bool,
    now: i64,
) -> (String, Vec<(String, String)>, Option<String>) {
    let own = |t: String| vec![(String::new(), t)];
    if flagged {
        return (
            "ended: the judge flagged an injection in what it read".into(),
            own("it was ended: the judge flagged an injection in what it read (its run shows what it did)".into()),
            None,
        );
    }
    let s = match ran {
        Err(e) => return (format!("failed: {e}"), own(format!("it failed: {e}")), None),
        Ok(s) => s,
    };
    if let Some(stop) = s.stopped {
        let l = limit(stop);
        return (
            format!("cut by a limit: {l}"),
            own(format!("it was cut short: {l}")),
            None,
        );
    }
    if s.aborted {
        return ("stopped".into(), own("it was stopped".into()), None);
    }
    if s.text.trim().is_empty() && !s.errors.is_empty() {
        let e = s.errors.join("; ");
        return (
            format!("failed: its model failed: {e}"),
            own(format!("it failed: its model failed: {e}")),
            None,
        );
    }
    match r::parse(&s.text) {
        Report::Nothing => ("nothing to tell".into(), vec![], None),
        Report::Entries(e) => {
            let told = r::to_tell(&mut st.told, &e, now);
            (
                format!("{} thing(s) found, {} told", e.len(), told.len()),
                told,
                None,
            )
        }
        Report::CouldNot { why, raw } => (
            format!("could not report: {why}"),
            own(format!(
                "the round could not report ({why}); `clio round` shows what it said"
            )),
            raw,
        ),
    }
}

/// A judge flag ends the round at once: after each tool and message, the mark its tool host
/// leaves in its run folder is looked at, and a `suspect` one cancels the run.
fn ender<'a>(
    d: &'a Dirs,
    run: &'a str,
    flagged: &'a AtomicBool,
    cancel: &'a Cancel,
) -> impl FnMut(&Event) + 'a {
    move |e| {
        if matches!(
            e,
            Event::ToolEnd { .. } | Event::TurnEnd | Event::MessageEnd(_)
        ) && super::pending::suspect_run(d, run)
        {
            flagged.store(true, Ordering::SeqCst);
            cancel.cancel();
        }
    }
}

/// Runs one round now. `by`: `timer` (only if still due once its lock is taken, and quiet when it
/// is not) or `person` (`clio round now`). `Ok(None)`: not due, or one is running already (from
/// the timer). Every round that ran is recorded.
pub fn run(d: &Dirs, by: &str, now: i64) -> Res<Option<Rec>> {
    let root = root(d);
    let Some(_lock) = try_lock(&root)? else {
        return if by == "timer" {
            Ok(None)
        } else {
            Err("a round is running already".into())
        };
    };
    let cfg = Config::load(d)?;
    if by == "timer" && !is_due(&cfg, &read_state(&root), now) {
        return Ok(None);
    }
    if let Err(why) = ready(d, &cfg) {
        if by == "timer" {
            blocked(&root, &why, now);
            return Ok(None);
        }
        return Err(format!("no round: {why}"));
    }
    with_state(&root, |st| {
        st.last_start_ms = Some(now);
        st.blocked = None;
    })?;
    let began = std::time::Instant::now();
    let data = request(d, &read_state(&root), now);
    let flagged = Arc::new(AtomicBool::new(false));
    let mut run_id = None;
    let mut aside = 0;
    let src = super::chat::local::LocalSource::new(Arc::new(d.clone())).round();
    let ran = src
        .open(&Target::New {
            project: d.home.clone(),
        })
        .map(|o| {
            run_id = Some(o.id.clone());
            let s = &o.session;
            s.queue_findings(&data, Some(false), ROUND_ASK);
            let cancel = Cancel::new();
            let mut watch = ender(d, &o.id, &flagged, &cancel);
            let r = s
                .run_prompt("", &cancel, &mut watch)
                .map_err(|e| e.to_string());
            aside = s.new_pending().len();
            s.shutdown();
            r
        })
        .unwrap_or_else(|e| Err(format!("it did not start: {e}")));
    let flagged = flagged.load(Ordering::SeqCst)
        || run_id
            .as_deref()
            .is_some_and(|id| super::pending::suspect_run(d, id));
    let (outcome, mut told, raw) = with_state(&root, |st| settle(st, ran, flagged, now))?;
    if aside > 0 {
        told.push((
            String::new(),
            format!("{aside} action(s) it found wait for your yes: the approval box asks when a chat opens (or clio pending)"),
        ));
    }
    for (key, text) in &told {
        push(&root, now, key, text);
    }
    if let (Some(id), Some(raw)) = (&run_id, &raw) {
        let _ = write_private(&d.runs.join(id).join("answer.txt"), raw.as_bytes());
    }
    let rec = Rec {
        start_ms: now,
        ms: u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX),
        by: by.into(),
        run: run_id,
        outcome,
        notices: told
            .iter()
            .map(|(k, t)| {
                line(&Notice {
                    at_ms: now,
                    name: k.clone(),
                    text: t.clone(),
                    seen: true,
                })
            })
            .collect(),
        aside,
        raw,
    };
    with_state(&root, |st| st.rounds.push(rec.clone()))?;
    Ok(Some(rec))
}

// ---- what the person sees --------------------------------------------------------------------

/// The lines of `clio round` and `/round`: how rounds are set, why they do not run (if so), and
/// the last rounds, newest first, with what each told.
pub fn lines(d: &Dirs, k: usize) -> Vec<String> {
    let cfg = Config::load(d).map(|c| c.round).unwrap_or_default();
    let st = read_state(&root(d));
    let mut out = vec![if cfg.every_hours == 0 {
        "rounds are off (round.every_hours: 0)".to_owned()
    } else if cfg.quiet.trim().is_empty() {
        format!("a round every {} h", cfg.every_hours)
    } else {
        format!(
            "a round every {} h, never between {}",
            cfg.every_hours,
            cfg.quiet.replace('-', " and ")
        )
    }];
    if let Some(w) = &st.blocked {
        out.push(format!("rounds do not run: {w}"));
    }
    if st.rounds.is_empty() {
        out.push("no round has run yet".into());
    }
    let now = now_ms();
    for x in st.rounds.iter().rev().take(k) {
        let day = (now - x.start_ms) / 86_400_000;
        let when = match day {
            0 => hhmm(x.start_ms),
            1 => format!("yesterday {}", hhmm(x.start_ms)),
            n => format!("{} ({n} days ago)", hhmm(x.start_ms)),
        };
        out.push(String::new());
        out.push(sanitize::line(&format!(
            "{when} · {} · by {} · {}{}",
            super::usage::ms_label(x.ms),
            if x.by == "person" { "you" } else { "the timer" },
            x.outcome,
            x.run
                .as_deref()
                .map_or_else(String::new, |r| format!(" · run {r}"))
        )));
        if x.notices.is_empty() && x.raw.is_none() {
            out.push("  nothing told".into());
        }
        for n in &x.notices {
            out.push(format!("  {n}"));
        }
        if let Some(raw) = &x.raw {
            out.push("  what it said instead of its block (data, sent nowhere):".into());
            out.extend(
                raw.lines()
                    .take(20)
                    .map(|l| format!("  │ {}", sanitize::line(l))),
            );
        }
    }
    out
}

/// `clio round [now]`, and `run-timer` from `run_due`'s scope.
pub fn cmd(d: &Dirs, args: &[String]) -> Res<()> {
    match args.first().map(String::as_str) {
        None | Some("list") => {
            for l in lines(d, 10) {
                println!("{l}");
            }
            println!("\nclio round now runs one now");
            Ok(())
        }
        Some("now") => match run(d, "person", now_ms())? {
            Some(x) => {
                println!("round: {}", x.outcome);
                for n in &x.notices {
                    println!("  {n}");
                }
                if let Some(id) = &x.run {
                    println!("next: clio actions --run {id}");
                }
                Ok(())
            }
            None => Ok(()),
        },
        Some("run-timer") => {
            if let Some(x) = run(d, "timer", now_ms())? {
                println!("round: {}", x.outcome);
            }
            Ok(())
        }
        Some(other) => Err(format!("clio round: unknown {other:?} (now)")),
    }
}

/// `/round` in the chat: the same as `clio round`, in a box.
pub fn chat_command(d: &Dirs) -> super::tui::commands::Modal {
    let mut m = super::tui::commands::Modal::text("rounds", lines(d, 10));
    m.foot = Some("clio round now runs one now · round.every_hours: 0 turns them off".into());
    m
}

/// For `clio doctor`: rounds are on, and the services timer, the only thing that starts them, is
/// not active.
pub fn timer_warning(d: &Dirs) -> Option<String> {
    let every = Config::load(d).ok()?.round.every_hours;
    (every > 0 && !services::timer_active(&d.home)).then(|| {
        format!(
            "rounds are on (every {every} h) but the services timer is not active: no round runs (clio services enable, or round.every_hours: 0)"
        )
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
