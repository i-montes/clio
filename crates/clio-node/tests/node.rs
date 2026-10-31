#![allow(clippy::unwrap_used)]

use clio_core::{Effect, FixedClock};
use clio_node::{
    Action, Approval, EffectLog, Error, Executor, LeaseTable, Registry, Sandbox, Tool, basic_tools,
};
use serde_json::json;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tempfile::TempDir;

struct Yes;
impl Approval for Yes {
    fn is_valid_for(&self, _: &str) -> bool {
        true
    }
}
struct Only(String);
impl Approval for Only {
    fn is_valid_for(&self, h: &str) -> bool {
        self.0 == h
    }
}
struct No;
impl Approval for No {
    fn is_valid_for(&self, _: &str) -> bool {
        false
    }
}

fn fixture() -> (TempDir, Sandbox) {
    let t = TempDir::new().unwrap();
    fs::create_dir(t.path().join("root")).unwrap();
    fs::create_dir(t.path().join("outside")).unwrap();
    let s = Sandbox::new(&t.path().join("root"), &t.path().join("scratch")).unwrap();
    (t, s)
}

fn executor(t: &TempDir, s: Sandbox) -> Executor {
    Executor {
        registry: Registry::basic().unwrap(),
        sandbox: s,
        log: EffectLog::open(&t.path().join("effects.jsonl")).unwrap(),
    }
}

fn act(id: &str, tool: &str, args: serde_json::Value) -> Action {
    Action {
        id: id.into(),
        tool: tool.into(),
        args,
    }
}

/// Every file and directory under `dir` with its content, to compare two states exactly.
fn state(dir: &Path) -> Vec<(String, Option<Vec<u8>>)> {
    fn go(base: &Path, d: &Path, out: &mut Vec<(String, Option<Vec<u8>>)>) {
        let mut es: Vec<_> = fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        es.sort();
        for p in es {
            let rel = p.strip_prefix(base).unwrap().display().to_string();
            if p.is_dir() {
                out.push((rel, None));
                go(base, &p, out);
            } else {
                out.push((rel, Some(fs::read(&p).unwrap())));
            }
        }
    }
    let mut v = Vec::new();
    go(dir, dir, &mut v);
    v
}

// S-13: paths with `..`, absolute paths and symlinks pointing out are rejected.
#[test]
fn s13_paths_that_leave_the_root_are_rejected() {
    let (t, s) = fixture();
    let out = t.path().join("outside");
    fs::write(out.join("secret.txt"), "x").unwrap();
    let root = t.path().join("root");
    symlink(&out, root.join("dirlink")).unwrap();
    symlink(out.join("secret.txt"), root.join("filelink")).unwrap();
    symlink(out.join("not-yet"), root.join("dangling")).unwrap();
    symlink(&root, root.join("inside")).unwrap(); // pointing in is fine
    for bad in [
        "../outside/secret.txt",
        "a/../../outside",
        out.join("secret.txt").to_str().unwrap(),
        "dirlink/secret.txt",
        "dirlink/new.txt",
        "filelink",
        "dangling",
    ] {
        assert!(matches!(s.read(bad), Err(Error::Escape(_))), "{bad}");
        assert!(
            matches!(s.write(bad, b"pwn"), Err(Error::Escape(_))),
            "{bad}"
        );
    }
    assert!(!out.join("not-yet").exists() && fs::read(out.join("secret.txt")).unwrap() == b"x");
    s.write("ok/file.txt", b"hi").unwrap();
    assert_eq!(s.read("inside/ok/file.txt").unwrap(), b"hi");
}

#[test]
fn t222_everything_written_stays_inside_the_sandbox() {
    let (t, s) = fixture();
    s.write("a/b/c.txt", b"1").unwrap();
    s.mkdir("d/e").unwrap();
    assert_eq!(s.read("a/b/c.txt").unwrap(), b"1");
    assert_eq!(s.list("a/b").unwrap(), vec!["c.txt"]);
    assert!(state(&t.path().join("outside")).is_empty());
    assert!(t.path().join("root/a/b/c.txt").exists());
}

fn fill(s: &Sandbox, n: usize) {
    for i in 0..n {
        s.write(&format!("d{}/f{i}.txt", i % 50), format!("v{i}").as_bytes())
            .unwrap();
    }
}

#[test]
fn t223_restore_removes_new_recovers_deleted_and_modified() {
    let (t, s) = fixture();
    let root = t.path().join("root");
    s.write("keep.txt", b"keep").unwrap();
    s.write("mod.txt", b"before").unwrap();
    s.write("gone/deep.txt", b"deep").unwrap();
    symlink("keep.txt", root.join("link")).unwrap();
    let before = state(&root);
    let snap = s.snapshot().unwrap();
    s.write("mod.txt", b"after").unwrap();
    s.write("new/x.txt", b"n").unwrap();
    s.write("newtop.txt", b"n").unwrap();
    fs::remove_dir_all(root.join("gone")).unwrap();
    fs::remove_file(root.join("link")).unwrap();
    s.restore(&snap).unwrap();
    assert_eq!(state(&root), before);
    assert_eq!(
        fs::read_link(root.join("link")).unwrap(),
        Path::new("keep.txt")
    );
}

