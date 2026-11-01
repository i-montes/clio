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
                .map(|_| Turn::Calls(vec![("read".to_owned(), json!({"path":"a.txt"}))]))
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
        "read as Pi sees it (extension + socket + gateway + executor), {} calls: median {:.2} ms, p95 {:.2} ms, max {:.2} ms",
        durations.len(),
        durations[durations.len() / 2],
        durations[durations.len() * 95 / 100],
        durations[durations.len() - 1]
    );
    assert!(starts[2] < Duration::from_secs(20));
}

/// The command tool costs a jail start (`bwrap` plus `prlimit`) on top of the round trips.
#[test]
fn jailed_command_latency_is_measured() {
    let Some(node) = pi_available() else { return };
    let w = world();
    let host = Toolhost::start(&w, true, None);
    for _ in 0..200 {
        let t = host.stderr.lock().unwrap().clone();
        if t.contains("is available") || t.contains("NOT offered") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if host.stderr.lock().unwrap().contains("NOT offered") {
        eprintln!("SKIPPED (no jail here)");
        assert!(std::env::var_os("CLIO_REQUIRE_JAIL").is_none());
        return;
    }
    let turns = (0..15)
        .map(|_| Turn::Calls(vec![("bash".to_owned(), json!({"command":"echo hi"}))]))
        .collect();
    let model = FakeModel::scripted(turns);
    let mut pi = Pi::start(&node, &w, &model, Some(&host.socket), &Opts::default());
    let ev = pi.prompt("run it many times");
    let mut d: Vec<f64> = ev
        .iter()
        .filter(|e| e["type"] == "tool_execution_end")
        .filter_map(|e| e["durationMs"].as_f64())
        .collect();
    d.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert!(d.len() >= 15, "{d:?}");
    eprintln!(
        "jailed echo as Pi sees it, {} calls: median {:.0} ms, max {:.0} ms",
        d.len(),
        d[d.len() / 2],
        d[d.len() - 1]
    );
}
