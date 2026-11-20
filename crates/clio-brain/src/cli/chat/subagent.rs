//! Subagents (S-73): the chat's model hands tasks to child agents with `subagent`, and waits.
//!
//! The guard sends the call to the chat's tool host, which holds it; the brain sees the tool
//! start, takes the call's arguments from the tool host (what the tool host received, so a call
//! Pi refused runs nothing), checks them against `subagent.*`, and runs the children: each one a
//! second agent (`review::second_agent`) in `<run>/subagents/<n>`, with its own Pi, tool host,
//! prompt (`clio-subagent@1`) and caps, the session's marks and day budget. Readers run up to
//! `subagent.max_parallel` at once; a writer is alone in its call. The reports go back to the tool
//! host, which marks the session, judges and wraps them before the model gets them.
//!
//! Every child stops when the call ends (`Esc`, the turn's end, the tool's end), and every child's
//! Pi and tool host are gone before the turn returns (`Turn` lives in a `thread::scope`). The
//! chat's row shows the children as `ToolUpdate`s of the call, injected into the parent's events.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clio_brain::control::{Budget, DayBudget, Stop};
use clio_brain::pi::budget::RunGuard;
use clio_brain::pi::rpc::{Cancel, Event, RpcClient, Usage};
use clio_brain::pi::text;
use clio_core::SystemClock;
use clio_toolhost::subagent::Child;
use serde_json::{Value, json};

use super::review::{Kind, Second, new_folder, second_agent, session_cfg};
use crate::cli::home::{Config, Dirs, ModelCfg, SubagentCfg};
use crate::cli::{jev, usage};

/// A report goes back at most this long (bytes); the whole one is kept in the child's folder.
pub const MAX_REPORT: usize = 50_000;
/// How long the brain looks for a call the model made before giving up on it (Pi refused it).
const TAKE_WAIT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(100);
const HOUR_MS: i64 = 3_600_000;

/// One task of a call, checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub name: String,
    pub task: String,
    pub mode: Child,
    pub model: String,
}

fn mode_word(m: Child) -> &'static str {
    match m {
        Child::Read => "read",
        Child::Write => "write",
    }
}

/// The call's tasks, or why it is refused: at most `max_per_call`, a writer only alone, no more
/// than `max_per_hour` children in the last hour (`recent` of them already), and each model one
/// the session's connector offers (`offered`; empty: unknown, any is taken).
pub fn plan(
    args: &Value,
    cfg: &SubagentCfg,
    recent: usize,
    session_model: &str,
    offered: &[String],
) -> Result<Vec<Task>, String> {
    let list = args["tasks"].as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        return Err("a call needs at least one task".into());
    }
    if list.len() > cfg.max_per_call as usize {
        return Err(format!(
            "a call holds at most {} tasks (subagent.max_per_call): split the work",
            cfg.max_per_call
        ));
    }
    let mut tasks = Vec::new();
    for t in &list {
        let name = text::line(t["name"].as_str().unwrap_or(""), 40);
        let mode = t["mode"]
            .as_str()
            .and_then(Child::parse)
            .ok_or_else(|| format!("«{name}»: mode is read or write"))?;
        let task = t["task"].as_str().unwrap_or("").trim().to_owned();
        if name.is_empty() || task.is_empty() {
            return Err("each task needs a name and a task".into());
        }
        let want = t["model"]
            .as_str()
            .filter(|m| !m.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| cfg.model.clone())
            .unwrap_or_else(|| session_model.to_owned());
        if want != session_model && !offered.is_empty() && !offered.contains(&want) {
            return Err(format!(
                "«{name}»: {} is not a model of this connector; leave `model` out for the session's",
                text::line(&want, 80)
            ));
        }
        tasks.push(Task {
            name,
            task,
            mode,
            model: want,
        });
    }
    let names: HashSet<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
    if names.len() < tasks.len() {
        return Err("each task needs a name of its own".into());
    }
    if tasks.iter().any(|t| t.mode == Child::Write) && tasks.len() > 1 {
        return Err(
            "a writing subagent runs alone in its call: one call for it, another for the readers"
                .into(),
        );
    }
    if recent + tasks.len() > cfg.max_per_hour as usize {
        return Err(format!(
            "this conversation launched {recent} subagents in the last hour; at most {} (subagent.max_per_hour)",
            cfg.max_per_hour
        ));
    }
    Ok(tasks)
}

