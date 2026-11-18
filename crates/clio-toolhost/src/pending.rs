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
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::ask::Class;
use crate::host::{Host, MAX_TIMEOUT_SECS, log_line, now_ms};
use crate::{BlockEdit, DecisionOut, Judging, MemoryPort, Request, Response, ToolHost, lock};

const DAY_MS: i64 = 24 * 3600 * 1000;
/// What is kept of an approved action's output.
const TAIL_BYTES: usize = 4000;
/// How long the reason of a secret request may be.
const MAX_WHY: usize = 200;

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
    /// A secret request the person answered: the value went to the vault (and nowhere else).
    Stored,
}

/// `request_secret`: where its requests wait (`state`, as the actions set aside), the run they
/// belong to, the vault's person socket (only `Status` and `List` are asked, at request time) and
/// how many questions a day the person gets, shared with the actions.
#[derive(Debug, Clone)]
pub struct SecretCfg {
    pub state: PathBuf,
    pub run: String,
    pub person: PathBuf,
    pub per_day: u32,
}

/// What a secret request asks for: name, destination (`tool:domain`) and why.
pub fn secret_of(p: &Pending) -> Option<(&str, &str, &str)> {
    let s = |k: &str| p.args.get(k).and_then(Value::as_str).unwrap_or("");
    (p.class == Class::Secret).then(|| (s("name"), s("destination"), s("why")))
}

/// A change to one of the person's blocks, as it was set aside: the block, the action (`add`,
/// `replace`, `remove`), the text to write and, for replace and remove, the entry and what it said
/// then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockChange<'a> {
    pub name: &'a str,
    pub action: &'a str,
    pub text: Option<&'a str>,
    pub id: Option<i64>,
    pub old: Option<&'a str>,
}

/// What block change `p` would write, read from its arguments (never from `what`).
pub fn block_of(p: &Pending) -> Option<BlockChange<'_>> {
    let s = |k: &str| p.args.get(k).and_then(Value::as_str);
    (p.class == Class::Memory).then(|| BlockChange {
        name: s("name").unwrap_or(""),
        action: s("action").unwrap_or(""),
        text: s("text"),
        id: p.args.get("id").and_then(Value::as_i64),
        old: s("old"),
    })
}

/// The person approves block change `id`: it is written as it was stored (same hash), with the
/// person's authority, through the same port a clean session writes with, and once. A replace or
/// a remove whose entry is gone or no longer says what it said when it was asked writes nothing.
/// The model has no way here: only the person's `clio` calls this, outside the jail.
pub fn approve_block(
    state: &Path,
    id: &str,
    now_ms: i64,
    port: &dyn MemoryPort,
) -> Result<Pending, String> {
    let mut p = check(state, id, now_ms)?;
    let Some(b) = block_of(&p) else {
        return Err(format!("{id} is not a change to a block"));
    };
    if hash_of(&p.tool, &p.args) != p.hash {
        return Err(format!(
            "{id} changed after it was set aside: nothing was written"
        ));
    }
    let edit = match (b.action, b.id, b.text) {
        ("add", None, Some(t)) => BlockEdit::Add(t.to_owned()),
        ("replace", Some(n), Some(t)) => BlockEdit::Replace(n, t.to_owned()),
        ("remove", Some(n), None) => BlockEdit::Remove(n),
        _ => return Err(format!("{id} is damaged: nothing was written")),
    };
    let name = b.name.to_owned();
    if let Some(n) = b.id {
        let now = port.block(&name)?;
        if !now
            .iter()
            .any(|e| e.id == n && Some(e.text.as_str()) == b.old)
        {
            return Err(format!(
                "entry m{n} of «{name}» changed or is gone since this was asked: nothing was written"
            ));
        }
    }
    // Claimed before it is written: a second approval, even from another process, finds it.
    claim(state, id).map_err(|_| format!("{id} was already answered"))?;
    let judging = Judging {
        tainted: false,
        session: p.session.clone(),
    };
    let written = port.block_edit(&name, &edit, &judging).inspect_err(|_| {
        // Nothing was written: it still waits.
        let _ = fs::remove_file(dir(state).join(format!("{id}.ran")));
    })?;
    let done = if matches!(edit, BlockEdit::Remove(_)) {
        format!(
            "removed from «{name}»: m{} is in quarantine, the person can restore it",
            written.id
        )
    } else {
        format!("saved in «{name}» as m{}", written.id)
    };
    p.state = State::Done {
        exit_code: None,
        output_tail: done,
    };
    save(state, &p)?;
    if let Err(e) = log_line(
        state,
        &p.run,
        &p.session,
        &p.tool,
        &p.what,
        false,
        "approved",
        "host/approved",
        None,
    ) {
        eprintln!("clio-toolhost: the action log lost a line: {e}");
    }
    Ok(p)
}

