//! The tool host: the only thing in a Pi session that runs effects on the machine (ADR 0007).
//!
//! Pi keeps its own tools (`read`, `write`, `edit`, `bash`, `find`, `ls`: truncation, diff, path
//! resolution, rendering). The `clio-guard` extension builds them with custom `operations`, and
//! each primitive operation (read a file, write a file, stat, mkdir, list, glob, run a command)
//! is a request to this service over a Unix socket. For every request the host does, in this
//! order, and fails closed at each step:
//!
//! 1. the gateway decides the normalized operation (`Gateway::decide_or_deny`, rules in JSON,
//!    deny by default);
//! 2. on an allow, the `clio-node` executor runs it inside the session's working copy (sandbox,
//!    canonical paths, symlinks out rejected, protected folders); a command (`exec`) runs ONLY in
//!    the `bwrap` jail, and only if the jail's self-test passed;
//! 3. the output is cleaned (escape sequences out) and scrubbed (known secret values and key
//!    patterns out);
//! 4. external content marks the session as tainted; the mark never goes away and is reported in
//!    every answer (S-05). The `filter` operation cleans, scrubs and wraps (`wrap_untrusted`) the
//!    final text of any tool result, whatever Pi made of it.
//!
//! Nothing in this crate talks to the network. See `README.md` for the protocol.

pub mod cli;
mod memory;
mod ops;
mod sanitize;
mod server;

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use clio_core::{Level, SystemClock};
use clio_gateway::{
    ActionRequest, ArgKind, ArgSpec, Budget, Decision, Egress, Gateway, Outcome, Rules,
    SessionInfo, Source, Telemetry, ToolDecl, wrap_untrusted,
};
use clio_node::{Action, EffectLog, Executor, Jail, Registry, Sandbox};
use clio_vault::{Scrubber, VaultClient, count_masks, has_mask};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

pub use memory::{Found, MemoryItem, MemoryPort, Origin};
pub use sanitize::sanitize;
pub use server::Server;

/// The operations the host knows (`filter` is handled apart: it has no effect). Anything else is
/// denied by the gateway (S-02). `exec` is declared only when the jail is proven.
pub const OPS: [&str; 7] = [
    "fs_read", "fs_stat", "fs_list", "fs_glob", "fs_write", "fs_mkdir", "exec",
];

/// Folders under the working copy that load code or settings by themselves (S-16): Pi
/// extensions, skills, hooks, git. Nothing here may write them. Mandatory: a rules file can add
/// to this list, never remove from it. `.agents` is where Pi looks for project skills.
pub const PROTECTED_PATHS: [&str; 11] = clio_node::READ_ONLY;

/// A request line longer than this is refused and the connection closed.
pub const MAX_REQUEST_BYTES: usize = 4 << 20;
/// The largest `content` of an `fs_write`.
const MAX_WRITE_BYTES: usize = 1 << 20;
/// What a tool may produce before it is cleaned: above this the output is withheld, not cut, so
/// a secret is never split by a cut before the scrubber saw it whole.
const MAX_RAW_BYTES: usize = 2 << 20;
/// A command longer than this is refused.
const MAX_COMMAND_BYTES: usize = 8192;
/// Breaker: this many denials in a row and the session stops being let through on its own.
const BREAKER: u32 = 10;
const MAX_SESSIONS: usize = 256;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: String,
    pub tool: String,
    pub args: Value,
    /// The Pi session id: taint and the gateway's breaker are kept per session.
    pub session: String,
    pub call_id: String,
    /// The caller's memory of the session's taint (restored from Pi's session file). It can only
    /// raise the mark here, never clear it.
    #[serde(default)]
    pub tainted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionOut {
    /// `allow`, `ask` or `deny`.
    pub outcome: String,
    pub rule: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub id: String,
    /// False: the tool did not run, or failed; `content` says why.
    pub ok: bool,
    /// Text of the answer (the cleaned output of a read or a command), or why the call failed.
    pub content: String,
    /// Structured part (listing, stat, exit code, filtered texts); `null` if there is none.
    #[serde(default)]
    pub data: Value,
    pub decision: DecisionOut,
    /// The session's taint after this call. It only ever goes from false to true.
    pub tainted: bool,
    /// The answer carries text from outside (a file, a listing, command output).
    pub external: bool,
}