/// Children started in this run since `since_ms`, from their folders.
fn launched_since(run_dir: &Path, since_ms: i64) -> usize {
    std::fs::read_dir(run_dir.join("subagents"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path().join("subagent.json")).ok())
        .filter_map(|t| serde_json::from_str::<Value>(&t).ok())
        .filter(|v| v["started_ms"].as_i64().unwrap_or(i64::MAX) >= since_ms)
        .count()
}

/// Where the session that calls stands, for its children.
pub struct Ctx<'a> {
    pub dirs: &'a Dirs,
    pub run: &'a str,
    pub run_dir: &'a Path,
    pub work: &'a Path,
    pub host: bool,
    /// The chat's tool host, its token and the Pi session it serves.
    pub socket: &'a Path,
    pub token: &'a str,
    pub session: &'a str,
    pub taint_file: &'a Path,
    pub day: Arc<Mutex<DayBudget>>,
    pub model: ModelCfg,
    /// The chat's Pi: the children's progress goes into its events.
    pub client: &'a RpcClient,
    /// The person's `Esc`.
    pub cancel: &'a Cancel,
}

/// A turn's subagent calls.
pub struct Turn<'a> {
    ctx: Ctx<'a>,
    ended: Mutex<HashSet<String>>,
    over: AtomicBool,
}

/// How a child's thread changes its row (and shows it).
type SetRow<'a> = &'a dyn Fn(&dyn Fn(&mut Row));

/// One child, as the chat's row shows it and its folder keeps it.
#[derive(Debug, Clone, Default)]
struct Row {
    name: String,
    mode: &'static str,
    model: String,
    state: &'static str,
    why: String,
    started_ms: Option<i64>,
    ended_ms: Option<i64>,
    steps: u32,
    usage: Usage,
    report: Option<String>,
}

impl Row {
    fn json(&self) -> Value {
        json!({
            "name": self.name, "mode": self.mode, "model": self.model, "state": self.state,
            "why": self.why, "started_ms": self.started_ms, "ended_ms": self.ended_ms,
            "steps": self.steps, "cost": self.usage.cost,
            "input": self.usage.input + self.usage.cache_read + self.usage.cache_write,
            "output": self.usage.output, "report": self.report,
        })
    }
}

/// `3 subagents: 2 done, 1 failed · 4 min · $0.42`.
fn summary(rows: &[Row], secs: u64) -> String {
    let mut by = Vec::<(&str, usize)>::new();
    for r in rows {
        match by.iter_mut().find(|(s, _)| *s == r.state) {
            Some((_, n)) => *n += 1,
            None => by.push((r.state, 1)),
        }
    }
    let cost: f64 = rows.iter().map(|r| r.usage.cost).sum();
    format!(
        "{} subagent{}: {} · {} · {}",
        rows.len(),
        if rows.len() == 1 { "" } else { "s" },
        by.iter()
            .map(|(s, n)| format!("{n} {s}"))
            .collect::<Vec<_>>()
            .join(", "),
        usage::duration(secs),
        usage::usd((cost.max(0.0) * 1e6).round() as u64)
    )
}

/// A report cut to `MAX_REPORT` bytes, on a character boundary.
fn capped(s: &str) -> String {
    if s.len() <= MAX_REPORT {
        return s.to_owned();
    }
    let mut end = MAX_REPORT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[cut: the whole report is in Clio's run folder]",
        &s[..end]
    )
}

