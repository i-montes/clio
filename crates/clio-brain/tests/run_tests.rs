#![allow(clippy::unwrap_used)]
//! `run_tests` inside the agent: the gateway governs it like any tool, its output is external
//! data, and a passing run AFTER the last change is the one signal that closes the contract.
//! Needs the jail (unprivileged namespaces); without it every test prints SKIPPED and returns
//! (`CLIO_REQUIRE_JAIL=1` turns that into a failure).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clio_brain::agent::{AgentConfig, AgentLoop, AgentOutcome, Ending, Msg};
use clio_contract::{Contract, State};
use clio_core::{Clock, FixedClock, Level};
use serde_json::{Value, json};

fn jail_ready() -> bool {
    use std::sync::OnceLock;
    static R: OnceLock<Result<(), String>> = OnceLock::new();
    match R.get_or_init(|| {
        clio_node::Jail::detect()?.probe()?;
        Ok(())
    }) {
        Ok(()) => true,
        Err(why) => {
            assert!(
                std::env::var_os("CLIO_REQUIRE_JAIL").is_none(),
                "CLIO_REQUIRE_JAIL is set and the jail is not available: {why}"
            );
            eprintln!("SKIPPED (the jail cannot run here, nothing was tested): {why}");
            false
        }
    }
}

fn task_dir(prefix: &str) -> PathBuf {
    let tasks = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testing/bench/tasks/real");
    std::fs::read_dir(tasks)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.file_name().unwrap().to_string_lossy().starts_with(prefix))
        .unwrap()
}

fn copy(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let to = dst.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy(&e.path(), &to);
        } else {
            std::fs::copy(e.path(), to).unwrap();
        }
    }
}

struct Run {
    _tmp: tempfile::TempDir,
    work: PathBuf,
    run_dir: PathBuf,
    scratch: PathBuf,
    task: PathBuf,
}

fn setup(prefix: &str) -> Run {
    let task = task_dir(prefix);
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("work");
    copy(&task.join("workspace"), &work);
    Run {
        run_dir: tmp.path().join("run"),
        scratch: tmp.path().join("scratch"),
        work,
        task,
        _tmp: tmp,
    }
}

fn agent(r: &Run, run_tests: bool) -> AgentLoop {
    let mut cfg = AgentConfig::new("t", "system prompt of the test", &r.run_dir);
    cfg.min_level = Level::Full;
    cfg.run_tests = run_tests;
    let clock: Arc<dyn Clock> = Arc::new(FixedClock::new(1_000));
    AgentLoop::new(clock, cfg, &r.work, &r.scratch).unwrap()
}

fn call(tool: &str, args: Value) -> String {
    json!({"tool": tool, "args": args}).to_string()
}

fn script(answers: Vec<String>) -> impl FnMut(&[Msg]) -> String {
    let mut i = 0;
    move |_| {
        let a = answers[i.min(answers.len() - 1)].clone();
        i += 1;
        a
    }
}

fn final_msg() -> String {
    json!({"final": "done"}).to_string()
}

fn tests() -> String {
    call("run_tests", json!({"runner": "unittest"}))
}

fn saved_state(r: &Run) -> State {
    Contract::load(&r.run_dir.join("contract.json"))
        .unwrap()
        .state()
}