/// Who answers a gateway "ask". In this phase nobody does: the default says no.
pub trait Asker: Send + Sync {
    fn approve(&self, decision: &Decision, tool: &str, args: &Map<String, Value>) -> bool;
}

pub struct NobodyHome;

impl Asker for NobodyHome {
    fn approve(&self, _: &Decision, _: &str, _: &Map<String, Value>) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JailMode {
    /// No `exec`.
    Off,
    /// `exec` only if `bwrap` is there, starts, and the jail's self-test passes.
    Auto,
}

pub struct Config {
    /// The session's working copy: the only folder the tools may touch.
    pub work: PathBuf,
    /// Snapshots and probes; must be outside `work`.
    pub scratch: PathBuf,
    /// The effect log (what was applied, per action id).
    pub run_dir: PathBuf,
    pub jail: JailMode,
    /// Known secret values to scrub besides the key patterns. `None`: patterns only.
    pub scrubber: Option<Scrubber>,
    /// The vault's gateway socket: its filter runs too, and if it cannot answer the output is
    /// withheld.
    pub vault: Option<VaultClient>,
    /// Extra rules (breaker threshold, more protected paths, ...). The working folder, the tool
    /// declarations and the mandatory protected paths are always the host's own.
    pub rules: Option<Rules>,
    /// The person's memory, read only. Without it `memory_search` and `memory_get` are not
    /// declared and the gateway denies them (S-02): the loose `clio-toolhost` binary has none.
    pub memory: Option<Arc<dyn MemoryPort>>,
}

impl Config {
    pub fn new(work: &Path, scratch: &Path, run_dir: &Path) -> Self {
        Self {
            work: work.to_path_buf(),
            scratch: scratch.to_path_buf(),
            run_dir: run_dir.to_path_buf(),
            jail: JailMode::Auto,
            scrubber: None,
            vault: None,
            rules: None,
            memory: None,
        }
    }
}

#[derive(Default)]
struct Taint {
    sources: Vec<&'static str>,
}

/// An approval for exactly the action the gateway just allowed.
struct Approved(String);

impl clio_node::Approval for Approved {
    fn is_valid_for(&self, hash: &str) -> bool {
        self.0 == hash
    }
}

pub struct ToolHost {
    gateway: Gateway,
    executor: Executor,
    descriptions: HashMap<String, String>,
    scrubber: Scrubber,
    vault: Option<VaultClient>,
    asker: Box<dyn Asker>,
    sessions: Mutex<HashMap<String, Taint>>,
    /// One call at a time: the tools change the working copy and the gateway keeps counters.
    busy: Mutex<()>,
    counter: AtomicU64,
    exec_status: Result<(), String>,
    memory: Option<Arc<dyn MemoryPort>>,
    memory_timeout: Duration,
    /// Where the memory tools keep their budget and their log.
    run_dir: PathBuf,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn digest_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn arg_specs(tool: &str) -> Vec<ArgSpec> {
    let a = |name: &str, kind, required| ArgSpec {
        name: name.into(),
        kind,
        required,
    };
    let path = a("path", ArgKind::Path, true);
    match tool {
        "fs_read" => vec![path, a("raw", ArgKind::Bool, false)],
        "fs_stat" => vec![path, a("mode", ArgKind::Text, false)],
        "fs_glob" => vec![
            path,
            a("pattern", ArgKind::Text, true),
            a("limit", ArgKind::Int, false),
        ],
        "fs_write" => vec![path, a("content", ArgKind::Text, true)],
        "exec" => vec![
            a("command", ArgKind::Text, true),
            a("timeout", ArgKind::Int, false),
        ],
        _ => vec![path],
    }
}

/// Valid session ids: what Pi uses (UUIDs) and a bit more, never control characters.
fn valid_session(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b':'))
}

