//! Host mode: what the list (`ask`) sets aside waits here for the person, in
//! `state/pending/<id>.json`. Nothing set aside runs until the person approves it, and then it
//! runs as it was saved (same hash), once, and never after it expired (24 hours). The person's yes
//! and a permission window's (`grant`) go through the same functions here; `granted` says which.

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
/// How many items of one session may wait for the person at once.
pub const MAX_WAITING: usize = 20;
/// What the model is told past it.
const TOO_MANY: &str = "too many actions wait for the person: wait for their answers";
/// An item this long past its time (answered or not) is removed when the store is read.
const KEEP_MS: i64 = 7 * DAY_MS;
/// What is kept of an approved action's output.
const TAIL_BYTES: usize = 4000;
/// How long the reason of a secret request may be.
const MAX_WHY: usize = 200;

/// What an approved call ran into: ok, its text, its data, and whether that text is outside text.
pub(crate) type Ran = (bool, String, Value, bool);

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
    /// The order it was set aside in this store, for items of the same millisecond (filled by
    /// `set_aside`; 0 in items older than it, which then go by id).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub seq: u64,
    pub expires_ms: i64,
    pub state: State,
    /// Why it waits when its class alone would not say it (a suspect session reaching out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Approved by the permission window of this id (`grant`), not by the person's own yes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted: Option<String>,
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
/// belong to and the vault's person socket (only `Status` and `List` are asked, at request time).
#[derive(Debug, Clone)]
pub struct SecretCfg {
    pub state: PathBuf,
    pub run: String,
    pub person: PathBuf,
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
    granted: Option<&str>,
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
    p.granted = granted.map(str::to_owned);
    save(state, &p)?;
    if let Err(e) = log_line(
        state,
        &p.run,
        &p.session,
        &p.tool,
        &p.what,
        false,
        "approved",
        approved_rule(granted, "host/approved"),
        None,
    ) {
        eprintln!("clio-toolhost: the action log lost a line: {e}");
    }
    Ok(p)
}

/// Name, description, SKILL.md and the other files (`path`, `content`) of a skill set aside.
pub type SkillOf<'a> = (&'a str, &'a str, &'a str, Vec<(&'a str, &'a str)>);

/// What a skill waiting to be registered is, read from its arguments: name, description, SKILL.md
/// and its files (`path`, `content`).
pub fn skill_of(p: &Pending) -> Option<SkillOf<'_>> {
    made_of(p, Class::Skill)
}

/// A service waiting to be registered (S-69), read the same way: name, description, its
/// `service.json` (kept as `skill_md`) and its files.
pub fn service_of(p: &Pending) -> Option<SkillOf<'_>> {
    made_of(p, Class::Service)
}

/// An MCP server waiting to be installed (S-70): name, description, its `mcp.json` (kept as
/// `skill_md`) and no files. npm's facts about it are `p.args["npm"]`.
pub fn mcp_of(p: &Pending) -> Option<SkillOf<'_>> {
    made_of(p, Class::Mcp)
}

fn made_of(p: &Pending, class: Class) -> Option<SkillOf<'_>> {
    let s = |k: &str| p.args.get(k).and_then(Value::as_str).unwrap_or("");
    let files = p.args["files"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|f| {
                    let g = |k: &str| f.get(k).and_then(Value::as_str).unwrap_or("");
                    (g("path"), g("content"))
                })
                .collect()
        })
        .unwrap_or_default();
    // A request about what Clio created (`manage`) shares the class, never the shape.
    (p.class == class && !crate::manage::is_manage(p))
        .then(|| (s("name"), s("description"), s("skill_md"), files))
}

/// The person's `clio` sets aside a reviewed skill draft (S-68): `args` holds the whole draft
/// (name, description, `skill_md`, `files`, `sha256`, `draft`), `reason` why it waits. Same store
/// as the rest; the model has no way here.
pub fn set_aside_skill(
    state: &Path,
    run: &str,
    session: &str,
    args: Value,
    reason: &str,
) -> Result<Pending, String> {
    set_aside_made(
        state,
        (run, session),
        args,
        reason,
        (Class::Skill, "skill_write"),
    )
}

