//! The tool host on its own (no Pi): what each layer does with a hostile request, over the
//! real socket where the socket matters.
#![allow(clippy::unwrap_used)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use clio_canaries::Canaries;
use clio_core::SystemClock;
use clio_toolhost::{Config, JailMode, Response, Server, ToolHost};
use clio_vault::{Scrubber, VaultClient};
use serde_json::{Value, json};
use tempfile::TempDir;

struct Env {
    _t: TempDir,
    work: std::path::PathBuf,
    host: ToolHost,
}

fn env_with(f: impl FnOnce(&mut Config)) -> Env {
    let t = tempfile::tempdir().unwrap();
    let r = t.path().canonicalize().unwrap();
    let (work, scratch, run) = (r.join("work"), r.join("scratch"), r.join("run"));
    std::fs::create_dir(&work).unwrap();
    let mut cfg = Config::new(&work, &scratch, &run);
    cfg.jail = JailMode::Off;
    f(&mut cfg);
    let host = ToolHost::new(cfg).unwrap();
    Env { _t: t, work, host }
}

fn env() -> Env {
    env_with(|_| {})
}

fn call(e: &Env, session: &str, tool: &str, args: Value) -> Response {
    call_t(e, session, tool, args, false)
}

fn call_t(e: &Env, session: &str, tool: &str, args: Value, tainted: bool) -> Response {
    let line = json!({"id":"i1","tool":tool,"args":args,"session":session,"call_id":"c1","tainted":tainted}).to_string();
    serde_json::from_str(&e.host.handle_line(&line)).unwrap()
}

#[test]
fn reads_are_external_and_taint_and_writes_are_not() {
    let e = env();
    std::fs::write(e.work.join("a.txt"), "hello").unwrap();
    let w = call(
        &e,
        "s1",
        "fs_write",
        json!({"path":"b/c.txt","content":"x"}),
    );
    assert!(w.ok && !w.external && !w.tainted, "{w:?}");
    assert_eq!(
        std::fs::read_to_string(e.work.join("b/c.txt")).unwrap(),
        "x"
    );
    assert_eq!(w.decision.outcome, "allow");
    // A stat says nothing about content: it does not taint either.
    let st = call(&e, "s1", "fs_stat", json!({"path":"a.txt"}));
    assert!(st.ok && !st.tainted && st.data["exists"] == true && st.data["is_dir"] == false);
    let r = call(&e, "s1", "fs_read", json!({"path":"a.txt"}));
    assert!(
        r.ok && r.external && r.tainted && r.content == "hello",
        "{r:?}"
    );
    // The mark does not go away, and is reported on every later answer, even a plain write.
    let w2 = call(&e, "s1", "fs_write", json!({"path":"d.txt","content":"y"}));
    assert!(w2.ok && w2.tainted && !w2.external);
    // Another session is not marked.
    assert!(!call(&e, "s2", "fs_mkdir", json!({"path":"z"})).tainted);
    // The caller says "tainted" (restored from Pi's session): it raises the mark, and a later
    // "not tainted" does not clear it.
    assert!(call_t(&e, "s3", "fs_mkdir", json!({"path":"y"}), true).tainted);
    assert!(call_t(&e, "s3", "fs_mkdir", json!({"path":"y2"}), false).tainted);
}

#[test]
fn list_and_glob_do_not_follow_links_and_are_external() {
    let e = env();
    std::fs::create_dir_all(e.work.join("src/deep")).unwrap();
    std::fs::write(e.work.join("src/a.py"), "").unwrap();
    std::fs::write(e.work.join("src/deep/b.py"), "").unwrap();
    std::fs::write(e.work.join("c.txt"), "").unwrap();
    std::os::unix::fs::symlink("/etc", e.work.join("etc_link")).unwrap();
    let l = call(&e, "s", "fs_list", json!({"path":"."}));
    assert!(l.ok && l.external && l.tainted, "{l:?}");
    assert_eq!(l.data["entries"], json!(["c.txt", "etc_link", "src"]));
    let g = call(
        &e,
        "s",
        "fs_glob",
        json!({"path":".","pattern":"**/*.py","limit":100}),
    );
    assert_eq!(
        g.data["paths"],
        json!(["src/a.py", "src/deep/b.py"]),
        "{g:?}"
    );
    let g = call(
        &e,
        "s",
        "fs_glob",
        json!({"path":".","pattern":"*.py","limit":1}),
    );
    assert_eq!(g.data["paths"], json!(["src/a.py"]));
    assert!(
        !call(&e, "s", "fs_list", json!({"path":"etc_link"})).ok,
        "a link out is not followed"
    );
    assert!(
        !call(
            &e,
            "s",
            "fs_glob",
            json!({"path":"etc_link","pattern":"*","limit":5})
        )
        .ok
    );
}

