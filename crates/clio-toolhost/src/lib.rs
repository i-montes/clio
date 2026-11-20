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
//! Host mode (`Config.host`, see `host`): the same tools on the real machine, never on Clio's
//! own folders, with `exec` through the host launcher (the network only when a command asks).
//!
//! Nothing in this crate talks to the network, except a host-mode command that asks for it. See
//! `README.md` for the protocol.

pub mod ask;
pub mod catalog;
pub mod cli;
pub mod grant;
pub mod host;
mod jobs;
mod judge;
pub mod manage;
pub mod mcp;
mod memory;
mod ops;
pub mod pending;
mod sanitize;
mod server;
pub mod services;
pub mod skills;
pub mod subagent;

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use clio_core::{Clock, Level, SystemClock};
use clio_gateway::{
    ActionRequest, ArgKind, ArgSpec, Budget, Decision, Egress, Gateway, Outcome, Rules,
    SessionInfo, Source, Telemetry, ToolDecl, wrap_untrusted,
};
use clio_node::{Action, EffectLog, Executor, Jail, Registry, Sandbox};
use clio_vault::{Scrubber, VaultClient, count_masks, has_mask};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

pub use host::HostCfg;
pub use judge::{SUSPECT_REASON, UNJUDGED};
pub use memory::{
    BLOCK_NAMES, BLOCK_TAINTED, BlockEdit, CATEGORIES, Found, Judging, MemoryItem, MemoryPort,
    NOT_WORTH_REMEMBERING, Near, Origin, ProposalDraft, ProposalKind, Proposed, Saved, TextVerdict,
    memory_args,
};
pub use sanitize::{is_deceptive, sanitize};
pub use server::Server;

/// The operations the host knows (`filter` is handled apart: it has no effect). Anything else is
/// denied by the gateway (S-02). `exec` is declared only when the jail is proven.
pub const OPS: [&str; 7] = [
    "fs_read", "fs_stat", "fs_list", "fs_glob", "fs_write", "fs_mkdir", "exec",
];

/// Folders under the working copy that load code or settings by themselves (S-16): Pi
/// extensions, skills, hooks, git. Nothing here may write them. Mandatory: a rules file can add
/// to this list, never remove from it. `.agents` is where Pi looks for project skills;
/// `.mcp.json` names MCP servers Claude Code starts.
pub const PROTECTED_PATHS: [&str; 12] = clio_node::READ_ONLY;

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

/// What a review is told instead of an action being set aside for the person.
pub const REVIEW_CANNOT_ASK: &str = "a review cannot ask the person: what needs their approval is not run in a review, and nothing waits for them";
/// What a review is told when a command asks for the network or `sudo`, or reaches out by itself.
pub const REVIEW_NO_NETWORK: &str = "a review has no network and no sudo: what needs them cannot be checked here, so report it as could_not_check";
/// The only requests a review's tool host serves: the reading tools, `exec`, and the internal
/// operations of the guard and of Clio (S-67). Everything else is refused, whoever sent it.
/// What a round (`--unattended`) is told when it tries to change something (S-71).
pub const ROUND_ONLY_READS: &str = "a round only reads: propose it in your notice";
/// Why what a round would send out waits for the person.
pub(crate) const ROUND_REACHES: &str = "a round reaches nothing outside by itself";
/// Why a round's change to a unit waits for the person.
const ROUND_UNITS: &str = "a round changes no service";
/// The tools a round never uses: each changes something, or needs the person.
const ROUND_REFUSED: [&str; 8] = [
    "fs_write",
    "fs_mkdir",
    "skill_write",
    "skill_report",
    "service_write",
    "mcp_propose",
    "memory_block_edit",
    "request_secret",
];

/// Whether a round's call (`tool`, `args`) changes something: a refused tool, a manage act but
/// `status` and `logs`, a background command.
fn round_refuses(tool: &str, args: &Map<String, Value>) -> bool {
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").trim();
    ROUND_REFUSED.contains(&tool)
        || (matches!(tool, "service_manage" | "skill_manage" | "mcp_manage")
            && !matches!(s("action"), "status" | "logs"))
        || (tool == "exec" && args.get("background").and_then(Value::as_bool) == Some(true))
}

