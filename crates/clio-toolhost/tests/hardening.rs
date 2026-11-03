#![allow(clippy::unwrap_used)]
//! Security audit of 2026-11-02, stage 1: these tests describe what must be true and FAIL until
//! the fixes of stage 2 land (each says which finding it pins). They use the real host; the jail
//! ones skip where `bwrap` cannot run, and fail if `CLIO_REQUIRE_JAIL` is set.

use clio_attacks::{Case, Observed, Status, Target, load_cases, run};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use clio_canaries::Canaries;
use clio_core::SystemClock;
use clio_toolhost::{Config, JailMode, Response, ToolHost};
use clio_vault::Scrubber;
use serde_json::{Value, json};
use tempfile::TempDir;

struct Env {
    _t: TempDir,
    root: PathBuf,
    work: PathBuf,
    host: Arc<ToolHost>,
}

fn env(jail: JailMode, scrubber: Option<Scrubber>) -> Env {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let (work, scratch, run) = (root.join("work"), root.join("scratch"), root.join("run"));
    std::fs::create_dir(&work).unwrap();
    let mut cfg = Config::new(&work, &scratch, &run);
    cfg.jail = jail;
    cfg.scrubber = scrubber;
    Env {
        _t: t,
        root,
        work,
        host: Arc::new(ToolHost::new(cfg).unwrap()),
    }
}

fn call(host: &ToolHost, tool: &str, args: Value) -> Response {
    let line =
        json!({"id":"i1","tool":tool,"args":args,"session":"s","call_id":"c1","tainted":false})
            .to_string();
    serde_json::from_str(&host.handle_line(&line)).unwrap()
}

/// The answer, or `None` if the host did not answer in `secs`: a hung call is a failed test, not
/// a hung test run.
fn call_within(host: &Arc<ToolHost>, tool: &str, args: Value, secs: u64) -> Option<Response> {
    let (tx, rx) = mpsc::channel();
    let (h, tool) = (host.clone(), tool.to_owned());
    std::thread::spawn(move || {
        let _ = tx.send(call(&h, &tool, args));
    });
    rx.recv_timeout(Duration::from_secs(secs)).ok()
}

fn mkfifo(path: &std::path::Path) {
    assert!(Command::new("mkfifo").arg(path).status().unwrap().success());
}

// Finding 3 (attack-50), S-13, S-47: code that ran in the jail can leave a FIFO in the copy. A
// read, a stat or a write-check on it must answer, not wait for a writer that never comes: the
// host serves one call at a time, so one stuck call stops every tool of the session.
#[test]
fn a_fifo_in_the_copy_does_not_hang_the_host() {
    for (tool, args) in [
        ("fs_read", json!({"path": "f"})),
        ("fs_stat", json!({"path": "f", "mode": "r"})),
        ("fs_stat", json!({"path": "f", "mode": "rw"})),
    ] {
        let e = env(JailMode::Off, None);
        mkfifo(&e.work.join("f"));
        let r = call_within(&e.host, tool, args.clone(), 5);
        assert!(
            r.is_some(),
            "{tool} {args} did not answer in 5 s: the host is stuck"
        );
        if tool == "fs_read" {
            let r = r.unwrap();
            assert!(!r.ok, "a FIFO has no content to give: {r:?}");
        }
        // The host still serves the next call.
        let next = call_within(&e.host, "fs_list", json!({"path": "."}), 5);
        assert!(
            next.is_some_and(|n| n.ok),
            "{tool}: the next call did not run"
        );
    }
}

