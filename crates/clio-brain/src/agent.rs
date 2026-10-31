//! The minimum agent, end to end: triage, contract (level 2 and up), a loop of model turns where
//! every tool call goes through the gateway and then the node executor inside the sandbox, with
//! the control layer on every step. Time comes from a `Clock`; the model is a trait.
//!
//! Turn protocol (plain text, so any chat model can speak it): the model answers with ONE JSON
//! object per turn, `{"tool": "...", "args": {...}}` or `{"final": "..."}`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clio_contract as contract;
use clio_core::{Clock, Effect, Level};
use clio_gateway::{
    ActionRequest, ArgKind, ArgSpec, Budget as GwBudget, Decision, Egress, Gateway, Outcome, Rules,
    SessionInfo, Source, ToolDecl, wrap_untrusted,
};
use clio_node::{Action as NodeAction, EffectLog, Registry, Sandbox};
use clio_triage::{PersonMessage, TriageInput, Triager};
use clio_vault::Scrubber;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::control::{Budget, Controller, DayBudget, OutputFilter, Stop};
use crate::verify::{Checklist, Criterion, ExternalSignal, Scope, Signal};

/// One chat message. Same JSON as the bench's `Request` messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Msg {
    pub role: String,
    pub content: String,
}

impl Msg {
    fn new(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Reply {
    pub text: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Something that answers a conversation. Closures `FnMut(&[Msg]) -> String` are models too
/// (scripted ones, for tests).
pub trait Model {
    fn complete(&mut self, messages: &[Msg]) -> Result<Reply, String>;
}

impl<F: FnMut(&[Msg]) -> String> Model for F {
    fn complete(&mut self, messages: &[Msg]) -> Result<Reply, String> {
        Ok(Reply {
            text: self(messages),
            ..Reply::default()
        })
    }
}

#[derive(Serialize)]
struct WireRequest<'a> {
    model: &'a str,
    messages: &'a [Msg],
    tools: Vec<String>,
    max_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct WireResponse {
    content: String,
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
}

/// A real model reached through a program that reads a `Request` as JSON on stdin and writes a
/// `Response` as JSON on stdout (the contract of `clio-bench --cmd`). No HTTP and no keys here.
/// A failed call is retried after each pause in `backoff_ms` (quota and network errors pass).
/// An empty answer is NOT retried: at temperature 0 it would come back empty again, so the loop
/// answers it with a protocol error, which changes the conversation.
/// shortcut: one process per call, and the sleeps block; fine for a bench, not for a server.
pub struct CommandModel {
    pub program: String,
    pub args: Vec<String>,
    pub model: String,
    pub max_tokens: u32,
    pub backoff_ms: Vec<u64>,
}

impl CommandModel {
    pub fn new(program: &str, args: &[String], model: &str) -> Self {
        Self {
            program: program.into(),
            args: args.to_vec(),
            model: model.into(),
            max_tokens: 4096,
            backoff_ms: vec![5_000, 20_000, 60_000],
        }
    }

    fn once(&self, messages: &[Msg]) -> Result<Reply, String> {
        let fail = |m: String| format!("{}: {m}", self.program);
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| fail(e.to_string()))?;
        let body = serde_json::to_vec(&WireRequest {
            model: &self.model,
            messages,
            tools: vec![],
            max_tokens: Some(self.max_tokens),
        })
        .map_err(|e| fail(e.to_string()))?;
        child
            .stdin
            .take()
            .ok_or_else(|| fail("no stdin".into()))?
            .write_all(&body)
            .map_err(|e| fail(e.to_string()))?;
        let out = child.wait_with_output().map_err(|e| fail(e.to_string()))?;
        if !out.status.success() {
            let err: String = String::from_utf8_lossy(&out.stderr)
                .chars()
                .take(300)
                .collect();
            return Err(fail(format!("exit {}: {}", out.status, err.trim())));
        }
        let r: WireResponse =
            serde_json::from_slice(&out.stdout).map_err(|e| fail(format!("bad answer: {e}")))?;
        Ok(Reply {
            text: r.content,
            input_tokens: r.input_tokens,
            output_tokens: r.output_tokens,
        })
    }
}

impl Model for CommandModel {
    fn complete(&mut self, messages: &[Msg]) -> Result<Reply, String> {
        let mut result = self.once(messages);
        for pause in self.backoff_ms.clone() {
            if result.is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(pause));
            result = self.once(messages);
        }
        result
    }
}

/// What the model asked for in one turn.
#[derive(Debug, Clone, PartialEq)]
pub enum Turn {
    Tool { name: String, args: Value },
    Final(String),
}

/// The first JSON object in `text` that is a turn. Tolerates fences and text around it.
pub fn parse_turn(text: &str) -> Result<Turn, String> {
    let mut saw_object = false;
    for (i, _) in text.match_indices('{') {
        let mut it = serde_json::Deserializer::from_str(&text[i..]).into_iter::<Value>();
        let Some(Ok(Value::Object(o))) = it.next() else {
            continue;
        };
        saw_object = true;
        if let Some(f) = o.get("final") {
            return Ok(Turn::Final(
                f.as_str().map_or_else(|| f.to_string(), str::to_owned),
            ));
        }
        if let Some(name) = o.get("tool").and_then(Value::as_str) {
            let args = o.get("args").cloned().unwrap_or(Value::Object(Map::new()));
            return Ok(Turn::Tool {
                name: name.into(),
                args,
            });
        }
    }
    Err(if saw_object {
        "the JSON object has neither \"tool\" nor \"final\"".into()
    } else {
        "no valid JSON object found".into()
    })
}

/// Why the loop ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    /// The model said it is done (not verified by anything outside it).
    Final,
    Stopped(Stop),
    ModelError(String),
    /// Level 3 needs a person to approve the contract and nobody did.
    NeedsApproval,
    /// The person asked to stop (`AgentLoop::cancel`); it ended after the step in progress.
    Cancelled,
}