/// A reviewed service draft, as `set_aside_skill` (S-69): it always waits for the person.
pub fn set_aside_service(
    state: &Path,
    run: &str,
    session: &str,
    args: Value,
    reason: &str,
) -> Result<Pending, String> {
    set_aside_made(
        state,
        (run, session),
        args,
        reason,
        (Class::Service, "service_write"),
    )
}

/// A skill from outside (`skill_add`, the design of 2026-11-20): `args` as
/// `skill_add::prepare` made them. It waits for the person's own yes, always.
pub fn set_aside_added(
    state: &Path,
    run: &str,
    session: &str,
    args: Value,
) -> Result<Pending, String> {
    set_aside_made(
        state,
        (run, session),
        args,
        crate::skill_add::WAITS,
        (Class::Skill, crate::skill_add::TOOL),
    )
}

/// An MCP server the model proposed, with npm's facts (S-70): it always waits for the person.
pub fn set_aside_mcp(
    state: &Path,
    run: &str,
    session: &str,
    args: Value,
    reason: &str,
) -> Result<Pending, String> {
    set_aside_made(
        state,
        (run, session),
        args,
        reason,
        (Class::Mcp, "mcp_propose"),
    )
}

fn set_aside_made(
    state: &Path,
    (run, session): (&str, &str),
    args: Value,
    reason: &str,
    (class, tool): (Class, &str),
) -> Result<Pending, String> {
    let rule = match class {
        Class::Service => "services/aside",
        Class::Mcp => "mcp/aside",
        _ => "skills/aside",
    };
    let now = now_ms();
    let name = args["name"].as_str().unwrap_or("").to_owned();
    let p = Pending {
        id: String::new(),
        class,
        tool: tool.into(),
        args,
        hash: String::new(),
        what: name,
        run: run.to_owned(),
        session: session.to_owned(),
        created_ms: now,
        seq: 0,
        expires_ms: now + DAY_MS,
        state: State::Waiting,
        reason: Some(reason.to_owned()),
        granted: None,
    };
    let p = set_aside(state, p)?;
    if let Err(e) = log_line(
        state, run, session, &p.tool, &p.what, false, "aside", rule, None,
    ) {
        eprintln!("clio-toolhost: the action log lost a line: {e}");
    }
    Ok(p)
}

/// The person approves skill `id`: `register` (the person's `clio`) writes it to the skills
/// folder as it was stored (same hash), once; a failed registration leaves it waiting.
pub fn approve_skill(
    state: &Path,
    id: &str,
    (now_ms, granted): (i64, Option<&str>),
    register: impl FnOnce(&Pending) -> Result<String, String>,
) -> Result<Pending, String> {
    approve_made(state, id, (now_ms, granted), Class::Skill, register)
}

/// The person approves MCP server `id` (S-70), as `approve_skill`: `install` downloads, checks,
/// probes and registers it, off.
pub fn approve_mcp(
    state: &Path,
    id: &str,
    (now_ms, granted): (i64, Option<&str>),
    install: impl FnOnce(&Pending) -> Result<String, String>,
) -> Result<Pending, String> {
    approve_made(state, id, (now_ms, granted), Class::Mcp, install)
}

/// The person approves service `id` (S-69), as `approve_skill`.
pub fn approve_service(
    state: &Path,
    id: &str,
    (now_ms, granted): (i64, Option<&str>),
    register: impl FnOnce(&Pending) -> Result<String, String>,
) -> Result<Pending, String> {
    approve_made(state, id, (now_ms, granted), Class::Service, register)
}

/// The person (or, when `granted`, their permission window) approves request `id` about what Clio
/// created (`manage`): `act` (the person's `clio`, with the person's own commands' functions)
/// carries it out as it was stored (same hash), once; a failure leaves it waiting.
pub fn approve_manage(
    state: &Path,
    id: &str,
    (now_ms, granted): (i64, Option<&str>),
    act: impl FnOnce(&Pending) -> Result<String, String>,
) -> Result<Pending, String> {
    let p = check(state, id, now_ms)?;
    if !crate::manage::is_manage(&p) {
        return Err(format!("{id} is not a request about what Clio created"));
    }
    approve_made(state, id, (now_ms, granted), p.class, act)
}