impl ToolHost {
    pub fn new(cfg: Config) -> Result<Self, String> {
        let e = |x: &dyn std::fmt::Display| x.to_string();
        let exec_status = match cfg.jail {
            JailMode::Off => Err("the jail is switched off".to_owned()),
            JailMode::Auto => prove_jail(),
        };
        let mut registry = Registry::new();
        for t in ops::tools(exec_status.is_ok()) {
            registry.register(t).map_err(|x| e(&x))?;
        }
        let (mut tools, mut descriptions) = (Vec::new(), HashMap::new());
        for name in OPS {
            let Some(found) = registry.get(name) else {
                continue; // `exec` without a proven jail: no declaration, so a deny (S-02)
            };
            let hash = digest_hex(&found.tool.description);
            tools.push(ToolDecl {
                name: name.into(),
                version: "1".into(),
                effect: found
                    .tool
                    .effect
                    .ok_or_else(|| format!("tool {name} has no declared effect"))?,
                egress: Egress::None,
                description_hash: hash.clone(),
                args: arg_specs(name),
            });
            descriptions.insert(name.to_owned(), hash);
        }
        if cfg.memory.is_some() {
            for decl in memory::decls() {
                descriptions.insert(decl.name.clone(), decl.description_hash.clone());
                tools.push(decl);
            }
        }
        let extra = cfg.rules.unwrap_or(Rules {
            sandbox_root: cfg.work.clone(),
            allowed_binaries: vec![],
            trusted_domains: vec![],
            protected_paths: vec![],
            day_zero_complete: false,
            breaker_threshold: BREAKER,
            tools: vec![],
            secret_bindings: vec![],
        });
        let mut protected: Vec<String> = PROTECTED_PATHS.iter().map(|p| (*p).to_owned()).collect();
        for p in extra.protected_paths {
            if !protected.contains(&p) {
                protected.push(p);
            }
        }
        let gateway = Gateway::new(Rules {
            sandbox_root: cfg.work.clone(),
            // `exec` takes a shell string, not an argv: it is not about binaries but about the
            // jail it runs in (see `bash_rule`), so no binary list applies (S-12).
            allowed_binaries: vec![],
            // No tool here has egress: nothing is trusted because nothing goes out.
            trusted_domains: vec![],
            protected_paths: protected,
            day_zero_complete: false,
            breaker_threshold: extra.breaker_threshold,
            tools,
            secret_bindings: vec![],
        })?;
        std::fs::create_dir_all(&cfg.run_dir).map_err(|x| e(&x))?;
        let log = EffectLog::open(&cfg.run_dir.join("effects.jsonl")).map_err(|x| e(&x))?;
        let mut sandbox = Sandbox::new(&cfg.work, &cfg.scratch).map_err(|x| e(&x))?;
        // A program that runs in the folder can edit files in place: snapshots must be copies.
        sandbox.set_copy_snapshots(true);
        Ok(Self {
            gateway,
            executor: Executor {
                registry,
                sandbox,
                log,
            },
            descriptions,
            scrubber: cfg
                .scrubber
                .unwrap_or_else(|| Scrubber::new(Arc::new(SystemClock))),
            vault: cfg.vault,
            asker: Box::new(NobodyHome),
            sessions: Mutex::new(HashMap::new()),
            busy: Mutex::new(()),
            counter: AtomicU64::new(0),
            exec_status,
            memory: cfg.memory,
            memory_timeout: memory::MEMORY_TIMEOUT,
            run_dir: cfg.run_dir,
        })
    }

    /// For tests: how long the memory has to answer (10 s by default).
    #[doc(hidden)]
    #[must_use]
    pub fn with_memory_timeout(mut self, d: Duration) -> Self {
        self.memory_timeout = d;
        self
    }

    #[must_use]
    pub fn with_asker(mut self, a: Box<dyn Asker>) -> Self {
        self.asker = a;
        self
    }

    /// `Ok` when `exec` (bash, in the jail) is offered; otherwise why it is not.
    pub fn exec_available(&self) -> Result<(), &str> {
        self.exec_status.as_ref().copied().map_err(String::as_str)
    }

    pub fn telemetry(&self) -> Telemetry {
        self.gateway.telemetry()
    }

    pub fn is_tainted(&self, session: &str) -> bool {
        lock(&self.sessions).contains_key(session)
    }

    /// One request line in, one response line out. Never panics, never executes on a bad line.
    pub fn handle_line(&self, line: &str) -> String {
        let resp = match serde_json::from_str::<Request>(line) {
            Ok(req) => self.handle(req),
            Err(e) => self.refuse(
                "",
                "protocol",
                format!("the request is not valid: {e}"),
                false,
            ),
        };
        serde_json::to_string(&resp).unwrap_or_else(|_| {
            r#"{"id":"","ok":false,"content":"[toolhost] internal error","data":null,"decision":{"outcome":"deny","rule":"fail-closed","reason":"internal error"},"tainted":true,"external":false}"#.into()
        })
    }