/// What the loop reports to whoever watches it (a screen, a progress line). Text fields come
/// from the model and from files: whoever shows them must sanitize them first.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Triage decided the level of the task.
    Started { level: Level },
    /// A model turn was accepted by the controller; `n` counts steps from 1.
    Step { n: u32 },
    /// The model answered something that is not a valid turn.
    ProtocolError { reason: String },
    /// The model asked for a tool and the gateway decided. `ran` is false when it was not run.
    Tool {
        name: String,
        target: String,
        outcome: Outcome,
        rule: String,
        ran: bool,
        result_chars: usize,
        tainted: bool,
    },
    /// The model said it is done.
    Final,
}

/// Watches the loop. The default method does nothing, so an observer only handles what it wants.
pub trait Observer {
    fn event(&mut self, _event: &Event) {}
}

/// Nobody is watching.
pub struct NoObserver;

impl Observer for NoObserver {}

/// The closing report: done, pending, next step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosingReport {
    pub done: Vec<String>,
    pub pending: Vec<String>,
    pub next_step: String,
}

#[derive(Debug, Clone)]
pub struct AgentOutcome {
    pub ending: Ending,
    pub final_message: Option<String>,
    pub level: Level,
    pub steps: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_micro: u64,
    pub elapsed_ms: i64,
    /// Tool calls the gateway denied or left waiting for a person.
    pub denied: u32,
    /// Turns whose answer was not a valid turn (empty, no JSON, wrong keys).
    pub protocol_errors: u32,
    pub tainted: bool,
    /// How many times `run_tests` ran, and whether the last run came after the last change and passed.
    pub tests_run: u32,
    pub verified_by_tests: bool,
    pub contract_state: Option<contract::State>,
    pub report: ClosingReport,
    pub transcript: Vec<Msg>,
}