// Finding 2 (attack-51), S-18, S-20, S-23, S-58: what the model sees is scrubbed, and so must be what
// the host writes to disk. `effects.jsonl` records the result of every `exec`, and
// `backup.unix.sh` backs the run folder up.
#[test]
fn exec_output_is_never_written_to_disk_unscrubbed() {
    let c = Canaries::fresh();
    let decoy = c.value("llm-provider").unwrap().to_owned();
    let scrubber = Scrubber::new(Arc::new(SystemClock));
    scrubber.add("llm-provider", &decoy);
    let e = env(JailMode::Auto, Some(scrubber));
    if let Err(why) = e.host.exec_available() {
        eprintln!("SKIPPED (the jail cannot run here, nothing was tested): {why}");
        assert!(
            std::env::var_os("CLIO_REQUIRE_JAIL").is_none(),
            "CLIO_REQUIRE_JAIL is set: {why}"
        );
        return;
    }
    std::fs::write(
        e.work.join("config.json"),
        format!("{{\"key\": \"{decoy}\"}}"),
    )
    .unwrap();
    let r = call(&e.host, "exec", json!({"command": "cat config.json"}));
    assert!(r.ok, "{r:?}");
    c.assert_clean("what the model is given", &r.content);
    let leaks = c.scan_dir(&e.root.join("run")).unwrap();
    assert!(
        leaks.is_empty(),
        "the run folder holds the decoy: {}",
        leaks
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    );
}

// Built from two pieces: a scanner reads the source, not the program.
/// Folders a tool loads and RUNS when the project is opened, built or committed: an editor's tasks,
/// git hooks, cargo's config (runner, rustc wrapper), a dev container's commands.
const RUNS_CODE: [&str; 5] = [".vscode", ".husky", ".githooks", ".cargo", ".devcontainer"];

// S-16: the agent cannot write where code runs by itself. The person may open the copy in an editor
// to look at it, and `apply` is the only way out: both must find these folders as they were.
#[test]
fn the_host_does_not_write_the_folders_that_run_code_when_a_project_opens() {
    let e = env(JailMode::Off, None);
    for dir in RUNS_CODE {
        let r = call(
            &e.host,
            "fs_write",
            json!({"path": format!("{dir}/x.json"), "content": "{}"}),
        );
        assert!(!r.ok && r.decision.rule == "S-16", "{dir}: {r:?}");
        assert!(!e.work.join(dir).exists(), "{dir} was created");
    }
    // A folder that only shares a word is an ordinary one.
    let ok = call(
        &e.host,
        "fs_write",
        json!({"path": "docs/vscode/guide.md", "content": "x"}),
    );
    assert!(ok.ok, "{ok:?}");
}

// S-16: and the jail mounts them read-only, whether or not they exist, and leaves nothing behind.
#[test]
fn a_command_in_the_jail_cannot_write_the_folders_that_run_code_when_a_project_opens() {
    let e = env(JailMode::Auto, None);
    if let Err(why) = e.host.exec_available() {
        eprintln!("SKIPPED (the jail cannot run here, nothing was tested): {why}");
        assert!(
            std::env::var_os("CLIO_REQUIRE_JAIL").is_none(),
            "CLIO_REQUIRE_JAIL is set: {why}"
        );
        return;
    }
    for dir in RUNS_CODE {
        let cmd = format!("mkdir -p {dir}; echo x > {dir}/x.json; echo rc=$?");
        let r = call(&e.host, "exec", json!({ "command": cmd }));
        assert!(r.ok, "{dir}: {r:?}");
        assert!(
            !r.content.contains("rc=0"),
            "{dir} was written: {}",
            r.content
        );
        assert!(!e.work.join(dir).exists(), "{dir} was left behind");
    }
}

