//! The bench: a set of tasks whose success is checked by something other than the model, run
//! several times each, with success, cost and time stored in clio-trace (D15).
//! Replay mode answers from a tape and never touches the network, so a run is reproducible; a
//! real run asks a command (see `CommandLlm`).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use clio_fake_llm::{Error as LlmError, Llm, Message, Request, Response};
use clio_trace::{Attr, Kind, Status, Trace, Usage, names};
use serde::{Deserialize, Serialize};

/// What decides that a task succeeded. Text checks only for now.
/// shortcut: no check that runs a command or a test suite; add one when the first real
/// task needs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Check {
    Equals(String),
    Contains(String),
    NotContains(String),
}

impl Check {
    pub fn passes(&self, output: &str) -> bool {
        match self {
            Self::Equals(v) => output.trim() == v,
            Self::Contains(v) => output.contains(v.as_str()),
            Self::NotContains(v) => !output.contains(v.as_str()),
        }
    }
}

/// One task of the bench: input, the check, and the limits it must stay inside.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub input: String,
    pub check: Check,
    pub max_steps: u32,
    pub max_cost_micro_usd: i64,
    pub max_duration_ms: i64,
}

/// Every `*.json` file under `dir` (and its subdirectories), ordered by id.
pub fn load_tasks(dir: &Path) -> Result<Vec<Task>, String> {
    let mut files = Vec::new();
    collect(dir, &mut files)?;
    let mut tasks = Vec::new();
    let mut seen = BTreeSet::new();
    for file in files {
        let text =
            std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let task: Task =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
        if !seen.insert(task.id.clone()) {
            return Err(format!("{}: duplicate task id {}", file.display(), task.id));
        }
        tasks.push(task);
    }
    tasks.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(tasks)
}

fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            collect(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
    Ok(())
}

pub struct Outcome {
    pub output: String,
    pub steps: u32,
    pub provider: String,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// What the bench measures. The real agent will implement this; until it exists, `SingleCall`
/// is the baseline: one model call with the task as the message.
pub trait Agent {
    fn prompt_version(&self) -> &str;
    fn run(&self, task: &Task) -> Result<Outcome, String>;
}

pub struct SingleCall {
    pub llm: Box<dyn Llm>,
    pub provider: String,
    pub model: String,
}

/// The request `SingleCall` sends; the tape is keyed by its hash, so this is the one place
/// that builds it.
pub fn request_for(model: &str, input: &str) -> Request {
    Request {
        model: model.into(),
        messages: vec![Message {
            role: "user".into(),
            content: input.into(),
        }],
        tools: vec![],
        max_tokens: Some(512),
    }
}

impl Agent for SingleCall {
    fn prompt_version(&self) -> &str {
        "single-call@1"
    }

    fn run(&self, task: &Task) -> Result<Outcome, String> {
        let r = self
            .llm
            .complete(&request_for(&self.model, &task.input))
            .map_err(|e| e.to_string())?;
        Ok(Outcome {
            output: r.content,
            steps: 1,
            provider: self.provider.clone(),
            model: self.model.clone(),
            input_tokens: r.input_tokens,
            output_tokens: r.output_tokens,
        })
    }
}

/// A real model reached through any program that reads a `Request` as JSON on stdin and writes
/// a `Response` as JSON on stdout. Keeps keys and HTTP out of this crate.
/// shortcut: one process per call; fine for a nightly bench of a few dozen tasks.
pub struct CommandLlm {
    pub program: String,
    pub args: Vec<String>,
}

impl Llm for CommandLlm {
    fn complete(&self, request: &Request) -> Result<Response, LlmError> {
        let fail = |m: String| LlmError::Tape(format!("{}: {m}", self.program));
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| fail(e.to_string()))?;
        let body = serde_json::to_vec(request).map_err(|e| fail(e.to_string()))?;
        child
            .stdin
            .take()
            .ok_or_else(|| fail("no stdin".into()))?
            .write_all(&body)
            .map_err(|e| fail(e.to_string()))?;
        let out = child.wait_with_output().map_err(|e| fail(e.to_string()))?;
        if !out.status.success() {
            return Err(fail(format!("exit {}", out.status)));
        }
        serde_json::from_slice(&out.stdout).map_err(|e| fail(format!("bad answer: {e}")))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskReport {
    pub task: String,
    pub runs: u32,
    pub successes: u32,
    pub cost_micro_usd: i64,
    pub duration_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub runs: u32,
    pub successes: u32,
    pub cost_micro_usd: i64,
    pub duration_ms: i64,
    pub tasks: Vec<TaskReport>,
}

impl Report {
    pub fn success_rate(&self) -> f64 {
        if self.runs == 0 {
            0.0
        } else {
            f64::from(self.successes) / f64::from(self.runs)
        }
    }

