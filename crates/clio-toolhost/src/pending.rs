//! Host mode: what the list (`ask`) sets aside waits here for the person, in
//! `state/pending/<id>.json`. Nothing set aside runs until the person approves it, and then it
//! runs as it was saved (same hash), once, and never after it expired (24 hours).
//!
//! Questions: at most `questions_per_day` set-aside actions a local day are asked about
//! (`ask: true`); the rest go to the day's summary. Ones the person answered at the terminal give
//! their question back (`answered_at_terminal`).

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use clio_node::Action;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::ask::Class;
use crate::host::{Host, now_ms};
use crate::{DecisionOut, Request, Response, ToolHost, lock};

const DAY_MS: i64 = 24 * 3600 * 1000;
/// What is kept of an approved action's output.
const TAIL_BYTES: usize = 4000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    pub id: String,
    pub class: Class,
    pub tool: String,
    /// As the action was asked for, paths absolute.
    pub args: Value,
    /// Over tool and canonical args: an action changed on disk does not run.
    pub hash: String,
    /// The command or the path, scrubbed, as it was set aside: for the log. What is shown and
    /// approved is `shown`, from `args`; `approve` refuses when the two no longer agree.
    pub what: String,
    pub run: String,
    pub session: String,
    pub created_ms: i64,
    pub expires_ms: i64,
    /// False: past today's questions, it goes to the day's summary.
    pub ask: bool,
    pub state: State,
    /// Why it waits when its class alone would not say it (a suspect session reaching out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Waiting,
    Done {
        exit_code: Option<i64>,
        /// The tail of the output, cleaned.
        output_tail: String,
    },
    Rejected,
    Expired,
}

/// Today's questions (local day).
#[derive(Default, Serialize, Deserialize)]
struct Questions {
    day: String,
    ids: Vec<String>,
}

/// The exact answer to the model.
pub(crate) fn aside_text(p: &Pending) -> String {
    let why = p
        .reason
        .as_deref()
        .map_or_else(String::new, |r| format!(" (reason: {r})"));
    format!(
        "apartado {}: «{}» necesita el visto bueno de la persona{why}. No lo intentes de otra forma. Sigue con lo demás y cuéntalo al final.",
        p.id, p.class
    )
}

/// What `p` does, its command or its path, read from its arguments every time it is shown or
/// approved: never from the stored `what`, which a rewritten file could make say something else.
pub fn shown(p: &Pending) -> &str {
    p.args
        .get("command")
        .or_else(|| p.args.get("path"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// Takes `id` for one answer: an approval and a rejection racing each other, even from two
/// processes, cannot both have it.
fn claim(state: &Path, id: &str) -> std::io::Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir(state).join(format!("{id}.ran")))
        .map(drop)
}

fn hash_of(tool: &str, args: &Value) -> String {
    Action {
        id: String::new(),
        tool: tool.into(),
        args: args.clone(),
    }
    .hash()
}

/// `date` in the local time zone: there is no date crate here.
fn local(ms: i64, fmt: &str) -> Option<String> {
    let o = std::process::Command::new("/usr/bin/date")
        .arg(format!("--date=@{}", ms.div_euclid(1000)))
        .arg(format!("+{fmt}"))
        .output()
        .ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).trim().to_owned())
}

fn dir(state: &Path) -> PathBuf {
    state.join("pending")
}

fn file(state: &Path, id: &str) -> Result<PathBuf, String> {
    if id.is_empty() || id.len() > 32 || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(format!("no hay ningún pendiente {id:?}"));
    }
    Ok(dir(state).join(format!("{id}.json")))
}

/// Written whole (temporary file, then rename), 0600.
fn write_private(path: &Path, text: &str) -> Result<(), String> {
    let e = |x: std::io::Error| format!("{}: {x}", path.display());
    if let Some(d) = path.parent() {
        crate::host::private_dir(d).map_err(e)?;
    }
    let tmp = path.with_extension("tmp");
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(text.as_bytes()).and_then(|()| f.sync_all()))
        .and_then(|()| fs::rename(&tmp, path))
        .map_err(e)
}

fn save(state: &Path, p: &Pending) -> Result<(), String> {
    let text = serde_json::to_string_pretty(p).map_err(|e| e.to_string())?;
    write_private(&file(state, &p.id)?, &text)
}

