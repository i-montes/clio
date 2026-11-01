//! Measures (and prints) Pi's start-up time with the extension, and what a tool call costs from
//! Pi's side: `durationMs` of `tool_execution_end` covers the extension, the socket, the gateway
//! and the executor. The asserts are loose on purpose (machines differ); the numbers are the point.
#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use common::fake_model::{FakeModel, Turn};
use common::*;
use serde_json::json;

#[test]
fn start_up_and_tool_call_latency_are_measured() {
    let Some(node) = pi_available() else { return };
    let mut starts = Vec::new();
    let mut durations: Vec<f64> = Vec::new();
    for round in 0..5 {
        let w = world();
        w.put("a.txt", "hello");
        let turns = if round == 0 {
            (0..40)
                .map(|_| Turn::Calls(vec![("read_file".to_owned(), json!({"path":"a.txt"}))]))
                .collect()
        } else {
            vec![]
        };
        let model = FakeModel::scripted(turns);
        let mut s = stack(&node, &w, &model, false, &Opts::default());
        starts.push(s.pi.started_in);
        if round == 0 {
            let ev = s.pi.prompt("read it many times");
            durations.extend(
                ev.iter()
                    .filter(|e| e["type"] == "tool_execution_end")
                    .filter_map(|e| e["durationMs"].as_f64()),
            );
        }
    }
    starts.sort();
    durations.sort_by(|a, b| a.partial_cmp(b).unwrap());
    eprintln!(
        "Pi start-up with the extension (spawn to first RPC answer), 5 runs: median {:?}, min {:?}, max {:?}",
        starts[2], starts[0], starts[4]
    );
    assert!(durations.len() >= 30, "{durations:?}");
    eprintln!(
        "read_file as Pi sees it (extension + socket + gateway + executor), {} calls: median {:.2} ms, p95 {:.2} ms, max {:.2} ms",
        durations.len(),
        durations[durations.len() / 2],
        durations[durations.len() * 95 / 100],
        durations[durations.len() - 1]
    );
    assert!(starts[2] < Duration::from_secs(20));
}