    pub fn cost_per_run(&self) -> f64 {
        if self.runs == 0 {
            0.0
        } else {
            self.cost_micro_usd as f64 / f64::from(self.runs)
        }
    }

    pub fn line(&self) -> String {
        format!(
            "bench: {}/{} ok ({:.1}%) · cost {} micro-USD · time {} ms",
            self.successes,
            self.runs,
            self.success_rate() * 100.0,
            self.cost_micro_usd,
            self.duration_ms
        )
    }
}

/// Runs every task `repeats` times. Each run is its own trace in `trace`: a root span with the
/// task id, the run number and whether it succeeded, and a child span for the model call with
/// its tokens, cost and prompt version. A run succeeds only if the check passes and the steps,
/// cost and time stay inside the task's limits. The time is what the trace's clock says, so a
/// replay with a fixed clock reports 0.
pub fn run(
    tasks: &[Task],
    agent: &dyn Agent,
    repeats: u32,
    trace: &Trace,
) -> Result<Report, String> {
    let t = |e: clio_trace::Error| e.to_string();
    let mut report = Report {
        runs: 0,
        successes: 0,
        cost_micro_usd: 0,
        duration_ms: 0,
        tasks: vec![],
    };
    for task in tasks {
        let mut tr = TaskReport {
            task: task.id.clone(),
            runs: 0,
            successes: 0,
            cost_micro_usd: 0,
            duration_ms: 0,
        };
        for n in 0..repeats {
            let id = trace.new_trace_id();
            let root = trace
                .start_span(&id, None, Kind::Agent, &format!("bench:{}", task.id))
                .map_err(t)?;
            let call = trace.child(&root, Kind::Model, "call").map_err(t)?;
            let attempt = agent.run(task);
            let version = BTreeMap::from([(
                names::ATTR_PROMPT_VERSION.to_string(),
                Attr::Text(agent.prompt_version().into()),
            )]);
            let checked = match &attempt {
                Ok(o) => {
                    let usage = Usage {
                        provider: &o.provider,
                        model: &o.model,
                        input_tokens: o.input_tokens,
                        output_tokens: o.output_tokens,
                    };
                    trace
                        .end_span(&call, Status::Ok, Some(usage), &version)
                        .map_err(t)?;
                    Some((task.check.passes(&o.output), o.steps))
                }
                Err(_) => {
                    trace
                        .end_span(&call, Status::Error, None, &version)
                        .map_err(t)?;
                    None
                }
            };
            let summary = trace.summary(&id).map_err(t)?;
            let success = checked.is_some_and(|(passed, steps)| {
                passed
                    && steps <= task.max_steps
                    && summary.cost_micro_usd <= task.max_cost_micro_usd
                    && summary.duration_ms <= task.max_duration_ms
            });
            let attrs = BTreeMap::from([
                ("task".to_string(), Attr::Text(task.id.clone())),
                ("run".to_string(), Attr::Int(i64::from(n))),
                ("success".to_string(), Attr::Bool(success)),
            ]);
            trace
                .end_span(
                    &root,
                    if success { Status::Ok } else { Status::Error },
                    None,
                    &attrs,
                )
                .map_err(t)?;
            tr.runs += 1;
            tr.successes += u32::from(success);
            tr.cost_micro_usd += summary.cost_micro_usd;
            tr.duration_ms += summary.duration_ms;
        }
        report.runs += tr.runs;
        report.successes += tr.successes;
        report.cost_micro_usd += tr.cost_micro_usd;
        report.duration_ms += tr.duration_ms;
        report.tasks.push(tr);
    }
    Ok(report)
}

/// How far a run may fall from the saved baseline before the gate blocks it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Thresholds {
    pub max_success_drop_points: f64,
    pub max_cost_rise_pct: f64,
}

/// What got worse than the baseline allows; empty means the gate passes.
pub fn regressions(baseline: &Report, now: &Report, limits: &Thresholds) -> Vec<String> {
    let mut out = Vec::new();
    let drop = (baseline.success_rate() - now.success_rate()) * 100.0;
    if drop > limits.max_success_drop_points {
        out.push(format!(
            "success fell {drop:.1} points (limit {})",
            limits.max_success_drop_points
        ));
    }
    let allowed = baseline.cost_per_run() * (1.0 + limits.max_cost_rise_pct / 100.0);
    if now.cost_per_run() > allowed {
        out.push(format!(
            "cost per run rose to {:.0} micro-USD (limit {allowed:.0})",
            now.cost_per_run()
        ));
    }
    out
}