#[test]
fn write_replaces_atomically_and_leaves_no_temporary_file() {
    let e = env();
    call(&e, "s", "fs_write", json!({"path":"f.txt","content":"one"}));
    call(&e, "s", "fs_write", json!({"path":"f.txt","content":"two"}));
    assert_eq!(
        std::fs::read_to_string(e.work.join("f.txt")).unwrap(),
        "two"
    );
    let names: Vec<_> = std::fs::read_dir(&e.work)
        .unwrap()
        .map(|x| x.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
}

#[test]
fn the_gateway_denies_by_default_and_each_deny_has_its_rule() {
    let e = env();
    std::fs::create_dir(e.work.join("extensions")).unwrap();
    std::os::unix::fs::symlink(e.work.join("extensions"), e.work.join("ln")).unwrap();
    std::os::unix::fs::symlink("/etc", e.work.join("etc")).unwrap();
    let cases = [
        ("bash", json!({"command":"id"}), "S-02"),
        ("read_file", json!({"path":"a"}), "S-02"),
        ("exec", json!({"command":"id"}), "S-02"), // no proven jail here: not declared
        ("fs_read", json!({"path":"../../etc/passwd"}), "S-13"),
        ("fs_read", json!({"path":"/etc/passwd"}), "S-13"),
        ("fs_read", json!({"path":"etc/passwd"}), "S-13"),
        ("fs_read", json!({"path":"~/.ssh/id_rsa"}), "S-13"),
        ("fs_stat", json!({"path":"../x"}), "S-13"),
        (
            "fs_write",
            json!({"path":"extensions/x.ts","content":"c"}),
            "S-16",
        ),
        (
            "fs_write",
            json!({"path":".pi/extensions/x.ts","content":"c"}),
            "S-16",
        ),
        ("fs_write", json!({"path":"ln/x.ts","content":"c"}), "S-16"),
        (
            "fs_write",
            json!({"path":".git/hooks/p","content":"c"}),
            "S-16",
        ),
        (
            "fs_write",
            json!({"path":".agents/skills/s","content":"c"}),
            "S-16",
        ),
        (
            "fs_write",
            json!({"path":"skills/s/SKILL.md","content":"c"}),
            "S-16",
        ),
        ("fs_mkdir", json!({"path":".claude"}), "S-16"),
        (
            "fs_write",
            json!({"path":"a.txt","content":"c","mode":"777"}),
            "step-1-schema",
        ),
        ("fs_write", json!({"path":"a.txt"}), "step-1-schema"),
        ("fs_read", json!({"path":5}), "step-1-schema"),
        ("fs_glob", json!({"path":"."}), "step-1-schema"),
    ];
    for (i, (tool, args, rule)) in cases.into_iter().enumerate() {
        // One session each: the breaker (tested apart) would open after ten denials in a row.
        let r = call(&e, &format!("deny{i}"), tool, args.clone());
        assert!(!r.ok, "{tool} {args}: {r:?}");
        assert_eq!(
            (r.decision.outcome.as_str(), r.decision.rule.as_str()),
            ("deny", rule),
            "{tool} {args}"
        );
        assert!(r.content.starts_with("[gateway]"), "{}", r.content);
    }
    assert!(
        !e.work.join("extensions/x.ts").exists()
            && !e.work.join(".pi").exists()
            && !e.work.join(".git").exists()
    );
}

#[test]
fn exec_is_not_offered_without_a_proven_jail() {
    let e = env();
    assert!(e.host.exec_available().is_err());
    let r = call(&e, "s", "exec", json!({"command":"echo hi"}));
    assert_eq!((r.ok, r.decision.rule.as_str()), (false, "S-02"));
}

#[test]
fn exec_runs_in_the_jail_when_it_can_and_has_its_own_rule() {
    let e = env_with(|c| c.jail = JailMode::Auto);
    if let Err(why) = e.host.exec_available() {
        eprintln!("SKIPPED (the jail cannot run here, nothing was tested): {why}");
        assert!(
            std::env::var_os("CLIO_REQUIRE_JAIL").is_none(),
            "CLIO_REQUIRE_JAIL is set: {why}"
        );
        return;
    }
    let r = call(
        &e,
        "s",
        "exec",
        json!({"command":"echo hello; echo oops >&2; exit 3"}),
    );
    assert!(r.ok && r.external && r.tainted, "{r:?}");
    assert!(
        r.content.contains("hello") && r.content.contains("oops"),
        "{}",
        r.content
    );
    assert_eq!(r.data["exit_code"], 3);
    // The jail's own view: no HOME of the person, nothing outside /work is writable.
    let r = call(
        &e,
        "s",
        "exec",
        json!({"command":"pwd; echo x > /usr/x 2>&1; echo $?; ls /home 2>&1 | wc -l"}),
    );
    assert!(r.content.starts_with("/work"), "{}", r.content);
    assert!(!std::path::Path::new("/usr/x").exists());
    // The shape rule: empty, NUL, too long.
    for bad in [
        json!({"command":"   "}),
        json!({"command":"echo \u{0}"}),
        json!({"command":"a".repeat(9000)}),
    ] {
        let r = call(&e, "s", "exec", bad);
        assert_eq!(
            (r.ok, r.decision.rule.as_str()),
            (false, "S-12/bash"),
            "{r:?}"
        );
    }
    // Protected folders: read-only inside the jail even when they do not exist; nothing is left.
    let r = call(
        &e,
        "s",
        "exec",
        json!({"command":"mkdir -p .pi/extensions && echo x > .pi/extensions/x.ts"}),
    );
    assert!(r.data["exit_code"].as_i64().unwrap_or(0) != 0, "{r:?}");
    assert!(
        !e.work.join(".pi").exists(),
        "no protected folder is left behind"
    );
    // A timeout is reported, not hidden.
    let r = call(&e, "s", "exec", json!({"command":"sleep 30","timeout":1}));
    assert_eq!(r.data["timed_out"], true, "{r:?}");
}

#[test]
fn raw_reads_keep_the_bytes_and_clean_ones_do_not_and_filter_cleans_and_wraps() {
    let canaries = Canaries::fresh();
    let secret = canaries.value("notion").unwrap().to_owned();
    let scrubber = Scrubber::new(Arc::new(SystemClock));
    scrubber.add("notion", &secret);
    let e = env_with(|c| c.scrubber = Some(scrubber));
    let (a, b) = secret.split_at(10);
    let body = format!("{secret}\n{a}\u{200b}{b}\n{a}\u{1b}[31m{b}\u{1b}[0m");
    std::fs::write(e.work.join("n.txt"), &body).unwrap();
    let r = call(&e, "s", "fs_read", json!({"path":"n.txt"}));
    assert!(
        r.ok && !r.content.contains(&secret) && !r.content.contains(b),
        "{}",
        r.content
    );
    assert!(r.content.matches("tachado").count() >= 3, "{}", r.content);
    // `edit` needs the real bytes (it writes the file back); they are filtered at the end.
    let raw = call(&e, "s", "fs_read", json!({"path":"n.txt","raw":true}));
    assert_eq!(raw.content, body);
    let f = call(
        &e,
        "s",
        "filter",
        json!({"items":[{"text":body,"wrap":"file"},{"text":format!("x {secret}")}]}),
    );
    assert!(f.ok, "{f:?}");
    let items = f.data["items"].as_array().unwrap();
    let wrapped = items[0].as_str().unwrap();
    assert!(
        wrapped.starts_with("<<<UNTRUSTED-DATA file ")
            && !wrapped.contains(&secret)
            && !wrapped.contains(b),
        "{wrapped}"
    );
    assert!(
        !items[1].as_str().unwrap().contains(&secret)
            && !items[1].as_str().unwrap().starts_with("<<<")
    );
    // Bad filter requests do nothing.
    assert!(
        !call(
            &e,
            "s",
            "filter",
            json!({"items":[{"text":"x","wrap":"memory"}]})
        )
        .ok
    );
    assert!(!call(&e, "s", "filter", json!({"items":"x"})).ok);
    // Names are cleaned by the filter, not by the listing (Pi must be able to stat each one).
    std::fs::write(e.work.join("bad\u{1b}[2Jname"), "x").unwrap();
    let l = call(&e, "s", "fs_list", json!({"path":"."}));
    assert!(l.data["entries"].to_string().contains("\\u001b"));
    let f = call(
        &e,
        "s",
        "filter",
        json!({"items":[{"text":l.data["entries"][0].as_str().unwrap()}]}),
    );
    assert!(!f.data["items"][0].as_str().unwrap().contains('\u{1b}'));
}

#[test]
fn if_the_vault_cannot_filter_the_output_is_withheld() {
    let dead = Path::new("/nonexistent/vault.sock");
    let e = env_with(|c| c.vault = Some(VaultClient::new(dead)));
    std::fs::write(e.work.join("a.txt"), "harmless but unfiltered").unwrap();
    let r = call(&e, "s", "fs_read", json!({"path":"a.txt"}));
    assert!(!r.content.contains("harmless"), "{}", r.content);
    assert!(r.content.contains("withheld"));
    let f = call(&e, "s", "filter", json!({"items":[{"text":"harmless"}]}));
    assert!(f.data["items"][0].as_str().unwrap().contains("withheld"));
}

#[test]
fn an_oversized_file_is_refused_and_an_oversized_write_too() {
    let e = env();
    std::fs::write(e.work.join("big.txt"), vec![b'a'; 9 << 20]).unwrap();
    let r = call(&e, "s", "fs_read", json!({"path":"big.txt"}));
    assert!(
        !r.ok && r.content.contains("more than"),
        "{}",
        &r.content[..100.min(r.content.len())]
    );
    let big = "z".repeat(2 << 20);
    let w = call(&e, "s", "fs_write", json!({"path":"w.txt","content":big}));
    assert_eq!(w.decision.rule, "toolhost-limit");
    assert!(!e.work.join("w.txt").exists());
}

#[test]
fn the_breaker_opens_after_ten_denials_and_an_ask_is_a_no() {
    let e = env();
    std::fs::write(e.work.join("ok.txt"), "fine").unwrap();
    for _ in 0..10 {
        assert_eq!(
            call(&e, "b", "fs_read", json!({"path":"../x"}))
                .decision
                .rule,
            "S-13"
        );
    }
    let r = call(&e, "b", "fs_read", json!({"path":"ok.txt"}));
    assert!(!r.ok, "{r:?}");
    assert_eq!(
        (r.decision.outcome.as_str(), r.decision.rule.as_str()),
        ("ask", "circuit-breaker")
    );
    assert!(!r.content.contains("fine"));
    // Another session is not affected.
    assert!(call(&e, "other", "fs_read", json!({"path":"ok.txt"})).ok);
}

#[test]
fn bad_lines_run_nothing() {
    let e = env();
    for line in [
        "",
        "not json",
        "{}",
        r#"{"id":"1","tool":"fs_write","args":"a","session":"s","call_id":"c"}"#,
        r#"{"id":"1","tool":"fs_write","args":{"path":"x","content":"c"},"session":"bad session\u001b","call_id":"c"}"#,
        r#"{"id":"1","tool":"fs_write","args":{"path":"x","content":"c"},"session":"s","call_id":"c","extra":1}"#,
    ] {
        let r: Response = serde_json::from_str(&e.host.handle_line(line)).unwrap();
        assert!(!r.ok, "{line}");
    }
    assert_eq!(std::fs::read_dir(&e.work).unwrap().count(), 0);
}

// -- the socket --

fn ask_over_socket(path: &Path, line: &str) -> String {
    let mut s = UnixStream::connect(path).unwrap();
    writeln!(s, "{line}").unwrap();
    let mut out = String::new();
    BufReader::new(s).read_line(&mut out).unwrap();
    out
}

fn private_dir(t: &TempDir) -> std::path::PathBuf {
    let d = t.path().join("sock");
    std::fs::create_dir(&d).unwrap();
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).unwrap();
    d
}

