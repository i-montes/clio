use std::collections::{BTreeMap, HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use clio_core::{Effect, Level};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::normalize::{NormalizedAction, confine, domain_matches, host_of, markers};
use crate::types::{ActionRequest, ArgKind, Decision, Egress, Outcome, ToolDecl};

/// The gateway's own rules, one JSON document (task 221: a format of our own; it gets unified
/// with the policies in phase 5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rules {
    /// Where reversible work may happen.
    pub sandbox_root: PathBuf,
    /// Binaries a command may start with (S-12). Everything else is denied.
    #[serde(default)]
    pub allowed_binaries: Vec<String>,
    /// Destinations already known; anything else is "new" (S-06).
    #[serde(default)]
    pub trusted_domains: Vec<String>,
    /// Paths under the sandbox (extensions, skills...) that nothing here may write (S-16).
    #[serde(default)]
    pub protected_paths: Vec<String>,
    /// S-56: until day zero is complete, anything outside the sandbox is denied. Defaults to
    /// false, so a missing field fails closed.
    #[serde(default)]
    pub day_zero_complete: bool,
    /// Consecutive denials in a session before it asks instead of letting the loop retry.
    #[serde(default = "default_breaker")]
    pub breaker_threshold: u32,
    pub tools: Vec<ToolDecl>,
    #[serde(default)]
    pub secret_bindings: Vec<SecretBinding>,
}

fn default_breaker() -> u32 {
    3
}

/// A secret tied to its destination (S-19).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretBinding {
    pub secret: String,
    pub tool: String,
    pub domains: Vec<String>,
}

/// The gateway does not depend on the vault: it asks this.
pub trait SecretPolicy: Send + Sync {
    /// May `secret` be resolved for `tool`, towards `domain` (None: no destination)?
    fn allowed(&self, secret: &str, tool: &str, domain: Option<&str>) -> bool;
}

pub struct StaticSecretPolicy(Vec<SecretBinding>);

impl SecretPolicy for StaticSecretPolicy {
    fn allowed(&self, secret: &str, tool: &str, domain: Option<&str>) -> bool {
        self.0.iter().any(|b| {
            b.secret == secret
                && b.tool == tool
                && match domain {
                    Some(d) => b.domains.iter().any(|x| domain_matches(d, x)),
                    None => b.domains.is_empty(),
                }
        })
    }
}

/// Step 9 hook: a standing policy that covers every dimension of the action. Phase 5 plugs the
/// real one in; until then nothing is covered.
pub trait Policy: Send + Sync {
    fn covers(&self, action: &NormalizedAction) -> bool;
}

pub struct NoPolicy;

impl Policy for NoPolicy {
    fn covers(&self, _: &NormalizedAction) -> bool {
        false
    }
}

/// Approval by a person (a terminal in this phase). It sees the normalized action.
pub trait Approver {
    fn approve(&self, decision: &Decision, action: &NormalizedAction) -> bool;
}

/// What a person approved: the hash of the exact action (S-14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub hash: String,
}