    fn clean(&self, text: &str) -> String {
        self.try_clean(text)
            .unwrap_or_else(|| "[withheld: the vault could not filter this text]".into())
    }

    /// `clean`, but `None` when the vault could not filter: for text that is an input to
    /// something else (a query), where the stand-in text for a withheld output would be wrong.
    fn try_clean(&self, text: &str) -> Option<String> {
        // In this order: a hidden character inside a value must not stop the scrubber.
        let t = self.scrubber.scrub(&sanitize(text));
        match &self.vault {
            None => Some(t),
            Some(v) => v.scrub(&t).ok(),
        }
    }

    fn mark(&self, session: &str, source: &'static str) -> bool {
        let mut s = lock(&self.sessions);
        if s.len() >= MAX_SESSIONS && !s.contains_key(session) {
            return true; // too many sessions: report tainted, which is the safe answer
        }
        let t = s.entry(session.to_owned()).or_default();
        let new = !t.sources.contains(&source);
        if new {
            t.sources.push(source);
        }
        // Every source seen by any session of this host, for the file below.
        let mut all: Vec<&'static str> =
            s.values().flat_map(|t| t.sources.iter().copied()).collect();
        all.sort_unstable();
        all.dedup();
        drop(s);
        self.gateway.mark_tainted(session);
        if new {
            self.record_taint(&all);
        }
        true
    }

    /// Leaves in the run folder that outside content was seen (`tainted.json`, only when it was).
    /// What the person is told when they apply the changes of a chat session comes from here: the
    /// service saw it, and Pi, which is not trusted, could say it read nothing. Best effort and
    /// saved in one piece; a failure is said in the log and tried again with the next new source.
    fn record_taint(&self, sources: &[&str]) {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let (path, tmp) = (
            self.run_dir.join("tainted.json"),
            self.run_dir.join("tainted.json.tmp"),
        );
        let text = json!({ "sources": sources }).to_string();
        let saved = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .and_then(|mut f| f.write_all(text.as_bytes()).and_then(|()| f.sync_all()))
            .and_then(|()| std::fs::rename(&tmp, &path));
        if let Err(e) = saved {
            eprintln!(
                "clio-toolhost: could not record the taint in {}: {e}",
                path.display()
            );
        }
    }

    fn refuse(&self, id: &str, rule: &str, why: String, tainted: bool) -> Response {
        let why = self.clean(&why);
        Response {
            id: id.to_owned(),
            ok: false,
            content: format!("[toolhost] the call was not run (rule {rule}): {why}"),
            data: Value::Null,
            decision: DecisionOut {
                outcome: "deny".into(),
                rule: rule.into(),
                reason: why,
            },
            tainted,
            external: false,
        }
    }

