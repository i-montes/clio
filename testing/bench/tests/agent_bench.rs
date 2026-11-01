#![allow(clippy::unwrap_used)]
//! `run_agent` end to end with a scripted model program (no network): the workspace is copied,
//! the agent works on the copy, the hidden check decides, and the run lands in the trace.

use std::path::Path;

use clio_bench::agent_bench::{AgentBenchConfig, run_agent};
use clio_bench::load_tasks;
use clio_trace::{SystemClock, Trace};

// The "model" is stateless, so it picks its move from how many messages it was sent.
const MODEL: &str = r#"import sys, json
r = json.load(sys.stdin)
n = len(r["messages"])
sol = open(sys.argv[1]).read()
moves = [
  {"tool": "read_file", "args": {"path": "dates.py"}},
  {"tool": "write_file", "args": {"path": "dates.py", "content": sol}},
  {"final": "done"},
]
print(json.dumps({"content": json.dumps(moves[(n - 2) // 2]), "input_tokens": 10, "output_tokens": 5}))"#;

#[test]
fn a_run_copies_the_workspace_runs_the_hidden_check_and_is_traced() {
    let tasks_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tasks/real");
    let tasks = load_tasks(&tasks_dir).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let solution = tasks_dir.join("real-01-fecha-invalida/solution/dates.py");
    let cfg = AgentBenchConfig {
        provider: "fake".into(),
        model: "replay-1".into(),
        system_prompt: "test prompt".into(),
        prompt_version: "test@1".into(),
        program: "python3".into(),
        args: vec![
            "-c".into(),
            MODEL.into(),
            solution.to_string_lossy().into_owned(),
        ],
        repeats: 2,
        tags: vec![],
        only: vec!["real-01".into()],
        run_root: tmp.path().join("runs"),
        cleanup: false,
        price_in_micro_per_mtok: 0,
        price_out_micro_per_mtok: 0,
        backoff_ms: vec![],
        pi: None,
    };
    let trace = Trace::open(tmp.path().join("trace.db"), SystemClock).unwrap();
    trace.set_price("fake", "replay-1", 0, 0, 0).unwrap();
    let report = run_agent(&tasks, &cfg, &trace).unwrap();

    assert_eq!((report.runs, report.successes), (2, 2), "{report:?}");
    assert_eq!(report.tasks[0].steps_total, 6);
    assert_eq!(report.tasks[0].details[0].input_tokens, 30);
    assert!(report.line().contains("2/2"));
    assert!(report.table().contains("real-01"));
    // The agent worked on a copy: the task's own workspace is untouched, and the run left a transcript.
    let original =
        std::fs::read_to_string(tasks_dir.join("real-01-fecha-invalida/workspace/dates.py"))
            .unwrap();
    assert_ne!(original, std::fs::read_to_string(&solution).unwrap());
    assert!(tmp.path().join("runs/real-01-0/transcript.json").exists());
}