/// Runs an action; only the gateway calls it, with the hash it checked.
pub trait Executor {
    fn run(&self, action: &NormalizedAction, hash: &str) -> Result<String, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecResult {
    Done(String),
    Denied(Decision),
    NotApproved(Decision),
    /// Away mode: the action waits, with its approval ready to sign.
    SetAside {
        decision: Decision,
        hash: String,
    },
    /// The action changed after it was approved.
    HashMismatch {
        approved: String,
        actual: String,
    },
    Failed(String),
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Telemetry {
    /// `"deny:S-12" -> n`
    pub decisions: BTreeMap<String, u64>,
    pub denials: u64,
    pub secret_to_foreign_destination: u64,
    pub alerts: Vec<String>,
    pub decision_micros_total: u64,
}

#[derive(Default)]
struct State {
    disabled: HashSet<String>,
    tainted: HashSet<String>,
    consecutive_denials: HashMap<String, u32>,
    telemetry: Telemetry,
    counter: u64,
}

struct Verdict {
    outcome: Outcome,
    rule: &'static str,
    reason: String,
}

fn deny(rule: &'static str, reason: impl Into<String>) -> Verdict {
    Verdict {
        outcome: Outcome::Deny,
        rule,
        reason: reason.into(),
    }
}

fn ask(rule: &'static str, reason: impl Into<String>) -> Verdict {
    Verdict {
        outcome: Outcome::Ask,
        rule,
        reason: reason.into(),
    }
}

pub struct Gateway {
    root: PathBuf,
    rules: Rules,
    tools: HashMap<String, ToolDecl>,
    secrets: Box<dyn SecretPolicy>,
    policy: Box<dyn Policy>,
    state: Mutex<State>,
}

impl Gateway {
    pub fn new(rules: Rules) -> Result<Self, String> {
        let secrets = Box::new(StaticSecretPolicy(rules.secret_bindings.clone()));
        let root = rules
            .sandbox_root
            .canonicalize()
            .map_err(|e| format!("sandbox_root: {e}"))?;
        let tools = rules
            .tools
            .iter()
            .map(|t| (t.name.clone(), t.clone()))
            .collect();
        Ok(Self {
            root,
            rules,
            tools,
            secrets,
            policy: Box::new(NoPolicy),
            state: Mutex::new(State::default()),
        })
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        Self::new(serde_json::from_str(json).map_err(|e| format!("rules: {e}"))?)
    }

    #[must_use]
    pub fn with_secret_policy(mut self, p: Box<dyn SecretPolicy>) -> Self {
        self.secrets = p;
        self
    }

    #[must_use]
    pub fn with_policy(mut self, p: Box<dyn Policy>) -> Self {
        self.policy = p;
        self
    }

    fn st(&self) -> MutexGuard<'_, State> {
        // The state is plain counters and sets: after a panic elsewhere it is still usable.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn telemetry(&self) -> Telemetry {
        self.st().telemetry.clone()
    }

    /// Mark a session as tainted on the gateway side (S-05); there is no way to unmark.
    pub fn mark_tainted(&self, session_id: &str) {
        self.st().tainted.insert(session_id.to_string());
    }

    /// A person reviewed the tool after its description changed (S-15): turn it back on.
    /// Returns false for an unknown tool. The pinned hash becomes `new_hash`.
    pub fn review_tool(&self, tool: &str, new_hash: &str) -> bool {
        let known = self.tools.contains_key(tool);
        if known {
            let mut st = self.st();
            st.disabled.remove(tool);
            st.telemetry.alerts.push(format!(
                "S-15: tool {tool} re-enabled after review (hash {new_hash} is only valid if the rules are updated)"
            ));
        }
        known
    }

    pub fn reset_breaker(&self, session_id: &str) {
        self.st().consecutive_denials.remove(session_id);
    }

    pub fn decide(&self, req: &ActionRequest) -> Decision {
        self.decide_full(req, true).0
    }

    /// S-11 / spec "fail closed": an error or a panic in here is a deny, never a pass.
    pub fn decide_or_deny(&self, req: &ActionRequest) -> Decision {
        catch_unwind(AssertUnwindSafe(|| self.decide(req)))
            .unwrap_or_else(|_| Decision::fail_closed("the gateway panicked while deciding"))
    }

    fn decide_full(
        &self,
        req: &ActionRequest,
        record: bool,
    ) -> (Decision, Option<NormalizedAction>) {
        let start = Instant::now();
        let mut st = self.st();
        let (mut v, norm) = match self.prepare(req, &mut st) {
            Err(v) => (v, None),
            Ok((tool, norm)) => (self.rest(req, &mut st, &tool, &norm), Some(norm)),
        };
        let sid = &req.session.id;
        let hash = norm.as_ref().map(NormalizedAction::hash);
        // Circuit breaker: after N denials in a row the session stops being let through on its
        // own. A deny stays a deny; what was going to pass now asks.
        let denials = st.consecutive_denials.get(sid).copied().unwrap_or(0);
        if v.outcome == Outcome::Allow && denials >= self.rules.breaker_threshold {
            v = ask(
                "circuit-breaker",
                format!("{denials} denials in a row in this session: a person must look"),
            );
        }
        match v.outcome {
            Outcome::Deny => *st.consecutive_denials.entry(sid.clone()).or_default() += 1,
            Outcome::Allow => {
                st.consecutive_denials.remove(sid);
            }
            Outcome::Ask => {}
        }
        st.counter += 1;
        let id = format!(
            "dec-{:06}-{}",
            st.counter,
            hash.as_deref().map_or("none", |h| &h[..8])
        );
        if record {
            let t = &mut st.telemetry;
            *t.decisions
                .entry(format!("{}:{}", v.outcome.as_str(), v.rule))
                .or_default() += 1;
            if v.outcome == Outcome::Deny {
                t.denials += 1;
            }
            t.decision_micros_total += start.elapsed().as_micros() as u64;
        }
        let d = Decision {
            outcome: v.outcome,
            rule: v.rule.to_string(),
            reason: v.reason,
            id,
            action_hash: hash,
        };
        (d, norm)
    }

    /// Steps 1 and 2: the declaration, the schema, the pinned description, and normalizing.
    fn prepare(
        &self,
        req: &ActionRequest,
        st: &mut State,
    ) -> Result<(ToolDecl, NormalizedAction), Verdict> {
        let tool = self.tools.get(&req.tool).ok_or_else(|| {
            deny(
                "S-02",
                format!("tool {:?} has no declaration: denied by default", req.tool),
            )
        })?;
        if st.disabled.contains(&tool.name) {
            return Err(deny(
                "S-15",
                format!("tool {} is disabled until a person reviews it", tool.name),
            ));
        }
        if req.version != tool.version || req.description_hash != tool.description_hash {
            st.disabled.insert(tool.name.clone());
            st.telemetry.alerts.push(format!(
                "S-15: version or description of tool {} changed; disabled",
                tool.name
            ));
            return Err(deny(
                "S-15",
                format!(
                    "tool {} changed (version or description hash): disabled until reviewed",
                    tool.name
                ),
            ));
        }
        // Schema: only declared arguments, required ones present, right types.
        if let Some(k) = req
            .args
            .keys()
            .find(|k| !tool.args.iter().any(|a| &a.name == *k))
        {
            return Err(deny("step-1-schema", format!("unknown argument {k:?}")));
        }
        let mut args = req.args.clone();
        let (mut paths, mut domains, mut argv) = (Vec::new(), Vec::new(), None);
        for spec in &tool.args {
            let Some(v) = req.args.get(&spec.name) else {
                if spec.required {
                    return Err(deny(
                        "step-1-schema",
                        format!("missing argument {:?}", spec.name),
                    ));
                }
                continue;
            };
            let bad = |what: &str| {
                deny(
                    "step-1-schema",
                    format!("argument {:?} must be {what}", spec.name),
                )
            };
            match (spec.kind, v) {
                (ArgKind::Bool, Value::Bool(_)) | (ArgKind::Text, Value::String(_)) => {}
                (ArgKind::Int, Value::Number(n)) if n.is_i64() || n.is_u64() => {}
                (ArgKind::Path, Value::String(s)) => {
                    let p = confine(&self.root, s).map_err(|e| deny("S-13", e))?;
                    args.insert(
                        spec.name.clone(),
                        Value::from(p.to_string_lossy().into_owned()),
                    );
                    paths.push(p);
                }
                (ArgKind::Url, Value::String(s)) => {
                    domains.push(host_of(s).map_err(|e| deny("step-2-domain", e))?);
                }
                (ArgKind::Argv, Value::Array(a)) => {
                    let list: Option<Vec<String>> =
                        a.iter().map(|x| x.as_str().map(str::to_owned)).collect();
                    let list = list.ok_or_else(|| bad("an array of strings"))?;
                    // The binary is a bare name from the list; the rest is data, never parsed.
                    match list.first() {
                        Some(b) if self.rules.allowed_binaries.contains(b) => {}
                        Some(b) => {
                            return Err(deny(
                                "S-12",
                                format!("binary {b:?} is not in the allowed list"),
                            ));
                        }
                        None => return Err(deny("S-12", "empty command")),
                    }
                    argv = Some(list);
                }
                (ArgKind::Argv, Value::String(_)) => {
                    return Err(deny(
                        "S-12",
                        "a command is an argv array, never a shell string",
                    ));
                }
                (ArgKind::Text | ArgKind::Path | ArgKind::Url, _) => return Err(bad("a string")),
                (ArgKind::Bool, _) => return Err(bad("a boolean")),
                (ArgKind::Int, _) => return Err(bad("an integer")),
                (ArgKind::Argv, _) => return Err(bad("an array of strings")),
            }
        }
        // Destination: from the URL arguments, or the tool's only declared domain.
        match &tool.egress {
            Egress::None => {}
            Egress::Any if domains.is_empty() => {
                return Err(deny("step-2-domain", "destination could not be resolved"));
            }
            Egress::Domains(list) if domains.is_empty() => domains.extend(list.iter().cloned()),
            _ => {}
        }
        let mut secrets = Vec::new();
        markers(&Value::Object(req.args.clone()), &mut secrets);
        secrets.sort();
        secrets.dedup();
        Ok((
            tool.clone(),
            NormalizedAction {
                tool: tool.name.clone(),
                version: tool.version.clone(),
                node: req.node.clone(),
                args,
                domains,
                paths,
                argv,
                secrets,
            },
        ))
    }

    /// Steps 3 to 10.
    fn rest(
        &self,
        req: &ActionRequest,
        st: &mut State,
        tool: &ToolDecl,
        n: &NormalizedAction,
    ) -> Verdict {
        let s = &req.session;
        // 3. Hard prohibitions: a deny here is final.
        if s.subagent && tool.effect > Effect::Read {
            return deny("S-17", "subagents only read");
        }
        if s.subagent && !n.secrets.is_empty() {
            return deny("S-17", "subagents receive no secrets");
        }
        if !self.rules.day_zero_complete
            && (tool.effect >= Effect::ExternalWrite || tool.egress != Egress::None)
        {
            return deny(
                "S-56",
                "day zero is not complete: nothing outside the sandbox",
            );
        }
        if let Egress::Domains(list) = &tool.egress
            && let Some(d) = n
                .domains
                .iter()
                .find(|d| !list.iter().any(|x| domain_matches(d, x)))
        {
            return deny("S-02", format!("domain {d} is not in the tool's list"));
        }
        if tool.effect >= Effect::LocalReversible
            && let Some(p) = n.paths.iter().find(|p| {
                self.rules
                    .protected_paths
                    .iter()
                    .any(|x| p.starts_with(self.root.join(x)))
            })
        {
            return deny(
                "S-16",
                format!("{} is a path nothing here may write", p.display()),
            );
        }
        // 4. Budget.
        if req.budget.exhausted() {
            return deny("S-47", "budget exhausted: hard stop");
        }
        // 5. Secrets, tied to tool and destination.
        for sec in &n.secrets {
            let ok = if n.domains.is_empty() {
                self.secrets.allowed(sec, &tool.name, None)
            } else {
                n.domains
                    .iter()
                    .all(|d| self.secrets.allowed(sec, &tool.name, Some(d)))
            };
            if !ok {
                st.telemetry.secret_to_foreign_destination += 1;
                st.telemetry.alerts.push(format!(
                    "S-19: secret {sec} requested for tool {} towards {:?}",
                    tool.name, n.domains
                ));
                return deny(
                    "S-19",
                    format!("secret {sec} is not allowed for this tool and destination"),
                );
            }
        }
        // 6. A tainted session does not send anything out on its own.
        let parent_tainted = s.parent.as_ref().is_some_and(|p| st.tainted.contains(p));
        let tainted = s.tainted || parent_tainted || st.tainted.contains(&s.id);
        if tainted {
            st.tainted.insert(s.id.clone());
        }
        if tainted
            && tool.egress != Egress::None
            && (tool.effect >= Effect::ExternalWrite
                || n.domains.iter().any(|d| {
                    !self
                        .rules
                        .trusted_domains
                        .iter()
                        .any(|t| domain_matches(d, t))
                }))
        {
            return ask(
                "S-06",
                "the session read external content and this action sends data out",
            );
        }
        // 7. The plan.
        if s.level >= Level::Full && !req.plan.contains(&tool.name) {
            return ask("step-7-plan", "the action is not in the plan");
        }
        // 8. Reversible and inside the sandbox (paths were confined in step 2).
        if tool.effect <= Effect::LocalReversible && tool.egress == Egress::None {
            return Verdict {
                outcome: Outcome::Allow,
                rule: "step-8-reversible-in-sandbox",
                reason: "reversible and inside the sandbox".into(),
            };
        }
        // 9. A standing policy. Irreversible actions never ride on one (S-14).
        if tool.effect < Effect::Irreversible && self.policy.covers(n) {
            return Verdict {
                outcome: Outcome::Allow,
                rule: "step-9-policy",
                reason: "a policy covers every dimension".into(),
            };
        }
        // 10.
        ask("S-14", "approval over the hash of this exact action")
    }

    /// Run `req` only if it is allowed, or approved by a person over its exact hash. In away
    /// mode what would ask is set aside instead of waiting; irreversible actions never run.
    pub fn execute(
        &self,
        req: &ActionRequest,
        approver: &dyn Approver,
        exec: &dyn Executor,
        away: bool,
    ) -> ExecResult {
        let (d, norm) = self.decide_full(req, true);
        let (Some(norm), Some(hash)) = (norm, d.action_hash.clone()) else {
            return ExecResult::Denied(d);
        };
        match d.outcome {
            Outcome::Deny => ExecResult::Denied(d),
            Outcome::Allow => self.run(&norm, &hash, exec),
            Outcome::Ask if away => ExecResult::SetAside { decision: d, hash },
            Outcome::Ask => {
                if !approver.approve(&d, &norm) {
                    return ExecResult::NotApproved(d);
                }
                self.reset_breaker(&req.session.id);
                self.execute_approved(req, &Approval { hash }, exec)
            }
        }
    }

    /// S-14: the action as it is now must hash to what was approved.
    pub fn execute_approved(
        &self,
        req: &ActionRequest,
        approval: &Approval,
        exec: &dyn Executor,
    ) -> ExecResult {
        let (d, norm) = self.decide_full(req, false);
        let (Some(norm), Some(actual)) = (norm, d.action_hash.clone()) else {
            return ExecResult::Denied(d);
        };
        if d.outcome == Outcome::Deny {
            return ExecResult::Denied(d);
        }
        if d.outcome == Outcome::Ask && actual != approval.hash {
            return ExecResult::HashMismatch {
                approved: approval.hash.clone(),
                actual,
            };
        }
        self.run(&norm, &actual, exec)
    }

    fn run(&self, norm: &NormalizedAction, hash: &str, exec: &dyn Executor) -> ExecResult {
        // The executor resolves secrets itself, last, inside its own process (spec: Ejecución).
        match catch_unwind(AssertUnwindSafe(|| exec.run(norm, hash))) {
            Ok(Ok(out)) => ExecResult::Done(out),
            Ok(Err(e)) => ExecResult::Failed(e),
            Err(_) => ExecResult::Failed("the executor panicked".into()),
        }
    }
}

/// Approval on the terminal: shows the action and reads y/N from stdin.
pub struct TerminalApprover;

impl Approver for TerminalApprover {
    fn approve(&self, d: &Decision, a: &NormalizedAction) -> bool {
        eprintln!(
            "[{}] {} ({})\n  {} {:?}",
            d.id, a.tool, d.rule, d.reason, a.args
        );
        eprint!("approve? [y/N] ");
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).is_ok() && line.trim().eq_ignore_ascii_case("y")
    }
}
