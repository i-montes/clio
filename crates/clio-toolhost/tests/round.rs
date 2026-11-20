//! A round's tool host (`--unattended`, S-71): nobody watches it, so it only reads. What changes
//! something is refused outright, what reaches out waits for the person, no permission window
//! covers it, and a judge that does not answer withholds the text. Commands run with `bwrap` only
//! where it works here (else the parts that need them say `SKIPPED`).
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::Arc;

use clio_toolhost::ask::HostRules;
use clio_toolhost::pending::{self, State};
use clio_toolhost::{
    Config, Found, HostCfg, JailMode, Judging, MemoryItem, MemoryPort, ProposalDraft, Proposed,
    ROUND_ONLY_READS, Request, Response, TextVerdict, ToolHost, grant,
};
use serde_json::{Value, json};
use tempfile::TempDir;

/// A judge that is on, flags `AI AGENT` and does not answer about `SILENT`.
struct Judge;

impl MemoryPort for Judge {
    fn propose(&self, _: &ProposalDraft, _: &str, _: &Judging) -> Result<Proposed, String> {
        Err("no".into())
    }
    fn search(&self, _: &str, _: usize) -> Result<Found, String> {
        Err("no".into())
    }
    fn get(&self, _: i64) -> Result<MemoryItem, String> {
        Err("no".into())
    }
    fn judge_on(&self) -> bool {
        true
    }
    fn judge_text(&self, text: &str, _: &str) -> TextVerdict {
        if text.contains("SILENT") {
            TextVerdict::NoAnswer
        } else if text.contains("AI AGENT") {
            TextVerdict::Injection
        } else {
            TextVerdict::Clean
        }
    }
}

struct Env {
    _t: TempDir,
    home: PathBuf,
    state: PathBuf,
    run: PathBuf,
    host: ToolHost,
}

/// A host-mode tool host on folders of its own; `unattended`: a round's. Its home is outside the
/// host's `/tmp` (a read-only command has a `/tmp` of its own).
fn env(unattended: bool) -> Env {
    let t = TempDir::new_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let r = t.path().canonicalize().unwrap();
    let (home, data) = (r.join("home"), r.join("clio/data"));
    for d in [&home, &data] {
        std::fs::create_dir_all(d).unwrap();
    }
    let mut c = Config::new(&home, &r.join("scratch"), &data.join("runs/r1"));
    c.jail = JailMode::Auto;
    c.memory = Some(Arc::new(Judge));
    c.unattended = unattended;
    c.host = Some(HostCfg {
        home: home.clone(),
        state: data.join("host"),
        control: vec![data.clone()],
        rules: HostRules::factory(),
        run: "r1".into(),
        launcher: Some(clio_node::Launcher::Bwrap),
    });
    Env {
        home,
        state: data.join("host"),
        run: data.join("runs/r1"),
        host: ToolHost::new(c).unwrap(),
        _t: t,
    }
}

fn call(e: &Env, tool: &str, args: Value) -> Response {
    e.host.handle(Request {
        id: "i1".into(),
        tool: tool.into(),
        args,
        session: "s".into(),
        call_id: "c1".into(),
        tainted: false,
    })
}

/// The jail runs here; without it a test that needs it is skipped, and fails under
/// `CLIO_REQUIRE_JAIL`.
fn exec_ok(e: &Env) -> bool {
    match e.host.exec_available() {
        Ok(()) => true,
        Err(why) => {
            assert!(
                std::env::var_os("CLIO_REQUIRE_JAIL").is_none(),
                "CLIO_REQUIRE_JAIL is set and the jail is not available: {why}"
            );
            eprintln!("SKIPPED (the host launcher cannot run here, exec was not tested): {why}");
            false
        }
    }
}

