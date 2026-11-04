#![allow(clippy::unwrap_used)]
//! Jev's questions about tools come from the catalog the guard reports, and from nowhere else:
//! every tool the guard registers has a `use_when`, and the catalog it builds is one Jev can be
//! asked about. Read from the guard's source, so the two cannot drift apart.

use clio_jev::{parse_catalog, tool_questions};
use serde_json::json;

fn guard() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../adapters/pi/extensions/clio-guard/index.ts"
    ))
    .unwrap()
}

/// The quoted names of `export const TOOL_NAMES = [...]`.
fn tool_names(src: &str) -> Vec<String> {
    let line = src
        .lines()
        .find(|l| l.contains("export const TOOL_NAMES"))
        .unwrap();
    line.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// The `name: "phrase",` lines of `export const USE_WHEN`.
fn use_when(src: &str) -> Vec<(String, String)> {
    src.lines()
        .skip_while(|l| !l.contains("export const USE_WHEN"))
        .skip(1)
        .take_while(|l| !l.starts_with('}'))
        .map(|l| {
            let (name, rest) = l.trim().split_once(": \"").unwrap();
            (name.to_owned(), rest.trim_end_matches("\",").to_owned())
        })
        .collect()
}

#[test]
fn every_tool_the_guard_registers_has_a_use_when_in_the_same_order() {
    let src = guard();
    let names = tool_names(&src);
    assert_eq!(names.len(), 17, "{names:?}");
    let got: Vec<String> = use_when(&src).into_iter().map(|(n, _)| n).collect();
    assert_eq!(got, names);
}

// S-62
#[test]
fn the_catalog_of_the_guard_is_one_jev_can_be_asked_about() {
    let src = guard();
    assert!(
        src.contains("catalog: TOOL_NAMES.map((name) => ({ name, use_when: USE_WHEN[name] }))"),
        "the guard reports the catalog at the start of the session"
    );
    let catalog: Vec<_> = use_when(&src)
        .into_iter()
        .map(|(name, use_when)| json!({"name": name, "use_when": use_when}))
        .collect();
    let hints = parse_catalog(&json!(catalog)).unwrap();
    let questions = tool_questions(&hints).unwrap();
    assert_eq!(questions.len(), tool_names(&src).len());
}