pub struct AgentConfig {
    pub task_id: String,
    pub system_prompt: String,
    /// For the trace, e.g. `clio-agent@1`.
    pub prompt_version: String,
    pub budget: Budget,
    pub day_max_cost_micro: u64,
    /// Micro-USD per million tokens.
    pub price_in_micro_per_mtok: u64,
    pub price_out_micro_per_mtok: u64,
    /// Where the contract record goes: a folder OUTSIDE the workspace.
    pub run_dir: PathBuf,
    /// Never below this, whatever triage says (tests use it to force the contract path).
    pub min_level: Level,
    /// Level 3 only: a person already approved the contract (a bench has no one to ask).
    pub person_approves_long: bool,
    pub max_result_chars: usize,
    /// Offer `run_tests` (the jail). The caller must have proven the jail (`Jail::probe` and
    /// `Jail::self_test`) before setting this; it also switches the sandbox to copy snapshots.
    pub run_tests: bool,
}

impl AgentConfig {
    pub fn new(task_id: &str, system_prompt: &str, run_dir: &Path) -> Self {
        Self {
            task_id: task_id.into(),
            system_prompt: system_prompt.into(),
            prompt_version: "clio-agent@1".into(),
            budget: Budget {
                max_steps: 40,
                max_ms: 600_000,
                max_cost_micro: 100_000,
                max_repeats: 4,
                max_retries: 3,
            },
            day_max_cost_micro: u64::MAX,
            price_in_micro_per_mtok: 0,
            price_out_micro_per_mtok: 0,
            run_dir: run_dir.into(),
            min_level: Level::Direct,
            person_approves_long: false,
            max_result_chars: 12_000,
            run_tests: false,
        }
    }
}

/// A person who can say yes to an action the gateway wants to ask about.
pub trait Asker {
    fn approve(&self, decision: &Decision, tool: &str, args: &Map<String, Value>) -> bool;
}

/// Nobody is there: every question is a no.
pub struct NobodyHome;

impl Asker for NobodyHome {
    fn approve(&self, _: &Decision, _: &str, _: &Map<String, Value>) -> bool {
        false
    }
}

/// An approval the gateway's decision produced for exactly this action.
struct Approved(String);

impl clio_node::Approval for Approved {
    fn is_valid_for(&self, hash: &str) -> bool {
        self.0 == hash
    }
}

fn digest_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// S-16: folders under the workspace root that load code or settings by themselves (Pi
/// extensions, skills, hooks). The agent may read them, never write them.
/// shortcut: root level only and case-sensitive names; widen when the node has other loaders.
pub const PROTECTED_PATHS: [&str; 5] = ["extensions", "skills", ".pi", ".claude", ".git"];

/// The declared arguments of the basic tools (the registry holds code, not schemas).
fn arg_specs(tool: &str) -> Vec<ArgSpec> {
    let a = |name: &str, kind| ArgSpec {
        name: name.into(),
        kind,
        required: true,
    };
    match tool {
        "write_file" => vec![a("path", ArgKind::Path), a("content", ArgKind::Text)],
        "run_tests" => vec![
            a("runner", ArgKind::Text),
            ArgSpec {
                name: "target".into(),
                kind: ArgKind::Path,
                required: false,
            },
        ],
        _ => vec![a("path", ArgKind::Path)],
    }
}