#[test]
fn the_socket_is_private_and_goes_away_with_the_server() {
    let e = env();
    let t = tempfile::tempdir().unwrap();
    let dir = private_dir(&t);
    let server = Server::start(Arc::new(e.host), &dir).unwrap();
    let mode = std::fs::metadata(server.path())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    let line = json!({"id":"1","tool":"fs_mkdir","args":{"path":"d"},"session":"s","call_id":"c"})
        .to_string();
    let r: Response = serde_json::from_str(&ask_over_socket(server.path(), &line)).unwrap();
    assert!(r.ok, "{r:?}");
    // Several requests on one connection, and an over-long line is refused.
    let mut s = UnixStream::connect(server.path()).unwrap();
    writeln!(s, "{line}\n{line}").unwrap();
    let mut rd = BufReader::new(s.try_clone().unwrap());
    for _ in 0..2 {
        let mut l = String::new();
        rd.read_line(&mut l).unwrap();
        assert!(serde_json::from_str::<Response>(&l).unwrap().ok);
    }
    s.write_all(&vec![b'a'; clio_toolhost::MAX_REQUEST_BYTES + 10])
        .unwrap();
    let mut l = String::new();
    rd.read_line(&mut l).unwrap();
    assert!(!serde_json::from_str::<Response>(&l).unwrap().ok);
    let p = server.path().to_path_buf();
    drop(server);
    assert!(!p.exists(), "the socket file is removed");
}

