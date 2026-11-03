#![allow(clippy::unwrap_used)]
//! `clio-bench agent --harness pi` machinery end to end with no network: the REAL Pi with the
//! guard and the real `clio toolhost`, against the fake model of `testing/fake-provider/` and a
//! tiny task. Needs Node, `npm ci` in `adapters/pi`, `python3` and a built `clio` (skipped with a
//! message otherwise; `CLIO_REQUIRE_PI=1` turns the skip into a failure).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use clio_bench::agent_bench::{AgentBenchConfig, run_agent};
use clio_bench::load_tasks;
use clio_bench::pi_bench::{PiBench, locate_clio};
use clio_brain::pi::PiPaths;
use clio_brain::pi::install::{find_node, pinned_version};
use clio_brain::pi::launch::{Provider, Secret};
use clio_trace::{SystemClock, Trace};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

struct Fake(Child);

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_fake(dir: &Path, script: &str) -> (Fake, String) {
    let sp = dir.join("script.json");
    std::fs::write(&sp, script).unwrap();
    let mut child = Command::new("python3")
        .arg(repo().join("testing/fake-provider/server.py"))
        .arg("--script")
        .arg(&sp)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    (Fake(child), v["base_url"].as_str().unwrap().to_owned())
}

/// Why this machine cannot run Pi, if it cannot.
fn missing() -> Option<String> {
    if find_node(&std::env::var("PATH").unwrap_or_default()).is_err() {
        return Some("Node 22.19+ no está instalado".into());
    }
    if !repo().join("adapters/pi/node_modules/.bin/pi").exists() {
        return Some("falta adapters/pi/node_modules".into());
    }
    locate_clio().err()
}

/// A task dir: `workspace/a.txt`, a hidden check that wants `out.txt` to say `hi`.
fn tiny_task(root: &Path, max_steps: u32) {
    let t = root.join("t-01");
    std::fs::create_dir_all(t.join("workspace")).unwrap();
    std::fs::write(t.join("workspace/a.txt"), "hola\n").unwrap();
    std::fs::write(
        t.join("check.py"),
        "import sys\nsys.exit(0 if open('out.txt').read().strip() == 'hi' else 1)\n",
    )
    .unwrap();
    std::fs::write(
        t.join("task.json"),
        format!(
            r#"{{"id":"tiny","input":"escribe hi en out.txt","workspace":"workspace",
"check":{{"kind":"command","value":"python3 {{task_dir}}/check.py"}},
"max_steps":{max_steps},"max_cost_micro_usd":100000,"max_duration_ms":120000,"tags":[]}}"#
        ),
    )
    .unwrap();
}

fn bench(tmp: &Path, base_url: &str) -> AgentBenchConfig {
    let paths = PiPaths::under_data(&tmp.join("data"));
    std::fs::create_dir_all(paths.root.clone()).unwrap();
    std::os::unix::fs::symlink(
        repo().join("adapters/pi"),
        paths.install_dir(&pinned_version().unwrap()),
    )
    .unwrap();
    let mut provider = Provider::opencode_go("fm");
    provider.id = "fakeprov".into();
    provider.base_url = base_url.into();
    let pi = PiBench::new(
        paths,
        provider,
        Secret::new(format!("decoy-{}-{}", "key", "3333")),
        tmp.join("run"),
    )
    .unwrap();
    AgentBenchConfig {
        provider: "fake".into(),
        model: "replay-1".into(),
        system_prompt: String::new(),
        prompt_version: "clio-agent@4".into(),
        program: String::new(),
        args: vec![],
        repeats: 1,
        tags: vec![],
        only: vec![],
        run_root: tmp.join("runs"),
        cleanup: false,
        price_in_micro_per_mtok: 0,
        price_out_micro_per_mtok: 0,
        backoff_ms: vec![],
        pi: Some(pi),
    }
}

fn go(max_steps: u32, script: &str) -> Option<(clio_bench::agent_bench::AgentReport, PathBuf)> {
    if let Some(why) = missing() {
        assert!(
            std::env::var("CLIO_REQUIRE_PI").as_deref() != Ok("1"),
            "CLIO_REQUIRE_PI=1 y {why}"
        );
        eprintln!("SKIPPED pi_bench: {why}");
        return None;
    }
    let tmp = tempfile::tempdir().unwrap();
    // The folder lives on past this function: the caller reads the run folder.
    let root = tmp.keep();
    tiny_task(&root.join("tasks"), max_steps);
    let (_fake, url) = start_fake(&root, script);
    let cfg = bench(&root, &url);
    let tasks = load_tasks(&root.join("tasks")).unwrap();
    let trace = Trace::open(root.join("trace.db"), SystemClock).unwrap();
    trace.set_price("fake", "replay-1", 0, 0, 0).unwrap();
    Some((run_agent(&tasks, &cfg, &trace).unwrap(), root))
}

#[test]
fn pi_writes_the_file_and_the_hidden_check_passes() {
    let script = r#"[{"tool_calls":[{"name":"write","arguments":{"reason":"r","path":"out.txt","content":"hi\n"}}],"usage":[100,5]},{"text":"listo","usage":[120,3]}]"#;
    let Some((report, root)) = go(10, script) else {
        return;
    };
    let d = &report.tasks[0].details[0];
    assert_eq!(
        (report.harness.as_str(), report.successes),
        ("pi", 1),
        "{d:?}"
    );
    assert_eq!((d.steps, d.tool_calls, d.denied), (2, 1, 0), "{d:?}");
    assert_eq!(d.ending, "Settled");
    assert!(d.input_tokens > 0 && d.output_tokens > 0, "{d:?}");
    assert!(d.check_passed);
    assert!(report.table().contains("tiny"));
    assert!(report.detail().contains("tiny #0: ok"));
    // The check never ran in the task's own folder, and the run left a transcript.
    assert!(!root.join("tasks/t-01/workspace/out.txt").exists());
    assert!(root.join("runs/tiny-0/transcript.json").exists());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn pi_is_aborted_when_it_crosses_the_step_limit_and_a_denial_is_counted() {
    // Forever: a write outside the folder (denied) and a read; the last item repeats.
    let script = r#"[{"tool_calls":[{"name":"write","arguments":{"reason":"r","path":".git/hook","content":"x\n"}},{"name":"read","arguments":{"reason":"r","path":"a.txt"}}],"usage":[10,2]}]"#;
    let Some((report, root)) = go(2, script) else {
        return;
    };
    let d = &report.tasks[0].details[0];
    assert_eq!(report.successes, 0, "{d:?}");
    assert!(d.ending.starts_with("Stopped(Steps)"), "{d:?}");
    assert!(d.denied >= 1, "{d:?}");
    let _ = std::fs::remove_dir_all(root);
}
