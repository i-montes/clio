# Clio usa clio-memory (entrega 4) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Clio builds against `i-montes/clio-memory` pinned by tag, deletes its copy of the engine (`crates/clio-memory`), and shares code with it where the two were duplicates. Everything Clio does today keeps working, and both programs write the same file safely.

**Architecture:**
- `clio-brain` depends on `clio-memory = { git = "https://github.com/i-montes/clio-memory", tag = "v0.2.1" }`; `cargo-deny` allows only that git source.
- Clio's `project.rs` becomes the library's `clio_memory::agent::project`.
- Clio's `memory_link` and `memory_port` stay (they hold Clio's run loop, Jev, the vault and the person's view). Their merge fix (`memory_consolidate`) comes from the library's `save_beside`.
- The engine's tests and acceptance list now run in the clio-memory repository's CI; Clio's rule coverage says so.

**Tech Stack:** Rust 2024; cargo git dependencies; GitHub Actions.

**Spec:** `docs/specs/2026-11-04-clio-memory-standalone-design.md` («Estructura de los repositorios», «Versiones», entrega 4), plus the «What Clio keeps» section of `docs/superpowers/plans/2026-11-04-clio-memory-repo.md`.

## Global Constraints

- Clio keeps, unchanged in behavior:
  - Jev: the guard before saving, the place and relations, and the memories and tools in front of the message;
  - the cost and usage measurement (runs, `clio usage`, Jev's usage);
  - the engine's telemetry feature (available, as it is today);
  - `close_run` and `recall_for` with the vault's scrubber;
  - `clio memory` and the TUI's memory screen;
  - the Pi tool host and its 17 tools;
  - `clio memory mcp`.
- One file shared by both: `~/.local/share/clio/memory.db`. Same scopes, actors and schema, and nothing renamed.
- Clio pins clio-memory by tag (`v0.2.1`) and moves it up when the schema changes. The README says so.
- `cargo-deny`: `unknown-git = "deny"` stays; only `https://github.com/i-montes/clio-memory` is allowed.
- Gates: `cargo fmt --check`, `clippy --workspace --all-targets --all-features -D warnings`, `CLIO_REQUIRE_PI=1 cargo test --workspace --all-features`, `scripts/rule-coverage.sh`, `scripts/check-prompts.py`, `npm run check`, `cargo deny check`. All green; the CI of the PR is green.
- Living docs updated in the same PR; historical records (`docs/specs`, `docs/superpowers/plans`, ADRs) are not touched.

## Review Focus

1. A memory file written by `clio-memory` (Claude Code over MCP, actor `mcp:claude-code`) and then opened by Clio. Clio's run loop, Pi chat, `clio memory list` and the TUI read it, and Clio writes to it, without errors and without renaming anything.
2. The build without network once the dependency is fetched. `cargo build --locked --offline` works after a first fetch, and CI fetches the tag.
3. Rule coverage after the engine's tests leave. Every rule that was proven only by `crates/clio-memory/tests` is still proven by a test in Clio, or is listed in `rule-coverage.exempt` with «checked in i-montes/clio-memory CI». `spec-clio-memory` is no longer enforced here, and the reason is written down.
4. `scripts/memory-dev.sh` and `.claude/skills/clio-memory/` (the old dogfooding with `-p clio-memory`) no longer build a crate that is gone. Replace them with the installed `clio-memory` binary, or remove them.
5. A merge in Clio of two memories of another project. It stays in that project, and two projects are never mixed.

---

### Task 1: The dependency, and the copy goes

**Files:**
- Modify: `crates/clio-brain/Cargo.toml` (git dependency, tag `v0.2.1`; feature `model2vec = ["clio-memory/model2vec"]` stays), `Cargo.lock`, `deny.toml` (`[sources] allow-git = ["https://github.com/i-montes/clio-memory"]`), `.github/workflows/ci.yml` (remove the `-p clio-memory --test recall t25` step; that check now lives in clio-memory's CI)
- Delete: `crates/clio-memory/`
- Modify: `scripts/rule-coverage.enforced` (drop `spec-clio-memory`, with a comment), `scripts/rule-coverage.exempt` (any rule left without a test in Clio: «checked in i-montes/clio-memory CI (tests/…)»)

- [ ] **Step 1:** Switch the dependency, delete the crate, then run `cargo build --workspace --all-features` and fix every compile error. Expected: none beyond paths. The library is the same engine plus `agent`, `tools`, `mcp` and `setup`.
- [ ] **Step 2:** Run `scripts/rule-coverage.sh` and note the rules it reports. Write each one in `.exempt` with where it is checked. Expected: green.
- [ ] **Step 3:** Test, Review Focus 1: `crates/clio-brain/tests/memory_shared_file.rs`, `a_file_written_by_the_standalone_memory_opens_in_clio`:
  - `clio_memory::agent::Memory::open_write(&data, …, Some("alfa"), "mcp:claude-code")` saves m1;
  - Clio's `MemoryLink::open_write_in(&data, …, Some("alfa"))` finds m1 and writes m2;
  - `MemoryLink::open_read` and `for_person().places()` list both, with no scope or actor duplicated.
- [ ] **Step 4:** Run the full gates, `cargo deny check` included. Commit `build: Clio uses i-montes/clio-memory v0.2.1; the engine's copy goes`.

### Task 2: One project detection, and the merge fix

**Files:**
- Delete: `crates/clio-brain/src/project.rs`. Add `pub use clio_memory::agent::project;` in `crates/clio-brain/src/lib.rs`, so `clio_brain::project::…` keeps working.
- Modify: `crates/clio-brain/src/memory_link.rs` (add `save_beside(&self, draft, session, beside: &MemoryRow, tainted) -> Res<Outcome>`, as in the library), `crates/clio-brain/src/memory_port.rs` (`consolidate`: compare `ScopeId`, not `Place`, and save with `save_beside` in that scope)
- Test: `crates/clio-brain/tests/memory_port.rs`, `a_merge_stays_in_the_memories_own_project_and_never_mixes_two`. This is Review Focus 5. From a port with no project: merging two of `alfa` lands in `proyecto alfa`; merging one of `alfa` and one of `beta` is refused with «solo se unen memorias del mismo sitio».

- [ ] **Step 1:** Write the failing test and run `cargo test -p clio-brain --test memory_port a_merge_stays`. Expected: FAIL (the merge lands in the session's place).
- [ ] **Step 2:** Implement. Run `cargo test -p clio-brain --all-features`. Expected: PASS, and the project-detection tests now run in the library.
- [ ] **Step 3:** Commit `fix(memory): one project detection, and a merge stays in its own project`.

### Task 3: Dogfooding, docs, version

**Files:**
- Delete or rewrite: `scripts/memory-dev.sh` and `.claude/skills/clio-memory/SKILL.md`, the old project skill that shadows the user's skill of the same name. Replaced by the installed `clio-memory` MCP and the user-level skill.
- Modify:
  - `README.md` (the crates list: clio-memory is now a dependency from its own repository, pinned by tag; how to move the tag);
  - `docs/uso-local.md` (where the memory engine lives; `clio-memory setup claude-code` as the way to use it from Claude Code, next to `clio memory mcp`);
  - `docs/lista-maestra.md` (item 300: the name is taken by our repo);
  - `docs/threat-model.md`, `docs/security.md` if they name `crates/clio-memory` paths.

- [ ] **Step 1:** Edit. `rg -n "crates/clio-memory|-p clio-memory" --glob '!docs/specs/**' --glob '!docs/superpowers/**'` must find nothing.
- [ ] **Step 2:** Run the full gates. Commit `docs: clio-memory lives in its own repository`.
- [ ] **Step 3:** Open a PR. Merge only with the person's yes; after merging, run `just install`.
