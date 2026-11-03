//! The memory tools through the whole Pi stack: the real Pi, the guard and the real tool host
//! over a real memory file. This is the smoke test (Task 4); the attack battery extends it.
#![allow(clippy::unwrap_used)]

use std::path::Path;

use crate::common::fake_model::{FakeModel, Turn};
use crate::common::*;
use clio_brain::memory_link::{EmbedModel, MemoryLink};
use clio_memory::{MemoryType, Origin as MOrigin, Proposal};
use serde_json::{Value, json};

const WORD: &str = "xilofono";
const HEAD: &str = "<<<UNTRUSTED-DATA ";

/// A memory file with one memory of the person and, if asked, one the agent wrote. Returns the
/// person's id. The writer is dropped before this returns: the service needs the lock.
fn seed(data: &Path, with_agent: bool) -> i64 {
    let link = MemoryLink::open_write(data, &EmbedModel::none()).unwrap();
    let person = link
        .remember(&format!("la persona prefiere el {WORD} de madera"), None)
        .unwrap()
        .0;
    if with_agent {
        let actor = link.actors().sesion;
        link.engine()
            .propose(Proposal {
                scope: link.scope(),
                actor,
                kind: MemoryType::Semantic,
                content: format!("el agente afinó el {WORD}"),
                topic_key: None,
                origin: MOrigin::Agent,
                session: None,
                episode: None,
                source: None,
                valid_from: None,
                valid_to: None,
            })
            .unwrap();
    }
    person
}

/// The text of every tool message in the model's last request.
fn tool_messages(model: &FakeModel) -> Vec<String> {
    let reqs = model.requests();
    reqs.last().unwrap()["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| match &m["content"] {
            Value::String(s) => s.clone(),
            Value::Array(a) => a.iter().filter_map(|p| p["text"].as_str()).collect(),
            _ => String::new(),
        })
        .collect()
}

/// Pi lists eight tools, a memory call reaches the service, what the model reads is wrapped
/// exactly once and with source `memory`, and the session is marked only for a memory that is
/// not the person's.
fn run(with_agent: bool) {
    let Some(node) = pi_available() else { return };
    let w = world();
    let data = w.tmp.path().canonicalize().unwrap().join("memory");
    let person = seed(&data, with_agent);
    let model = FakeModel::scripted(vec![
        Turn::Calls(vec![
            (
                "memory_search".to_owned(),
                json!({"query": WORD, "reason": "Buscar en la memoria"}),
            ),
            (
                "memory_get".to_owned(),
                json!({"id": person, "reason": "Leer un recuerdo"}),
            ),
        ]),
        Turn::Text("done".into()),
    ]);
    let host = Toolhost::start_with_memory(&w, false, None, Some(&data));
    let mut pi = Pi::start(&node, &w, &model, Some(&host.socket), &Opts::default());
    let ev = pi.prompt("what do you know about me?");

    let ends = tool_ends(&ev);
    assert_eq!(ends.len(), 2, "{ends:?}");
    assert!(ends.iter().all(|(_, err, _)| !err), "{ends:?}");
    assert_eq!(
        declared_tools(&model).len(),
        8,
        "{:?}",
        declared_tools(&model)
    );

    let seen = tool_messages(&model);
    assert_eq!(seen.len(), 2, "{seen:?}");
    for text in &seen {
        assert!(text.contains(WORD), "{text}");
        assert!(text.starts_with(&format!("{HEAD}memory ")), "{text}");
        assert_eq!(text.matches(HEAD).count(), 1, "wrapped twice: {text}");
    }

    let taints = pi
        .entries()
        .iter()
        .filter(|e| e["customType"] == "clio-guard.taint")
        .count();
    assert_eq!(taints, usize::from(with_agent));
}

// S-60
#[test]
fn pi_lists_eight_tools_and_a_memory_call_reaches_the_host() {
    run(false);
}

// S-60
#[test]
fn a_memory_the_agent_wrote_marks_the_pi_session() {
    run(true);
}
