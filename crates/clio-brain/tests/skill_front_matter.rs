#![allow(clippy::unwrap_used)]
//! A skill the person adds is registered under the name Pi reads (S-68): every front matter Clio
//! accepts (`clio_toolhost::skills::added_front_matter`) is read by Pi's own `parseFrontmatter`
//! (the `yaml` package, from `adapters/pi/node_modules`) to the same name and description.
//! Needs Node and Pi (skipped with a message without them, a failure with `CLIO_REQUIRE_PI=1`).

mod pi_support;

use std::process::Command;

use clio_toolhost::skills::added_front_matter;
use serde_json::{Value, json};

/// What Clio accepts, in the shapes skills are written in (ponytail's folded descriptions,
/// superpowers' plain and quoted ones) and at the edges of what it reads.
const ACCEPTED: &[&str] = &[
    "---\nname: brainstorming\ndescription: \"You MUST use this before any creative work - creating features.\"\n---\n\nBody.\n",
    "---\nname: systematic-debugging\ndescription: Use when encountering any bug, test failure, or unexpected behavior, before proposing fixes\n---\n",
    "---\nname: ponytail\ndescription: >\n  Lazy senior dev mode: the smallest change that fully solves the task, and a\n  reply a busy human understands in one read. Use for \"ponytail\", \"be lazy\".\nargument-hint: \"[lite|full|ultra|off]\"\nlicense: MIT\n---\n",
    "---\nname: ponytail-debt\ndescription: >\n  List every `shortcut:` comment (and older `ponytail:` ones) as a debt ledger.\n  changes nothing.\n---\n",
    "---\nname: a\ndescription: >-\n  Folded,\n  stripped.\n---\n",
    "---\nname: b\ndescription: |\n  Literal\n  lines.\n---\n",
    "---\nname: c\ndescription: |-\n  Literal,\n  stripped.\n---\n",
    "---\nname: d\ndescription: 'It''s quoted: once'\n---\n",
    "---\nname: e\ndescription: Plain with a comment # here\nmeta:\n  name: not-this-one\nallowed-tools: [Read, Grep]\n---\n",
    "---\n# a comment\ndescription: No name, so the folder's.\n---\n",
    "---\ndescription: Spaces at the end   \n---\n",
];

#[test]
fn pi_reads_every_accepted_front_matter_as_clio_does() {
    let Some(node) = pi_support::need("pi_reads_every_accepted_front_matter_as_clio_does", true)
    else {
        return;
    };
    let module = pi_support::repo()
        .join("adapters/pi/node_modules/@earendil-works/pi-coding-agent/dist/utils/frontmatter.js");
    assert!(module.exists(), "{}", module.display());
    let t = tempfile::tempdir().unwrap();
    let script = t.path().join("fm.mjs");
    std::fs::write(
        &script,
        r#"import { pathToFileURL } from "url";
const { parseFrontmatter } = await import(pathToFileURL(process.argv[2]).href);
const docs = JSON.parse(process.argv[3]);
console.log(JSON.stringify(docs.map((d) => {
  const f = parseFrontmatter(d).frontmatter;
  return { name: f.name ?? null, description: f.description ?? null };
})));
"#,
    )
    .unwrap();
    let out = Command::new(&node.path)
        .arg(&script)
        .arg(&module)
        .arg(json!(ACCEPTED).to_string())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let pi: Vec<Value> = serde_json::from_slice(&out.stdout).unwrap();
    for (md, pi) in ACCEPTED.iter().zip(pi) {
        let (name, description) = added_front_matter(md).unwrap_or_else(|e| panic!("{md:?}: {e}"));
        assert_eq!(pi["name"], json!(name), "{md:?}");
        assert_eq!(pi["description"], json!(description), "{md:?}");
    }
}