/// A child's caps: `subagent.*`, the task's anti-loop and retries.
fn budget(cfg: &Config) -> Budget {
    let mut b = super::local::task_budget(&cfg.limits);
    b.max_steps = cfg.subagent.max_steps;
    b.max_ms = i64::try_from(cfg.subagent.max_seconds.saturating_mul(1000)).unwrap_or(i64::MAX);
    b.max_cost_micro = cfg.subagent.max_cost_micro_usd;
    b
}

fn limit(stop: Stop) -> String {
    let key = match stop {
        Stop::Steps => " (subagent.max_steps)",
        Stop::Time => " (subagent.max_seconds)",
        Stop::Cost => " (subagent.max_cost_micro_usd)",
        _ => "",
    };
    format!("{}{key}", stop.reason())
}

fn suspect_in(file: &Path) -> bool {
    std::fs::read_to_string(file).is_ok_and(|t| t.contains("\"suspect\""))
}

impl<'a> Turn<'a> {
    pub fn new(ctx: Ctx<'a>) -> Self {
        Self {
            ctx,
            ended: Mutex::default(),
            over: AtomicBool::new(false),
        }
    }

    /// Pi ended the tool `call`: its children stop.
    pub fn tool_ended(&self, call: &str) {
        super::local::locked(&self.ended).insert(call.to_owned());
    }

    /// The turn is over: every child stops.
    pub fn finish(&self) {
        self.over.store(true, Ordering::SeqCst);
    }

    fn stopped(&self, call: &str) -> bool {
        self.ctx.cancel.is_cancelled()
            || self.over.load(Ordering::SeqCst)
            || super::local::locked(&self.ended).contains(call)
    }

    /// The chat's row for `call`: one line per child, and the end's note once `end` says it.
    fn show(&self, call: &str, rows: &[Row], end: Option<&str>) {
        self.ctx.client.inject(Event::ToolUpdate {
            id: call.to_owned(),
            name: "subagent".into(),
            partial: json!({
                "subagents": rows.iter().map(Row::json).collect::<Vec<_>>(),
                "summary": end,
            }),
        });
    }

    fn done(&self, call: &str, v: &Value) -> bool {
        let c = &self.ctx;
        jev::subagent_done(c.socket, c.session, c.token, call, v)
    }

