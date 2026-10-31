use std::path::PathBuf;

use clio_bench::{
    Check, Report, SingleCall, Task, Thresholds, load_tasks, regressions, request_for, run,
};
use clio_fake_llm::{Record, Replay, Response, Scripted, Tape};
use clio_trace::{FixedClock, Trace};

fn tmp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("clio-bench-{name}-{}", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

fn task(id: &str, input: &str, expect: &str) -> Task {
    Task {
        id: id.into(),
        input: input.into(),
        check: Check::Contains(expect.into()),
        max_steps: 1,
        max_cost_micro_usd: 1_000,
        max_duration_ms: 1_000,
    }
}

fn trace(name: &str) -> (Trace, PathBuf) {
    let path = tmp(name);
    let t = Trace::open(&path, FixedClock::new(0)).expect("trace");
    t.set_price("fake", "replay-1", 0, 3_000_000, 15_000_000)
        .expect("price");
    (t, path)
}

/// A tape holding `answers` (input, answer) for the model `replay-1`.
fn tape(name: &str, answers: &[(&str, &str)]) -> PathBuf {
    let path = tmp(name);
    let record = Record::new(
        Scripted::new(answers.iter().map(|(_, a)| Response {
            content: (*a).into(),
            input_tokens: 10,
            output_tokens: 5,
        })),
        Tape::open(&path).expect("tape"),
    );
    for (input, _) in answers {
        use clio_fake_llm::Llm;
        record
            .complete(&request_for("replay-1", input))
            .expect("record");
    }
    path
}

fn agent(tape: &PathBuf) -> SingleCall {
    SingleCall {
        llm: Box::new(Replay::new(Tape::open(tape).expect("tape"))),
        provider: "fake".into(),
        model: "replay-1".into(),
    }
}

#[test]
fn two_replay_runs_give_exactly_the_same_report() {
    let tasks = [task("a", "¿2+2?", "4"), task("b", "¿capital?", "París")];
    let tape = tape("same-tape", &[("¿2+2?", "4"), ("¿capital?", "París")]);
    let report = |n: &str| {
        let (t, path) = trace(n);
        let r = run(&tasks, &agent(&tape), 3, &t).expect("run");
        let _ = std::fs::remove_file(path);
        r
    };
    assert_eq!(report("same-1"), report("same-2"));
}

#[test]
fn success_cost_and_time_come_back_and_every_run_is_stored_in_the_trace() {
    let tasks = [task("a", "¿2+2?", "4")];
    let tape = tape("stored-tape", &[("¿2+2?", "4")]);
    let (t, path) = trace("stored");
    let r = run(&tasks, &agent(&tape), 2, &t).expect("run");
    // 10 in * 3 + 5 out * 15 = 105 micro-USD per run
    assert_eq!(
        (r.runs, r.successes, r.cost_micro_usd, r.duration_ms),
        (2, 2, 210, 0)
    );
    let conn = rusqlite::Connection::open(&path).expect("open");
    let roots: i64 = conn
        .query_row("SELECT COUNT(*) FROM spans WHERE name = 'bench:a' AND json_extract(attrs, '$.success') = 1", [], |x| x.get(0))
        .expect("roots");
    let calls: i64 = conn
        .query_row("SELECT COUNT(*) FROM spans WHERE name = 'call' AND json_extract(attrs, '$.\"clio.prompt.version\"') = 'single-call@1'", [], |x| x.get(0))
        .expect("calls");
    assert_eq!((roots, calls), (2, 2));
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_task_with_no_recording_fails_instead_of_passing() {
    let tape = tape("missing-tape", &[("¿2+2?", "4")]);
    let (t, path) = trace("missing");
    let r = run(
        &[task("a", "¿2+2?", "4"), task("b", "nunca grabada", "x")],
        &agent(&tape),
        1,
        &t,
    )
    .expect("run");
    assert_eq!((r.runs, r.successes), (2, 1));
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_wrong_answer_or_a_blown_limit_is_a_failure() {
    let tape = tape("limits-tape", &[("q1", "otra cosa"), ("q2", "ok")]);
    let (t, path) = trace("limits");
    let mut dear = task("dear", "q2", "ok");
    dear.max_cost_micro_usd = 50; // the run costs 105
    let r = run(&[task("wrong", "q1", "ok"), dear], &agent(&tape), 1, &t).expect("run");
    assert_eq!(r.successes, 0);
    let _ = std::fs::remove_file(path);
}

fn report(runs: u32, successes: u32, cost: i64) -> Report {
    Report {
        runs,
        successes,
        cost_micro_usd: cost,
        duration_ms: 0,
        tasks: vec![],
    }
}

#[test]
fn the_gate_blocks_a_drop_in_success_or_a_rise_in_cost_and_lets_the_rest_through() {
    let limits = Thresholds {
        max_success_drop_points: 5.0,
        max_cost_rise_pct: 20.0,
    };
    let base = report(100, 90, 10_000);
    assert!(regressions(&base, &report(100, 90, 10_000), &limits).is_empty());
    assert!(regressions(&base, &report(100, 86, 10_000), &limits).is_empty());
    assert_eq!(
        regressions(&base, &report(100, 80, 10_000), &limits).len(),
        1
    );
    assert_eq!(
        regressions(&base, &report(100, 90, 12_500), &limits).len(),
        1
    );
    assert_eq!(
        regressions(&base, &report(100, 70, 15_000), &limits).len(),
        2
    );
}

#[test]
fn the_shipped_trivial_tasks_load_and_a_duplicate_id_or_unknown_field_is_refused() {
    let tasks =
        load_tasks(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tasks")).expect("tasks");
    assert_eq!(tasks.len(), 20);

    let dir = tmp("dups");
    std::fs::create_dir_all(&dir).expect("dir");
    let one = r#"{"id":"x","input":"i","check":{"kind":"equals","value":"v"},"max_steps":1,"max_cost_micro_usd":1,"max_duration_ms":1}"#;
    std::fs::write(dir.join("a.json"), one).expect("a");
    std::fs::write(dir.join("b.json"), one).expect("b");
    assert!(
        load_tasks(&dir)
            .expect_err("dup")
            .contains("duplicate task id")
    );
    std::fs::write(dir.join("b.json"), one.replace("\"id\"", "\"idd\"")).expect("b");
    assert!(load_tasks(&dir).is_err());
    let _ = std::fs::remove_dir_all(dir);
}
