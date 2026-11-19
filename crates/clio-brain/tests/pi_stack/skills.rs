//! Skills through the whole stack (S-68): the real Pi, in its jail, started with Clio's skills
//! folder, lists a registered skill in the system prompt; reading its SKILL.md reaches the model
//! as Clio's own text (not wrapped, not marking the session), while any other file is outside data
//! as before; and nothing in the jail can write the folder.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use crate::common::fake_model::{FakeModel, Turn, last_tool_text};
use crate::common::*;
use crate::stack::{call, system_text, taint_entries};
use serde_json::json;

const MD: &str = "---\nname: demo\ndescription: \"Says hello in the house style. Use when the person wants a greeting.\"\n---\n\nSay hello, then the date.\n\n## How to verify\n\ntrue\n";

/// `<data>/skills` with `demo` registered and enabled.
fn skills_of(w: &World) -> PathBuf {
    let root = w.tmp.path().canonicalize().unwrap().join("data/skills");
    let live = root.join("live/demo");
    std::fs::create_dir_all(&live).unwrap();
    std::fs::create_dir_all(root.join("records")).unwrap();
    std::fs::write(
        root.join("records/demo.json"),
        json!({"name": "demo", "enabled": true}).to_string(),
    )
    .unwrap();
    std::fs::write(live.join("SKILL.md"), MD).unwrap();
    root
}

fn p(path: &Path) -> String {
    path.display().to_string()
}

// S-68: Pi lists the skill; its file reads as Clio's own; another file still marks and is wrapped.
#[test]
fn a_registered_skill_is_listed_and_reads_unwrapped_without_marking() {
    let Some(node) = pi_available() else {
        return;
    };
    let w = world();
    let root = skills_of(&w);
    let skill = root.join("live/demo/SKILL.md");
    w.put("notes.txt", "a note\n");
    let model = FakeModel::scripted(vec![
        call(
            "read",
            json!({"path": p(&skill), "reason": "Read the skill"}),
        ),
        Turn::Text("hello".into()),
        call(
            "read",
            json!({"path": "notes.txt", "reason": "Read the notes"}),
        ),
    ]);
    let o = Opts {
        skills: Some(root.clone()),
        ..Opts::default()
    };
    let mut s = stack(&node, &w, &model, false, &o);
    // Pi offers it as a command of source `skill`, from Clio's folder (what the launch checks).
    let cmds = s.pi.command(
        json!({"type": "get_commands"}),
        std::time::Duration::from_secs(10),
    );
    let skills: Vec<_> = cmds["data"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["source"] == "skill")
        .collect();
    assert_eq!(skills.len(), 1, "{cmds}");
    assert_eq!(skills[0]["sourceInfo"]["path"], p(&skill), "{cmds}");
    let ev = s.pi.prompt("say hello");
    let ends = tool_ends(&ev);
    assert_eq!(ends.len(), 1, "{ends:?}");
    assert!(!ends[0].1, "the read failed: {ends:?}");
    let reqs = model.requests();
    let system = system_text(&reqs[0]);
    assert!(
        system.contains("demo") && system.contains(&p(&skill)),
        "{system}"
    );
    let shown = last_tool_text(&reqs[1]).unwrap();
    assert!(shown.contains("Say hello, then the date."), "{shown}");
    assert!(
        !shown.contains("UNTRUSTED-DATA"),
        "a skill is not outside data: {shown}"
    );
    assert_eq!(
        taint_entries(&s.pi.entries()),
        0,
        "reading a skill marks nothing"
    );
    // The same session reads a file of its copy: outside data, as before.
    s.pi.prompt("and the notes");
    let reqs = model.requests();
    let last = reqs.last().unwrap();
    let note = last_tool_text(last).unwrap();
    assert!(
        note.contains("UNTRUSTED-DATA") && note.contains("a note"),
        "{note}"
    );
    assert!(taint_entries(&s.pi.entries()) > 0);
    // The skill's earlier result is still Clio's own in what the model is sent.
    let all = last["messages"].to_string();
    let at = all.find("Say hello, then the date.").unwrap();
    assert!(!all[..at].ends_with("UNTRUSTED-DATA"), "re-wrapped later");
    // Nothing reached the folder.
    assert_eq!(std::fs::read_to_string(&skill).unwrap(), MD);
}

// S-68: the guard's write tools refuse the skills folder, and Pi's `skill_write` stores only a
// draft (with no judge it stores nothing): the folder is unchanged.
#[test]
fn nothing_the_model_does_writes_the_skills_folder() {
    let Some(node) = pi_available() else {
        return;
    };
    let w = world();
    let root = skills_of(&w);
    let skill = root.join("live/demo/SKILL.md");
    let model = FakeModel::scripted(vec![
        call(
            "write",
            json!({"path": p(&skill), "content": "owned", "reason": "x"}),
        ),
        call(
            "edit",
            json!({"path": p(&skill), "edits": [{"oldText": "Say hello", "newText": "Owned"}], "reason": "x"}),
        ),
        call(
            "skill_write",
            json!({"name": "demo", "description": "Says hello. Use when the person wants a greeting.",
            "body": "Owned.", "verify": "true", "reason": "x"}),
        ),
    ]);
    let o = Opts {
        skills: Some(root.clone()),
        ..Opts::default()
    };
    let mut s = stack(&node, &w, &model, false, &o);
    let ev = s.pi.prompt("change the skill");
    let ends = tool_ends(&ev);
    assert_eq!(ends.len(), 3, "{ends:?}");
    assert!(ends.iter().all(|e| e.1), "every one refused: {ends:?}");
    assert_eq!(std::fs::read_to_string(&skill).unwrap(), MD);
    assert!(!root.join("live/demo/owned").exists());
}