fn load(state: &Path, id: &str) -> Result<Pending, String> {
    let text = fs::read_to_string(file(state, id)?)
        .map_err(|_| format!("no hay ningún pendiente {id}"))?;
    serde_json::from_str(&text).map_err(|e| format!("el pendiente {id} está dañado: {e}"))
}

/// Every pending action of this machine, oldest first.
pub fn list(state: &Path) -> Vec<Pending> {
    let mut all: Vec<Pending> = fs::read_dir(dir(state))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_str(&fs::read_to_string(e.path()).ok()?).ok())
        .collect();
    all.sort_by(|a, b| (a.created_ms, &a.id).cmp(&(b.created_ms, &b.id)));
    all
}

/// Why a pending that is not waiting does not run.
fn not_waiting(p: &Pending) -> String {
    match p.state {
        State::Waiting => format!("{} espera", p.id),
        State::Done { .. } => format!("{} ya se ejecutó", p.id),
        State::Rejected => format!("{} se rechazó: no se ejecuta", p.id),
        State::Expired => format!("{} venció: no se ejecutó", p.id),
    }
}

/// `id` as saved, if it still waits and has not expired at `now_ms` (one past its time is marked
/// `Expired` here). Nothing is run.
pub fn check(state: &Path, id: &str, now_ms: i64) -> Result<Pending, String> {
    let mut p = load(state, id)?;
    if p.state != State::Waiting {
        return Err(not_waiting(&p));
    }
    if now_ms >= p.expires_ms {
        p.state = State::Expired;
        save(state, &p)?;
        let when = local(p.expires_ms, "%Y-%m-%d %H:%M")
            .unwrap_or_else(|| format!("{} (ms)", p.expires_ms));
        return Err(format!("venció el {when}: no se ejecutó"));
    }
    Ok(p)
}

pub fn reject(state: &Path, id: &str) -> Result<Pending, String> {
    let mut p = load(state, id)?;
    if p.state != State::Waiting {
        return Err(not_waiting(&p));
    }
    claim(state, id).map_err(|_| format!("{id} ya se respondió"))?;
    p.state = State::Rejected;
    save(state, &p)?;
    Ok(p)
}