    pub fn handle(&self, req: Request) -> Response {
        let _one_at_a_time = lock(&self.busy);
        if !valid_session(&req.session) || req.id.len() > 128 || req.call_id.len() > 128 {
            return self.refuse(
                &req.id,
                "protocol",
                "bad session, id or call_id".into(),
                true,
            );
        }
        if req.tainted {
            self.mark(&req.session, "restored");
        }
        let tainted = self.is_tainted(&req.session);
        let Value::Object(map) = req.args.clone() else {
            return self.refuse(
                &req.id,
                "protocol",
                "\"args\" must be a JSON object".into(),
                tainted,
            );
        };
        if req.tool == "filter" {
            return self.filter(&req, &map, tainted);
        }
        if map
            .get("content")
            .and_then(Value::as_str)
            .is_some_and(|c| c.len() > MAX_WRITE_BYTES)
        {
            return self.refuse(
                &req.id,
                "toolhost-limit",
                "content is too large".into(),
                tainted,
            );
        }
        // What the model was shown has marks where a value was hidden (S-18). Written back whole,
        // the mark would replace the real value in the file. `edit` reads and writes real bytes.
        if map
            .get("content")
            .and_then(Value::as_str)
            .is_some_and(has_mask)
        {
            return self.refuse(
                &req.id,
                "S-18/mask",
                "the text carries a value that was crossed out (tachado): the file holds the real value, which you were not shown. Do not write the whole file back; change only the lines you need with `edit`".into(),
                tainted,
            );
        }
        let ar = ActionRequest {
            tool: req.tool.clone(),
            version: "1".into(),
            description_hash: self
                .descriptions
                .get(&req.tool)
                .cloned()
                .unwrap_or_default(),
            args: map.clone(),
            node: "local".into(),
            session: SessionInfo {
                id: req.session.clone(),
                parent: None,
                level: Level::Simple,
                tainted,
                subagent: false,
            },
            actor: "agent".into(),
            budget: Budget::default(),
            plan: vec![],
        };
        let d = self.gateway.decide_or_deny(&ar);
        let mut allowed = match d.outcome {
            Outcome::Allow => true,
            Outcome::Ask => {
                let yes = self.asker.approve(&d, &req.tool, &map);
                if yes {
                    self.gateway.reset_breaker(&req.session);
                }
                yes
            }
            Outcome::Deny => false,
        };
        let mut out = DecisionOut {
            outcome: d.outcome.as_str().into(),
            rule: d.rule.clone(),
            reason: self.clean(&d.reason),
        };
        // The gateway's rules know paths and domains; what is specific to a shell string is here.
        if allowed
            && req.tool == "exec"
            && let Some(why) = bash_rule(&map)
        {
            allowed = false;
            out = DecisionOut {
                outcome: "deny".into(),
                rule: "S-12/bash".into(),
                reason: why.into(),
            };
        }
        if !allowed {
            let why = if d.outcome == Outcome::Ask {
                format!(
                    "it needs a person's approval and nobody is available ({})",
                    out.reason
                )
            } else {
                out.reason.clone()
            };
            let content = format!(
                "[gateway] {} was not run (rule {}): {why}",
                self.clean(&req.tool),
                out.rule
            );
            return Response {
                id: req.id,
                ok: false,
                content,
                data: Value::Null,
                decision: out,
                tainted: self.is_tainted(&req.session),
                external: false,
            };
        }
        // Memory is not an executor action: it has its own presentation, and it taints the session
        // by the origin of what it returns, not by being external (files and commands always do).
        if memory::MEMORY_OPS.contains(&req.tool.as_str()) {
            return self.memory_call(&req, &map, out);
        }
        let action = Action {
            id: format!(
                "{}:{}:{}",
                req.session,
                req.call_id,
                self.counter.fetch_add(1, Ordering::SeqCst)
            ),
            tool: req.tool.clone(),
            args: Value::Object(map),
        };
        let approval = Approved(action.hash());
        let ran = catch_unwind(AssertUnwindSafe(|| {
            self.executor.execute(&action, &approval)
        }));
        let (ok, content, data, external) = match ran {
            Ok(Ok(o)) => self.present(&req, &o.result),
            Ok(Err(e)) => (
                false,
                format!("{} failed: {}", req.tool, self.clean(&e.to_string())),
                Value::Null,
                false,
            ),
            Err(_) => (false, "the tool panicked".to_owned(), Value::Null, false),
        };
        if external {
            self.mark(
                &req.session,
                if req.tool == "exec" {
                    "command"
                } else {
                    "file"
                },
            );
        }
        Response {
            id: req.id,
            ok,
            content,
            data,
            decision: out,
            tainted: self.is_tainted(&req.session),
            external,
        }
    }