#[test]
fn t224_cancel_restores_exactly_and_takes_seconds() {
    let (t, s) = fixture();
    let root = t.path().join("root");
    fill(&s, 5000);
    let before = state(&root);
    let snap = s.snapshot().unwrap();
    for i in 0..5000 {
        s.write(&format!("d{}/f{i}.txt", i % 50), b"changed")
            .unwrap();
    }
    for i in 0..500 {
        s.write(&format!("extra/n{i}.txt"), b"new").unwrap();
    }
    fs::remove_dir_all(root.join("d3")).unwrap();
    let start = Instant::now();
    s.restore(&snap).unwrap();
    let took = start.elapsed();
    eprintln!("restore of 5000 files: {took:?}");
    assert_eq!(state(&root), before);
    assert!(took.as_secs() < 5, "{took:?}");
}

#[test]
fn t225_a_snapshot_costs_no_copy_when_hard_links_work() {
    use std::os::unix::fs::MetadataExt;
    let (t, s) = fixture();
    s.write("big.bin", &vec![7u8; 1 << 20]).unwrap();
    let snap = s.snapshot().unwrap();
    let nlink = fs::metadata(t.path().join("root/big.bin")).unwrap().nlink();
    assert_eq!(nlink, 2); // same inode as the snapshot copy
    snap.discard();
}

// S-56: before day zero only the sandbox is reachable; nothing outside it can be touched.
#[test]
fn t02_an_action_outside_the_sandbox_is_denied() {
    let (t, s) = fixture();
    let ex = executor(&t, s);
    let out = t.path().join("outside/x.txt");
    for p in [out.to_str().unwrap(), "../outside/x.txt"] {
        let a = act("1", "write_file", json!({"path": p, "content": "no"}));
        assert!(matches!(ex.execute(&a, &Yes), Err(Error::Escape(_))));
    }
    assert!(!out.exists());
}

#[test]
fn t226_the_same_action_applies_once_and_returns_the_same_result() {
    let (t, s) = fixture();
    let ex = executor(&t, s);
    let a = act(
        "a1",
        "write_file",
        json!({"path": "f.txt", "content": "one"}),
    );
    let first = ex.execute(&a, &Yes).unwrap();
    // The file is changed behind the log's back: a second execution must not rewrite it.
    fs::write(t.path().join("root/f.txt"), "other").unwrap();
    let second = ex.execute(&a, &Yes).unwrap();
    assert!(first.applied && !second.applied);
    assert_eq!(first.result, second.result);
    assert_eq!(
        fs::read_to_string(t.path().join("root/f.txt")).unwrap(),
        "other"
    );
    // same id, different action: refused
    let b = act(
        "a1",
        "write_file",
        json!({"path": "f.txt", "content": "two"}),
    );
    assert!(matches!(ex.execute(&b, &Yes), Err(Error::IdReused(_))));
}

#[test]
fn t226_the_effect_log_survives_a_restart_and_a_torn_line() {
    let (t, s) = fixture();
    let a = act("p1", "make_dir", json!({"path": "made"}));
    executor(&t, s).execute(&a, &Yes).unwrap();
    let log = t.path().join("effects.jsonl");
    let mut text = fs::read_to_string(&log).unwrap();
    assert_eq!(text.lines().count(), 1);
    text.push_str("{\"id\":\"half"); // a crash in the middle of an append
    fs::write(&log, text).unwrap();
    let s2 = Sandbox::new(&t.path().join("root"), &t.path().join("scratch2")).unwrap();
    let ex = executor(&t, s2);
    fs::remove_dir(t.path().join("root/made")).unwrap();
    let again = ex.execute(&a, &Yes).unwrap();
    assert!(!again.applied);
    assert!(!t.path().join("root/made").exists());
}

#[test]
fn t228_a_second_holder_is_rejected_until_release_or_expiry() {
    let clock = Arc::new(FixedClock::new(1_000));
    let leases = LeaseTable::new(clock.clone());
    leases.acquire("repo:clio", "task-1", 500).unwrap();
    assert!(matches!(
        leases.acquire("repo:clio", "task-2", 500),
        Err(Error::Held { .. })
    ));
    leases.acquire("path:/other", "task-2", 500).unwrap(); // another resource is free
    leases.acquire("repo:clio", "task-1", 500).unwrap(); // the owner renews
    clock.advance(499);
    assert!(leases.acquire("repo:clio", "task-2", 500).is_err());
    clock.advance(1); // renewed at 1000 for 500 ms: expires at 1500
    assert!(leases.acquire("repo:clio", "task-2", 500).is_ok());
    assert!(!leases.release("repo:clio", "task-1")); // not the owner any more
    assert!(leases.release("repo:clio", "task-2"));
    assert!(leases.acquire("repo:clio", "task-3", 10).is_ok());
}