#[test]
fn a_folder_others_can_enter_is_refused() {
    let e = env();
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().join("open");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let err = Server::start(Arc::new(e.host), &dir)
        .err()
        .expect("must refuse");
    assert!(err.contains("0700"), "{err}");
}

/// The binary: READY, answers, and exits by itself when its stdin closes (no orphan).
#[test]
fn the_binary_serves_and_stops_when_its_parent_lets_go() {
    use std::process::{Command, Stdio};
    let t = tempfile::tempdir().unwrap();
    let r = t.path().canonicalize().unwrap();
    let (work, scratch, run) = (r.join("work"), r.join("scratch"), r.join("run"));
    std::fs::create_dir(&work).unwrap();
    let dir = private_dir(&t);
    let mut child = Command::new(env!("CARGO_BIN_EXE_clio-toolhost"))
        .args(["--socket-dir"])
        .arg(&dir)
        .args(["--work"])
        .arg(&work)
        .args(["--scratch"])
        .arg(&scratch)
        .args(["--run-dir"])
        .arg(&run)
        .args(["--jail", "off"])
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut ready = String::new();
    out.read_line(&mut ready).unwrap();
    let sock = ready
        .strip_prefix("READY ")
        .expect(&ready)
        .trim()
        .to_owned();
    let line = json!({"id":"1","tool":"fs_write","args":{"path":"x","content":"c"},"session":"s","call_id":"c"}).to_string();
    let t0 = Instant::now();
    let resp: Response = serde_json::from_str(&ask_over_socket(Path::new(&sock), &line)).unwrap();
    eprintln!("socket round trip (fs_write, cold): {:?}", t0.elapsed());
    assert!(resp.ok);
    drop(child.stdin.take()); // the parent lets go
    let end = Instant::now() + std::time::Duration::from_secs(10);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(
            Instant::now() < end,
            "the toolhost did not exit when its stdin closed"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert!(status.success());
    assert!(!Path::new(&sock).exists());
}

#[test]
fn latency_of_a_call_through_the_socket() {
    let e = env();
    std::fs::write(e.work.join("a.txt"), "hello").unwrap();
    let t = tempfile::tempdir().unwrap();
    let dir = private_dir(&t);
    let server = Server::start(Arc::new(e.host), &dir).unwrap();
    let line =
        json!({"id":"1","tool":"fs_read","args":{"path":"a.txt"},"session":"s","call_id":"c"})
            .to_string();
    let mut times = Vec::new();
    for _ in 0..200 {
        let t0 = Instant::now();
        let r: Response = serde_json::from_str(&ask_over_socket(server.path(), &line)).unwrap();
        times.push(t0.elapsed());
        assert!(r.ok);
    }
    times.sort();
    eprintln!(
        "fs_read over the socket, 200 calls: median {:?}, p95 {:?}, max {:?}",
        times[100], times[190], times[199]
    );
    assert!(times[100] < std::time::Duration::from_millis(50));
}