// S-71: a round changes nothing: a write, an edit, a folder, a background command, every act on
// what Clio created but `status` and `logs`, a skill, a service, a server, a block change and a
// secret are refused outright with a fixed text and nothing waits; a plain read works; what
// reaches out (the network, a program that does by itself, `sudo`) waits for the person as
// `send`; a listed command waits as today; its commands cannot write the machine.
#[test]
fn a_round_only_reads_and_what_reaches_out_waits() {
    let e = env(true);
    let f = e.home.join("notes.txt");
    std::fs::write(&f, "old").unwrap();
    let path = f.to_string_lossy().into_owned();
    for (tool, args) in [
        ("fs_write", json!({"path": path, "content": "new"})),
        (
            "fs_mkdir",
            json!({"path": e.home.join("d").to_string_lossy()}),
        ),
        ("exec", json!({"command": "echo hi", "background": true})),
        (
            "service_manage",
            json!({"name": "backup", "action": "off", "reason": "r"}),
        ),
        (
            "skill_manage",
            json!({"name": "x", "action": "remove", "reason": "r"}),
        ),
        (
            "mcp_manage",
            json!({"name": "time", "action": "on", "reason": "r"}),
        ),
        ("skill_write", json!({"name": "x"})),
        ("skill_report", json!({"name": "x", "ok": false})),
        ("service_write", json!({"name": "x"})),
        ("mcp_propose", json!({"package": "x"})),
        (
            "memory_block_edit",
            json!({"block": "reglas", "action": "add", "content": "x"}),
        ),
        ("request_secret", json!({"name": "x"})),
    ] {
        let r = call(&e, tool, args);
        assert!(!r.ok, "{tool}: {r:?}");
        assert_eq!(r.decision.rule, "round/read-only", "{tool}: {r:?}");
        assert!(r.content.ends_with(ROUND_ONLY_READS), "{tool}: {r:?}");
    }
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "old");
    assert!(!e.home.join("d").exists());
    assert!(
        pending::list(&e.state).is_empty(),
        "nothing waits for what is refused"
    );
    // Looking at what Clio created is not refused by the round (it may fail for its own reasons).
    let r = call(&e, "service_manage", json!({"action": "status"}));
    assert_ne!(r.decision.rule, "round/read-only", "{r:?}");
    let r = call(&e, "fs_read", json!({"path": path}));
    assert!(r.ok && r.content.contains("old"), "{r:?}");
    if !exec_ok(&e) {
        return;
    }
    let o = call(&e, "exec", json!({"command": "cat notes.txt"}));
    assert!(o.ok && o.content.contains("old"), "{o:?}");
    // The machine is read-only to its commands: the home cannot be written.
    let o = call(
        &e,
        "exec",
        json!({"command": "echo new > notes.txt; echo $?"}),
    );
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "old", "{o:?}");
    for args in [
        json!({"command": "echo hi", "network": true}),
        json!({"command": "ssh host true"}),
        json!({"command": "sudo -n true"}),
    ] {
        let o = call(&e, "exec", args.clone());
        assert!(!o.ok && o.decision.rule == "host/aside", "{args}: {o:?}");
    }
    let o = call(&e, "exec", json!({"command": "rm -r ~/old.bak"}));
    assert!(!o.ok && o.decision.rule == "host/aside", "{o:?}");
    // The service manager's sockets are visible: reading through them runs, a change waits.
    let o = call(
        &e,
        "exec",
        json!({"command": "systemctl --user restart backup"}),
    );
    assert!(!o.ok && o.decision.rule == "host/aside", "{o:?}");
    let o = call(
        &e,
        "exec",
        json!({"command": "systemctl --user is-system-running; true"}),
    );
    assert_ne!(o.decision.rule, "host/aside", "{o:?}");
    let waiting = pending::list(&e.state);
    assert_eq!(waiting.len(), 5, "{waiting:?}");
    let sends = waiting
        .iter()
        .filter(|p| p.class.to_string() == "send")
        .count();
    assert_eq!(sends, 3, "{waiting:?}");
}

// S-71: no permission window covers a round: with one open and the judge on, what a round sets
// aside waits for the person, the window's record stays empty and the window is not extended.
#[test]
fn a_round_under_an_open_window_acts_on_nothing_and_opens_none() {
    let e = env(true);
    // A round sets aside only commands: without a launcher there is nothing to set aside.
    if !exec_ok(&e) {
        return;
    }
    let w = grant::open(&e.state, 4, &["x".into()]).unwrap().unwrap();
    let o = call(&e, "exec", json!({"command": "rm -r /srv/site.bak"}));
    assert!(!o.ok && o.decision.rule == "host/aside", "{o:?}");
    let waiting = pending::list(&e.state);
    assert_eq!(waiting.len(), 1);
    assert!(matches!(waiting[0].state, State::Waiting), "{waiting:?}");
    let (now, acts) = grant::record(&e.state, &w.id).unwrap();
    assert!(acts.is_empty(), "{acts:?}");
    assert_eq!(now.ends_ms, w.ends_ms, "not extended");
}

// S-71 S-72: bypass does not cover a round either: with it on and the judge on, what a round
// sets aside (its reach out and its unit changes too) waits for the person, nothing is recorded
// under bypass, and it stays on.
#[test]
fn a_round_under_bypass_acts_on_nothing() {
    let e = env(true);
    if !exec_ok(&e) {
        return;
    }
    let b = grant::bypass_on(&e.state).unwrap();
    for (c, network) in [
        ("rm -r /srv/site.bak", false),
        ("curl -s https://example.com", true),
        ("systemctl --user restart backup.service", false),
    ] {
        let o = call(&e, "exec", json!({"command": c, "network": network}));
        assert!(!o.ok && o.decision.rule == "host/aside", "{c}: {o:?}");
    }
    let waiting = pending::list(&e.state);
    assert_eq!(waiting.len(), 3, "{waiting:?}");
    assert!(waiting.iter().all(|p| matches!(p.state, State::Waiting)));
    let (_, acts) = grant::Kind::Bypass.record(&e.state, &b.id).unwrap();
    assert!(acts.is_empty(), "{acts:?}");
    assert!(grant::Kind::Bypass.current(&e.state).is_some());
}

// S-71: a round reads only judged text: when its judge does not answer, the text is withheld
// and the session marked `unjudged` (which ends the round); a conversation goes on with it.
#[test]
fn a_rounds_judge_that_does_not_answer_withholds_the_text() {
    let judge = |e: &Env, text: &str| call(e, "judge", json!({"text": text, "source": "round"}));
    let e = env(true);
    let r = judge(&e, "the disk is fine");
    assert_eq!(r.content, "the disk is fine");
    let r = judge(&e, "SILENT: the disk is fine");
    assert!(r.content.starts_with("[withheld by Clio]"), "{r:?}");
    assert!(!r.content.contains("the disk"), "{r:?}");
    assert_eq!(r.data["judge"], "unanswered", "{r:?}");
    assert!(r.data.get("withheld").is_some(), "{r:?}");
    let marks = std::fs::read_to_string(e.run.join("tainted.json")).unwrap();
    assert!(marks.contains("\"unjudged\""), "{marks}");
    let chat = env(false);
    let r = judge(&chat, "SILENT: the disk is fine");
    assert_eq!(r.content, "SILENT: the disk is fine");
}