/// The tool section of the prompt, from the registry: name, arguments, what it does.
pub fn tool_help(registry: &Registry) -> String {
    registry
        .visible()
        .iter()
        .map(|t| {
            let args: Vec<String> = arg_specs(&t.name)
                .iter()
                .map(|a| format!("\"{}\"", a.name))
                .collect();
            format!(
                "- {} (args: {}): {}",
                t.name,
                args.join(", "),
                t.description
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// What the last `run_tests` said, kept as the external signal for the contract.
#[derive(Debug, Default)]
struct TestState {
    runs: u32,
    /// Result of the last run; `None` before the first one.
    last: Option<TestRun>,
    /// A file was written after the last run: its result no longer says anything.
    wrote_since: bool,
}

#[derive(Debug, Clone)]
struct TestRun {
    passed: bool,
    exit_code: Option<i64>,
    tail: String,
}

pub struct AgentLoop {
    clock: Arc<dyn Clock>,
    cfg: AgentConfig,
    gateway: Gateway,
    executor: clio_node::Executor,
    descriptions: Vec<(String, String)>,
    pub scrubber: Option<Scrubber>,
    pub filter: Option<Box<dyn OutputFilter>>,
    pub asker: Box<dyn Asker>,
    pub observer: Box<dyn Observer>,
    /// When set to true, the loop ends with `Ending::Cancelled` before its next model call.
    pub cancel: Option<Arc<AtomicBool>>,
    tests: TestState,
}

impl AgentLoop {
    /// `workdir` is the only place the agent may touch. `scratch` and `cfg.run_dir` must be
    /// outside it.
    pub fn new(
        clock: Arc<dyn Clock>,
        cfg: AgentConfig,
        workdir: &Path,
        scratch: &Path,
    ) -> Result<Self, String> {
        let e = |x: &dyn std::fmt::Display| x.to_string();
        let mut registry = Registry::basic().map_err(|x| e(&x))?;
        if cfg.run_tests {
            clio_node::register_run_tests(&mut registry).map_err(|x| e(&x))?;
        }
        let mut tools = Vec::new();
        let mut descriptions = Vec::new();
        for t in registry.visible() {
            let hash = digest_hex(&t.description);
            tools.push(ToolDecl {
                name: t.name.clone(),
                version: "1".into(),
                effect: t.effect.unwrap_or(Effect::Irreversible),
                egress: Egress::None,
                description_hash: hash.clone(),
                args: arg_specs(&t.name),
            });
            descriptions.push((t.name.clone(), hash));
        }
        let gateway = Gateway::new(Rules {
            sandbox_root: workdir.to_path_buf(),
            allowed_binaries: vec![],
            trusted_domains: vec![],
            protected_paths: PROTECTED_PATHS.iter().map(|p| (*p).to_owned()).collect(),
            day_zero_complete: false,
            breaker_threshold: 3,
            tools,
            secret_bindings: vec![],
        })?;
        std::fs::create_dir_all(&cfg.run_dir).map_err(|x| e(&x))?;
        let log = EffectLog::open(&cfg.run_dir.join("effects.jsonl")).map_err(|x| e(&x))?;
        let mut sandbox = Sandbox::new(workdir, scratch).map_err(|x| e(&x))?;
        // A program that runs in the folder can edit files in place: snapshots must be copies.
        sandbox.set_copy_snapshots(cfg.run_tests);
        Ok(Self {
            clock,
            cfg,
            gateway,
            executor: clio_node::Executor {
                registry,
                sandbox,
                log,
            },
            descriptions,
            scrubber: None,
            filter: None,
            asker: Box::new(NobodyHome),
            observer: Box::new(NoObserver),
            cancel: None,
            tests: TestState::default(),
        })
    }

    pub fn gateway(&self) -> &Gateway {
        &self.gateway
    }

    fn cost(&self, r: &Reply) -> u64 {
        let micro = u128::from(r.input_tokens) * u128::from(self.cfg.price_in_micro_per_mtok)
            + u128::from(r.output_tokens) * u128::from(self.cfg.price_out_micro_per_mtok);
        micro.div_ceil(1_000_000) as u64
    }

    /// Scrub, cut, filter and wrap a tool result as data the model must not obey.
    fn present(&self, source: Source, raw: &str, keep_end: bool) -> String {
        let mut text = self
            .scrubber
            .as_ref()
            .map_or_else(|| raw.to_owned(), |s| s.scrub(raw));
        let n = text.chars().count();
        if n > self.cfg.max_result_chars {
            text = if keep_end {
                // Test output: the summary of the failures is at the end.
                let tail: String = text.chars().skip(n - self.cfg.max_result_chars).collect();
                format!("[truncated: the end is shown]\n{tail}")
            } else {
                text.chars()
                    .take(self.cfg.max_result_chars)
                    .collect::<String>()
                    + "\n[truncated]"
            };
        }
        if let Some(f) = &self.filter {
            text = f
                .filter(&text)
                .unwrap_or_else(|why| format!("[withheld: {why}]"));
        }
        wrap_untrusted(source, &text)
    }

    pub fn run(
        &mut self,
        model: &mut dyn Model,
        instruction: &str,
    ) -> Result<AgentOutcome, String> {
        let clock = self.clock.clone();
        let id = self.cfg.task_id.clone();
        self.tests = TestState::default();

        // 1. Triage, over the person's message only (S-07).
        let mut triager = Triager::rules_only(clock.clone());
        let input = TriageInput::new(PersonMessage::from_person(instruction), "local", "person");
        let level = triager.triage(&input).0.level.max(self.cfg.min_level);

        // 2. Contract, level 2 and up: objective = the person's instruction, saved outside the workspace.
        let record = self.cfg.run_dir.join("contract.json");
        let mut contract = None;
        if level >= Level::Full {
            let b = self.cfg.budget;
            let mut c = contract::Contract::draft(
                format!("{id}-contract"),
                level,
                instruction,
                contract::Origin::Person,
                format!("{id}-trace"),
                clock.as_ref(),
            )
            .map_err(|x| x.to_string())?;
            let p = contract::Actor::Person;
            let ce = |x: contract::Error| x.to_string();
            c.set_scope(
                p,
                contract::Scope {
                    included: vec![".".into()],
                    excluded: vec![],
                },
            )
            .map_err(ce)?;
            c.set_budget(
                p,
                contract::Budget {
                    max_steps: u64::from(b.max_steps),
                    max_ms: b.max_ms,
                    max_cost_micro_cents: b.max_cost_micro.saturating_mul(100),
                },
            )
            .map_err(ce)?;
            c.add_criterion(contract::Criterion::new(
                "the objective is met",
                Some(contract::Check {
                    kind: contract::CheckKind::Command,
                    detail: "a check outside the model".into(),
                }),
            ));
            let digest =
                (level == Level::Long && self.cfg.person_approves_long).then(|| c.digest());
            if level == Level::Long && digest.is_none() {
                return Ok(self.outcome_without_loop(level, Ending::NeedsApproval));
            }
            c.approve(p, digest.as_deref(), clock.as_ref())
                .map_err(ce)?;
            c.start(&record, clock.as_ref()).map_err(ce)?;
            contract = Some(c);
        }

        // 3. The loop.
        self.observer.event(&Event::Started { level });
        let mut ctl = Controller::new(clock.clone(), &id, level, self.cfg.budget);
        let mut day = DayBudget::new(self.cfg.day_max_cost_micro);
        let mut session = clio_gateway::Session::new(format!("{id}-session"));
        let plan: Vec<String> = if level >= Level::Full {
            self.descriptions.iter().map(|(n, _)| n.clone()).collect()
        } else {
            vec![]
        };
        let system = format!(
            "{}\n\nTools:\n{}",
            self.cfg.system_prompt,
            tool_help(&self.executor.registry)
        );
        let mut messages = vec![Msg::new("system", system), Msg::new("user", instruction)];
        let (mut tin, mut tout, mut denied, mut n_calls, mut protocol_errors) =
            (0u64, 0u64, 0u32, 0u32, 0u32);
        let mut done: Vec<String> = Vec::new();
        let (ending, final_message) = loop {
            if self
                .cancel
                .as_ref()
                .is_some_and(|c| c.load(Ordering::Relaxed))
            {
                break (Ending::Cancelled, None);
            }
            // Hard stop before spending on a model call.
            if let Err(s) = ctl.check_limits(0, &mut day) {
                break (Ending::Stopped(s), None);
            }
            let reply = match model.complete(&messages) {
                Ok(r) => r,
                Err(m) => break (Ending::ModelError(m), None),
            };
            tin += u64::from(reply.input_tokens);
            tout += u64::from(reply.output_tokens);
            let cost = self.cost(&reply);
            messages.push(Msg::new("assistant", reply.text.clone()));
            let turn = parse_turn(&reply.text);
            let key = match &turn {
                Ok(Turn::Tool { name, args }) => format!("{name}:{args}"),
                Ok(Turn::Final(_)) => "final".to_owned(),
                // Junk repeated verbatim is a loop; an empty answer is not an action, and the
                // step budget bounds those.
                Err(_) if reply.text.trim().is_empty() => format!("empty:{}", messages.len()),
                Err(_) => format!("invalid:{}", digest_hex(&reply.text)),
            };
            if let Err(s) = ctl.begin_step(&key, cost, &mut day) {
                break (Ending::Stopped(s), None);
            }
            self.observer.event(&Event::Step {
                n: ctl.checkpoint().steps,
            });
            if let Some(c) = contract.as_mut() {
                // The contract mirrors the budget; if it closes itself the controller stops us next turn.
                let _ = c.spend(1, cost.saturating_mul(100), clock.as_ref());
            }
            match turn {
                Err(mut why) => {
                    protocol_errors += 1;
                    if reply.text.trim().is_empty() {
                        why = "your reply was empty".into();
                    }
                    self.observer.event(&Event::ProtocolError {
                        reason: why.clone(),
                    });
                    messages.push(Msg::new(
                    "user",
                    format!(
                        "[protocol error] {why}. Reply with exactly one JSON object: {{\"tool\":\"<name>\",\"args\":{{...}}}} or {{\"final\":\"<message>\"}}."
                    ),
                    ));
                }
                Ok(Turn::Final(m)) => {
                    self.observer.event(&Event::Final);
                    break (Ending::Final, Some(m));
                }
                Ok(Turn::Tool { name, args }) => {
                    n_calls += 1;
                    let (text, ok) = self.call_tool(
                        &name,
                        args,
                        n_calls,
                        level,
                        &plan,
                        &mut session,
                        ctl.checkpoint().cost_micro,
                    );
                    match ok {
                        Some(effect_line) => done.extend(effect_line),
                        None => denied += 1,
                    }
                    messages.push(Msg::new("user", text));
                }
            }
        };

        // 4. Closing: report and contract state.
        let stop = match &ending {
            Ending::Stopped(s) => Some(*s),
            _ => None,
        };
        let mut pending = Vec::new();
        // The only thing that can close the contract as done: the jail's `run_tests` passed AFTER
        // the last change. It is a signal from outside the model (verify::ExternalSignal).
        let mut checklist = Checklist {
            criteria: vec![Criterion::new(
                "goal",
                "the project's tests pass after the last change",
                Scope::Goal,
            )],
        };
        if let (Some(last), false) = (&self.tests.last, self.tests.wrote_since) {
            checklist.apply(
                "goal",
                Signal::External(ExternalSignal::TestResult {
                    name: "run_tests".into(),
                    passed: last.passed,
                }),
            );
        }
        let verified = matches!(ending, Ending::Final) && checklist.is_done();
        let next_step = match &ending {
            Ending::Final if verified => {
                done.push("run_tests passed after the last change".into());
                pending
                    .push("the tests passed; they may not cover everything the task asks".into());
                "read the diff and decide whether to apply it".to_owned()
            }
            Ending::Final => {
                pending.push(match &self.tests.last {
                    None => "nothing outside the model has verified the result: no tests were run"
                        .to_owned(),
                    Some(t) if self.tests.wrote_since => format!(
                        "files changed after the last test run ({})",
                        if t.passed {
                            "it had passed"
                        } else {
                            "it had failed"
                        }
                    ),
                    Some(t) => format!("the last test run FAILED (exit code {:?})", t.exit_code),
                });
                "run the checks on the result".to_owned()
            }
            Ending::Stopped(s) => {
                pending.push(format!("the task was not finished: stopped by {s:?}"));
                "look at what was done and raise the budget or split the task".into()
            }
            Ending::ModelError(m) => {
                pending.push(format!("the model could not be reached: {m}"));
                "try again when the model answers".into()
            }
            Ending::NeedsApproval => {
                pending.push("the contract was not approved".into());
                "ask the person to approve the contract".into()
            }
            Ending::Cancelled => {
                pending.push("the person cancelled the task".into());
                "look at what was done, or start again".into()
            }
        };
        let contract_state = contract.map(|mut c| {
            let next = next_step.clone();
            let ck = clock.as_ref();
            if !c.state().is_closed() {
                let _ = if stop.is_some() || !matches!(ending, Ending::Final) {
                    c.close_failed(&next, ck)
                } else if verified {
                    let tail = self.tests.last.as_ref().map(|t| t.tail.clone());
                    let evidence = contract::Evidence {
                        observed: format!(
                            "run_tests passed after the last change. End of its output: {}",
                            tail.unwrap_or_default()
                        ),
                    };
                    c.check_criterion(0, Some(evidence))
                        .and_then(|()| c.close_ok(&next, ck))
                        .or_else(|_| c.block(ck))
                } else {
                    c.block(ck) // done by the model's word only: waits for a check from outside
                };
            }
            let _ = c.save(&record);
            c.state()
        });
        let cp = ctl.checkpoint();
        Ok(AgentOutcome {
            ending,
            final_message,
            level,
            steps: cp.steps,
            input_tokens: tin,
            output_tokens: tout,
            cost_micro: cp.cost_micro,
            elapsed_ms: cp.elapsed_ms,
            denied,
            protocol_errors,
            tainted: session.tainted(),
            tests_run: self.tests.runs,
            verified_by_tests: verified,
            contract_state,
            report: ClosingReport {
                done,
                pending,
                next_step,
            },
            transcript: messages,
        })
    }

    /// The message for the model after `run_tests`, and the signal for the contract. What the
    /// tests print is external content: scrubbed, wrapped as data, and it taints the session. The
    /// exit code is ours (the jail's), so it goes outside the markers.
    fn tests_message(&mut self, result: &Value, session: &mut clio_gateway::Session) -> String {
        let passed = result["passed"].as_bool().unwrap_or(false);
        let exit_code = result["exit_code"].as_i64();
        let output = result["output"].as_str().unwrap_or_default();
        let tail: String = {
            let n = output.chars().count();
            output.chars().skip(n.saturating_sub(400)).collect()
        };
        self.tests.runs += 1;
        self.tests.wrote_since = false;
        self.tests.last = Some(TestRun {
            passed,
            exit_code,
            tail: self
                .scrubber
                .as_ref()
                .map_or(tail.clone(), |s| s.scrub(&tail)),
        });
        session.mark_external(Source::ToolResult);
        self.gateway.mark_tainted(session.id());
        let how = if result["timed_out"].as_bool() == Some(true) {
            "killed: time limit".to_owned()
        } else {
            format!(
                "exit code {}",
                exit_code.map_or("none".into(), |c| c.to_string())
            )
        };
        format!(
            "Result of run_tests: {} ({how}).\n{}",
            if passed { "PASSED" } else { "FAILED" },
            self.present(Source::ToolResult, output, true)
        )
    }

    fn outcome_without_loop(&self, level: Level, ending: Ending) -> AgentOutcome {
        AgentOutcome {
            ending,
            final_message: None,
            level,
            steps: 0,
            input_tokens: 0,
            output_tokens: 0,
            cost_micro: 0,
            elapsed_ms: 0,
            denied: 0,
            protocol_errors: 0,
            tainted: false,
            tests_run: 0,
            verified_by_tests: false,
            contract_state: None,
            report: ClosingReport {
                done: vec![],
                pending: vec!["the contract was not approved".into()],
                next_step: "ask the person to approve the contract".into(),
            },
            transcript: vec![],
        }
    }

    /// One tool call, reported to the observer. See `decide_and_run`.
    #[allow(clippy::too_many_arguments)]
    fn call_tool(
        &mut self,
        name: &str,
        args: Value,
        n: u32,
        level: Level,
        plan: &[String],
        session: &mut clio_gateway::Session,
        spent: u64,
    ) -> (String, Option<Vec<String>>) {
        let text = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default();
        let target = if name == "run_tests" {
            format!("{} {}", text("runner"), text("target"))
                .trim()
                .to_owned()
        } else {
            text("path").to_owned()
        };
        let (text, ran, outcome, rule) =
            self.decide_and_run(name, args, n, level, plan, session, spent);
        self.observer.event(&Event::Tool {
            name: name.to_owned(),
            target,
            outcome,
            rule,
            ran: ran.is_some(),
            result_chars: text.chars().count(),
            tainted: session.tainted(),
        });
        (text, ran)
    }

    /// One tool call: gateway first, executor only on an allow. Returns the message for the model,
    /// `Some(done lines)` when the call ran (`None` when it did not), and the gateway's outcome
    /// and rule (a malformed call never reached the gateway: denied by `protocol`).
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    fn decide_and_run(
        &mut self,
        name: &str,
        args: Value,
        n: u32,
        level: Level,
        plan: &[String],
        session: &mut clio_gateway::Session,
        spent: u64,
    ) -> (String, Option<Vec<String>>, Outcome, String) {
        let Value::Object(map) = args else {
            return (
                "[protocol error] \"args\" must be a JSON object".into(),
                None,
                Outcome::Deny,
                "protocol".into(),
            );
        };
        let req = ActionRequest {
            tool: name.into(),
            version: "1".into(),
            description_hash: self
                .descriptions
                .iter()
                .find(|(t, _)| t == name)
                .map(|(_, h)| h.clone())
                .unwrap_or_default(),
            args: map.clone(),
            node: "local".into(),
            session: SessionInfo {
                id: session.id().into(),
                parent: None,
                level,
                tainted: session.tainted(),
                subagent: false,
            },
            actor: "agent".into(),
            budget: GwBudget {
                spent,
                limit: self.cfg.budget.max_cost_micro,
            },
            plan: plan.to_vec(),
        };
        let d = self.gateway.decide_or_deny(&req);
        let allowed = match d.outcome {
            Outcome::Allow => true,
            Outcome::Ask => self.asker.approve(&d, name, &map),
            Outcome::Deny => false,
        };
        if !allowed {
            let why = if d.outcome == Outcome::Ask {
                format!(
                    "it needs a person's approval and nobody is available ({})",
                    d.reason
                )
            } else {
                d.reason.clone()
            };
            return (
                format!("[gateway] {name} was not run (rule {}): {why}", d.rule),
                None,
                d.outcome,
                d.rule,
            );
        }
        let action = NodeAction {
            id: format!("{}-{n}", self.cfg.task_id),
            tool: name.into(),
            args: Value::Object(map.clone()),
        };
        let approval = Approved(action.hash());
        match self.executor.execute(&action, &approval) {
            Ok(out) => {
                let effect = self
                    .executor
                    .registry
                    .get(name)
                    .and_then(|f| f.tool.effect)
                    .unwrap_or(Effect::Irreversible);
                if name == "run_tests" {
                    return (
                        self.tests_message(&out.result, session),
                        Some(vec![]),
                        d.outcome,
                        d.rule,
                    );
                }
                if effect > Effect::Read {
                    self.tests.wrote_since = true;
                }
                let (source, raw) = match (name, out.result.get("content").and_then(Value::as_str))
                {
                    ("read_file", Some(c)) => (Source::File, c.to_owned()),
                    ("list_dir", _) => (Source::File, out.result.to_string()),
                    _ => (Source::ToolResult, out.result.to_string()),
                };
                if source == Source::File {
                    // S-05: anything that came from a file marks the session, for the gateway too.
                    session.mark_external(Source::File);
                    self.gateway.mark_tainted(session.id());
                }
                let lines = if effect > Effect::Read {
                    vec![format!(
                        "{name} {}",
                        map.get("path").and_then(Value::as_str).unwrap_or("")
                    )]
                } else {
                    vec![]
                };
                (
                    format!("Result of {name}:\n{}", self.present(source, &raw, false)),
                    Some(lines),
                    d.outcome,
                    d.rule,
                )
            }
            Err(e) => (
                format!("[error] {name} failed: {e}"),
                Some(vec![]),
                d.outcome,
                d.rule,
            ),
        }
    }
}
