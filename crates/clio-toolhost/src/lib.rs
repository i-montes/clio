//! The tool host: the only thing in a Pi session that runs effects on the machine (ADR 0007).
//!
//! Pi runs the agent loop; its built-in tools are off. The model gets five tools registered by
//! the `clio-guard` extension, and each of them is just a call to this service over a Unix
//! socket. For every request the host does, in this order, and fails closed at each step:
//!
//! 1. the gateway decides (`Gateway::decide_or_deny`, rules in JSON, deny by default);
//! 2. on an allow, the `clio-node` executor runs the tool inside the session's working copy
//!    (sandbox, canonical paths, symlinks out rejected, protected folders), `run_tests` inside
//!    the `bwrap` jail and only if the jail's self-test passed;
//! 3. the output is cleaned (escape sequences out), scrubbed (known secret values and key
//!    patterns out) and cut;
//! 4. external content is wrapped as data (`wrap_untrusted`) and marks the session as tainted;
//!    the mark never goes away and is reported in every answer (S-05).
//!
//! Nothing in this crate talks to the network. See `README.md` for the protocol.

mod sanitize;
mod server;

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use clio_core::{Level, SystemClock};
use clio_gateway::{
    ActionRequest, ArgKind, ArgSpec, Budget, Decision, Egress, Gateway, Outcome, Rules,
    SessionInfo, Source, Telemetry, ToolDecl, wrap_untrusted,
};
use clio_node::{Action, EffectLog, Executor, Jail, Registry, Sandbox, register_run_tests};
use clio_vault::{Scrubber, VaultClient};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

pub use sanitize::sanitize;
pub use server::Server;

/// The only tools the host knows. Anything else is denied by the gateway (S-02).
pub const TOOLS: [&str; 5] = [
    "read_file",
    "list_dir",
    "write_file",
    "make_dir",
    "run_tests",
];

/// Folders under the working copy that load code or settings by themselves (S-16): Pi
/// extensions, skills, hooks, git. Nothing here may write them. Mandatory: a rules file can add
/// to this list, never remove from it. `.agents` is where Pi looks for project skills.
pub const PROTECTED_PATHS: [&str; 6] =
    ["extensions", "skills", ".pi", ".claude", ".git", ".agents"];

