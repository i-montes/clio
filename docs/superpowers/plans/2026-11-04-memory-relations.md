# Relaciones entre recuerdos Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every memory the model reads shows its nearest neighbors, so the model can walk the memory until it has enough. Saving something nearly identical to an existing memory says so. Typed relations survive corrections, and Jev judges the 3 nearest neighbors when it is on.

**Architecture:**
- **Engine (clio-memory):** finds a memory's neighbors with the vector index it already has (text search when there is no vector), and copies typed relations from one memory to another.
- **Library agent layer:** filters neighbors to what the session may read and formats them. Both tool layers show them: the library's MCP (`src/tools.rs`) and Clio's tool host (`clio-toolhost`, fed by `clio-brain`'s `LinkPort`). Neither stores neighbors.

**Tech Stack:** Rust 2024; clio-memory (engine, SQLite, Granite); Clio (`clio-brain`, `clio-toolhost`, `clio-jev`).

**Spec:** `docs/specs/2026-11-04-memory-relations-design.md`. Evidence: `clio-memory/bench/relations/results.md`.

## Global Constraints

- **Neighbors are computed when read, never stored.** They are active, current memories of the scopes the session reads, never the memory itself.
- **Exact text the model sees:**
  - `memory_get` ends with one line, up to 5 neighbors: `Cerca: m29 «preview» · m5 «preview»`
  - a neighbor whose origin is `externo` is written as `m61 (externo) «preview»`
  - each search result line ends with ` · cerca: m29, m5, m61` (3 ids, no text)
  - the save warning is ` · muy parecida a m5 «preview»: si la corrige o la contradice, usa memory_update o memory_consolidate`
- **Preview:** the title, else the start of the text, cleaned like any returned text (`clean_text` / the tool host's `tidy`) and cut to 60 characters.
- **`memory_context` does not change.**
- **Limits stay as they are:** 4096 bytes per answer, 20 reads and 24 KiB per session (S-47), with neighbors included. `memory_get` keeps taking one `id`.
- **Taint, in Clio:** a neighbor that is not the person's own taints the session, like any returned memory (S-05).
- **Near-duplicate floor:** `granite-embedding-311m-multilingual-r2@384` → 0.88 (re-measured in Rust in Task 3). No floor, so no warning, for any other model.
- **Copying relations:** typed relations are **copied** (never moved) to the new memory after `memory_update` and after consolidation. `conflicts_with` is not copied, and neither is a relation to the new memory itself or one it already has. `by` and `confidence` are kept.
- **Jev:** after the save, it judges the 3 nearest neighbors.
  - «contradice» is stored only if Jev recognizes ≥ 2 of the 3 bench contradictions and calls no other bench pair a contradiction.
  - Every other type is stored only if its precision on the bench is ≥ 80 %.
- **Process:**
  - in both repos: fmt, `clippy --all-targets --all-features -D warnings`, tests with `--all-features`, and `cargo deny` all green;
  - Clio also runs its gates from the earlier plans (rule coverage, check-prompts, `npm run check`);
  - merges, tags and pushes to `main` only with Iván's yes;
  - living docs are updated in the same PR; historical records are never touched.

## Review Focus

1. **A neighbor that is forgotten, archived, replaced, or from a project the session does not read.** It never appears, not even as a bare id. Tests: Task 1 (engine) and Task 2 (tools).
2. **A neighbor whose text has terminal escape sequences, line breaks, or something that looks like a secret.** Its preview comes out as clean as a normal read, and on a single line. Tests: Task 2 (MCP) and Task 4 (Clio).
3. **A memory with no vector** (old, saved without a model, or the model just changed). It still gets neighbors, from text search. A file with no memories, or with a single one, gives an empty `Cerca:` line: no line at all, never an error. Test: Task 1.
4. **A full answer.** A long memory plus its line of neighbors still fits in 4096 bytes. If it does not, the neighbor line is shortened before the memory's text is. Tests: Task 2 and Task 4.
5. **Undoing a correction** (`memory_revert`). The old memory comes back with its relations, and the copies on the reverted memory are no longer shown. Test: Task 2.

---

### Task 1: Neighbors and copying relations in the engine (clio-memory)

Branch `feat/relations`, created from `bench/relations`.

**Files:**
- Modify:
  - `src/store.rs` (trait `Store`: `fn vector_of(&self, id: MemoryId, model: &str) -> Result<Option<Vec<f32>>>`)
  - `src/sqlite/mod.rs` (implement it)
  - `src/engine.rs`
- Test: `tests/neighbors.rs` (new)

**Interfaces:**
- Produces:
  - `Engine::neighbors(&self, id: MemoryId, scopes: &[ScopeId], k: usize) -> Result<Vec<(MemoryRow, f32)>>`
    - best first; active and current memories only (the default `SearchOpts` visibility); never `id` itself;
    - with the current embedder and a stored vector of its model, it calls `VectorIndex::nearest` for each scope with that vector and merges by score;
    - otherwise it calls `Engine::search` for each scope with the memory's content, and merges.
  - `Engine::copy_relations(&self, from: MemoryId, to: MemoryId) -> Result<usize>`
    - copies the `RelationRow`s of `from` to `to`, on whichever side `from` is;
    - skips `ConflictsWith`, skips relations whose other end is `to`, and skips pairs `to` already has;
    - returns how many it copied.

- [ ] **Step 1: Write the failing tests** in `tests/neighbors.rs`. Use a test embedder (copy `Buckets` from Clio's `tests/memory_port.rs`) on a fresh file.
  - `neighbors_are_the_nearest_active_memories_of_the_scopes_read_and_never_itself`: 4 memories in scope A and 1 in scope B. Asking for neighbors of m1 in `[A]` gives 3 rows, without m1 and without B's memory. After `forget` of one of them, it no longer appears.
  - `a_memory_without_a_vector_gets_its_neighbors_from_text_search`: engine without an embedder, two memories that share the word «restic»; each is the other's first neighbor.
  - `a_lone_memory_has_no_neighbors`: empty `Vec`, no error.
  - `relations_are_copied_except_contradictions_and_self`: m1 related to m2, m1 conflicts with m3, m1 related to m4. `copy_relations(m1, m4)` returns 1 (only m2). m4 is now related to m2 with the same `by` and `confidence`, and m1 keeps all three.
- [ ] **Step 2: Run** `cargo test --locked --all-features --test neighbors`. Expected: FAIL to compile (no `neighbors`).
- [ ] **Step 3: Implement** `Store::vector_of`, `Engine::neighbors` and `Engine::copy_relations` as in Interfaces. `copy_relations` takes the write guard, like `relate`.
- [ ] **Step 4: Run** the same command. Expected: PASS. Then run the full gates.
- [ ] **Step 5: Commit** `feat(engine): a memory's neighbors, and copying its relations`.

### Task 2: Neighbors in the library's MCP tools, and relations that follow corrections

**Files:**
- Modify:
  - `src/agent/memory.rs`
  - `src/tools.rs` (`read`, `render`, `save` for `memory_update`, `consolidate`)
  - `src/mcp.rs` (`INSTRUCTIONS`)
  - `skill/SKILL.md`
- Test: `tests/relations_tools.rs` (new; drive `tools::Session` like the existing tool tests do)

**Interfaces:**
- Consumes: `Engine::neighbors` and `Engine::copy_relations` from Task 1.
- Produces:
  - `pub fn preview(row: &MemoryRow) -> String` in `src/agent/memory.rs`, re-exported as `clio_memory::agent::preview`. It gives the title or the start of the text, through `clean_text`, cut to 60 chars with `…`.
  - `agent::Memory::neighbors(&self, row: &MemoryRow, k: usize) -> Vec<MemoryRow>`. It calls `Engine::neighbors` with `self.reading_scopes()` and keeps those that `self.get` returns. On errors it returns empty.
  - `agent::Memory::update` calls `engine.copy_relations(old, new)` when the outcome is `OpKind::Update`. `tools.rs` `consolidate` calls it for each merged id, onto the new memory.

- [ ] **Step 1: Write the failing tests:**
  - `memory_get_ends_with_the_neighbors_line`: 3 related memories. The answer to `memory_get` for one contains `Cerca: m` with 2 ids, and not its own id.
  - `an_external_neighbor_is_marked`: the neighbor saved with origin `External` appears as `m{n} (externo) «`.
  - `a_neighbor_preview_is_cleaned_and_on_one_line`: a neighbor with title `"hola\u{1b}[31m rojo\nlinea"`. The answer has no `\u{1b}`, and the neighbors line has no line break.
  - `search_lines_carry_three_neighbor_ids`: each result line of `memory_search` ends with `· cerca: m` followed by at most 3 ids.
  - `the_answer_cap_holds_with_neighbors`: a 1,600-character memory with 5 neighbors that have long titles. The answer is ≤ 4096 bytes and still holds the memory's text.
  - `a_correction_keeps_the_relations_and_undoing_it_brings_them_back`: m1 related to m2 (engine `relate`). Run `memory_update` on m1 (gives m3). The relations of m3 now include m2. `memory_revert` of that change makes m1 active again, its notes still say `relacionada con m2`, and m3's are not shown (m3 is not active).
  - `consolidating_carries_the_relations_of_both`.
- [ ] **Step 2: Run** `cargo test --locked --all-features --test relations_tools`. Expected: FAIL.
- [ ] **Step 3: Implement.** In `render`:
  - for `memory_get`, append the `Cerca:` line after the memory line, inside the same data wrapper;
  - for search, add the 3 ids to each line;
  - if the answer would exceed the cap, shorten the `Cerca:` line first (fewer neighbors), never the memory below what it shows today.

  In `INSTRUCTIONS` and `skill/SKILL.md`, add one sentence: each memory brings the memories near it; open them with `memory_get` when they serve what you are looking for, and stop when you have enough.
- [ ] **Step 4: Run** the test file, then the full gates. Expected: PASS.
- [ ] **Step 5: Commit** `feat(tools): each memory shows its neighbors; relations follow corrections`.

### Task 3: The warning when saving, the threshold measured in Rust, and release v0.4.0

**Files:**
- Modify:
  - `src/agent/memory.rs`
  - `src/tools.rs` (`with_vectors` → the warning, after the queue)
  - `README.md` (neighbors, the warning)
  - `Cargo.toml` (version `0.4.0`)
  - `bench/relations/results.md` (the Rust measurement)
- Create: `examples/relations_floor.rs` (with feature `granite`; `[[example]]` with `required-features`)
- Test: `tests/relations_tools.rs`

**Interfaces:**
- Produces:
  - `pub fn near_duplicate_floor(model: &str) -> Option<f32>` in `src/agent/memory.rs`. It returns `Some(0.88)` for `GRANITE_ID` (adjusted to the Step 1 measurement) and `None` for anything else.
  - `agent::Memory::near_duplicate_at(&self, id: MemoryId, floor: f32) -> Option<MemoryRow>`. It returns the first neighbor when its score is ≥ `floor`.
  - `agent::Memory::near_duplicate(&self, id: MemoryId) -> Option<MemoryRow>`. It calls `near_duplicate_at` with the floor of the embedder in use, and returns `None` without an embedder or a floor.

- [ ] **Step 1: Measure.** `examples/relations_floor.rs` embeds `bench/embeddings/memories.jsonl` and `bench/relations/extra.jsonl` with `Granite::load` and prints the 10 most similar pairs. Run `CLIO_MEMORY_GRANITE_DIR=~/.cache/clio-models/granite-embedding-311m-multilingual-r2 cargo run --release --features granite --example relations_floor`.
  - Expected: the 3 contradictions (m2–m62, m5–m61, m8–m63) are among the top 5.
  - Take as the floor the lowest of those 3, rounded down to 0.01. Write it in `results.md`.
- [ ] **Step 2: Write the failing tests:**
  - `near_duplicate_at_finds_the_twin_only`: with a test embedder, two texts with the same vector and a third that is different. `near_duplicate_at(second, 0.99)` is the first memory, and `near_duplicate_at(third, 0.99)` is `None`.
  - `saving_a_near_duplicate_says_so`: with a test embedder whose `model_id()` is `GRANITE_ID` (handmade vectors), saving two texts with the same vector through `memory_save` makes the second answer contain `muy parecida a m1 «`.
  - `no_floor_no_warning`: with model id `"buckets"`, the same saves give no warning.
- [ ] **Step 3: Run** the tests. Expected: FAIL.
- [ ] **Step 4: Implement** both functions. In `save` (not in `memory_session_summary`), after `embed_queue`, append the warning when `near_duplicate(o.memory)` is `Some`.
- [ ] **Step 5: Run** the full gates, and the Granite tests with `CLIO_MEMORY_GRANITE_DIR` set. Expected: PASS.
- [ ] **Step 6: Commit** `feat: saving something nearly identical says so; v0.4.0`. Push the branch and open a PR that says what changes and what was measured.
- [ ] **Step 7: With Iván's yes:** merge, tag `v0.4.0` on the merge commit, and push the tag.

### Task 4: Clio shows the same (Pi chat and `clio memory mcp`)

Branch `feat/memory-relations`, from `main`.

**Files:**
- Modify:
  - `crates/clio-brain/Cargo.toml` (clio-memory `rev` = the commit of `v0.4.0`), `Cargo.lock`
  - `crates/clio-toolhost/src/memory.rs`
  - `crates/clio-brain/src/memory_link.rs`
  - `crates/clio-brain/src/memory_port.rs`
  - `docs/uso-local.md`
- Test:
  - `crates/clio-toolhost/tests/` (the memory tool tests that already exist; add to them)
  - `crates/clio-brain/tests/memory_port.rs`

**Interfaces:**
- Consumes: `Engine::neighbors`, `Engine::copy_relations`, `clio_memory::agent::{preview, near_duplicate_floor}`.
- Produces:
  - In `clio-toolhost`: `pub struct Near { pub id: i64, pub origin: Origin, pub preview: String }`.
  - `MemoryItem` gains `pub near: Vec<Near>`; `MemoryItem::plain` gives it empty.
  - `Proposed` gains `pub similar: Option<Near>`.
  - In `clio-brain`: `MemoryLink::neighbors(&self, row: &MemoryRow, k: usize) -> Vec<MemoryRow>`, filtered by `self.get`, the same way as the library.
  - `MemoryLink::near_duplicate(&self, id: MemoryId) -> Option<MemoryRow>`.
  - `LinkPort::item` fills `near`: 5 for a single get, 3 for search results.

- [ ] **Step 1: Write the failing tests:**
  - toolhost, with a fake port: `memory_get_shows_the_neighbors_line`, `an_external_neighbor_is_marked`, `a_neighbor_preview_is_cleaned_and_on_one_line`, `search_lines_carry_three_neighbor_ids`, `the_answer_cap_holds_with_neighbors`;
  - toolhost: `a_neighbor_that_is_not_the_persons_taints_the_session`. The item read is `Origin::Person` and one neighbor is `Origin::Agent`, so the session ends up tainted;
  - toolhost: `a_near_duplicate_save_says_so`. A `Proposed` with `similar` gives the text from Global Constraints;
  - clio-brain: `a_correction_in_clio_keeps_the_relations` (`update_as_agent`) and `a_merge_in_clio_carries_the_relations`.
- [ ] **Step 2: Run** `cargo test --workspace --all-features`. Expected: FAIL.
- [ ] **Step 3: Implement.** In the tool host, the formatting and taint mirror the library: same text, and preview cleaned with `tidy`. In `memory_link`:
  - `update_as_agent` calls `copy_relations` on `OpKind::Update`;
  - `memory_port` `consolidate` calls it for each merged id;
  - after `embed_now`, `propose` fills `similar` from `near_duplicate`.
- [ ] **Step 4: Run** all of Clio's gates. Expected: PASS.
- [ ] **Step 5: Commit** `feat(memory): Clio shows each memory's neighbors and warns on near duplicates`. Open the PR. Merge only with Iván's yes.

### Task 5: Jev judges the 3 neighbors, after measuring it (needs the vault)

**Files:**
- Modify:
  - `crates/clio-brain/src/memory_port.rs` (the relation step moves to after the save)
  - `crates/clio-brain/src/main.rs` (`mod tests`: the measurement as an `#[ignore]` test, since `cli::jev::client` lives in the binary)
  - `clio-memory/bench/relations/results.md`
- Test: `crates/clio-brain/tests/memory_port.rs`

**Interfaces:**
- Consumes:
  - `MemoryJudge::relation(&self, text: &str, other: &str) -> Option<(Relation, f32)>` (exists);
  - `MemoryLink::neighbors` (Task 4);
  - `MemoryLink::relate_judged` (exists).
- Produces: after a save of kind `Add`, `propose` asks Jev about each of the 3 nearest neighbors and stores the verdict whose type is allowed.
  - Allowed types: `const JEV_TYPES: &[Relation]` in `memory_port.rs`, filled from Step 2's result.
  - Jev is asked before the save only for the guard and the place, as today.

- [ ] **Step 1: Ask Iván to open the vault** (`clio vault start`, his passphrase). Without it, this task waits, and the others are not affected.
- [ ] **Step 2: Measure** with `jev_relations_on_the_bench` (ignored test in `main.rs`).
  - For each candidate pair of `bench/relations` (the 5 Granite neighbors of each memory, from the `vec.npz` that `run.py` can write, or recomputed in Rust with `Granite`), call `MemoryJudge::relation`.
  - Print, per type: precision and recall against `pairs.jsonl`, and the verdict on the 3 contradictions.
  - Run `CLIO_BENCH_RELATIONS=../clio-memory/bench/relations cargo test -p clio-brain --bin clio jev_relations_on_the_bench -- --ignored --nocapture`.
  - Write the table in `results.md`, and decide `JEV_TYPES` with the bar from Global Constraints.
- [ ] **Step 3: Write the failing test** `jev_judges_the_three_nearest_after_the_save`, with a fake judge that answers `Related` for everything. Saving next to 4 similar memories leaves exactly 3 relations, with `by = "jev"`. A type not in `JEV_TYPES` stores nothing.
- [ ] **Step 4: Implement**, then run Clio's gates. Expected: PASS.
- [ ] **Step 5: Commit, PR, and merge with Iván's yes.** The result of the measurement goes in the PR description.

### Task 6: On this machine

- [ ] **Install:** in clio-memory, `cargo install --locked --path . --features model2vec,granite` from the `v0.4.0` tag; in Clio, `just install` from `main`.
- [ ] **Verify:**
  - from a new Claude Code session, `memory_get` of a real memory (for example the one about Resizable BAR) shows neighbors that make sense;
  - in Clio, `memory_get` from the Pi chat (or through `clio memory mcp`) shows the same.
- [ ] **Update clio-memory memory m18:** the relations are done; say what was decided and where the test is.
