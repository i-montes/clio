//! The bench with the real agent: each run copies the task's `workspace/` to a private folder,
//! lets the `AgentLoop` work there, then runs the task's hidden check on what it left behind.
//! The agent never sees `check.py`, `solution/` or `bad_solutions/`: it only gets the copy.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clio_brain::agent::{AgentConfig, AgentLoop, CommandModel};
use clio_core::SystemClock;
use clio_node::jail::{Jail, Job};
use clio_trace::{Attr, Kind, Status, Trace, Usage, names};
use serde::{Deserialize, Serialize};

use crate::pi_bench::PiBench;
use crate::{Check, Task};

pub struct AgentBenchConfig {
    pub provider: String,
    pub model: String,
    pub system_prompt: String,
    pub prompt_version: String,
    pub program: String,
    pub args: Vec<String>,
    pub repeats: u32,
    /// Empty means every task; otherwise a task needs at least one of these tags.
    pub tags: Vec<String>,
    /// Empty means every task; otherwise only these ids.
    pub only: Vec<String>,
    /// Where run folders go; each run gets `<task>-<n>` inside it.
    pub run_root: PathBuf,
    /// Delete a run's folder when it ends (the transcript goes with it).
    pub cleanup: bool,
    pub price_in_micro_per_mtok: u64,
    pub price_out_micro_per_mtok: u64,
    /// Pauses between retries of a failed model call.
    pub backoff_ms: Vec<u64>,
    /// `Some`: Pi is the harness (and `system_prompt`, `program` and `backoff_ms` are not used).
    pub pi: Option<PiBench>,
    /// Run the task checks without the jail when there is none (see `run_check`). Off by default:
    /// the check runs code the model wrote.
    pub unjailed_checks: bool,
    /// `Some(models)`: every run gets an empty memory of its own (in its run folder) that all its
    /// sessions read and write through the toolhost; the vector models best first, none for text
    /// only. `None`: no memory tools, the baseline. Pi only.
    pub memory: Option<Vec<PathBuf>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunDetail {
    pub run: u32,
    pub success: bool,
    pub steps: u32,
    pub duration_ms: i64,
    pub cost_micro_usd: i64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// How the agent ended: `Final`, `Stopped(Steps)`, `ModelError(..)`...
    pub ending: String,
    pub level: String,
    pub denied: u32,
    pub protocol_errors: u32,
    pub tainted: bool,
    /// Tool calls the agent made (Pi reports it; the builtin loop leaves 0).
    #[serde(default)]
    pub tool_calls: u32,
    /// Seconds of setup (toolhost and Pi start) included in `duration_ms`.
    #[serde(default)]
    pub setup_ms: i64,
    pub check_passed: bool,
    /// The check's last words, trimmed; for diagnosis, never for tuning prompts.
    pub check_tail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTaskReport {
    pub task: String,
    pub runs: u32,
    pub successes: u32,
    pub steps_total: u64,
    pub cost_micro_usd: i64,
    pub duration_ms: i64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub details: Vec<RunDetail>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentReport {
    #[serde(default = "builtin")]
    pub harness: String,
    pub prompt_version: String,
    pub provider: String,
    pub model: String,
    pub runs: u32,
    pub successes: u32,
    pub cost_micro_usd: i64,
    pub duration_ms: i64,
    pub tasks: Vec<AgentTaskReport>,
}

fn builtin() -> String {
    "builtin".into()
}

/// What a run reports, whichever harness ran it.
struct Done {
    steps: u32,
    input_tokens: u64,
    output_tokens: u64,
    ending: String,
    level: String,
    denied: u32,
    protocol_errors: u32,
    tainted: bool,
    tool_calls: u32,
    setup_ms: i64,
}

impl AgentReport {
    pub fn line(&self) -> String {
        let pct = if self.runs == 0 {
            0.0
        } else {
            f64::from(self.successes) * 100.0 / f64::from(self.runs)
        };
        format!(
            "bench agent {} ({}): {}/{} ok ({pct:.1}%) · cost {} micro-USD · time {} ms",
            self.prompt_version,
            self.harness,
            self.successes,
            self.runs,
            self.cost_micro_usd,
            self.duration_ms
        )
    }

    /// One row per task: success over runs, mean steps, tokens, time, tool calls and denials.
    pub fn table(&self) -> String {
        let mut out = String::from(
            "task        ok     steps(mean)  tokens(in/out)  ms(mean)  tools  denied\n",
        );
        for t in &self.tasks {
            let n = f64::from(t.runs.max(1));
            let sum = |f: fn(&RunDetail) -> u32| t.details.iter().map(f).sum::<u32>();
            out.push_str(&format!(
                "{:<11} {}/{}    {:<12.1} {}/{}  {:.0}  {}  {}\n",
                t.task,
                t.successes,
                t.runs,
                t.steps_total as f64 / n,
                t.input_tokens,
                t.output_tokens,
                t.duration_ms as f64 / n,
                sum(|d| d.tool_calls),
                sum(|d| d.denied)
            ));
        }
        out
    }

    /// One line per run: how it ended, the numbers and what the hidden check said.
    pub fn detail(&self) -> String {
        let mut out = String::new();
        for t in &self.tasks {
            for d in &t.details {
                out.push_str(&format!(
                    "{} #{}: {} steps={} tools={} denied={} ms={} setup={} tok={}/{} ending={} check={}\n",
                    t.task,
                    d.run,
                    if d.success { "ok" } else { "FAIL" },
                    d.steps,
                    d.tool_calls,
                    d.denied,
                    d.duration_ms,
                    d.setup_ms,
                    d.input_tokens,
                    d.output_tokens,
                    d.ending,
                    if d.check_passed { "passed" } else { "failed" }
                ));
            }
        }
        out
    }
}

/// Copy a folder tree. Symbolic links are skipped: the agent gets plain files only.
fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    let e = |p: &Path, x: std::io::Error| format!("{}: {x}", p.display());
    std::fs::create_dir_all(dst).map_err(|x| e(dst, x))?;
    for entry in std::fs::read_dir(src).map_err(|x| e(src, x))? {
        let entry = entry.map_err(|x| e(src, x))?;
        let kind = entry.file_type().map_err(|x| e(src, x))?;
        let to = dst.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &to)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &to).map_err(|x| e(&to, x))?;
        }
    }
    Ok(())
}

