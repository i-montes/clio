#![allow(clippy::unwrap_used)]
//! The effect log keeps facts, never text (S-18, S-20, S-23): a tool that prints something is
//! written to disk without its text, so nothing the scrubber would have hidden from the model
//! can sit in `effects.jsonl` (the run folder is also backed up).

use clio_node::EffectLog;
use serde_json::json;

// S-58
#[test]
fn the_log_keeps_facts_and_not_the_text_of_a_result() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("effects.jsonl");
    let log = EffectLog::open(&path).unwrap();
    let printed = json!({"output": "sk-secret-text\nsecond line", "exit_code": 2, "timed_out": false,
        "nested": {"names": ["a-secret-file.txt"]}});
    let (first, applied) = log.apply("a1", "h1", || Ok(printed.clone())).unwrap();
    // The caller of the tool gets the whole result: the filter in front of the model cleans it.
    assert!(applied && first == printed);
    // The disk gets facts: booleans, numbers and how long each text was.
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(
        !on_disk.contains("sk-secret-text") && !on_disk.contains("a-secret-file"),
        "{on_disk}"
    );
    assert!(
        on_disk.contains("\"exit_code\":2") && on_disk.contains("\"bytes\":26"),
        "{on_disk}"
    );
    // The same action again is not run, and what it returns is what was recorded: the facts.
    let (again, applied) = log
        .apply("a1", "h1", || panic!("must not run twice"))
        .unwrap();
    assert!(!applied);
    assert_eq!(again["exit_code"], 2);
    assert_eq!(again["output"], json!({"bytes": 26}));
    // A restart reads the same facts back.
    let log = EffectLog::open(&path).unwrap();
    let (after_restart, applied) = log
        .apply("a1", "h1", || panic!("must not run twice"))
        .unwrap();
    assert!(!applied && after_restart == again);
}