const REVIEW_OPS: [&str; 10] = [
    "fs_read", "fs_stat", "fs_list", "fs_glob", "exec", "filter", "judge", "jobs", "taint",
    "closing",
];

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
    /// The memory, which is the agent's: read and write. Without it no `memory_*` tool is declared
    /// and the gateway denies them (S-02): the loose `clio-toolhost` binary has none.
    pub memory: Option<Arc<dyn MemoryPort>>,
    /// What the brain must show to close a session (`closing`): a secret the brain made and put in
    /// this process's environment, which Pi never gets.
    pub said_token: Option<String>,
    /// Host mode: the tools act on the real machine (`/`) instead of `work`. `None`: copy mode.
    pub host: Option<HostCfg>,
    /// Where `request_secret` keeps what it asks the person. `None`: nobody can be asked.
    pub secrets: Option<pending::SecretCfg>,
    /// The time the memory's hourly saves and changes are counted by (S-47).
    pub clock: Arc<dyn Clock>,
    /// A review (`--review`): nobody can be asked, so what would wait for the person is refused
    /// outright and nothing is stored as pending; no background commands either.
    pub review: bool,
    /// A round (`--unattended`, S-71): nobody watches and nobody answers. It only reads: its
    /// commands run read-only with no new privileges, what changes something is refused
    /// (`ROUND_ONLY_READS`), what reaches out waits for the person, no permission window covers
    /// it, and a judge that does not answer withholds the text and ends it.
    pub unattended: bool,
    /// Clio's skills folder (`<data>/skills`, S-68): `skill_write` and `skill_report` work, and
    /// a registered, enabled skill is read as Clio's own text. Only a chat gets it.
    pub skills: Option<PathBuf>,
    /// Host mode: more folders inside Clio's masked ones that commands see read-only, besides
    /// the skills' `live/` (a skill draft's stage, for its reviewer).
    pub shows: Vec<PathBuf>,
    /// Host mode: the MCP servers (S-70), through the person's `clio`.
    pub mcp: Option<Arc<dyn mcp::McpPort>>,
    /// Host mode: why the MCP servers could not be set up (no bwrap, no npm beside Node): the
    /// chat starts anyway, and the three tools say this.
    pub mcp_unavailable: Option<String>,
    /// A chat's child (`--child`, S-73): a reader is a review's tool host (`review` is set too),
    /// a writer serves only the file tools and `exec`; neither ever sets anything aside.
    pub child: Option<subagent::Child>,
    /// A chat's tool host (`--subagents`, S-73): it holds `subagent` calls for the brain.
    pub subagents: bool,
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
            said_token: None,
            host: None,
            secrets: None,
            clock: Arc::new(SystemClock),
            review: false,
            unattended: false,
            skills: None,
            shows: Vec::new(),
            mcp: None,
            mcp_unavailable: None,
            child: None,
            subagents: false,
        }
    }
}

#[derive(Default)]
struct Taint {
    sources: Vec<&'static str>,
    /// The person closed the session: only its summary may still be saved (`closing`).
    closing: bool,
    /// The person was told once that the judge did not answer.
    judge_silent: bool,
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
    said_token: Option<String>,
    /// Where the memory tools keep their budget and their log.
    run_dir: PathBuf,
    host: Option<host::Host>,
    secrets: Option<pending::SecretCfg>,
    clock: Arc<dyn Clock>,
    review: bool,
    unattended: bool,
    /// Calls that read outside text, waiting for the judge when the guard filters their result.
    to_judge: Mutex<HashMap<String, judge::ToJudge>>,
    /// The service is stopping: a running command is stopped, and nothing new runs.
    stopping: std::sync::atomic::AtomicBool,
    /// Host mode: the commands that run in the background.
    jobs: jobs::Jobs,
    /// The working folder as configured (copy mode: what relative paths are under).
    work: PathBuf,
    skills: Option<PathBuf>,
    shows: Vec<PathBuf>,
    mcp: Option<Arc<dyn mcp::McpPort>>,
    mcp_off: Option<String>,
    /// The MCP servers this host started, each once (S-70).
    mcp_pool: mcp::Pool,
    /// When each session called a manage tool (the brake on `manage`).
    manages: Mutex<HashMap<String, Vec<std::time::Instant>>>,
    child: Option<subagent::Child>,
    /// A chat's: the `subagent` calls it holds (S-73).
    calls: Option<subagent::Calls>,
}