/// Runs the task's check with no shell. Returns (passed, last lines of its output).
///
/// The check imports and runs what the model wrote in the workspace, after the model read the
/// injection bait of the hostile tasks: that code must not run as the person, with their keys and
/// the network. It runs in the same `bwrap` jail as the agent's own commands (no network, none of
/// the person's files or environment, `/work` its working folder). Without a jail it does not run,
/// unless `unjailed` (`CLIO_BENCH_CHECK_JAIL=off`, for a machine with nothing of the person's), and
/// then with an empty environment.
fn run_check(
    task: &Task,
    work: &Path,
    sink: &Path,
    out: &Path,
    unjailed: bool,
) -> Result<(bool, String), String> {
    let Check::Command(line) = &task.check else {
        return Err(format!(
            "task {}: only command checks run on a workspace",
            task.id
        ));
    };
    let dir = task.dir.canonicalize().map_err(|e| e.to_string())?;
    match Jail::detect().and_then(|j| j.probe().map(|()| j)) {
        Ok(jail) => run_check_jailed(&jail, line, &dir, work, sink, out),
        Err(why) if unjailed => {
            eprintln!(
                "warning: the check of {} runs WITHOUT a jail ({why})",
                task.id
            );
            run_check_bare(line, &dir, work, sink, out)
        }
        Err(why) => Err(format!(
            "the check of {} runs code the model wrote and needs the jail ({why}); \
             CLIO_BENCH_CHECK_JAIL=off runs it without one, on a machine with nothing of yours",
            task.id
        )),
    }
}

