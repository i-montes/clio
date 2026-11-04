#![allow(clippy::unwrap_used)] // tests: panicking is the failure mode
//! One memory file for Clio and for the standalone clio-memory (Claude Code over MCP): what one
//! writes the other reads and writes, with no scope or actor made twice.

use clio_brain::memory_link::{EmbedModel, MemoryLink};
use clio_memory::State;
use clio_memory::agent::{Draft, Memory, Place};

#[test]
fn a_file_written_by_the_standalone_memory_opens_in_clio() {
    let t = tempfile::tempdir().unwrap();
    let data = t.path();
    // Claude Code saves first, in project alfa.
    let m1 = {
        let cc = Memory::open_write(
            data,
            &clio_memory::agent::EmbedModel::none(),
            Some("alfa"),
            "mcp:claude-code",
        )
        .unwrap();
        cc.save(
            &Draft::plain(
                "el despliegue usa compose",
                clio_memory::MemoryType::Semantic,
            ),
            "mcp:claude-code",
            Place::Project,
            false,
        )
        .unwrap()
        .memory
    };
    // Clio opens the same file in the same project, finds it and writes its own.
    let m2 = {
        let clio = MemoryLink::open_write_in(data, &EmbedModel::none(), Some("alfa")).unwrap();
        let (found, _) = clio.search_all("despliegue compose", 5).unwrap();
        assert_eq!(found.iter().map(|r| r.id).collect::<Vec<_>>(), [m1]);
        clio.remember("prefiere respuestas cortas", None).unwrap()
    };
    // Both are there, once; nothing of the file was made twice.
    let person = MemoryLink::open_read(data, &EmbedModel::none())
        .unwrap()
        .unwrap()
        .for_person();
    assert_eq!(person.get(m1).unwrap().state, State::Active);
    assert_eq!(person.get(m2).unwrap().state, State::Active);
    let engine = person.engine();
    let mut scopes: Vec<String> = engine
        .scopes()
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    scopes.sort();
    assert_eq!(scopes, ["clio", "lecciones", "proyecto:alfa"]);
    let names: Vec<String> = engine
        .actors()
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    for n in [
        "persona",
        "agente",
        "sesion",
        "importador",
        "mcp:claude-code",
    ] {
        assert_eq!(
            names.iter().filter(|x| *x == n).count(),
            1,
            "{n}: {names:?}"
        );
    }
    // And the standalone side reads what Clio wrote.
    let cc = Memory::open_read(
        data,
        &clio_memory::agent::EmbedModel::none(),
        Some("alfa"),
        "mcp:claude-code",
    )
    .unwrap()
    .unwrap();
    assert_eq!(cc.get(m2).unwrap().content, "prefiere respuestas cortas");
}