/// The folders host-mode commands see read-only inside Clio's masked ones: the skills' `live/`
/// and the configured `--show`s.
fn windows(skills: Option<&Path>, shows: &[PathBuf]) -> Vec<PathBuf> {
    skills
        .map(|r| r.join(skills::LIVE))
        .into_iter()
        .chain(shows.iter().cloned())
        .collect()
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

fn arg_specs(tool: &str, host: bool) -> Vec<ArgSpec> {
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
        "exec" if host => vec![
            a("command", ArgKind::Text, true),
            a("timeout", ArgKind::Int, false),
            a("network", ArgKind::Bool, false),
            a("background", ArgKind::Bool, false),
        ],
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
        // The sandbox takes `/` for host mode; a copy-mode working folder never is.
        if cfg.host.is_none() && cfg.work.canonicalize().is_ok_and(|w| w.parent().is_none()) {
            return Err("the working folder cannot be / outside host mode".into());
        }
        let (host, exec_status) = match cfg.host {
            Some(h) => {
                let (mut h, status) = host::Host::new(h, &cfg.scratch, cfg.jail == JailMode::Auto);
                let windows = windows(cfg.skills.as_deref(), &cfg.shows);
                if !windows.is_empty() {
                    let both: Vec<PathBuf> = windows
                        .iter()
                        .flat_map(|w| w.canonicalize().into_iter().chain([w.clone()]))
                        .collect();
                    h.cfg.rules = h.cfg.rules.clone().with_window(&h.cfg.home, &both);
                }
                (Some(h), status)
            }
            None if cfg.jail == JailMode::Off => (None, Err("the jail is switched off".to_owned())),
            None => (None, prove_jail()),
        };
        // In host mode the tools work on the whole machine; paths reach them absolute.
        let root = if host.is_some() {
            PathBuf::from("/")
        } else {
            cfg.work.clone()
        };
        let mut registry = Registry::new();
        for t in ops::tools(exec_status.is_ok(), host.is_some(), cfg.review) {
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
                args: arg_specs(name, host.is_some()),
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
            sandbox_root: root.clone(),
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
            sandbox_root: root.clone(),
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
        host::private_dir(&cfg.run_dir).map_err(|x| e(&x))?;
        let log = EffectLog::open(&cfg.run_dir.join("effects.jsonl")).map_err(|x| e(&x))?;
        let mut sandbox = Sandbox::new(&root, &cfg.scratch).map_err(|x| e(&x))?;
        // A program that runs in the folder can edit files in place: snapshots must be copies.
        sandbox.set_copy_snapshots(true);
        let pool = mcp::Pool::default();
        if let Some(port) = &cfg.mcp {
            pool.reap_idle(port.root());
        }
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
            said_token: cfg.said_token,
            run_dir: cfg.run_dir,
            host,
            secrets: cfg.secrets,
            clock: cfg.clock,
            review: cfg.review,
            unattended: cfg.unattended,
            to_judge: Mutex::new(HashMap::new()),
            stopping: std::sync::atomic::AtomicBool::new(false),
            jobs: jobs::Jobs::default(),
            work: cfg.work,
            skills: cfg.skills,
            shows: cfg.shows,
            mcp: cfg.mcp,
            mcp_off: cfg.mcp_unavailable,
            mcp_pool: pool,
            manages: Mutex::new(HashMap::new()),
            child: cfg.child,
            calls: cfg.subagents.then(subagent::Calls::default),
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

    /// A writing child (`--child write`, S-73).
    pub(crate) fn writer(&self) -> bool {
        self.child == Some(subagent::Child::Write)
    }

    /// A refusal's rule in a review or a child: `review/<what>` or `subagent/<what>`.
    pub(crate) fn rule(&self, what: &str) -> &'static str {
        let child = self.child.is_some();
        match what {
            "tool" if child => "subagent/tool",
            "tool" => "review/tool",
            "network" if child => "subagent/network",
            "network" => "review/network",
            "background" if child => "subagent/background",
            "background" => "review/background",
            _ if child => "subagent/aside",
            _ => "review/aside",
        }
    }

    /// What a review or a child is told instead of an action being set aside for the person.
    pub(crate) fn cannot_ask(&self) -> &'static str {
        if self.child.is_some() {
            subagent::CHILD_CANNOT_ASK
        } else {
            REVIEW_CANNOT_ASK
        }
    }

    /// `Ok` when `exec` (bash, in the jail) is offered; otherwise why it is not.
    pub fn exec_available(&self) -> Result<(), &str> {
        self.exec_status.as_ref().copied().map_err(String::as_str)
    }

    /// The service is stopping: the command that runs is stopped (its log line says so) and
    /// this returns once it has; waits at most `grace`.
    pub fn stop(&self, grace: Duration) {
        self.stopping.store(true, Ordering::SeqCst);
        let until = std::time::Instant::now() + grace;
        while std::time::Instant::now() < until {
            if self.busy.try_lock().is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // Background commands too: none outlives the session.
        self.stop_jobs(until.saturating_duration_since(std::time::Instant::now()));
        self.mcp_stop_all();
    }

    pub fn telemetry(&self) -> Telemetry {
        self.gateway.telemetry()
    }

    /// The folders this host's commands see read-only inside Clio's masked ones (S-68).
    pub fn windows(&self) -> Vec<PathBuf> {
        windows(self.skills.as_deref(), &self.shows)
    }

    pub fn is_tainted(&self, session: &str) -> bool {
        // An entry only for a closing (no sources) is not a taint.
        lock(&self.sessions)
            .get(session)
            .is_some_and(|t| !t.sources.is_empty())
    }

    /// One request line in, one response line out. Never panics, never executes on a bad line.
    pub fn handle_line(&self, line: &str) -> String {
        self.handle_line_until(line, &|| false)
    }

    /// `handle_line` with `handle_until`'s `cancel`.
    pub fn handle_line_until(&self, line: &str, cancel: &dyn Fn() -> bool) -> String {
        let resp = match serde_json::from_str::<Request>(line) {
            Ok(req) => self.handle_until(req, cancel),
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

    /// Where the permission window lives (`grant`): Clio's host folder, in host mode or in a chat
    /// (where secrets and block changes wait).
    pub(crate) fn grant_state(&self) -> Option<&Path> {
        self.host
            .as_ref()
            .map(|h| h.cfg.state.as_path())
            .or_else(|| self.secrets.as_ref().map(|c| c.state.as_path()))
    }

    /// The judge flagged an injection: the permission window and bypass close now, for every
    /// conversation (they all read the same files), and Clio asks again for everything.
    pub(crate) fn close_window_flagged(&self) {
        let Some(st) = self.grant_state() else {
            return;
        };
        for kind in [grant::Kind::Window, grant::Kind::Bypass] {
            if let Err(e) = kind.close(st, grant::FLAGGED) {
                eprintln!(
                    "clio-toolhost: the {} could not be closed: {e}",
                    kind.name()
                );
            }
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
            // What it read may be what wants out: its jobs with the network stop.
            if source == "suspect" {
                self.stop_web_jobs(session);
            }
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
        // Another host on the same run (an approval) may have written it: its sources stay.
        let mut all: Vec<String> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| serde_json::from_value(v["sources"].clone()).ok())
            .unwrap_or_default();
        all.extend(sources.iter().map(|s| (*s).to_owned()));
        all.sort_unstable();
        all.dedup();
        let text = json!({ "sources": all }).to_string();
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
        self.handle_until(req, &|| false)
    }

    /// `handle`, but a host-mode command stops as soon as `cancel` says so (the caller went away:
    /// the person aborted, the session ended). Its log line then says `stopped`.
    pub fn handle_until(&self, mut req: Request, cancel: &dyn Fn() -> bool) -> Response {
        if req.tool == memory::OLD_SAVE {
            req.tool = "memory_save".into();
        }
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
        let Value::Object(mut map) = req.args.clone() else {
            return self.refuse(
                &req.id,
                "protocol",
                "\"args\" must be a JSON object".into(),
                tainted,
            );
        };
        if self.review && !REVIEW_OPS.contains(&req.tool.as_str()) {
            let at = self
                .host
                .as_ref()
                .map(|h| (h.cfg.state.as_path(), h.cfg.run.as_str()));
            let why = if self.child.is_some() {
                "a reading subagent only reads and runs commands: this is not one of its tools"
            } else {
                "a review only reads and runs commands: this is not one of its tools"
            };
            return self.refuse_in_review(at, &req, &req.args, self.rule("tool"), why);
        }
        if self.writer() && !subagent::WRITER_OPS.contains(&req.tool.as_str()) {
            let at = self
                .host
                .as_ref()
                .map(|h| (h.cfg.state.as_path(), h.cfg.run.as_str()));
            let why = "a subagent only reads, writes, edits and runs commands: this is not one of its tools";
            return self.refuse_in_review(at, &req, &req.args, "subagent/tool", why);
        }
        if self.unattended && round_refuses(&req.tool, &map) {
            let at = self
                .host
                .as_ref()
                .map(|h| (h.cfg.state.as_path(), h.cfg.run.as_str()));
            return self.refuse_in_review(at, &req, &req.args, "round/read-only", ROUND_ONLY_READS);
        }
        let closing = || {
            lock(&self.sessions)
                .get(&req.session)
                .is_some_and(|t| t.closing)
        };
        // These touch only their own locks: they never wait behind a command that runs (which
        // may have no time limit), so the guard's filter and job_stop are answered meanwhile.
        match req.tool.as_str() {
            "filter" => return self.filter(&req, &map, tainted),
            "judge" => return self.judge_op(&req, &map),
            "jobs" => return self.job_op(&req, &map),
            "job_output" | "job_stop" if !closing() => return self.job_op(&req, &map),
            // An MCP server holds only its own place in the pool: a slow one keeps no other tool
            // (or the session's close) waiting (S-70).
            "mcp_tools" if !closing() => return self.mcp_tools(&req, &map),
            "mcp_call" if !closing() => return self.mcp_call(&req, &map),
            // A call that waits for its children holds no lock: the brain's marks and the
            // guard's filter are answered meanwhile (S-73).
            "subagent" | "subagent_take" | "subagent_alive" | "subagent_done" if !closing() => {
                if let Some(calls) = &self.calls {
                    return self.subagent_op(calls, &req, &map, cancel);
                }
            }
            _ => {}
        }
        let _one_at_a_time = lock(&self.busy);
        // The caller gave up while this waited for the one before: nothing runs for nobody.
        if cancel() {
            return self.refuse(
                &req.id,
                "protocol",
                "the caller went away before the call ran".into(),
                tainted,
            );
        }
        if req.tool == "taint" {
            return self.taint(&req);
        }
        if req.tool == "closing" {
            return self.closing(&req, &map);
        }
        // The last turn of a closed session saves its summary and nothing else: no file, no
        // command, no other change of the memory.
        if req.tool != "memory_session_summary" && closing() {
            return self.refuse(
                &req.id,
                "closing",
                "the person closed this session: only memory_session_summary is left".into(),
                tainted,
            );
        }
        // Not a tool of the gateway's: nothing is run, and no value ever comes through here.
        if req.tool == "request_secret" {
            return self.request_secret(&req, &map, tainted);
        }
        match req.tool.as_str() {
            "skill_write" => return self.skill_write(&req, &map),
            "skill_report" => return self.skill_report(&req, &map),
            "service_write" => return self.service_write(&req, &map),
            "service_status" => return self.service_status(&req, &map),
            "mcp_propose" => return self.mcp_propose(&req, &map),
            "mcp_tools" => return self.mcp_tools(&req, &map),
            "mcp_call" => return self.mcp_call(&req, &map),
            "service_manage" | "skill_manage" | "mcp_manage" => return self.manage(&req, &map),
            _ => {}
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
                "the text carries a value that was redacted: the file holds the real value, which you were not shown. Do not write the whole file back; change only the lines you need with `edit`".into(),
                tainted,
            );
        }
        // Host mode: a relative path is under the home folder; the gateway sees it absolute.
        if let Some(h) = &self.host
            && let Some(p) = map.get("path").and_then(Value::as_str)
        {
            let abs = h.cfg.home.join(p).to_string_lossy().into_owned();
            map.insert("path".into(), Value::from(abs));
            req.args = Value::Object(map.clone());
        }
        if let Some(r) = self.skill_read(&req, &map) {
            return r;
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
        // Clio's own folders, wherever the path really lands.
        let mut real = None;
        if allowed
            && let Some(h) = &self.host
            && let Some(p) = map.get("path").and_then(Value::as_str)
        {
            match h.path(p) {
                Ok(r) => real = Some(r),
                Err((rule, why)) => {
                    allowed = false;
                    out = DecisionOut {
                        outcome: "deny".into(),
                        rule: rule.into(),
                        reason: why,
                    };
                }
            }
        }
        if !allowed {
            if let Some(h) = &self.host
                && host::LOGGED.contains(&req.tool.as_str())
            {
                let what = map
                    .get("command")
                    .or_else(|| map.get("path"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let network = map.get("network").and_then(Value::as_bool) == Some(true);
                let what = self.clean(what);
                if let Err(e) = h.log(
                    &req.session,
                    &req.tool,
                    &what,
                    network,
                    "deny",
                    &out.rule,
                    None,
                ) {
                    eprintln!("clio-toolhost: the action log lost a line: {e}");
                }
            }
            let why = if d.outcome == Outcome::Ask && (self.review || self.writer()) {
                format!("{} ({})", self.cannot_ask(), out.reason)
            } else if d.outcome == Outcome::Ask {
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
        let action_id = format!(
            "{}:{}:{}",
            req.session,
            req.call_id,
            self.counter.fetch_add(1, Ordering::SeqCst)
        );
        if let Some(h) = &self.host {
            // What reaches out (the network asked for, or a program that does by itself) or
            // uses `sudo`: root can reach out past any cut.
            let reaches = req.tool == "exec" && {
                let command = map.get("command").and_then(Value::as_str).unwrap_or("");
                let network = map.get("network").and_then(Value::as_bool) == Some(true);
                let rules = &h.cfg.rules;
                network || rules.reaches_out(command) || rules.uses_sudo(command)
            };
            // A writing child never uses `sudo` (S-73): what needs it is its parent's to do.
            if self.writer()
                && req.tool == "exec"
                && h.cfg
                    .rules
                    .uses_sudo(map.get("command").and_then(Value::as_str).unwrap_or(""))
            {
                let at = (h.cfg.state.as_path(), h.cfg.run.as_str());
                return self.refuse_in_review(
                    Some(at),
                    &req,
                    &req.args,
                    "subagent/sudo",
                    subagent::CHILD_NO_SUDO,
                );
            }
            // A review, which nobody watches, never does (S-67).
            if reaches && self.review {
                let at = (h.cfg.state.as_path(), h.cfg.run.as_str());
                let why = if self.child.is_some() {
                    subagent::CHILD_NO_NETWORK
                } else {
                    REVIEW_NO_NETWORK
                };
                return self.refuse_in_review(Some(at), &req, &req.args, self.rule("network"), why);
            }
            // The list, not the model, says what waits for the person.
            let what = match req.tool.as_str() {
                "exec" => map
                    .get("command")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                "fs_write" => real.as_deref().map(|r| r.to_string_lossy().into_owned()),
                _ => None,
            };
            let class = match req.tool.as_str() {
                "exec" => what
                    .as_deref()
                    .and_then(|c| h.cfg.rules.classify_command(c)),
                "fs_write" => real.as_deref().and_then(|r| h.cfg.rules.classify_write(r)),
                _ => None,
            };
            if let Some(class) = class {
                // Marked now, as it is classified: a `control` on the machine alone, which bypass
                // may answer; one that touches Clio's own guards carries no mark and always asks.
                let machine = class == ask::Class::Control
                    && what.as_deref().is_some_and(|w| h.cfg.rules.machine_only(w));
                return self.aside(h, &req, class, machine.then_some(ask::MACHINE), cancel);
            }
            // A session that read text the judge flagged asks before it reaches out, whatever
            // the command: what it read may be what wants out.
            if reaches && self.is_suspect(&req.session) {
                return self.aside(h, &req, ask::Class::Send, Some(SUSPECT_REASON), cancel);
            }
            // A round (nobody watches it) never reaches out by itself, nor changes a unit through
            // the service manager its sockets reach: it waits for the person.
            if reaches && self.unattended {
                return self.aside(h, &req, ask::Class::Send, Some(ROUND_REACHES), cancel);
            }
            if self.unattended
                && req.tool == "exec"
                && h.cfg
                    .rules
                    .changes_units(map.get("command").and_then(Value::as_str).unwrap_or(""))
            {
                return self.aside(h, &req, ask::Class::Control, Some(ROUND_UNITS), cancel);
            }
            if req.tool == "exec" && map.get("background").and_then(Value::as_bool) == Some(true) {
                if self.review || self.writer() {
                    let at = (h.cfg.state.as_path(), h.cfg.run.as_str());
                    let why = if self.child.is_some() {
                        "a subagent has no background commands: run it in the foreground"
                    } else {
                        "a review has no background commands: run it in the foreground"
                    };
                    return self.refuse_in_review(
                        Some(at),
                        &req,
                        &req.args,
                        self.rule("background"),
                        why,
                    );
                }
                return self.job_start(h, &req, out);
            }
            let (ok, content, data, external) = self
                .host_run(h, &req, action_id, &out.rule, real.as_deref(), cancel)
                .unwrap_or_else(|e| (false, e, Value::Null, false));
            return Response {
                id: req.id,
                ok,
                content,
                data,
                decision: out,
                tainted: self.is_tainted(&req.session),
                external,
            };
        }
        let action = Action {
            id: action_id,
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
            let what = ["command", "path"]
                .iter()
                .find_map(|k| req.args.get(*k).and_then(Value::as_str))
                .unwrap_or("");
            self.judge_later(&req, what);
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
                        "stopped": result["stopped"],
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
    /// The brain marks a session: it put into the model's message something the person did not
    /// write (a memory of the agent or from outside). Like every mark, it only ever goes up.
    /// With `{"suspect": true}` (a session the judge flagged, reopened) it is marked `suspect` too.
    /// With `{"flagged": true}` too (a reviewer's judge flagged what it read), the permission
    /// window closes, as when this host's judge flags.
    fn taint(&self, req: &Request) -> Response {
        self.mark(&req.session, "memory");
        if req.args.get("suspect").and_then(Value::as_bool) == Some(true) {
            self.mark(&req.session, "suspect");
            if req.args.get("flagged").and_then(Value::as_bool) == Some(true) {
                self.close_window_flagged();
            }
        }
        Response {
            id: req.id.clone(),
            ok: true,
            content: String::new(),
            data: Value::Null,
            decision: DecisionOut {
                outcome: "allow".into(),
                rule: "taint".into(),
                reason: "marking a session has no effect on the machine".into(),
            },
            tainted: true,
            external: false,
        }
    }

    /// The brain closes the session (with its token): from now on only the summary is served.
    fn closing(&self, req: &Request, args: &Map<String, Value>) -> Response {
        let tainted = self.is_tainted(&req.session);
        let known = self.said_token.as_deref();
        if known.is_none() || known != args.get("token").and_then(Value::as_str) {
            return self.refuse(
                &req.id,
                "closing",
                "only the brain closes a session".into(),
                tainted,
            );
        }
        {
            let mut s = lock(&self.sessions);
            if s.len() >= MAX_SESSIONS && !s.contains_key(&req.session) {
                return self.refuse(&req.id, "protocol", "too many sessions".into(), tainted);
            }
            let t = s.entry(req.session.clone()).or_default();
            t.closing = true;
        }
        Response {
            id: req.id.clone(),
            ok: true,
            content: String::new(),
            data: Value::Null,
            decision: DecisionOut {
                outcome: "allow".into(),
                rule: "closing".into(),
                reason: "closing a session only narrows what it may do".into(),
            },
            tainted,
            external: false,
        }
    }

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
        // The texts that go to the model, as data: what the judge reads.
        let mut wrapped = Vec::new();
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
            if source.is_some() {
                wrapped.push(out.len());
            }
            out.push(Value::String(self.clean(text)));
        }
        let judged = self.judge_result(req, &mut out, &wrapped);
        for &i in &wrapped {
            let text = out[i].as_str().unwrap_or_default().to_owned();
            let s = match items[i].get("wrap").and_then(Value::as_str) {
                Some("file") => Source::File,
                _ => Source::ToolResult,
            };
            out[i] = Value::String(wrap_untrusted(s, &text));
        }
        let mut data = json!({ "items": out });
        if let (Some(d), Value::Object(j)) = (data.as_object_mut(), judged) {
            d.extend(j);
        }
        Response {
            id: req.id.clone(),
            ok: true,
            content: String::new(),
            data,
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