fn run_check_jailed(
    jail: &Jail,
    line: &str,
    task_dir: &Path,
    work: &Path,
    sink: &Path,
    out: &Path,
) -> Result<(bool, String), String> {
    let line = line.replace("{task_dir}", "/task");
    let mut parts = line.split_whitespace();
    let program = parts.next().ok_or("empty check command")?;
    // `argv[0]` is a path inside the jail, whose programs are the system's.
    let program = ["/usr/bin", "/bin"]
        .iter()
        .map(|d| Path::new(d).join(program))
        .find(|p| p.is_file())
        .ok_or_else(|| format!("{program}: not found in /usr/bin"))?;
    let job = Job {
        work: work.to_path_buf(),
        argv: std::iter::once(program.to_string_lossy().into_owned())
            .chain(parts.map(str::to_owned))
            .collect(),
        env: vec![
            ("TASK_DIR".into(), "/task".into()),
            ("SINK_LOG".into(), "/sink.log".into()),
        ],
        mounts: vec![
            (task_dir.to_path_buf(), "/task".into()),
            (sink.to_path_buf(), "/sink.log".into()),
        ],
        ..Job::default()
    };
    let o = jail.run(&job)?;
    let text = format!("{}{}", o.stdout, o.stderr);
    std::fs::write(out, &text).map_err(|e| e.to_string())?;
    Ok(if o.timed_out {
        (false, "the check timed out".into())
    } else {
        (o.success(), tail_of(&text))
    })
}

/// The last lines of the output, on one line.
fn tail_of(text: &str) -> String {
    let tail: Vec<&str> = text.lines().rev().take(6).collect();
    let tail = tail.into_iter().rev().collect::<Vec<_>>().join(" | ");
    tail.chars().take(400).collect()
}

/// The check without a jail: only an empty environment (no keys, no `HOME`).
fn run_check_bare(
    line: &str,
    dir: &Path,
    work: &Path,
    sink: &Path,
    out: &Path,
) -> Result<(bool, String), String> {
    let line = line.replace("{task_dir}", &dir.to_string_lossy());
    let mut parts = line.split_whitespace();
    let program = parts.next().ok_or("empty check command")?;
    let file = std::fs::File::create(out).map_err(|e| e.to_string())?;
    let mut child = Command::new(program)
        .args(parts)
        .current_dir(work)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("TASK_DIR", dir)
        .env("SINK_LOG", sink)
        .stdout(Stdio::from(file.try_clone().map_err(|e| e.to_string())?))
        .stderr(Stdio::from(file))
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break Some(s);
        }
        // shortcut: a fixed two-minute limit for any check; make it per task if one needs more.
        if start.elapsed() > Duration::from_secs(120) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let tail = tail_of(&std::fs::read_to_string(out).unwrap_or_default());
    Ok(match status {
        Some(s) => (s.success(), tail),
        None => (false, "the check timed out".into()),
    })
}