/// A request line longer than this is refused and the connection closed.
pub const MAX_REQUEST_BYTES: usize = 4 << 20;
/// The largest `content` of a `write_file`.
const MAX_WRITE_BYTES: usize = 1 << 20;
/// What a tool may produce before it is cleaned: above this the output is withheld, not cut, so
/// a secret is never split by a cut before the scrubber saw it whole.
const MAX_RAW_BYTES: usize = 2 << 20;
/// What the model gets of an output (after cleaning).
const MAX_RESULT_CHARS: usize = 32_000;
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
    /// What the model sees. External text inside is wrapped as data.
    pub content: String,
    pub decision: DecisionOut,
    /// The session's taint after this call. It only ever goes from false to true.
    pub tainted: bool,
    /// `content` carries text from outside (a file, a listing, test output).
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
    /// No `run_tests`.
    Off,
    /// `run_tests` only if `bwrap` is there, starts, and the jail's self-test passes.
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
    run_tests: Result<(), String>,
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
    match tool {
        "write_file" => vec![
            a("path", ArgKind::Path, true),
            a("content", ArgKind::Text, true),
        ],
        "run_tests" => vec![
            a("runner", ArgKind::Text, true),
            a("target", ArgKind::Path, false),
        ],
        _ => vec![a("path", ArgKind::Path, true)],
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
        let mut registry = Registry::basic().map_err(|x| e(&x))?;
        let run_tests = match cfg.jail {
            JailMode::Off => Err("the jail is switched off".to_owned()),
            JailMode::Auto => prove_jail().and_then(|()| {
                register_run_tests(&mut registry).map_err(|x| format!("run_tests: {x}"))
            }),
        };
        let (mut tools, mut descriptions) = (Vec::new(), HashMap::new());
        for name in TOOLS {
            let Some(found) = registry.get(name) else {
                continue; // run_tests without a proven jail: no declaration, so a deny (S-02)
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
            // No tool here runs a command line, so no binary is allowed (S-12).
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
                .unwrap_or_else(|| Scrubber::new(std::sync::Arc::new(SystemClock))),
            vault: cfg.vault,
            asker: Box::new(NobodyHome),
            sessions: Mutex::new(HashMap::new()),
            busy: Mutex::new(()),
            counter: AtomicU64::new(0),
            run_tests,
        })
    }

    #[must_use]
    pub fn with_asker(mut self, a: Box<dyn Asker>) -> Self {
        self.asker = a;
        self
    }

    /// `Ok` when `run_tests` is offered; otherwise why it is not.
    pub fn run_tests_available(&self) -> Result<(), &str> {
        self.run_tests.as_ref().copied().map_err(String::as_str)
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
            r#"{"id":"","ok":false,"content":"[toolhost] internal error","decision":{"outcome":"deny","rule":"fail-closed","reason":"internal error"},"tainted":true,"external":false}"#.into()
        })
    }

    fn clean(&self, text: &str) -> String {
        // In this order: a hidden character inside a value must not stop the scrubber.
        let t = self.scrubber.scrub(&sanitize(text));
        match &self.vault {
            None => t,
            Some(v) => v
                .scrub(&t)
                .unwrap_or_else(|_| "[withheld: the vault could not filter this text]".into()),
        }
    }

    fn mark(&self, session: &str, source: &'static str) -> bool {
        let mut s = lock(&self.sessions);
        if s.len() >= MAX_SESSIONS && !s.contains_key(session) {
            return true; // too many sessions: report tainted, which is the safe answer
        }
        let t = s.entry(session.to_owned()).or_default();
        if !t.sources.contains(&source) {
            t.sources.push(source);
        }
        drop(s);
        self.gateway.mark_tainted(session);
        true
    }

    fn refuse(&self, id: &str, rule: &str, why: String, tainted: bool) -> Response {
        let why = self.clean(&why);
        Response {
            id: id.to_owned(),
            ok: false,
            content: format!("[toolhost] the call was not run (rule {rule}): {why}"),
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
        let allowed = match d.outcome {
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
        let out = DecisionOut {
            outcome: d.outcome.as_str().into(),
            rule: d.rule.clone(),
            reason: self.clean(&d.reason),
        };
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
                d.rule
            );
            return Response {
                id: req.id,
                ok: false,
                content,
                decision: out,
                tainted: self.is_tainted(&req.session),
                external: false,
            };
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
        let (ok, content, external) = match ran {
            Ok(Ok(o)) => self.present(&req, &o.result),
            Ok(Err(e)) => (
                false,
                format!(
                    "[tool error] {} failed: {}",
                    req.tool,
                    self.clean(&e.to_string())
                ),
                false,
            ),
            Err(_) => (false, "[tool error] the tool panicked".to_owned(), false),
        };
        if external {
            self.mark(
                &req.session,
                if req.tool == "run_tests" {
                    "tool-result"
                } else {
                    "file"
                },
            );
        }
        Response {
            id: req.id,
            ok,
            content,
            decision: out,
            tainted: self.is_tainted(&req.session),
            external,
        }
    }

    /// The text for the model: `(ok, content, external)`.
    fn present(&self, req: &Request, result: &Value) -> (bool, String, bool) {
        let path = req
            .args
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let raw = match req.tool.as_str() {
            "read_file" => result
                .get("content")
                .and_then(Value::as_str)
                .map(str::to_owned),
            "list_dir" => result.get("entries").and_then(Value::as_array).map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
            }),
            "run_tests" => result
                .get("output")
                .and_then(Value::as_str)
                .map(str::to_owned),
            "write_file" | "make_dir" => {
                let what = if req.tool == "write_file" {
                    let n = req
                        .args
                        .get("content")
                        .and_then(Value::as_str)
                        .map_or(0, str::len);
                    format!("wrote {n} bytes to")
                } else {
                    "created the folder".to_owned()
                };
                return (true, format!("ok: {what} {}", self.clean(path)), false);
            }
            _ => None,
        };
        let Some(raw) = raw else {
            return (
                false,
                "[tool error] the tool returned an unexpected result".into(),
                false,
            );
        };
        if raw.len() > MAX_RAW_BYTES {
            return (
                false,
                format!(
                    "[withheld] the output is {} bytes, more than the {MAX_RAW_BYTES} allowed",
                    raw.len()
                ),
                false,
            );
        }
        let keep_end = req.tool == "run_tests";
        let text = cut(&self.clean(&raw), keep_end);
        if req.tool == "run_tests" {
            let passed = result["passed"].as_bool() == Some(true);
            let how = if result["timed_out"].as_bool() == Some(true) {
                "killed: time limit".to_owned()
            } else {
                format!(
                    "exit code {}",
                    result["exit_code"]
                        .as_i64()
                        .map_or("none".into(), |c| c.to_string())
                )
            };
            // The exit code is the jail's own word: it goes outside the markers.
            return (
                true,
                format!(
                    "Result of run_tests: {} ({how}).\n{}",
                    if passed { "PASSED" } else { "FAILED" },
                    wrap_untrusted(Source::ToolResult, &text)
                ),
                true,
            );
        }
        (true, wrap_untrusted(Source::File, &text), true)
    }
}

/// Keeps `MAX_RESULT_CHARS`: the start, or (test output) the end, where the failures are.
fn cut(text: &str, keep_end: bool) -> String {
    let n = text.chars().count();
    if n <= MAX_RESULT_CHARS {
        return text.to_owned();
    }
    if keep_end {
        let tail: String = text.chars().skip(n - MAX_RESULT_CHARS).collect();
        format!("[truncated: the end is shown]\n{tail}")
    } else {
        text.chars().take(MAX_RESULT_CHARS).collect::<String>() + "\n[truncated]"
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