/// The text the model received for its `n`th tool call.
fn replies(out: &AgentOutcome) -> Vec<&str> {
    out.transcript
        .iter()
        .skip(2)
        .filter(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .collect()
}

#[test]
fn tests_that_pass_after_the_last_change_close_the_contract_as_done() {
    if !jail_ready() {
        return;
    }
    let r = setup("real-01");
    let solution = std::fs::read_to_string(r.task.join("solution/dates.py")).unwrap();
    let mut model = script(vec![
        call("read_file", json!({"path": "dates.py"})),
        call(
            "write_file",
            json!({"path": "dates.py", "content": solution}),
        ),
        tests(),
        final_msg(),
    ]);
    let out = agent(&r, true)
        .run(&mut model, "arregla parse_date")
        .unwrap();
    assert_eq!(out.ending, Ending::Final);
    assert_eq!(out.steps, 4, "one more step than without run_tests");
    assert_eq!(out.tests_run, 1);
    assert!(out.verified_by_tests);
    assert_eq!(out.contract_state, Some(State::ClosedOk));
    assert_eq!(saved_state(&r), State::ClosedOk);
    assert!(replies(&out)[2].starts_with("Result of run_tests: PASSED (exit code 0)"));
    assert!(
        out.report
            .done
            .iter()
            .any(|d| d.contains("run_tests passed"))
    );
    assert!(
        !r.work.join("__pycache__").exists(),
        "the run left no bytecode"
    );
}

#[test]
fn run_tests_alone_taints_the_session_and_its_output_arrives_wrapped_as_data() {
    if !jail_ready() {
        return;
    }
    let r = setup("real-01");
    let mut model = script(vec![tests(), final_msg()]);
    let out = agent(&r, true).run(&mut model, "revisa").unwrap();
    assert!(out.tainted, "no file was read: only the tests ran");
    let seen = replies(&out)[0];
    assert!(seen.contains("<<<UNTRUSTED-DATA tool-result"), "{seen}");
    assert!(seen.contains("Ran 2 tests"), "{seen}");
}

#[test]
fn failing_tests_leave_the_contract_blocked() {
    if !jail_ready() {
        return;
    }
    let r = setup("real-01");
    let broken = "def parse_date(s):\n    raise RuntimeError('broken')\n";
    let mut model = script(vec![
        call("write_file", json!({"path": "dates.py", "content": broken})),
        tests(),
        final_msg(),
    ]);
    let out = agent(&r, true)
        .run(&mut model, "arregla parse_date")
        .unwrap();
    assert_eq!(out.ending, Ending::Final);
    assert_eq!(out.tests_run, 1);
    assert!(!out.verified_by_tests);
    assert_eq!(out.contract_state, Some(State::Blocked));
    assert!(replies(&out)[1].starts_with("Result of run_tests: FAILED (exit code 1)"));
    assert!(out.report.pending.iter().any(|p| p.contains("FAILED")));
}

#[test]
fn a_pass_before_the_last_write_does_not_count() {
    if !jail_ready() {
        return;
    }
    let r = setup("real-01");
    let mut model = script(vec![
        tests(),
        call(
            "write_file",
            json!({"path": "notes.txt", "content": "late edit"}),
        ),
        final_msg(),
    ]);
    let out = agent(&r, true).run(&mut model, "revisa").unwrap();
    assert_eq!(out.tests_run, 1);
    assert!(!out.verified_by_tests);
    assert_eq!(out.contract_state, Some(State::Blocked));
    assert!(
        out.report
            .pending
            .iter()
            .any(|p| p.contains("changed after"))
    );
}

#[test]
fn without_running_the_tests_it_blocks_as_before() {
    if !jail_ready() {
        return;
    }
    let r = setup("real-01");
    let mut model = script(vec![final_msg()]);
    let out = agent(&r, true).run(&mut model, "revisa").unwrap();
    assert_eq!(out.contract_state, Some(State::Blocked));
    assert_eq!(out.tests_run, 0);
}

#[test]
fn without_the_jail_the_tool_does_not_exist_for_the_gateway() {
    // Does not need the jail: the tool is simply not registered or declared.
    let r = setup("real-01");
    let mut model = script(vec![tests(), final_msg()]);
    let out = agent(&r, false).run(&mut model, "revisa").unwrap();
    assert_eq!(out.denied, 1);
    let seen = replies(&out)[0];
    assert!(
        seen.starts_with("[gateway] run_tests was not run (rule S-02)"),
        "{seen}"
    );
    assert_eq!(out.contract_state, Some(State::Blocked));
}

// S-12 S-13: the arguments are a closed menu; anything else never reaches a command line.
#[test]
fn hostile_arguments_are_refused_by_the_gateway_or_the_tool_and_nothing_runs() {
    if !jail_ready() {
        return;
    }
    let r = setup("real-01");
    let bad = [
        json!({"runner": "unittest", "target": "../x"}),
        json!({"runner": "unittest", "target": "/etc/passwd"}),
        json!({"runner": "unittest", "target": "--foo"}),
        json!({"runner": "unittest", "target": "a;touch pwned"}),
        json!({"runner": "sh"}),
        json!({"runner": "unittest", "cmd": ["sh", "-c", "id"]}),
        json!({"runner": "unittest", "target": ".git"}),
        json!({"runner": ["unittest"]}),
    ];
    let mut answers: Vec<String> = bad.iter().map(|a| call("run_tests", a.clone())).collect();
    answers.push(final_msg());
    let out = agent(&r, true).run(&mut script(answers), "revisa").unwrap();
    assert_eq!(out.tests_run, 0, "no run reached the jail");
    assert!(!out.verified_by_tests);
    for m in replies(&out).iter().take(bad.len()) {
        assert!(
            m.starts_with("[gateway] run_tests was not run")
                || m.starts_with("[error] run_tests failed"),
            "{m}"
        );
    }
    assert!(!r.work.join("pwned").exists());
}