/// The person typed the value of secret request `id`: `store` puts it in the vault. Claimed
/// first, so it is stored once; a failed store leaves it waiting.
pub fn answer_secret(
    state: &Path,
    id: &str,
    now_ms: i64,
    store: impl FnOnce(&Pending) -> Result<(), String>,
) -> Result<Pending, String> {
    let mut p = check(state, id, now_ms)?;
    if p.class != Class::Secret {
        return Err(format!("{id} is not a secret request"));
    }
    if hash_of(&p.tool, &p.args) != p.hash {
        return Err(format!(
            "{id} changed after it was asked: nothing was stored"
        ));
    }
    claim(state, id).map_err(|_| format!("{id} was already answered"))?;
    if let Err(e) = store(&p) {
        let _ = fs::remove_file(dir(state).join(format!("{id}.ran")));
        return Err(e);
    }
    p.state = State::Stored;
    save(state, &p)?;
    Ok(p)
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
    let limit = if unlimited(p) {
        format!(
            " If the person approves it, it runs with a time limit of {MAX_TIMEOUT_SECS} s, since it asked for none."
        )
    } else {
        String::new()
    };
    format!(
        "set aside {}: «{}» needs the person's approval{why}. Do not try it another way. Go on with the rest and say so at the end.{limit}",
        p.id, p.class
    )
}

/// What `p` does, its command or its path (a secret request: its name), read from its arguments
/// every time it is shown or approved: never from the stored `what`, which a rewritten file could
/// make say something else.
pub fn shown(p: &Pending) -> &str {
    p.args
        .get("command")
        .or_else(|| p.args.get("path"))
        .or_else(|| p.args.get("name"))
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
        return Err(format!("no pending item {id:?}"));
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
    let text = fs::read_to_string(file(state, id)?).map_err(|_| format!("no pending item {id}"))?;
    serde_json::from_str(&text).map_err(|e| format!("pending item {id} is damaged: {e}"))
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
        State::Waiting => format!("{} is waiting", p.id),
        State::Done { .. } => format!("{} already ran", p.id),
        State::Rejected => format!("{} was rejected: it does not run", p.id),
        State::Expired => format!("{} expired: it did not run", p.id),
        State::Stored => format!("{} was already answered", p.id),
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
        return Err(format!("expired on {when}: it did not run"));
    }
    Ok(p)
}