    /// The text of a result, as it should reach the model: `(ok, content, data, external)`.
    fn present(&self, req: &Request, result: &Value) -> (bool, String, Value, bool) {
        let too_big = |n: usize| {
            (
                false,
                format!(
                    "[withheld] the output is {n} bytes, more than the {MAX_RAW_BYTES} allowed"
                ),
                Value::Null,
                false,
            )
        };
        match req.tool.as_str() {
            "fs_read" | "exec" => {
                let key = if req.tool == "exec" {
                    "output"
                } else {
                    "content"
                };
                let raw = result.get(key).and_then(Value::as_str).unwrap_or_default();
                if raw.len() > MAX_RAW_BYTES {
                    return too_big(raw.len());
                }
                // `raw` is for `edit`: it reads the file, changes a part and writes it back, so
                // cleaning here would damage the file. Its result is filtered at the end anyway.
                let keep = req.args.get("raw").and_then(Value::as_bool) == Some(true);
                let text = if keep {
                    raw.to_owned()
                } else {
                    self.clean(raw)
                };
                // How many values were struck out, for the guard to record: the person
                // supervises from the chat and wants to see it (S-18, S-36).
                let masked = count_masks(&text).saturating_sub(count_masks(raw));
                let data = if req.tool == "exec" {
                    json!({
                        "exit_code": result["exit_code"],
                        "signal": result["signal"],
                        "timed_out": result["timed_out"],
                        "timeout_secs": result["timeout_secs"],
                        "masked": masked,
                    })
                } else {
                    json!({ "masked": masked })
                };
                (true, text, data, true)
            }
            // Names are external text. They stay as they are here so Pi can stat each one; the
            // result Pi builds from them is filtered at the end.
            "fs_list" => (true, String::new(), result.clone(), true),
            "fs_glob" => (true, String::new(), result.clone(), true),
            "fs_stat" => (true, String::new(), result.clone(), false),
            "fs_write" | "fs_mkdir" => (true, String::new(), Value::Null, false),
            _ => (
                false,
                "the tool returned an unexpected result".into(),
                Value::Null,
                false,
            ),
        }
    }

    /// `filter`: no effect, no path. Cleans and scrubs texts and wraps the ones that ask for it
    /// as external data. `{"items": [{"text": "...", "wrap": "file" | "tool-result"}]}`.
    fn filter(&self, req: &Request, args: &Map<String, Value>, tainted: bool) -> Response {
        let bad = |why: &str| self.refuse(&req.id, "protocol", why.into(), tainted);
        let Some(items) = args.get("items").and_then(Value::as_array) else {
            return bad("`items` must be an array");
        };
        if items.len() > 256 {
            return bad("too many items");
        }
        let mut total = 0;
        let mut out = Vec::new();
        for it in items {
            let Some(text) = it.get("text").and_then(Value::as_str) else {
                return bad("every item needs a `text`");
            };
            total += text.len();
            if total > MAX_RAW_BYTES * 2 {
                return bad("the texts are too large");
            }
            let source = match it.get("wrap").and_then(Value::as_str) {
                None => None,
                Some("file") => Some(Source::File),
                Some("tool-result") => Some(Source::ToolResult),
                Some(_) => return bad("unknown wrap source"),
            };
            let clean = self.clean(text);
            out.push(Value::String(match source {
                Some(s) => wrap_untrusted(s, &clean),
                None => clean,
            }));
        }
        Response {
            id: req.id.clone(),
            ok: true,
            content: String::new(),
            data: json!({ "items": out }),
            decision: DecisionOut {
                outcome: "allow".into(),
                rule: "filter".into(),
                reason: "cleaning text has no effect".into(),
            },
            tainted: self.is_tainted(&req.session),
            external: false,
        }
    }
}

/// The rule for a shell string, on top of the gateway's: it is not empty, has no NUL and is of
/// a sane size. Everything else a command might do is held by the jail, not by reading it: no
/// network, no home, only the working copy, protected folders read-only, limits on time,
/// processes and output (S-12, S-13, S-16).
fn bash_rule(args: &Map<String, Value>) -> Option<&'static str> {
    let c = args.get("command").and_then(Value::as_str)?;
    if c.trim().is_empty() {
        Some("the command is empty")
    } else if c.contains('\0') {
        Some("the command has a NUL byte")
    } else if c.len() > MAX_COMMAND_BYTES {
        Some("the command is too long")
    } else {
        None
    }
}

/// The jail is offered only if it is there, starts and passes its self-test (S-12, S-13).
fn prove_jail() -> Result<(), String> {
    let jail = Jail::detect()?;
    jail.probe()?;
    let st = jail.self_test()?;
    if st.ok() {
        Ok(())
    } else {
        let bad: Vec<String> = st
            .checks
            .iter()
            .filter(|c| !c.passed)
            .map(|c| format!("{} ({})", c.name, c.detail))
            .collect();
        Err(format!("the jail's self-test failed: {}", bad.join("; ")))
    }
}

/// How long a client should wait for the host before it gives up and treats the tool as failed.
pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(180);