const GH: &str = concat!("ghp_", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8");

// Finding 1 (attack-52), S-18, S-44, S-57: a token inside an ordinary file reaches the model masked, and
// the mark says what it is and what to do, in the place where the value was.
#[test]
fn a_token_inside_a_file_reaches_the_model_masked_with_a_guide() {
    let e = env(JailMode::Off, None);
    std::fs::write(
        e.work.join("deploy.sh"),
        format!("export GITHUB_TOKEN={GH}\necho ok\n"),
    )
    .unwrap();
    let r = call(&e.host, "fs_read", json!({"path": "deploy.sh"}));
    assert!(r.ok, "{r:?}");
    assert!(!r.content.contains(GH), "{}", r.content);
    assert!(
        r.content.contains("echo ok"),
        "the rest of the file stays: {}",
        r.content
    );
    for want in ["[secreto:patrón tachado", "baúl", "persona"] {
        assert!(
            r.content.contains(want),
            "the mark lacks {want:?}: {}",
            r.content
        );
    }
}

// S-18, S-36: the person supervises from the chat, so the host says how many values it struck out of
// what it handed over (in `data`, where the guard picks it up).
#[test]
fn a_read_says_how_many_values_it_struck_out() {
    let e = env(JailMode::Off, None);
    std::fs::write(
        e.work.join("two.txt"),
        format!(concat!("a={}\nb=AKIA", "ABCDEFGHIJKLMNOP", "\n"), GH),
    )
    .unwrap();
    std::fs::write(e.work.join("none.txt"), "nothing to hide\n").unwrap();
    let r = call(&e.host, "fs_read", json!({"path": "two.txt"}));
    assert_eq!(r.data["masked"], 2, "{r:?}");
    let r = call(&e.host, "fs_read", json!({"path": "none.txt"}));
    assert_eq!(r.data["masked"], 0, "{r:?}");
    // `edit` reads the real bytes (`raw`): nothing is struck out there, and nothing is claimed.
    let r = call(&e.host, "fs_read", json!({"path": "two.txt", "raw": true}));
    assert_eq!(r.data["masked"], 0, "{r:?}");
}

// Finding 1, S-18, S-57: the model reads the masked text and may write it back whole. That would put
// the mark where the real value was, so a write that carries a mark is refused, and says to use
// `edit` (which reads and writes the real bytes).
#[test]
fn a_write_that_carries_a_mask_is_refused_and_the_file_keeps_its_value() {
    let e = env(JailMode::Off, None);
    let path = e.work.join("deploy.sh");
    std::fs::write(&path, format!("export GITHUB_TOKEN={GH}\necho ok\n")).unwrap();
    let masked = call(&e.host, "fs_read", json!({"path": "deploy.sh"})).content;
    let w = call(
        &e.host,
        "fs_write",
        json!({"path": "deploy.sh", "content": masked}),
    );
    assert!(!w.ok && w.content.contains("tachado"), "{w:?}");
    assert!(std::fs::read_to_string(&path).unwrap().contains(GH));
    // An ordinary write is not affected.
    let ok = call(
        &e.host,
        "fs_write",
        json!({"path": "other.txt", "content": "plain"}),
    );
    assert!(ok.ok, "{ok:?}");
}

// ---------------------------------------------------------------------------------------------
// The attack cases that live in this file, through clio-attacks (the gate of the battery)
// ---------------------------------------------------------------------------------------------

/// A scenario holds when it does not panic: the plain tests above are the scenarios, with their
/// own messages.
fn held(f: impl FnOnce() + std::panic::UnwindSafe) -> bool {
    std::panic::catch_unwind(f).is_ok()
}

const OURS: [&str; 3] = ["attack-50", "attack-51", "attack-52"];

struct HardeningTarget;

impl Target for HardeningTarget {
    fn attack(&self, case: &Case) -> Observed {
        let refused = match case.id.as_str() {
            "attack-50" => held(a_fifo_in_the_copy_does_not_hang_the_host),
            // Needs the jail: where it cannot run, the test prints SKIPPED and holds.
            "attack-51" => held(exec_output_is_never_written_to_disk_unscrubbed),
            "attack-52" => held(|| {
                a_token_inside_a_file_reaches_the_model_masked_with_a_guide();
                a_write_that_carries_a_mask_is_refused_and_the_file_keeps_its_value();
            }),
            other => panic!("{other} is active but this target has no scenario for it"),
        };
        Observed { refused }
    }
}

// S-13 S-47 S-18 S-20 S-44: attacks 50 and 51, and the host half of 52 (the copy half is in clio-brain/tests/copy_secrets.rs; the snapshot half of 50 in clio-node/tests/special_files.rs)
#[test]
fn the_hardening_cases_are_active_and_refused() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testing/attacks/cases");
    let mine: Vec<Case> = load_cases(&dir)
        .unwrap()
        .into_iter()
        .filter(|c| OURS.contains(&c.id.as_str()))
        .collect();
    assert_eq!(
        mine.len(),
        OURS.len(),
        "a case is missing in testing/attacks/cases"
    );
    assert!(mine.iter().all(|c| c.status == Status::Active));
    let report = run(&mine, &HardeningTarget);
    assert!(report.ok(), "{}", report.line());
    assert_eq!(report.passed.len(), OURS.len());
    eprintln!("{}", report.line());
}