pub fn reject(state: &Path, id: &str) -> Result<Pending, String> {
    let mut p = load(state, id)?;
    if p.state != State::Waiting {
        return Err(not_waiting(&p));
    }
    claim(state, id).map_err(|_| format!("{id} was already answered"))?;
    p.state = State::Rejected;
    save(state, &p)?;
    if p.class == Class::Memory
        && let Err(e) = log_line(
            state,
            &p.run,
            &p.session,
            &p.tool,
            &p.what,
            false,
            "rejected",
            "host/rejected",
            None,
        )
    {
        eprintln!("clio-toolhost: the action log lost a line: {e}");
    }
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

/// A secret's destination, strict: the vault binds the value to the host and every name under it,
/// so `http_fetch:com` would hand it to every `.com`. The tool is `[a-z_]+`; the host is
/// `[a-z0-9.-]+` with at least two labels, none empty (no leading, trailing or double dot).
// shortcut: no public suffix list, so `co.uk` passes; add one if hosts like that get asked for.
fn destination(s: &str) -> Option<clio_vault::Dest> {
    let (tool, host) = s.split_once(':')?;
    // A host name has no case: `API.example.com` is `api.example.com`.
    let host = host.to_ascii_lowercase();
    let tool_ok = !tool.is_empty() && tool.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
    let host_ok = host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        && host.split('.').count() >= 2
        && host.split('.').all(|l| !l.is_empty());
    (tool_ok && host_ok).then(|| clio_vault::Dest::new(tool, &host))
}

/// A command that asked for no time limit.
fn unlimited(p: &Pending) -> bool {
    p.tool == "exec" && !p.args.get("timeout").is_some_and(Value::is_u64)
}

/// What an approved action runs with: as saved, but a command with no limit gets the longest,
/// since nobody in the chat can stop it.
fn run_args(p: &Pending) -> Value {
    let mut args = p.args.clone();
    if unlimited(p)
        && let Some(a) = args.as_object_mut()
    {
        a.insert("timeout".into(), Value::from(MAX_TIMEOUT_SECS));
    }
    args
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
        let at = (
            h.cfg.state.as_path(),
            h.cfg.run.as_str(),
            h.cfg.rules.questions_per_day(),
        );
        self.aside_at(at, req, req.args.clone(), class, reason)
    }

    /// `aside` with where it waits given apart (`state`, `run`, the day's questions) and the
    /// arguments it keeps: a block change waits in a chat that is not in host mode too.
    pub(crate) fn aside_at(
        &self,
        (state, run, per_day): (&Path, &str, u32),
        req: &Request,
        args: Value,
        class: Class,
        reason: Option<&str>,
    ) -> Response {
        let now = now_ms();
        let network = args["network"].as_bool() == Some(true);
        let mut draft = Pending {
            id: String::new(),
            class,
            tool: req.tool.clone(),
            args,
            hash: String::new(),
            what: String::new(),
            run: run.to_owned(),
            session: req.session.clone(),
            created_ms: now,
            expires_ms: now + DAY_MS,
            ask: false,
            state: State::Waiting,
            reason: reason.map(str::to_owned),
        };
        draft.what = self.clean(shown(&draft));
        let (content, rule) = match set_aside(state, draft, per_day) {
            Ok(p) => {
                if let Err(e) = log_line(
                    state,
                    run,
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
            .ok_or("this service is not in host mode")?;
        let _one_at_a_time = lock(&self.busy);
        let state = &h.cfg.state;
        let mut p = check(state, id, now_ms)?;
        if p.class == Class::Secret {
            return Err(format!(
                "{id} asks for a secret: it is answered with the value, not run"
            ));
        }
        if p.class == Class::Memory {
            return Err(format!(
                "{id} changes one of your blocks: it is written when approved, not run"
            ));
        }
        if p.run != h.cfg.run {
            return Err(format!("{id} belongs to another session ({})", p.run));
        }
        if hash_of(&p.tool, &p.args) != p.hash || self.clean(shown(&p)) != p.what {
            return Err(format!(
                "{id} changed after it was set aside: it does not run"
            ));
        }
        if p.tool == "exec" {
            self.exec_available()
                .map_err(|why| format!("no console: {why}"))?;
        }
        let real = match p.args.get("path").and_then(Value::as_str) {
            Some(raw) => Some(h.path(raw).map_err(|(_, why)| why)?),
            None => None,
        };
        // Claimed before it runs: a second approval, even from another process, finds it.
        claim(state, id).map_err(|_| format!("{id} already ran"))?;
        let req = Request {
            id: id.into(),
            tool: p.tool.clone(),
            args: run_args(&p),
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

    /// `request_secret`: checks the name, the destination and the vault now, and stores the
    /// request for the person; the value is asked for when the turn ends, by the chat, and never
    /// comes through here.
    pub(crate) fn request_secret(
        &self,
        req: &Request,
        args: &Map<String, Value>,
        tainted: bool,
    ) -> Response {
        let refuse = |why: String| self.refuse(&req.id, "request_secret", why, tainted);
        let Some(cfg) = &self.secrets else {
            return refuse("nobody can be asked for a secret here".into());
        };
        let arg = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").trim();
        let name = arg("name");
        if !clio_vault::valid_name(name) {
            return refuse(
                "`name` must be 1 to 64 lowercase letters, digits, '-', '_' or '.'".into(),
            );
        }
        let Some(dest) = destination(arg("destination")) else {
            return refuse(
                "`destination` must be TOOL:HOST, as in http_fetch:api.github.com: the tool in a-z and '_', the host a full name with a dot inside (a-z, 0-9, '-' and '.'), never a bare suffix such as com".into(),
            );
        };
        let why: String = self
            .clean(arg("why"))
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(MAX_WHY)
            .collect();
        if why.trim().is_empty() {
            return refuse("`why` must say in one line what the secret is for".into());
        }
        // The person's socket, and only to ask whether the vault is open and what names it has.
        let vault = clio_vault::VaultClient::new(&cfg.person)
            .with_timeout(std::time::Duration::from_secs(5));
        let open = vault
            .call(&clio_vault::proto::Request::Status)
            .ok()
            .and_then(|r| r.data)
            .map(|d| d["locked"].as_bool() == Some(false));
        match open {
            None => {
                return refuse(
                    "the vault is not running, so nothing can be stored: ask the person to start it (`clio vault start`)".into(),
                );
            }
            Some(false) => {
                return refuse(
                    "the vault is locked, so nothing can be stored: ask the person to unlock it (`clio vault start`)".into(),
                );
            }
            Some(true) => {}
        }
        let names = vault
            .call(&clio_vault::proto::Request::List)
            .ok()
            .filter(|r| r.ok)
            .and_then(|r| r.data);
        let Some(names) = names else {
            return refuse("the vault did not say which secrets it has: try again later".into());
        };
        if names
            .as_array()
            .is_some_and(|a| a.iter().any(|m| m["name"] == name))
        {
            return refuse(format!(
                "a secret named {name} already exists: pick another name, or ask the person to run `clio vault put {name}`"
            ));
        }
        let now = now_ms();
        let draft = Pending {
            id: String::new(),
            class: Class::Secret,
            tool: "request_secret".into(),
            args: json!({
                "name": name,
                "destination": format!("{}:{}", dest.tool, dest.domain),
                "why": why.trim(),
            }),
            hash: String::new(),
            what: name.to_owned(),
            run: cfg.run.clone(),
            session: req.session.clone(),
            created_ms: now,
            expires_ms: now + DAY_MS,
            ask: false,
            state: State::Waiting,
            reason: None,
        };
        let content = match set_aside(&cfg.state, draft, cfg.per_day) {
            Ok(p) if p.ask => format!(
                "requested {}: the person will be asked when the turn ends; go on with what does not need it",
                p.id
            ),
            Ok(p) => format!(
                "requested {id}: today's questions to the person are used up, so the chat will not ask; the person can answer with `clio approve {id}`. Go on with what does not need it",
                id = p.id
            ),
            Err(e) => return refuse(format!("the request could not be stored ({e})")),
        };
        Response {
            id: req.id.clone(),
            ok: true,
            content: content.clone(),
            data: Value::Null,
            decision: DecisionOut {
                outcome: "allow".into(),
                rule: "request_secret".into(),
                reason: content,
            },
            tainted,
            external: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_approved_command_without_a_limit_gets_the_longest_and_the_model_is_told() {
        let mut p = Pending {
            id: "ab12".into(),
            class: Class::Delete,
            tool: "exec".into(),
            args: json!({"command": "rm -rf ~/x"}),
            hash: String::new(),
            what: String::new(),
            run: "r".into(),
            session: "s".into(),
            created_ms: 0,
            expires_ms: 1,
            ask: true,
            state: State::Waiting,
            reason: None,
        };
        assert_eq!(run_args(&p)["timeout"], MAX_TIMEOUT_SECS);
        assert!(aside_text(&p).contains("time limit of 3600 s"));
        p.args = json!({"command": "rm -rf ~/x", "timeout": 20});
        assert_eq!(run_args(&p)["timeout"], 20);
        assert!(!aside_text(&p).contains("time limit"));
        p.tool = "fs_write".into();
        p.args = json!({"path": "/x", "content": ""});
        assert!(run_args(&p).get("timeout").is_none());
    }

    /// Items written before the classes had English names (`borrar`, `gastar`, `enviar`) still
    /// load, and are written back with the English name.
    #[test]
    fn a_pending_item_written_with_a_spanish_class_still_loads() {
        let state = std::env::temp_dir().join(format!("clio-pending-old-{}", std::process::id()));
        let _ = fs::remove_dir_all(&state);
        for (old, class, new) in [
            ("borrar", Class::Delete, "delete"),
            ("gastar", Class::Spend, "spend"),
            ("enviar", Class::Send, "send"),
            ("control", Class::Control, "control"),
        ] {
            let id = format!("a{old}");
            let item = format!(
                r#"{{"id":"{id}","class":"{old}","tool":"exec","args":{{"command":"x"}},"hash":"h","what":"x","run":"r","session":"s","created_ms":0,"expires_ms":1,"ask":true,"state":"waiting"}}"#
            );
            write_private(&file(&state, &id).unwrap(), &item).unwrap();
            let p = load(&state, &id).unwrap();
            assert_eq!(p.class, class, "{old}");
            save(&state, &p).unwrap();
            let again = fs::read_to_string(file(&state, &id).unwrap()).unwrap();
            assert!(again.contains(&format!("\"class\": \"{new}\"")), "{again}");
        }
        let _ = fs::remove_dir_all(&state);
    }
}