// S-02: a tool with no declared effect cannot be registered, so it cannot be called.
#[test]
fn s02_a_tool_without_a_declared_effect_is_refused() {
    let mut r = Registry::new();
    let mut t = basic_tools().remove(0);
    t.effect = None;
    assert!(matches!(r.register(t), Err(Error::NoEffect(_))));
    assert!(r.get("read_file").is_none());
    let (tmp, s) = fixture();
    let ex = Executor {
        registry: r,
        ..executor(&tmp, s)
    };
    let a = act("x", "read_file", json!({"path": "."}));
    assert!(matches!(ex.execute(&a, &Yes), Err(Error::UnknownTool(_))));
}

// S-12: no shell or command tool exists; anything above sandbox-reversible is refused.
// S-56: only read and sandbox-reversible tools in this phase.
#[test]
fn s56_tools_with_external_or_irreversible_effects_are_refused() {
    let mut r = Registry::new();
    for e in [Effect::ExternalWrite, Effect::Irreversible] {
        let mut t = basic_tools().remove(0);
        t.name = format!("t-{e:?}");
        t.effect = Some(e);
        assert!(matches!(r.register(t), Err(Error::PhaseLimit(..))));
    }
    assert!(r.visible().is_empty());
}

#[test]
fn t229_basic_tools_declare_their_effect() {
    let r = Registry::basic().unwrap();
    let eff = |n: &str| r.get(n).unwrap().tool.effect;
    assert_eq!(eff("read_file"), Some(Effect::Read));
    assert_eq!(eff("list_dir"), Some(Effect::Read));
    assert_eq!(eff("write_file"), Some(Effect::LocalReversible));
    assert_eq!(eff("make_dir"), Some(Effect::LocalReversible));
    let (t, s) = fixture();
    let ex = Executor {
        registry: r,
        ..executor(&t, s)
    };
    ex.execute(
        &act(
            "1",
            "write_file",
            json!({"path": "d/a.txt", "content": "hola"}),
        ),
        &Yes,
    )
    .unwrap();
    let read = ex
        .execute(&act("2", "read_file", json!({"path": "d/a.txt"})), &Yes)
        .unwrap();
    assert_eq!(read.result["content"], "hola");
    let ls = ex
        .execute(&act("3", "list_dir", json!({"path": "d"})), &Yes)
        .unwrap();
    assert_eq!(ls.result["entries"], json!(["a.txt"]));
}

#[test]
fn t230_few_tools_are_visible_and_the_rest_are_found_by_keyword() {
    let r = Registry::basic().unwrap();
    let visible: Vec<_> = r.visible().iter().map(|t| t.name.clone()).collect();
    assert_eq!(visible, vec!["list_dir", "read_file", "write_file"]);
    assert_eq!(r.search("mkdir")[0].name, "make_dir");
    assert_eq!(r.search("create a folder")[0].name, "make_dir");
    assert!(r.search("send email").is_empty());
}

#[test]
fn t231_a_not_recommended_tool_is_flagged_and_still_needs_approval() {
    let (t, s) = fixture();
    let ex = executor(&t, s);
    let a = act("m1", "make_dir", json!({"path": "x"}));
    assert!(!ex.registry.get("make_dir").unwrap().recommended);
    assert!(matches!(ex.execute(&a, &No), Err(Error::NotApproved)));
    let o = ex.execute(&a, &Yes).unwrap();
    assert!(!o.recommended && o.applied);
}

#[test]
fn t227_without_a_valid_approval_nothing_happens() {
    let (t, s) = fixture();
    let ex = executor(&t, s);
    let a = act("w1", "write_file", json!({"path": "f.txt", "content": "x"}));
    assert!(matches!(ex.execute(&a, &No), Err(Error::NotApproved)));
    // approval of another action (different args) does not cover this one
    let other = act("w1", "write_file", json!({"path": "f.txt", "content": "y"}));
    assert!(matches!(
        ex.execute(&a, &Only(other.hash())),
        Err(Error::NotApproved)
    ));
    assert!(!t.path().join("root/f.txt").exists());
    // and nothing was recorded: the right approval still applies it
    assert!(ex.execute(&a, &Only(a.hash())).unwrap().applied);
}

#[test]
fn t227_the_hash_ignores_key_order_but_not_content() {
    let a = act("i", "t", json!({"a": 1, "b": {"c": 2, "d": [1, 2]}}));
    let b = act("i", "t", json!({"b": {"d": [1, 2], "c": 2}, "a": 1}));
    assert_eq!(a.hash(), b.hash());
    let c = act("i", "t", json!({"a": 1, "b": {"c": 3, "d": [1, 2]}}));
    assert_ne!(a.hash(), c.hash());
    assert_eq!(a.hash().len(), 64);
}

#[test]
fn a_tool_can_be_registered_once() {
    let mut r = Registry::new();
    let mk = || -> Tool { basic_tools().remove(0) };
    r.register(mk()).unwrap();
    assert!(matches!(r.register(mk()), Err(Error::Duplicate(_))));
}