fn one_run(task: &Task, n: u32, cfg: &AgentBenchConfig) -> Result<(Done, bool, String), String> {
    let base = cfg.run_root.join(format!("{}-{n}", task.id));
    let (work, run_dir, scratch) = (base.join("work"), base.join("run"), base.join("scratch"));
    let ws = task.workspace.as_ref().ok_or("task has no workspace")?;
    copy_tree(&task.dir.join(ws), &work)?;
    let sink = base.join("sink.log");
    std::fs::write(&sink, "").map_err(|e| e.to_string())?;
    if cfg.pi.is_none() && cfg.memory.is_some() {
        return Err("the memory needs --harness pi".into());
    }
    let done = if let Some(pi) = &cfg.pi {
        let id = format!("{}-{n}-{}", task.id, std::process::id());
        let memory_dir = base.join("memory");
        let memory = cfg
            .memory
            .as_deref()
            .map(|models| (memory_dir.as_path(), models));
        // Earlier sessions first, then the task's own prompt; one session keeps the old layout.
        let prompts: Vec<&str> = task
            .sessions
            .iter()
            .map(String::as_str)
            .chain([task.input.as_str()])
            .collect();
        let many = prompts.len() > 1;
        let mut total = Done {
            steps: 0,
            input_tokens: 0,
            output_tokens: 0,
            ending: String::new(),
            level: String::new(),
            denied: 0,
            protocol_errors: 0,
            tainted: false,
            tool_calls: 0,
            setup_ms: 0,
        };
        let (mut endings, mut transcripts) = (Vec::new(), Vec::new());
        for (i, prompt) in prompts.iter().enumerate() {
            let (sid, dir) = if many {
                (format!("{id}-s{i}"), run_dir.join(format!("s{i}")))
            } else {
                (id.clone(), run_dir.clone())
            };
            let d = pi.run(
                &sid,
                &work,
                &dir,
                prompt,
                task.max_steps,
                task.max_duration_ms,
                memory,
            )?;
            total.steps += d.steps;
            total.input_tokens += d.input_tokens;
            total.output_tokens += d.output_tokens;
            total.denied += d.denied;
            total.tainted |= d.tainted;
            total.tool_calls += d.tool_calls;
            total.setup_ms += d.setup_ms;
            endings.push(d.ending);
            transcripts.push(d.transcript);
        }
        total.ending = endings.join(" -> ");
        let transcript = if many {
            serde_json::Value::Array(transcripts)
        } else {
            transcripts.pop().unwrap_or_default()
        };
        std::fs::write(
            base.join("transcript.json"),
            serde_json::to_string_pretty(&transcript).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        total
    } else {
        let mut acfg = AgentConfig::new(&format!("{}-{n}", task.id), &cfg.system_prompt, &run_dir);
        acfg.prompt_version = cfg.prompt_version.clone();
        acfg.budget.max_steps = task.max_steps;
        acfg.budget.max_ms = task.max_duration_ms;
        acfg.budget.max_cost_micro = u64::try_from(task.max_cost_micro_usd).unwrap_or(0);
        acfg.price_in_micro_per_mtok = cfg.price_in_micro_per_mtok;
        acfg.price_out_micro_per_mtok = cfg.price_out_micro_per_mtok;
        acfg.person_approves_long = true; // a bench has no one to ask; the task is the person's request
        let mut agent = AgentLoop::new(Arc::new(SystemClock), acfg, &work, &scratch)?;
        agent.scrubber = Some(clio_vault::Scrubber::default());
        let mut model = CommandModel::new(&cfg.program, &cfg.args, &cfg.model);
        model.backoff_ms = cfg.backoff_ms.clone();
        let o = agent.run(&mut model, &task.input)?;
        // Keep the conversation for the post-mortem (it lives with the run folder, not the workspace).
        std::fs::write(
            base.join("transcript.json"),
            serde_json::to_string_pretty(&o.transcript).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        Done {
            steps: o.steps,
            input_tokens: o.input_tokens,
            output_tokens: o.output_tokens,
            ending: format!("{:?}", o.ending),
            level: format!("{:?}", o.level),
            denied: o.denied,
            protocol_errors: o.protocol_errors,
            tainted: o.tainted,
            tool_calls: 0,
            setup_ms: 0,
        }
    };
    let (passed, tail) = run_check(
        task,
        &work,
        &sink,
        &base.join("check.out"),
        cfg.unjailed_checks,
    )?;
    Ok((done, passed, tail))
}

/// Runs every task with a workspace `cfg.repeats` times. Like `run`, each run is a trace: a root
/// span with task, run, success and steps, and a child span with the tokens, cost and prompt
/// version. A run succeeds only if the check passes and steps, cost and time stay inside the
/// task's limits.
pub fn run_agent(
    tasks: &[Task],
    cfg: &AgentBenchConfig,
    trace: &Trace,
) -> Result<AgentReport, String> {
    let t = |e: clio_trace::Error| e.to_string();
    let mut report = AgentReport {
        harness: match (&cfg.pi, &cfg.memory) {
            (Some(_), Some(_)) => "pi+memory",
            (Some(_), None) => "pi",
            (None, _) => "builtin",
        }
        .into(),
        prompt_version: cfg.prompt_version.clone(),
        provider: cfg.provider.clone(),
        model: cfg.model.clone(),
        runs: 0,
        successes: 0,
        cost_micro_usd: 0,
        duration_ms: 0,
        tasks: vec![],
    };
    // The builtin loop has no sessions: a task with earlier sessions is Pi's only.
    let wanted = |task: &Task| {
        task.workspace.is_some()
            && (cfg.pi.is_some() || task.sessions.is_empty())
            && (cfg.tags.is_empty() || task.tags.iter().any(|g| cfg.tags.contains(g)))
            && (cfg.only.is_empty() || cfg.only.contains(&task.id))
    };
    for task in tasks.iter().filter(|x| wanted(x)) {
        let mut tr = AgentTaskReport {
            task: task.id.clone(),
            runs: 0,
            successes: 0,
            steps_total: 0,
            cost_micro_usd: 0,
            duration_ms: 0,
            input_tokens: 0,
            output_tokens: 0,
            details: vec![],
        };
        for n in 0..cfg.repeats {
            let id = trace.new_trace_id();
            let root = trace
                .start_span(&id, None, Kind::Agent, &format!("bench:{}", task.id))
                .map_err(t)?;
            let call = trace.child(&root, Kind::Model, "call").map_err(t)?;
            let attempt = one_run(task, n, cfg);
            let version = BTreeMap::from([(
                names::ATTR_PROMPT_VERSION.to_string(),
                Attr::Text(cfg.prompt_version.clone()),
            )]);
            let (outcome, passed, tail) = match attempt {
                Ok(x) => {
                    let usage = Usage {
                        provider: &cfg.provider,
                        model: &cfg.model,
                        input_tokens: u32::try_from(x.0.input_tokens).unwrap_or(u32::MAX),
                        output_tokens: u32::try_from(x.0.output_tokens).unwrap_or(u32::MAX),
                    };
                    trace
                        .end_span(&call, Status::Ok, Some(usage), &version)
                        .map_err(t)?;
                    (Some(x.0), x.1, x.2)
                }
                Err(m) => {
                    trace
                        .end_span(&call, Status::Error, None, &version)
                        .map_err(t)?;
                    (None, false, format!("run failed to start: {m}"))
                }
            };
            let summary = trace.summary(&id).map_err(t)?;
            let steps = outcome.as_ref().map_or(0, |o| o.steps);
            let success = outcome.is_some()
                && passed
                && steps <= task.max_steps
                && summary.cost_micro_usd <= task.max_cost_micro_usd
                && summary.duration_ms <= task.max_duration_ms;
            let attrs = BTreeMap::from([
                ("task".to_string(), Attr::Text(task.id.clone())),
                ("run".to_string(), Attr::Int(i64::from(n))),
                ("success".to_string(), Attr::Bool(success)),
                ("steps".to_string(), Attr::Int(i64::from(steps))),
            ]);
            trace
                .end_span(
                    &root,
                    if success { Status::Ok } else { Status::Error },
                    None,
                    &attrs,
                )
                .map_err(t)?;
            let (tin, tout) = outcome
                .as_ref()
                .map_or((0, 0), |o| (o.input_tokens, o.output_tokens));
            tr.details.push(RunDetail {
                run: n,
                success,
                steps,
                duration_ms: summary.duration_ms,
                cost_micro_usd: summary.cost_micro_usd,
                input_tokens: tin,
                output_tokens: tout,
                ending: outcome
                    .as_ref()
                    .map_or_else(|| "NotStarted".into(), |o| o.ending.clone()),
                level: outcome
                    .as_ref()
                    .map_or_else(String::new, |o| o.level.clone()),
                denied: outcome.as_ref().map_or(0, |o| o.denied),
                protocol_errors: outcome.as_ref().map_or(0, |o| o.protocol_errors),
                tainted: outcome.as_ref().is_some_and(|o| o.tainted),
                tool_calls: outcome.as_ref().map_or(0, |o| o.tool_calls),
                setup_ms: outcome.as_ref().map_or(0, |o| o.setup_ms),
                check_passed: passed,
                check_tail: tail,
            });
            tr.runs += 1;
            tr.successes += u32::from(success);
            tr.steps_total += u64::from(steps);
            tr.cost_micro_usd += summary.cost_micro_usd;
            tr.duration_ms += summary.duration_ms;
            tr.input_tokens += tin;
            tr.output_tokens += tout;
            if cfg.cleanup {
                let _ = std::fs::remove_dir_all(cfg.run_root.join(format!("{}-{n}", task.id)));
            }
        }
        report.runs += tr.runs;
        report.successes += tr.successes;
        report.cost_micro_usd += tr.cost_micro_usd;
        report.duration_ms += tr.duration_ms;
        report.tasks.push(tr);
    }
    Ok(report)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // The check runs code the model wrote (an injected model may have planted anything in the
    // workspace) so it must run with what a hostile program should have: none of the person's
    // environment or files, and no network. This `check.py` PASSES only if all of that holds.
    const PROBE: &str = r#"import os, socket, sys
home, port = sys.argv[1], int(sys.argv[2])
bad = []
if os.environ.get("HOME") == home: bad.append("sees the person's HOME variable")
if os.environ.get("PATH_FROM_TEST_MARK"): bad.append("sees the test's environment")
if os.path.exists(home): bad.append("sees the person's home folder")
try:
    socket.create_connection(("127.0.0.1", port), timeout=2).close()
    bad.append("reached the network")
except OSError:
    pass
print("; ".join(bad) or "isolated")
sys.exit(1 if bad else 0)
"#;

    fn task(dir: &Path, line: String) -> Task {
        Task {
            id: "probe".into(),
            input: String::new(),
            check: Check::Command(line),
            max_steps: 1,
            max_cost_micro_usd: 1,
            max_duration_ms: 1,
            workspace: None,
            tags: vec![],
            sessions: vec![],
            dir: dir.to_path_buf(),
        }
    }

    /// `workspace/` with a folder laid over it; a `.delete` file in the folder lists paths to remove
    /// (the same rule as `verify_real_tasks.py`).
    fn attempt(task: &Task, over: Option<&Path>, root: &Path) -> (bool, String) {
        let work = root.join("work");
        copy_tree(&task.dir.join(task.workspace.as_ref().unwrap()), &work).unwrap();
        if let Some(o) = over {
            copy_tree(o, &work).unwrap();
            let _ = std::fs::remove_file(work.join(".delete"));
            for p in std::fs::read_to_string(o.join(".delete"))
                .unwrap_or_default()
                .lines()
            {
                let _ = std::fs::remove_file(work.join(p.trim()));
            }
        }
        let sink = root.join("sink.log");
        std::fs::write(&sink, "").unwrap();
        run_check(task, &work, &sink, &root.join("out"), false).unwrap()
    }

    // Moving the checks into the jail must not change what they decide: each of the real tasks'
    // checks accepts its reference solution and rejects each bad one, as `verify_real_tasks.py`
    // says without the jail.
    #[test]
    fn the_real_checks_decide_the_same_inside_the_jail() {
        if Jail::detect().and_then(|j| j.probe()).is_err() {
            assert!(std::env::var_os("CLIO_REQUIRE_JAIL").is_none());
            eprintln!("SKIPPED (no jail on this machine, nothing was tested)");
            return;
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tasks");
        let mut tasks = crate::load_tasks(&root.join("real")).unwrap();
        tasks.extend(crate::load_tasks(&root.join("memory")).unwrap());
        let mut problems = Vec::new();
        let mut n = 0;
        for t in tasks.iter().filter(|t| t.dir.join("solution").is_dir()) {
            let tmp = tempfile::tempdir().unwrap();
            let (ok, tail) = attempt(t, Some(&t.dir.join("solution")), tmp.path());
            n += 1;
            if !ok {
                problems.push(format!("{}: refuses its own solution: {tail}", t.id));
            }
            let bad_root = t.dir.join("bad_solutions");
            for bad in std::fs::read_dir(&bad_root).into_iter().flatten().flatten() {
                let tmp = tempfile::tempdir().unwrap();
                if attempt(t, Some(&bad.path()), tmp.path()).0 {
                    problems.push(format!("{}: accepts {:?}", t.id, bad.file_name()));
                }
            }
        }
        assert!(n >= 28, "only {n} tasks have a solution");
        assert!(problems.is_empty(), "{problems:#?}");
    }

    // S-03 S-18 (second audit): the bench's check does not run unjailed with the person's keys.
    #[test]
    fn the_check_runs_without_the_persons_environment_files_or_network() {
        if clio_node::jail::Jail::detect()
            .and_then(|j| j.probe())
            .is_err()
        {
            assert!(
                std::env::var_os("CLIO_REQUIRE_JAIL").is_none(),
                "CLIO_REQUIRE_JAIL is set and the jail cannot run here"
            );
            eprintln!("SKIPPED (no jail on this machine, nothing was tested)");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let (task_dir, work) = (tmp.path().join("task"), tmp.path().join("work"));
        std::fs::create_dir_all(&task_dir).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(task_dir.join("check.py"), PROBE).unwrap();
        let sink = tmp.path().join("sink.log");
        std::fs::write(&sink, "").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let home = std::env::var("HOME").unwrap();
        let t = task(
            &task_dir,
            format!("python3 {{task_dir}}/check.py {home} {port}"),
        );
        let (passed, tail) = run_check(&t, &work, &sink, &tmp.path().join("out"), false).unwrap();
        assert!(passed, "the check saw the outside: {tail}");
        assert!(tail.contains("isolated"), "{tail}");
    }
}
