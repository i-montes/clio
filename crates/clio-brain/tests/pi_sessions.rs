#![allow(clippy::unwrap_used)]
//! Listing Pi's session files: real-shaped files, hostile names, broken files and many sessions.
//! No Node needed.

use std::path::Path;
use std::time::{Duration, Instant};

use clio_brain::pi::sessions::list_sessions;
use serde_json::json;

fn header(id: &str, cwd: &str) -> String {
    json!({"type":"session","version":3,"id":id,"timestamp":"2026-10-31T12:00:00.000Z","cwd":cwd})
        .to_string()
}
fn user(i: u32, text: &str) -> String {
    json!({"type":"message","id":format!("u{i}"),"parentId":null,"timestamp":"2026-10-31T12:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":text}],"timestamp":1}}).to_string()
}
fn assistant(i: u32) -> String {
    json!({"type":"message","id":format!("a{i}"),"parentId":null,"timestamp":"2026-10-31T12:00:02.000Z","message":{"role":"assistant","content":[{"type":"text","text":"ok"}]}}).to_string()
}
fn system() -> String {
    json!({"type":"message","id":"s","parentId":null,"timestamp":"2026-10-31T12:00:00.500Z","message":{"role":"system","content":"","sections":{"preamble":"x"}}}).to_string()
}
fn write(dir: &Path, name: &str, lines: &[String]) {
    std::fs::write(dir.join(name), lines.join("\n") + "\n").unwrap();
}

#[test]
fn lists_titles_names_counts_and_dates() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "2026-10-31T12-00-00-000Z_aaa.jsonl",
        &[
            header("aaa", "/home/x/proj"),
            system(),
            user(1, "  fix the\nlogin bug  "),
            assistant(1),
            user(2, "thanks"),
            assistant(2),
        ],
    );
    let named = json!({"type":"session_info","id":"n1","parentId":"a2","timestamp":"2026-10-31T12:05:00.000Z","name":"Login work"}).to_string();
    write(
        d.path(),
        "2026-10-31T13-00-00-000Z_bbb.jsonl",
        &[header("bbb", "/home/x/other"), user(1, "first"), named],
    );
    // Pi may group by folder: one level down is read too.
    std::fs::create_dir(d.path().join("--home-x-proj--")).unwrap();
    write(
        &d.path().join("--home-x-proj--"),
        "t_ccc.jsonl",
        &[header("ccc", "/home/x/proj")],
    );
    std::fs::write(d.path().join("notes.txt"), "not a session").unwrap();

    let l = list_sessions(d.path());
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    assert_eq!(l.sessions.len(), 3);
    let by = |id: &str| l.sessions.iter().find(|s| s.id == id).unwrap();
    let a = by("aaa");
    assert_eq!(
        (a.title.as_str(), a.messages, a.approx, a.name.as_deref()),
        ("fix the login bug", 4, false, None)
    );
    assert_eq!(a.cwd, "/home/x/proj");
    assert_eq!(a.created_ms, Some(1_793_448_000_000));
    let b = by("bbb");
    assert_eq!(
        (b.title.as_str(), b.name.as_deref()),
        ("Login work", Some("Login work"))
    );
    assert_eq!(by("ccc").title, "(sin mensajes)");
}

#[test]
fn hostile_names_and_titles_are_sanitized() {
    let d = tempfile::tempdir().unwrap();
    let evil = json!({"type":"session_info","id":"n","parentId":null,"timestamp":"2026-10-31T12:05:00.000Z","name":"\u{1b}[2J\u{1b}]0;pwned\u{7}name\u{202e}x"}).to_string();
    write(d.path(), "a_aaa.jsonl", &[header("aaa", "/x"), evil]);
    write(
        d.path(),
        "b_bbb.jsonl",
        &[
            header("bbb", "/x\u{1b}[31m/y"),
            user(1, "\u{1b}[31mred\u{1b}[0m \u{0}hi\u{202e}"),
        ],
    );
    let l = list_sessions(d.path());
    let by = |id: &str| l.sessions.iter().find(|s| s.id == id).unwrap();
    assert_eq!(by("aaa").title, "namex");
    assert_eq!(by("bbb").title, "red hi");
    assert_eq!(by("bbb").cwd, "/x/y");
    assert!(
        by("bbb").cwd_path.is_none(),
        "a folder with control characters is never offered as a path"
    );
    for s in &l.sessions {
        assert!(!format!("{s:?}").contains('\u{1b}') || format!("{s:?}").contains("\\u{1b}"));
        assert!(!s.title.chars().any(char::is_control) && !s.cwd.chars().any(char::is_control));
    }
}