fn questions(state: &Path) -> Questions {
    fs::read_to_string(state.join("questions.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_questions(state: &Path, q: &Questions) -> Result<(), String> {
    let text = serde_json::to_string(q).map_err(|e| e.to_string())?;
    write_private(&state.join("questions.json"), &text)
}

/// The person answered `id` at the terminal: it was not one of the day's questions.
pub fn answered_at_terminal(state: &Path, id: &str) -> Result<(), String> {
    let mut q = questions(state);
    q.ids.retain(|x| x != id);
    save_questions(state, &q)
}

/// Saves `p` (id, hash and `ask` are filled here), or returns the one already waiting for the
/// same action in the same session.
pub(crate) fn set_aside(state: &Path, mut p: Pending, per_day: u32) -> Result<Pending, String> {
    p.hash = hash_of(&p.tool, &p.args);
    if let Some(old) = list(state).into_iter().find(|o| {
        o.session == p.session
            && o.hash == p.hash
            && o.state == State::Waiting
            && o.expires_ms > p.created_ms
    }) {
        return Ok(old);
    }
    for n in 0u32.. {
        let seed = format!("{}|{}|{}|{n}", p.session, p.hash, p.created_ms);
        let h = Sha256::digest(seed.as_bytes());
        p.id = h[..4].iter().map(|b| format!("{b:02x}")).collect();
        if !file(state, &p.id)?.exists() {
            break;
        }
    }
    // Shortcut: the UTC day number when `date` cannot run, which splits the day elsewhere.
    let today = local(p.created_ms, "%F")
        .unwrap_or_else(|| format!("utc-{}", p.created_ms.div_euclid(DAY_MS)));
    let mut q = questions(state);
    if q.day != today {
        q = Questions {
            day: today,
            ids: vec![],
        };
    }
    p.ask = q.ids.len() < per_day as usize;
    if p.ask {
        q.ids.push(p.id.clone());
        save_questions(state, &q)?;
    }
    save(state, &p)?;
    Ok(p)
}

fn tail(s: &str) -> String {
    let mut i = s.len().saturating_sub(TAIL_BYTES);
    while !s.is_char_boundary(i) {
        i += 1;
    }
    s[i..].to_owned()
}

impl ToolHost {
    /// Sets the action aside instead of running it, logs it, and gives the model the exact answer.
    pub(crate) fn aside(
        &self,
        h: &Host,
        req: &Request,
        class: Class,
        reason: Option<&str>,
    ) -> Response {
        let now = now_ms();
        let mut draft = Pending {
            id: String::new(),
            class,
            tool: req.tool.clone(),
            args: req.args.clone(),
            hash: String::new(),
            what: String::new(),
            run: h.cfg.run.clone(),
            session: req.session.clone(),
            created_ms: now,
            expires_ms: now + DAY_MS,
            ask: false,
            state: State::Waiting,
            reason: reason.map(str::to_owned),
        };
        draft.what = self.clean(shown(&draft));
        let network = req.args["network"].as_bool() == Some(true);
        let (content, rule) = match set_aside(&h.cfg.state, draft, h.cfg.rules.questions_per_day())
        {
            Ok(p) => {
                if let Err(e) = h.log(
                    &req.session,
                    &req.tool,
                    &p.what,
                    network,
                    "aside",
                    "host/aside",
                    None,
                ) {
                    eprintln!("clio-toolhost: the action log lost a line: {e}");
                }
                (aside_text(&p), "host/aside")
            }
            // Fails closed: it was not set aside, and it does not run either.
            Err(e) => (
                format!(
                    "[toolhost] {} was not run: it needs the person's approval and could not be set aside ({e})",
                    req.tool
                ),
                "host/aside-failed",
            ),
        };
        Response {
            id: req.id.clone(),
            ok: false,
            content: content.clone(),
            data: Value::Null,
            decision: DecisionOut {
                outcome: "ask".into(),
                rule: rule.into(),
                reason: content,
            },
            tainted: self.is_tainted(&req.session),
            external: false,
        }
    }

    /// The person approves `id`: it runs as it was saved, through the same path as an allowed
    /// action (log, copy before change) and without being classified again. Only a waiting,
    /// unexpired, unchanged pending of this host's run runs, and only once. Works from a fresh
    /// `ToolHost` built in host mode with that run's `run_dir` and `HostCfg.run`.
    pub fn approve(&self, id: &str, now_ms: i64) -> Result<Pending, String> {
        let h = self
            .host
            .as_ref()
            .ok_or("este servicio no está en modo anfitrión")?;
        let _one_at_a_time = lock(&self.busy);
        let state = &h.cfg.state;
        let mut p = check(state, id, now_ms)?;
        if p.run != h.cfg.run {
            return Err(format!("{id} es de otra sesión ({})", p.run));
        }
        if hash_of(&p.tool, &p.args) != p.hash || self.clean(shown(&p)) != p.what {
            return Err(format!("{id} cambió después de apartarse: no se ejecuta"));
        }
        if p.tool == "exec" {
            self.exec_available()
                .map_err(|why| format!("no hay consola: {why}"))?;
        }
        let real = match p.args.get("path").and_then(Value::as_str) {
            Some(raw) => Some(h.path(raw).map_err(|(_, why)| why)?),
            None => None,
        };
        // Claimed before it runs: a second approval, even from another process, finds it.
        claim(state, id).map_err(|_| format!("{id} ya se ejecutó"))?;
        let req = Request {
            id: id.into(),
            tool: p.tool.clone(),
            args: p.args.clone(),
            session: p.session.clone(),
            call_id: "approve".into(),
            tainted: false,
        };
        let ran = self.host_run(
            h,
            &req,
            format!("approve:{id}"),
            "host/approved",
            real.as_deref(),
            &|| false,
        );
        // It did not run (its line before could not be logged): it still waits.
        let (_, content, data, _) = ran.inspect_err(|_| {
            let _ = fs::remove_file(dir(state).join(format!("{id}.ran")));
        })?;
        p.state = State::Done {
            exit_code: data.get("exit_code").and_then(Value::as_i64),
            output_tail: tail(&content),
        };
        save(state, &p)?;
        Ok(p)
    }
}