    /// Runs the call `call` to its end (on a thread of its own): never fails, what went wrong is
    /// in its answer.
    pub fn run_call(&self, call: &str) {
        let c = &self.ctx;
        let began = Instant::now();
        let args = loop {
            if self.stopped(call) || began.elapsed() > TAKE_WAIT {
                return;
            }
            if let Some(a) = jev::subagent_take(c.socket, c.session, c.token, call) {
                break a;
            }
            std::thread::sleep(POLL);
        };
        let cfg = match session_cfg(c.dirs, &c.model) {
            Ok(cfg) => cfg,
            Err(e) => {
                self.done(call, &json!({ "refused": format!("Clio's config: {e}") }));
                return;
            }
        };
        let offered = usage::pi_models(c.dirs, &cfg.model.provider)
            .unwrap_or_else(|| usage::catalog_models(c.dirs, &cfg.model.provider));
        let recent = launched_since(c.run_dir, crate::cli::now_ms() - HOUR_MS);
        let tasks = match plan(&args, &cfg.subagent, recent, &c.model.model, &offered) {
            Ok(t) => t,
            Err(e) => {
                self.done(call, &json!({ "refused": e }));
                return;
            }
        };
        let rows = Mutex::new(
            tasks
                .iter()
                .map(|t| Row {
                    name: t.name.clone(),
                    mode: mode_word(t.mode),
                    model: t.model.clone(),
                    state: "waiting",
                    ..Row::default()
                })
                .collect::<Vec<_>>(),
        );
        self.show(call, &super::local::locked(&rows), None);
        let cancels: Vec<Cancel> = tasks.iter().map(|_| Cancel::new()).collect();
        let (next, left) = (AtomicUsize::new(0), AtomicUsize::new(tasks.len()));
        let reports = Mutex::new(vec![String::new(); tasks.len()]);
        let flagged = AtomicBool::new(false);
        let workers = (cfg.subagent.max_parallel as usize).clamp(1, tasks.len());
        std::thread::scope(|sc| {
            for _ in 0..workers {
                sc.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::SeqCst);
                        let Some(t) = tasks.get(i) else { break };
                        let set = |f: &dyn Fn(&mut Row)| {
                            let mut r = super::local::locked(&rows);
                            f(&mut r[i]);
                            self.show(call, &r, None);
                        };
                        let (report, flag) = self.child(&cfg, t, &cancels[i], &set);
                        flagged.fetch_or(flag, Ordering::SeqCst);
                        super::local::locked(&reports)[i] = report;
                        left.fetch_sub(1, Ordering::SeqCst);
                    }
                });
            }
            // The call's end stops its children; the scope waits for every one of them.
            while left.load(Ordering::SeqCst) > 0 {
                if self.stopped(call) {
                    cancels.iter().for_each(Cancel::cancel);
                }
                std::thread::sleep(POLL);
            }
        });
        let rows = rows.into_inner().unwrap_or_else(|e| e.into_inner());
        let end = summary(&rows, began.elapsed().as_secs());
        let mut note = end.clone();
        for r in &rows {
            note.push_str(&format!(
                "\n- {} ({}, {}): {}",
                r.name, r.mode, r.model, r.state
            ));
            if !r.why.is_empty() {
                note.push_str(&format!(": {}", r.why));
            }
        }
        let reports = reports.into_inner().unwrap_or_else(|e| e.into_inner());
        let given: Vec<Value> = rows
            .iter()
            .zip(&reports)
            .filter(|(r, text)| r.state != "flagged" && !text.trim().is_empty())
            .map(|(r, text)| json!({ "name": r.name, "text": capped(text) }))
            .collect();
        self.show(call, &rows, Some(&end));
        self.done(
            call,
            &json!({
                "note": note,
                "reports": given,
                "suspect": flagged.load(Ordering::SeqCst),
            }),
        );
    }

    /// One child, start to end: its report, and whether the judge flagged it.
    fn child(&self, cfg: &Config, t: &Task, cancel: &Cancel, set: SetRow<'_>) -> (String, bool) {
        let c = &self.ctx;
        let fail = |state: &'static str, why: String| {
            set(&|r: &mut Row| {
                r.state = state;
                r.why.clone_from(&why);
                r.ended_ms.get_or_insert(crate::cli::now_ms());
            });
        };
        if cancel.is_cancelled() {
            fail("stopped", "the call was stopped before it started".into());
            return (String::new(), false);
        }
        let (n, dir) = match new_folder(c.run_dir, "subagents") {
            Ok(x) => x,
            Err(e) => {
                fail("failed", e);
                return (String::new(), false);
            }
        };
        let started = crate::cli::now_ms();
        let keep = |row: &Row| {
            let mut v = row.json();
            v["task"] = json!(t.task);
            v["n"] = json!(n);
            v["started_ms"] = json!(started);
            let _ = crate::cli::write_private(
                &dir.join("subagent.json"),
                serde_json::to_string_pretty(&v)
                    .unwrap_or_default()
                    .as_bytes(),
            );
        };
        let report_file = dir.join("report.txt");
        set(&|r: &mut Row| {
            r.state = "running";
            r.started_ms = Some(started);
            r.report = Some(report_file.display().to_string());
            keep(r);
        });
        let marked = std::fs::read_to_string(c.taint_file)
            .ok()
            .map(|t| t.contains("\"suspect\""));
        let (state, why, text, usage, steps, flagged) =
            self.run_child(cfg, t, n, &dir, marked, cancel);
        let _ = crate::cli::write_private(&report_file, text::data(&text).as_bytes());
        // A flag in a child flags the conversation now: the window and bypass close.
        if flagged {
            jev::flag_session(c.socket, c.session);
        }
        set(&|r: &mut Row| {
            r.state = state;
            r.why.clone_from(&why);
            r.ended_ms = Some(crate::cli::now_ms());
            r.usage = usage;
            r.steps = steps;
            keep(r);
        });
        (text, flagged)
    }

    #[allow(clippy::type_complexity)]
    fn run_child(
        &self,
        cfg: &Config,
        t: &Task,
        n: u32,
        dir: &Path,
        marked: Option<bool>,
        cancel: &Cancel,
    ) -> (&'static str, String, String, Usage, u32, bool) {
        let c = &self.ctx;
        let id = format!("{}.sub-{n}", c.run);
        let mut cc = cfg.clone();
        cc.model.model.clone_from(&t.model);
        let started = second_agent(
            &Second {
                dirs: c.dirs,
                cfg: &cc,
                dir,
                id: &id,
                work: c.work,
                host: c.host,
                marked,
            },
            Kind::Child(t.mode),
        );
        let (toolhost, pi) = match started {
            Ok(x) => x,
            Err(e) => {
                return (
                    "failed",
                    format!("it did not start: {e}"),
                    String::new(),
                    Usage::default(),
                    0,
                    false,
                );
            }
        };
        let mut guard = RunGuard::new(Arc::new(SystemClock), &id, budget(cfg), c.day.clone());
        let tainted = dir.join("toolhost/tainted.json");
        let (mut steps, mut flagged) = (0u32, false);
        let ask = format!(
            "Your task, as the {} subagent «{}»:\n\n{}",
            if t.mode == Child::Write {
                "writing"
            } else {
                "reading"
            },
            t.name,
            t.task
        );
        let ran = pi
            .client
            .run_prompt_limited(&ask, cancel, &mut guard, &mut |e| match e {
                Event::ToolStart { .. } => steps += 1,
                // The judge flagged what it read: the child ends here.
                Event::ToolEnd { .. } if marked != Some(true) && suspect_in(&tainted) => {
                    flagged = true;
                    cancel.cancel();
                }
                _ => {}
            });
        drop(pi);
        toolhost.stop();
        flagged |= marked != Some(true) && suspect_in(&tainted);
        // What a writer changed is the run's: `clio undo` takes it back with the parent's.
        let mut joined = String::new();
        if c.host
            && t.mode == Child::Write
            && let Err(e) = clio_toolhost::host::fold_journal(
                &dir.join("toolhost"),
                &c.run_dir.join("toolhost"),
            )
        {
            joined = format!("; its changes are not in this session's undo: {e}");
        }
        let (state, why, text, usage) = match ran {
            Err(e) => ("failed", e.to_string(), String::new(), Usage::default()),
            Ok(s) => {
                let (state, why) = if flagged {
                    (
                        "flagged",
                        "the judge flagged what it read: it was stopped and its report withheld"
                            .to_owned(),
                    )
                } else if cancel.is_cancelled() || s.aborted {
                    ("stopped", "it was stopped".to_owned())
                } else if let Some(stop) = s.stopped {
                    ("failed", format!("it reached its limit: {}", limit(stop)))
                } else if s.text.trim().is_empty() && !s.errors.is_empty() {
                    (
                        "failed",
                        format!("its model failed: {}", s.errors.join("; ")),
                    )
                } else {
                    ("done", String::new())
                };
                (state, why, s.text, s.usage)
            }
        };
        (state, format!("{why}{joined}"), text, usage, steps, flagged)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn task(name: &str, mode: &str) -> Value {
        json!({"name": name, "task": "look", "mode": mode})
    }

    // S-73: the caps hold before any child starts.
    #[test]
    fn a_call_over_the_caps_is_refused_before_anything_runs() {
        let cfg = SubagentCfg::default();
        let p =
            |tasks: Vec<Value>, recent| plan(&json!({ "tasks": tasks }), &cfg, recent, "m", &[]);
        let nine: Vec<Value> = (0..9).map(|i| task(&format!("r{i}"), "read")).collect();
        assert!(p(nine, 0).unwrap_err().contains("at most 8"));
        let eight: Vec<Value> = (0..8).map(|i| task(&format!("r{i}"), "read")).collect();
        assert_eq!(p(eight.clone(), 0).unwrap().len(), 8);
        assert!(
            p(vec![task("a", "write"), task("b", "write")], 0)
                .unwrap_err()
                .contains("alone")
        );
        assert!(
            p(vec![task("a", "write"), task("b", "read")], 0)
                .unwrap_err()
                .contains("alone")
        );
        assert_eq!(
            p(vec![task("a", "write")], 0).unwrap()[0].mode,
            Child::Write
        );
        // The hour's cap counts what ran already.
        assert!(p(vec![task("a", "read")], 39).is_ok());
        assert!(
            p(vec![task("a", "read")], 40)
                .unwrap_err()
                .contains("max_per_hour")
        );
        assert!(p(eight, 33).unwrap_err().contains("max_per_hour"));
        assert!(p(vec![task("a", "read"), task("a", "read")], 0).is_err());
        assert!(p(vec![task("a", "delete")], 0).is_err());
        assert!(p(vec![], 0).is_err());
        let small = SubagentCfg {
            max_per_call: 2,
            ..SubagentCfg::default()
        };
        let three: Vec<Value> = (0..3).map(|i| task(&format!("r{i}"), "read")).collect();
        assert!(plan(&json!({ "tasks": three }), &small, 0, "m", &[]).is_err());
    }

    #[test]
    fn a_child_runs_on_a_model_of_the_connector() {
        let mut cfg = SubagentCfg::default();
        let offered = ["m".to_owned(), "small".to_owned()];
        let one = |t: Value, cfg: &SubagentCfg| plan(&json!({"tasks": [t]}), cfg, 0, "m", &offered);
        assert_eq!(one(task("a", "read"), &cfg).unwrap()[0].model, "m");
        let mut t = task("a", "read");
        t["model"] = json!("small");
        assert_eq!(one(t.clone(), &cfg).unwrap()[0].model, "small");
        t["model"] = json!("other");
        assert!(one(t, &cfg).unwrap_err().contains("not a model"));
        cfg.model = Some("small".into());
        assert_eq!(one(task("a", "read"), &cfg).unwrap()[0].model, "small");
    }

    #[test]
    fn the_end_note_counts_each_state_and_a_report_is_capped() {
        let row = |state, cost| Row {
            state,
            usage: Usage {
                cost,
                ..Usage::default()
            },
            ..Row::default()
        };
        let rows = [row("done", 0.2), row("done", 0.12), row("failed", 0.1)];
        assert_eq!(
            summary(&rows, 240),
            "3 subagents: 2 done, 1 failed · 4 min · $0.42"
        );
        let long = "é".repeat(MAX_REPORT);
        let c = capped(&long);
        assert!(c.len() < MAX_REPORT + 100 && c.ends_with("run folder]"));
        assert_eq!(capped("short"), "short");
    }

    #[test]
    fn the_hour_counts_the_children_in_the_run_folder() {
        let d = tempfile::tempdir().unwrap();
        for (n, at) in [(1, 10), (2, 5_000_000), (3, 6_000_000)] {
            let f = d.path().join(format!("subagents/{n}"));
            std::fs::create_dir_all(&f).unwrap();
            std::fs::write(
                f.join("subagent.json"),
                json!({"n": n, "started_ms": at}).to_string(),
            )
            .unwrap();
        }
        assert_eq!(launched_since(d.path(), 1_000_000), 2);
    }
}