#[test]
fn broken_files_are_skipped_with_a_warning_and_do_not_break_the_list() {
    let d = tempfile::tempdir().unwrap();
    write(
        d.path(),
        "ok_ok1.jsonl",
        &[header("ok1", "/x"), user(1, "hello")],
    );
    std::fs::write(d.path().join("empty_e.jsonl"), "").unwrap();
    std::fs::write(
        d.path().join("trunc_t.jsonl"),
        "{\"type\":\"session\",\"ver",
    )
    .unwrap();
    std::fs::write(
        d.path().join("garbage_g.jsonl"),
        b"\xff\xfe\x00\x01 not json\n",
    )
    .unwrap();
    write(
        d.path(),
        "wrong_w.jsonl",
        &["{\"type\":\"message\",\"id\":\"x\"}".to_owned()],
    );
    // A good file cut in the middle of its last line (a crash while writing) still lists.
    let mut cut = format!("{}\n{}\n", header("cut1", "/x"), user(1, "hi"));
    cut.push_str("{\"type\":\"message\",\"id\":\"half");
    std::fs::write(d.path().join("cut_cut1.jsonl"), cut).unwrap();
    let l = list_sessions(d.path());
    let ids: Vec<&str> = l.sessions.iter().map(|s| s.id.as_str()).collect();
    assert!(
        ids.contains(&"ok1") && ids.contains(&"cut1") && ids.len() == 2,
        "{ids:?}"
    );
    assert_eq!(l.warnings.len(), 4, "{:?}", l.warnings);
    assert!(l.warnings.iter().all(|w| w.starts_with("se ignora")));
    assert!(
        list_sessions(Path::new("/nonexistent-clio-dir"))
            .sessions
            .is_empty()
    );
}

#[test]
fn a_big_file_is_sampled_not_loaded_and_hundreds_of_sessions_list_fast() {
    let d = tempfile::tempdir().unwrap();
    let mut lines = vec![header("big", "/x"), user(0, "the first thing")];
    for i in 0..6000 {
        lines.push(assistant(i));
        lines.push(user(i + 1, &"filler ".repeat(10)));
    }
    let late = json!({"type":"session_info","id":"n","parentId":null,"timestamp":"2026-10-31T12:05:00.000Z","name":"renamed late"}).to_string();
    lines.push(late);
    write(d.path(), "big_big.jsonl", &lines);
    assert!(
        std::fs::metadata(d.path().join("big_big.jsonl"))
            .unwrap()
            .len()
            > 512 * 1024
    );
    for i in 0..400 {
        write(
            d.path(),
            &format!("s{i}_s{i}.jsonl"),
            &[
                header(&format!("s{i}"), "/x"),
                user(1, &format!("task number {i}")),
                assistant(1),
            ],
        );
    }
    let t = Instant::now();
    let l = list_sessions(d.path());
    let took = t.elapsed();
    assert_eq!(l.sessions.len(), 401);
    let big = l.sessions.iter().find(|s| s.id == "big").unwrap();
    assert!(
        big.approx && big.messages > 8000 && big.messages < 16_000,
        "{}",
        big.messages
    );
    assert_eq!(big.name.as_deref(), Some("renamed late"));
    assert!(took < Duration::from_secs(3), "listing took {took:?}");
    eprintln!("401 sessions listed in {took:?}");
}
