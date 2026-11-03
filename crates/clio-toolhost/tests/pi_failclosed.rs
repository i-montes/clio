//! Fail-closed start-up: without the extension, or with an extension that cannot load (missing or
//! wrong prompt file), Pi has NO tools, and the start-up check says so.
#![allow(clippy::unwrap_used)]

mod common;

use common::fake_model::{FakeModel, Turn};
use common::*;
use serde_json::json;

fn one_call_model() -> FakeModel {
    FakeModel::scripted(vec![
        Turn::Calls(vec![(
            "write_file".to_owned(),
            json!({"path":"x.txt","content":"c"}),
        )]),
        Turn::Text("done".into()),
    ])
}

#[test]
fn without_the_extension_there_are_no_tools() {
    let Some(node) = pi_available() else { return };
    let w = world();
    let model = one_call_model();
    let mut s = stack(
        &node,
        &w,
        &model,
        false,
        &Opts {
            extension: false,
            ..Opts::default()
        },
    );
    assert!(
        s.pi.verify_tools(&["read_file"]).is_err(),
        "no extension, no report"
    );
    let ev = s.pi.prompt("write");
    assert_eq!(
        declared_tools(&model),
        Vec::<String>::new(),
        "the model was told about no tools"
    );
    assert!(tool_ends(&ev).iter().all(|(_, err, _)| *err));
    assert!(!w.work.join("x.txt").exists());
}

fn broken_prompt(label: &str, name: &str, body: &str) {
    let Some(node) = pi_available() else { return };
    let w = world();
    let file = w.tmp.path().join(name);
    if !body.is_empty() {
        std::fs::write(&file, body).unwrap();
    }
    let model = one_call_model();
    let host = Toolhost::start(&w, false, None);
    // Pi itself refuses to start when an extension it was told to load does not load.
    let err = Pi::try_start(
        &node,
        &w,
        &model,
        Some(&host.socket),
        &Opts {
            prompt_file: Some(file),
            ..Opts::default()
        },
    )
    .err()
    .unwrap_or_else(|| panic!("{label}: Pi started although the guard did not load"));
    assert!(
        err.contains("clio-guard") && err.contains("Failed to load extension"),
        "{label}: {err:?}"
    );
    assert!(
        model.requests().is_empty(),
        "{label}: the model was never reached"
    );
    assert!(!w.work.join("x.txt").exists(), "{label}");
}

#[test]
fn a_missing_prompt_file_fails_closed_with_a_clear_error() {
    broken_prompt("missing", "clio-agent@4.md", "");
}

#[test]
fn a_prompt_with_the_wrong_version_line_fails_closed() {
    broken_prompt(
        "wrong version line",
        "clio-agent@4.md",
        "version: 3\n\nYou are Clio.\n",
    );
}

#[test]
fn a_prompt_file_that_does_not_match_the_declared_version_fails_closed() {
    broken_prompt(
        "name does not match",
        "clio-agent@9.md",
        "version: 9\n\nYou are Clio.\n",
    );
}