/// The action log's rule for an approval: the person's own (`rule`), bypass's or a permission
/// window's.
fn approved_rule<'a>(granted: Option<&str>, rule: &'a str) -> &'a str {
    match granted {
        Some(g) if g.starts_with(crate::grant::BYPASS_ID) => "bypass/approved",
        Some(_) => "grant/approved",
        None => rule,
    }
}

fn approve_made(
    state: &Path,
    id: &str,
    (now_ms, granted): (i64, Option<&str>),
    class: Class,
    register: impl FnOnce(&Pending) -> Result<String, String>,
) -> Result<Pending, String> {
    let mut p = check(state, id, now_ms)?;
    if p.class != class {
        return Err(format!("{id} is not a {class}"));
    }
    // Second lock (`grant` is the first): a skill from outside is the person's own yes, always.
    if p.tool == crate::skill_add::TOOL && granted.is_some() {
        return Err(format!(
            "{id} is a skill from outside: only the person's own yes adds it"
        ));
    }
    if hash_of(&p.tool, &p.args) != p.hash {
        return Err(format!(
            "{id} changed after it was set aside: nothing was registered"
        ));
    }
    claim(state, id).map_err(|_| format!("{id} was already answered"))?;
    let done = register(&p).inspect_err(|_| {
        let _ = fs::remove_file(dir(state).join(format!("{id}.ran")));
    })?;
    p.state = State::Done {
        exit_code: None,
        output_tail: done,
    };
    p.granted = granted.map(str::to_owned);
    save(state, &p)?;
    if let Err(e) = log_line(
        state,
        &p.run,
        &p.session,
        &p.tool,
        &p.what,
        false,
        "approved",
        approved_rule(
            granted,
            match class {
                Class::Service => "services/approved",
                Class::Mcp => "mcp/approved",
                _ => "skills/approved",
            },
        ),
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
        .or_else(|| p.args.get("call"))
        .or_else(|| p.args.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// `p` was taken for an answer that never wrote what came of it (Clio ended while it ran): it may
/// have run in part, so it is not waiting any more and is never offered again.
pub fn interrupted(state: &Path, p: &Pending) -> bool {
    p.state == State::Waiting && dir(state).join(format!("{}.ran", p.id)).exists()
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
pub fn local(ms: i64, fmt: &str) -> Option<String> {
    let secs = ms.div_euclid(1000).to_string();
    // GNU `date` (Linux) takes `--date=@<secs>`, BSD's (macOS) `-r <secs>`; `/bin/date` is on both.
    [vec![format!("--date=@{secs}")], vec!["-r".to_owned(), secs]]
        .iter()
        .find_map(|args| {
            let o = std::process::Command::new("/bin/date")
                .args(args)
                .arg(format!("+{fmt}"))
                .stderr(std::process::Stdio::null())
                .output()
                .ok()?;
            o.status
                .success()
                .then(|| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        })
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

/// Every pending action of this machine, oldest first (of one millisecond, in the order they
/// were set aside). One more than 7 days past its time is removed here (with its claim), best
/// effort.
pub fn list(state: &Path) -> Vec<Pending> {
    let now = now_ms();
    let mut all: Vec<Pending> = fs::read_dir(dir(state))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_str(&fs::read_to_string(e.path()).ok()?).ok())
        .filter(|p: &Pending| {
            // Past its time every item is over, answered or not.
            if now.saturating_sub(p.expires_ms) > KEEP_MS {
                let _ = fs::remove_file(dir(state).join(format!("{}.json", p.id)));
                let _ = fs::remove_file(dir(state).join(format!("{}.ran", p.id)));
                return false;
            }
            true
        })
        .collect();
    all.sort_by(|a, b| (a.created_ms, a.seq, &a.id).cmp(&(b.created_ms, b.seq, &b.id)));
    all
}

fn is_zero(n: &u64) -> bool {
    *n == 0
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
    if matches!(
        p.class,
        Class::Memory | Class::Skill | Class::Service | Class::Mcp
    ) && let Err(e) = log_line(
        state,
        &p.run,
        &p.session,
        &p.tool,
        &p.what,
        false,
        "rejected",
        "host/rejected",
        None,
    ) {
        eprintln!("clio-toolhost: the action log lost a line: {e}");
    }
    Ok(p)
}

/// Saves `p` (id and hash are filled here), or returns the one already waiting for the same
/// action in the same session. A session with `MAX_WAITING` items waiting gets no more.
pub(crate) fn set_aside(state: &Path, mut p: Pending) -> Result<Pending, String> {
    p.hash = hash_of(&p.tool, &p.args);
    let all = list(state);
    p.seq = all.iter().map(|o| o.seq).max().unwrap_or(0) + 1;
    let waiting: Vec<Pending> = all
        .into_iter()
        .filter(|o| {
            o.session == p.session && o.state == State::Waiting && o.expires_ms > p.created_ms
        })
        .collect();
    if let Some(old) = waiting.iter().find(|o| o.hash == p.hash) {
        return Ok(old.clone());
    }
    if waiting.len() >= MAX_WAITING {
        return Err(TOO_MANY.into());
    }
    for n in 0u32.. {
        let seed = format!("{}|{}|{}|{n}", p.session, p.hash, p.created_ms);
        let h = Sha256::digest(seed.as_bytes());
        p.id = h[..4].iter().map(|b| format!("{b:02x}")).collect();
        if !file(state, &p.id)?.exists() {
            break;
        }
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
        cancel: &dyn Fn() -> bool,
    ) -> Response {
        let at = (h.cfg.state.as_path(), h.cfg.run.as_str());
        self.aside_at(at, req, req.args.clone(), (class, reason), cancel)
    }

    /// In a review (it cannot ask the person) the action is refused outright, nothing is stored as
    /// pending, and the refusal goes to the action log (`at`: its state folder and run).
    pub(crate) fn refuse_in_review(
        &self,
        at: Option<(&Path, &str)>,
        req: &Request,
        args: &Value,
        rule: &str,
        why: &str,
    ) -> Response {
        if let Some((state, run)) = at
            && crate::host::LOGGED.contains(&req.tool.as_str())
        {
            let what = args["command"].as_str().or(args["path"].as_str());
            let what = self.clean(what.unwrap_or(""));
            let network = args["network"].as_bool() == Some(true);
            if let Err(e) = log_line(
                state,
                run,
                &req.session,
                &req.tool,
                &what,
                network,
                "deny",
                rule,
                None,
            ) {
                eprintln!("clio-toolhost: the action log lost a line: {e}");
            }
        }
        let content = format!("[toolhost] {} was not run: {why}", self.clean(&req.tool));
        Response {
            id: req.id.clone(),
            ok: false,
            content: content.clone(),
            data: Value::Null,
            decision: DecisionOut {
                outcome: "deny".into(),
                rule: rule.into(),
                reason: content,
            },
            tainted: self.is_tainted(&req.session),
            external: false,
        }
    }

    /// `aside` with where it waits given apart (`state`, `run`) and the arguments it keeps: a
    /// block change waits in a chat that is not in host mode too.
    pub(crate) fn aside_at(
        &self,
        (state, run): (&Path, &str),
        req: &Request,
        args: Value,
        (class, reason): (Class, Option<&str>),
        cancel: &dyn Fn() -> bool,
    ) -> Response {
        if self.review {
            return self.refuse_in_review(
                Some((state, run)),
                req,
                &args,
                "review/aside",
                crate::REVIEW_CANNOT_ASK,
            );
        }
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
            seq: 0,
            expires_ms: now + DAY_MS,
            state: State::Waiting,
            reason: reason.map(str::to_owned),
            granted: None,
        };
        draft.what = self.clean(shown(&draft));
        let (content, rule) = match set_aside(state, draft) {
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
                match self.granted(state, req, &p, cancel) {
                    Ok(done) => return done,
                    Err(None) => (aside_text(&p), "host/aside"),
                    Err(Some(why)) => (
                        format!(
                            "{} (It could not run under the person's {}: {why}.)",
                            aside_text(&p),
                            if crate::grant::Kind::Bypass.current(state).is_some() {
                                "bypass"
                            } else {
                                "permission window"
                            }
                        ),
                        "host/aside",
                    ),
                }
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

    /// Bypass is on, or a permission window is open (`grant`): what was just set aside (`p`, for
    /// call `req`) is approved by it at once when it may answer it (`grant::answering`: never
    /// what changes Clio's own guards, a secret or a block change; a window, besides, never
    /// control, a service or an MCP server), through the same path as the person's yes, recorded
    /// in its record before it runs, and its real result goes back as the call's own. Never in a
    /// `suspect` session, never in a review or a round, never with the judge off (it then
    /// closes). `Err(None)`: it waits; `Err(Some(why))`: it could not do it, and it waits.
    fn granted(
        &self,
        state: &Path,
        req: &Request,
        p: &Pending,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Response, Option<String>> {
        // A session whose judge did not answer acts under neither: it asks (S-72).
        if self.review
            || self.unattended
            || self.is_suspect(&req.session)
            || self.is_unjudged(&req.session)
        {
            return Err(None);
        }
        let w = crate::grant::answering(state, p).ok_or(None)?;
        let kind = crate::grant::Kind::of(&w);
        if !self.memory.as_ref().is_some_and(|m| m.judge_on()) {
            if let Err(e) = kind.close(state, crate::grant::JUDGE_OFF) {
                eprintln!(
                    "clio-toolhost: the {} could not be closed: {e}",
                    kind.name()
                );
            }
            return Err(None);
        }
        let w = kind.begin(state, p, &p.what).ok_or(None)?;
        let now = now_ms();
        let (ok, content, data, external) = self
            .approve_as(&p.id, now, Some(&w.id), Some((req, cancel)))
            .map(|(_, ran)| ran)
            .map_err(|e| {
                crate::grant::done(state, &w, &p.id, crate::grant::NOT_RUN);
                Some(e)
            })?;
        let code = data.get("exit_code").and_then(Value::as_i64);
        crate::grant::done(state, &w, &p.id, &crate::grant::outcome(ok, code));
        let rule = approved_rule(Some(&w.id), "");
        if p.tool == "mcp_call" {
            return Ok(self.mcp_outside(req, ok, rule, &content, &p.what));
        }
        let reason = if w.bypass {
            "approved under the person's bypass (their standing yes), so it ran".to_owned()
        } else {
            let left = crate::grant::left_text(w.ends_ms - now);
            format!("approved under the person's permission window ({left} left), so it ran")
        };
        Ok(Response {
            id: req.id.clone(),
            ok,
            content,
            data,
            decision: DecisionOut {
                outcome: "allow".into(),
                rule: rule.into(),
                reason,
            },
            tainted: self.is_tainted(&req.session),
            external,
        })
    }

    /// The person approves `id`: it runs as it was saved, through the same path as an allowed
    /// action (log, copy before change) and without being classified again. Only a waiting,
    /// unexpired, unchanged pending of this host's run runs, and only once. Works from a fresh
    /// `ToolHost` built in host mode with that run's `run_dir` and `HostCfg.run`.
    pub fn approve(&self, id: &str, now_ms: i64) -> Result<Pending, String> {
        self.approve_by(id, now_ms, None)
    }

    /// `approve`, saying who approved: the person (`None`) or the permission window `granted`
    /// (the brain approves what waits while one is open).
    pub fn approve_by(
        &self,
        id: &str,
        now_ms: i64,
        granted: Option<&str>,
    ) -> Result<Pending, String> {
        let _one_at_a_time = lock(&self.busy);
        self.approve_as(id, now_ms, granted, None).map(|(p, _)| p)
    }

    /// The one path of every yes: the person's (`granted` is `None`), or a permission window's
    /// (`granted`: its id). `call`: the call it answers when the window approves it inline, while
    /// that call holds the host, so the result is that call's own (judged and marked as any), it
    /// stops when that call is cancelled, and a command keeps its own time limit and runs in the
    /// background when it asked to, as an unwindowed run would. What ran, and its result (ok,
    /// text, data, outside text).
    pub(crate) fn approve_as(
        &self,
        id: &str,
        now_ms: i64,
        granted: Option<&str>,
        call: Option<(&Request, &dyn Fn() -> bool)>,
    ) -> Result<(Pending, Ran), String> {
        let h = self
            .host
            .as_ref()
            .ok_or("this service is not in host mode")?;
        let rule = approved_rule(granted, "host/approved");
        let granted = granted.map(str::to_owned);
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
        if matches!(p.class, Class::Skill | Class::Service | Class::Mcp) {
            return Err(format!(
                "{id} is a {}: it is registered by `clio approve`, not run",
                p.class
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
        // An MCP call: the exact server, tool and arguments shown, through the session's client.
        if p.tool == "mcp_call" {
            claim(state, id).map_err(|_| format!("{id} already ran"))?;
            let (ok, raw) = self.mcp_approved(&p.args).inspect_err(|_| {
                let _ = fs::remove_file(dir(state).join(format!("{id}.ran")));
            })?;
            let text = self.clean(&raw);
            if let Err(e) = log_line(
                state,
                &p.run,
                &p.session,
                "mcp_call",
                &p.what,
                false,
                if ok { "approved" } else { "failed" },
                rule,
                None,
            ) {
                eprintln!("clio-toolhost: the action log lost a line: {e}");
            }
            p.state = State::Done {
                exit_code: Some(i64::from(!ok)),
                output_tail: tail(&text),
            };
            p.granted = granted;
            save(state, &p)?;
            return Ok((p, (ok, raw, Value::Null, true)));
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
        // Under a window, as the call it answers: its result is judged when the guard filters it.
        let (rid, call_id, args) = call.map_or_else(
            || (id.to_owned(), "approve".to_owned(), run_args(&p)),
            |(r, _)| (r.id.clone(), r.call_id.clone(), p.args.clone()),
        );
        let req = Request {
            id: rid,
            tool: p.tool.clone(),
            args,
            session: p.session.clone(),
            call_id,
            tainted: false,
        };
        let background = req.args.get("background").and_then(Value::as_bool) == Some(true);
        let ran = match call {
            Some(_) if p.tool == "exec" && background => {
                let r = self.job_start(
                    h,
                    &req,
                    DecisionOut {
                        outcome: "allow".into(),
                        rule: rule.into(),
                        reason: "approved under the person's permission window".into(),
                    },
                );
                Ok((r.ok, r.content, r.data, r.external))
            }
            _ => self.host_run(
                h,
                &req,
                format!("approve:{id}"),
                rule,
                real.as_deref(),
                call.map_or(&|| false, |(_, c)| c),
            ),
        };
        // It did not run (its line before could not be logged): it still waits.
        let ran = ran.inspect_err(|_| {
            let _ = fs::remove_file(dir(state).join(format!("{id}.ran")));
        })?;
        p.state = State::Done {
            exit_code: ran.2.get("exit_code").and_then(Value::as_i64),
            output_tail: tail(&ran.1),
        };
        p.granted = granted;
        save(state, &p)?;
        Ok((p, ran))
    }

    /// `request_secret`: checks the name, the destination and the vault now, and stores the
    /// request for the person; the chat asks for the value (when it opens the boxes, or at once
    /// with `/pending`), and it never comes through here.
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
            seq: 0,
            expires_ms: now + DAY_MS,
            state: State::Waiting,
            reason: None,
            granted: None,
        };
        let content = match set_aside(&cfg.state, draft) {
            Ok(p) => format!(
                "requested {}: the person is asked for it in the chat when this turn ends (a permission window never answers a secret), and their answer reaches you in a later turn; go on with what does not need it",
                p.id
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

/// A waiting delete of `session`, for tests.
#[cfg(test)]
pub(crate) fn tests_item(session: &str) -> Pending {
    tests_item_at(session, 0, now_ms())
}

#[cfg(test)]
fn tests_item_at(session: &str, n: usize, now: i64) -> Pending {
    Pending {
        id: String::new(),
        class: Class::Delete,
        tool: "exec".into(),
        args: json!({"command": format!("rm -r /srv/old{n}")}),
        hash: String::new(),
        what: String::new(),
        run: "r".into(),
        session: session.into(),
        created_ms: now,
        seq: 0,
        expires_ms: now + DAY_MS,
        state: State::Waiting,
        reason: None,
        granted: None,
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
            seq: 0,
            expires_ms: 1,
            state: State::Waiting,
            reason: None,
            granted: None,
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

    fn item(session: &str, n: usize, now: i64) -> Pending {
        super::tests_item_at(session, n, now)
    }

    // A session has at most 20 items waiting: the 21st is refused with a text for the model,
    // the same action again still gets its id, and another session is not counted.
    #[test]
    fn at_most_twenty_items_of_a_session_wait() {
        let t = tempfile::tempdir().unwrap();
        let now = now_ms();
        let ids: Vec<String> = (0..MAX_WAITING)
            .map(|n| set_aside(t.path(), item("s", n, now)).unwrap().id)
            .collect();
        assert_eq!(
            set_aside(t.path(), item("s", 99, now)).unwrap_err(),
            "too many actions wait for the person: wait for their answers"
        );
        assert_eq!(set_aside(t.path(), item("s", 3, now)).unwrap().id, ids[3]);
        assert!(set_aside(t.path(), item("other", 99, now)).is_ok());
        // One answered makes room.
        reject(t.path(), &ids[0]).unwrap();
        assert!(set_aside(t.path(), item("s", 99, now)).is_ok());
    }

    // Items of the same millisecond are listed in the order they were set aside, whatever their
    // ids; an older item without a sequence goes first, by time.
    #[test]
    fn items_of_one_millisecond_are_listed_as_they_were_set_aside() {
        let t = tempfile::tempdir().unwrap();
        let now = now_ms();
        let mut old = item("s", 100, now - 1);
        old.id = "ffffffff".into();
        save(t.path(), &old).unwrap();
        let ids: Vec<String> = (0..8)
            .map(|n| set_aside(t.path(), item("s", n, now)).unwrap().id)
            .collect();
        let listed: Vec<String> = list(t.path()).into_iter().map(|p| p.id).collect();
        assert_eq!(listed[0], "ffffffff");
        assert_eq!(listed[1..], ids[..]);
    }

    // A permission window never answers a secret request: the value is the person's to type.
    #[test]
    fn a_window_never_answers_a_secret() {
        let t = tempfile::tempdir().unwrap();
        let st = t.path().join("state");
        std::fs::create_dir_all(t.path().join("home")).unwrap();
        let host = ToolHost::new(crate::Config::new(
            &t.path().join("home"),
            &t.path().join("scratch"),
            &t.path().join("run"),
        ))
        .unwrap();
        crate::grant::open(&st, 4, &["a1".into()]).unwrap().unwrap();
        let mut p = super::tests_item("s");
        p.class = Class::Secret;
        p.tool = "request_secret".into();
        p.args = json!({"name": "t", "destination": "http_fetch:api.example.com", "why": "x"});
        let req = Request {
            id: "i".into(),
            tool: "request_secret".into(),
            args: p.args.clone(),
            session: "s".into(),
            call_id: "c".into(),
            tainted: false,
        };
        assert!(matches!(host.granted(&st, &req, &p, &|| false), Err(None)));
        assert!(
            crate::grant::record(&st, &crate::grant::read(&st).unwrap().id)
                .unwrap()
                .1
                .is_empty()
        );
    }

    // Reading the store removes what is more than a week past its time, answered or not, with
    // its claim; what is younger stays.
    #[test]
    fn items_a_week_past_their_time_are_removed_when_the_store_is_read() {
        let t = tempfile::tempdir().unwrap();
        let now = now_ms();
        let mut old = item("s", 1, now - 9 * DAY_MS);
        old.id = "aa11".into();
        old.state = State::Rejected;
        save(t.path(), &old).unwrap();
        fs::write(dir(t.path()).join("aa11.ran"), "").unwrap();
        let mut stale = item("s", 2, now - 9 * DAY_MS);
        stale.id = "bb22".into();
        save(t.path(), &stale).unwrap();
        let mut recent = item("s", 3, now - 3 * DAY_MS);
        recent.id = "cc33".into();
        recent.state = State::Rejected;
        save(t.path(), &recent).unwrap();
        let ids: Vec<String> = list(t.path()).into_iter().map(|p| p.id).collect();
        assert_eq!(ids, ["cc33"]);
        assert!(!dir(t.path()).join("aa11.ran").exists());
        assert!(!dir(t.path()).join("bb22.json").exists());
    }
}
