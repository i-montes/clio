# clio-memory vectors and hybrid recall Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add piece 2 of clio-memory: stored vectors with an exact Rust-safe scan, `embed_pending`, hybrid `recall` (FTS + vectors, RRF, recency, abstention, token budget), fixed blocks and `context()` in compact or JSON format.

**Architecture:** Vectors are derived data in a `vectors` table (not in the operation log). `VectorIndex::nearest` scans rows already filtered by scope and visibility, dots the raw BLOB bytes against the query and keeps the top k. Text and vector search share one SQL visibility predicate. `Engine` gets an optional `Embedder` and `TokenCounter`; `recall` fuses and filters, `context` renders blocks then recall under separate budgets.

**Tech Stack:** Rust 2024 (`unsafe_code = "forbid"`), existing deps (`rusqlite`, `serde_json`, `rust-stemmers`, ...), optional `model2vec-rs` behind feature `model2vec`.

**Spec:** `docs/specs/2026-10-29-clio-memory-recall-design.md` (this piece), `docs/specs/2026-10-29-clio-memory-core-design.md` (what exists) and `docs/specs/clio-memory-spec-v0.2.md`.

## Global Constraints

- Crate `crates/clio-memory`, branch `feat/clio-memory-recall` (stacked on `feat/clio-memory-core`); commits local only, never push.
- `unsafe_code = "forbid"`, `clippy::unwrap_used = "deny"` (test helper modules start with `#![allow(clippy::unwrap_used)]`); `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features` and `scripts/rule-coverage.sh` must pass. `cargo deny`, `cargo audit`, `just` and `gitleaks` are not installed: do not claim they ran.
- Storage stays private (`store` and `sqlite` modules are not exported); everything is reached through `Engine`.
- Sync API, times only from `Clock`, vectors stored as little-endian `f32`, L2-normalized (dot product = cosine); one model id per vector row, never compared across models.
- `propose` is untouched and never calls the embedder. The crate never opens network connections.
- Schema additions go into the version-1 DDL without bumping `VERSION` (no real v1 files exist).
- Review/promote-style powers stay person-only (`require_person`); block editing is one of them.
- A rule id (S-xx) appears in a comment above each test that proves it (`scripts/rule-coverage.sh` reads `tests/*.rs`).

## Review Focus

- A query that is empty, whitespace, or full of FTS syntax must not error from `recall`, with or without an embedder; empty means `abstained = true`. (Task 4)
- An embedder that returns the wrong number of vectors, a wrong dimension, NaN or an all-zero vector must fail with `BadVector` and store nothing for that batch, leaving earlier vectors intact. (Task 2, Task 3)
- Vectors of another model are never compared; switching models with a different dimension works. (Task 2, Task 3)
- A block entry whose memory was replaced (follow the chain), archived, or moved out of active state must never render a stale or quarantined fact. (Task 7)
- A budget smaller than one line or than a block header (including 0) yields empty sections and no panic or half line; dates before 1970 or far in the future format without panic. (Task 5, Task 7)

---

## File Structure

```
crates/clio-memory/
  Cargo.toml                 optional dep model2vec-rs, feature `model2vec`
  src/embed.rs               Embedder trait, unit_vector()
  src/vector.rs              dot(), to_blob(), VectorIndex trait
  src/token.rs               TokenCounter, ApproxTokens
  src/recall.rs              RRF fusion + recency (pure)
  src/format.rs              year-month, compact line, JSON item
  src/model2vec.rs           (feature) Model2VecEmbedder::from_dir
  src/model.rs, error.rs     new types and errors
  src/store.rs, src/sqlite/{mod,schema}.rs   new queries and tables
  src/engine.rs              embed_pending, recall, blocks, context
  tests/common/mod.rs        HashEmbedder + fixtures
  tests/embed.rs  tests/recall.rs  tests/context.rs
```

---

### Task 1: Shared visibility predicate and `kind` filter

**Files:** Modify `src/model.rs`, `src/sqlite/mod.rs`; Test: `src/sqlite/tests.rs`.

**Produces:** `SearchOpts` gains `pub kind: Option<MemoryType>` (default `None`). In `sqlite/mod.rs`, `fn visibility(opts: &SearchOpts) -> String` returns the SQL fragment (using named params `:as_of`, `:known_at`, `:kind`, over alias `m`) covering state set, validity, knowledge and kind; `search` is rewritten on named params and uses it. Task 2 reuses it.

- [ ] **Step 1: Failing test** `search_filters_by_kind` in `src/sqlite/tests.rs`: two active memories matching "dato", one `Semantic`, one `Procedural`; `SearchOpts { kind: Some(Procedural), ..default }` returns only the procedural one; `kind: None` returns both.
- [ ] **Step 2:** Run `cargo test -p clio-memory --lib search_filters_by_kind`. Expected: FAIL (no field `kind`).
- [ ] **Step 3:** Add the field; extract `visibility`; rewrite `search` with `rusqlite::named_params!`. All existing search tests must stay green.
- [ ] **Step 4:** Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] **Step 5:** Commit `refactor(memory): shared visibility predicate and kind filter`.

---

### Task 2: Embedder trait, vector math, vector storage and exact scan

**Files:** Create `src/embed.rs`, `src/vector.rs`; Modify `src/error.rs`, `src/store.rs`, `src/sqlite/{mod,schema}.rs`, `src/lib.rs`; Test: unit tests inline and in `src/sqlite/tests.rs`.

**Produces:**
```rust
// embed.rs (public)
pub trait Embedder: Send + Sync { fn model_id(&self) -> &str; fn dim(&self) -> usize; fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>; }
pub(crate) fn unit_vector(v: Vec<f32>, dim: usize) -> Result<Vec<f32>>;   // BadVector on wrong length, NaN/inf, or zero norm; returns L2-normalized
// vector.rs
pub(crate) fn dot(blob: &[u8], q: &[f32]) -> f32;       // 8 accumulators over 32-byte chunks, scalar tail
pub(crate) fn to_blob(v: &[f32]) -> Vec<u8>;
pub(crate) trait VectorIndex { fn nearest(&self, scope: ScopeId, model: &str, query: &[f32], opts: &SearchOpts, k: usize) -> Result<Vec<(MemoryId, f32)>>; }  // best first
// Error gains: BadVector, NoEmbedder, Embedding(String)
// Store (which now requires VectorIndex as supertrait) gains:
fn pending_embeddings(&self, model: &str, limit: usize) -> Result<Vec<(MemoryId, String)>>; // not Archived, no vector for model, ordered by id
fn put_vectors(&self, model: &str, dim: usize, vectors: &[(MemoryId, Vec<f32>)]) -> Result<()>; // one transaction, INSERT OR REPLACE
```
Table `vectors(memory INTEGER NOT NULL, model TEXT NOT NULL, dim INTEGER NOT NULL, vec BLOB NOT NULL, PRIMARY KEY(memory, model)) WITHOUT ROWID`. `nearest` joins `memories m` to `vectors v` on `v.memory = m.id AND v.model = :model AND v.dim = :dim`, filters `m.scope = :scope AND <visibility>`, streams rows with `dot` and keeps the top k in a small sorted `Vec`.

- [ ] **Step 1: Failing tests:** `dot_matches_naive_sum` (random-ish 256 and 37-length vectors, `dot(to_blob(a), b)` within 1e-4 of the naive sum); `unit_vector_rejects_bad_input` (wrong length, a NaN, all zeros → `BadVector`); `unit_vector_normalizes` (norm 1 ± 1e-6). In `src/sqlite/tests.rs`: `nearest_respects_scope_state_and_time` (three memories with hand-made unit vectors across two scopes, one archived, one replaced with validity [100,200): default call returns only active ones of the scope; `as_of: Some(150)` includes the replaced one; `known_at` hides one learned later), `nearest_ignores_other_models` (vector stored under model "a" is invisible to a query with model "b"; different `dim` rows are skipped), `pending_embeddings_skips_archived_and_embedded` and `put_vectors_replaces_and_is_atomic` (a batch containing an unknown memory id fails and stores none of the batch).
- [ ] **Step 2:** Run `cargo test -p clio-memory --lib`. Expected: FAIL (items missing).
- [ ] **Step 3:** Implement the signatures above; register modules in `lib.rs` (`pub use embed::Embedder;`, keep `vector`/`store` private).
- [ ] **Step 4:** Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(memory): embedder trait, vector storage and exact nearest-neighbour scan`.

---

### Task 3: `Engine::embed_pending`

**Files:** Modify `src/engine.rs`; Test: `tests/embed.rs`, `tests/common/mod.rs`.

**Produces:**
```rust
impl Engine {
  pub fn with_embedder(self, e: Arc<dyn Embedder>) -> Self;
  pub fn with_token_counter(self, c: Arc<dyn TokenCounter>) -> Self;   // stub field now, used in Task 5
  pub fn embed_pending(&self, limit: usize) -> Result<usize>;           // NoEmbedder without an embedder
}
```
Behavior: reads pending ids (no write lock held), embeds in batches of 32 with `Embedder::embed`, requires exactly one vector per text, validates each with `unit_vector(v, embedder.dim())`, then stores each batch under the write guard. A bad batch returns `BadVector` and stores nothing from that batch; earlier batches stay. Returns how many were stored.
Test helper in `tests/common/mod.rs`: `HashEmbedder { model: String, dim: usize }` implementing `Embedder` (lowercase, split on non-alphanumerics, drop a small Spanish stop list `en el la los las de un una y a`, map synonyms `auto|coche|carro → vehiculo`, hash each token into `dim` buckets, count, no normalization); `fixture_with_embedder()` returning the existing `Fixture` plus the engine rebuilt `.with_embedder(Arc::new(HashEmbedder))`.

- [ ] **Step 1: Failing tests** in `tests/embed.rs`: `embed_pending_embeds_everything_once` (3 memories: first call with limit 10 returns 3, second returns 0); `embed_pending_skips_archived`; `embed_pending_respects_limit` (limit 2 of 3 returns 2, next call 1); `changing_model_makes_everything_pending_again` (embed under model "a" dim 64, then engine with model "b" dim 32 returns the full count, and recall-side `nearest` for "b" works); `bad_embedder_output_stores_nothing` (an embedder returning an all-zero vector, one returning the wrong dimension, one returning too few vectors → `BadVector`; a later good run still embeds everything); `no_embedder_is_an_error` (`Err(NoEmbedder)`); `propose_never_embeds` (after `propose` with an embedder set, `pending_embeddings` count is unchanged until `embed_pending`; check through `embed_pending` returning 1).
- [ ] **Step 2:** Run `cargo test -p clio-memory --test embed`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit `feat(memory): embed_pending`.

---

### Task 4: Fusion and `recall`

**Files:** Create `src/recall.rs`; Modify `src/model.rs`, `src/engine.rs`; Test: `tests/recall.rs`, inline unit tests.

**Produces:**
```rust
// model.rs
pub struct RecallOpts { pub search: SearchOpts, pub min_vector_score: f32, pub min_text_score: f64, pub token_budget: Option<usize> }
//   Default: search = SearchOpts::default() (its `limit` is the number of results returned), min_vector_score 0.30, min_text_score 0.25 (provisional; tune in Step 5), token_budget None
pub struct RecallHit { pub memory: MemoryRow, pub score: f64, pub text: Option<f64>, pub vector: Option<f32>, pub recency: f64 }
pub struct Recall { pub hits: Vec<RecallHit>, pub abstained: bool }
// recall.rs (pure)
pub(crate) const RRF_K: f64 = 60.0;
pub(crate) fn recency(now: i64, known_since: i64) -> f64;   // in (0,1], 1 = just learned, halves every 30 days
pub(crate) fn fuse(text: &[(MemoryId, f64)], vector: &[(MemoryId, f32)], now: i64, known_since: impl Fn(MemoryId) -> i64) -> Vec<(MemoryId, f64, Option<f64>, Option<f32>, f64)>;
//   score = (sum of 1/(RRF_K + rank) over lists the id appears in, rank from 1) * (1.0 + 0.1 * recency); sorted best first, ties by id
// Engine
pub fn recall(&self, scope: ScopeId, query: &str, opts: &RecallOpts) -> Result<Recall>;
```
`recall` flow: pool size `max(limit * 5, 50)`; text hits via `Store::search` (pool), keep those with `score >= min_text_score`; if an embedder is set, embed the query, `unit_vector` it, `nearest` (pool), keep `cos >= min_vector_score`; fuse; truncate to `search.limit`; `abstained = hits.is_empty()` before any budget cut; empty or whitespace query → `abstained = true` without touching the embedder. Budget handling arrives in Task 5.

- [ ] **Step 1: Failing tests:** unit: `rrf_adds_reciprocal_ranks` (id in both lists at rank 1 scores `2/61` before the recency factor), `recency_breaks_ties_for_equal_relevance`, `recency_halves_every_30_days`. Integration (`tests/recall.rs`, fixture with HashEmbedder, memories "vive en Medellín", "prefiere respuestas breves", "tiene un coche rojo", all embedded): `hybrid_finds_what_text_misses` (query "auto" returns the car memory with `text == None` and `vector.is_some()`); `text_only_without_embedder` and `text_only_before_embed_pending` (memory without a vector still found by text); `t12_abstains_when_nothing_is_relevant` (queries "receta de tiramisú", "clima en Tokio" → `abstained == true`, no hits; queries "dónde vive", "qué coche tiene" → not abstained and the right memory first); `t04_external_memory_hidden_from_recall_until_promoted` (S-25 comment; also with embedding present); `t05_recall_never_crosses_scopes`; `t06_t17_recall_with_dates` (replaced fact found with `as_of`, learned-later fact hidden with `known_at`); `recall_hit_carries_score_breakdown`; `hostile_and_empty_queries_do_not_error` (`""`, `"   "`, `a" OR NEAR( * -x`, `***` with and without embedder; empty ones abstain); `limit_caps_results`.
- [ ] **Step 2:** Run `cargo test -p clio-memory`. Expected: FAIL.
- [ ] **Step 3:** Implement `recall.rs` and `Engine::recall`.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Adjust the two default thresholds only as far as needed for `t12` to pass and the positive cases to stay found (lean towards abstaining); record the final values in a code comment as provisional.
- [ ] **Step 6:** Commit `feat(memory): hybrid recall with abstention`.

---

### Task 5: Token counting, formats and recall budget

**Files:** Create `src/token.rs`, `src/format.rs`; Modify `src/model.rs`, `src/engine.rs`; Test: inline unit tests, `tests/recall.rs`.

**Produces:**
```rust
// token.rs
pub trait TokenCounter: Send + Sync { fn count(&self, text: &str) -> usize; }
pub struct ApproxTokens;   // chars().count().div_ceil(4)
// model.rs
pub enum Format { Compact, Json }
// format.rs (crate-private)
pub(crate) fn year_month(secs: i64) -> String;                       // "2026-03"; 1970-01 for 0; no panic for any i64 (days-from-civil arithmetic, no dependency)
pub(crate) fn compact_line(m: &MemoryRow, historical: bool) -> String;
pub(crate) fn json_item(m: &MemoryRow) -> String;                    // serde_json of the row
```
`compact_line`: `[m<id>] <content>` with, in this order, ` · desde <ym>` only when `valid_from != known_since`, ` · hasta <ym>` when `valid_to` is set, ` · sabido <ym>` when `historical`; a quarantined row is `[m<id>] ⚠ cuarentena: <content>...`. `recall` applies `token_budget`: hits are kept in order while the running total of `counter.count(compact_line(..))` fits, stopping at the first that does not fit (never skipping to smaller ones); if nothing fits the result is empty with `abstained = false`.

- [ ] **Step 1: Failing tests:** unit: `approx_tokens_rounds_up` (`""`→0, `"abcd"`→1, `"abcde"`→2); `year_month_known_dates` (0→`1970-01`, 2026-03-15 UTC → `2026-03`, -1 → `1969-12`, `i64::MAX` and `i64::MIN` do not panic); `compact_line_variants` (desde omitted when equal; present otherwise; hasta; sabido; quarantine marker; no trailing separators). `tests/recall.rs`: `budget_stops_at_first_line_that_does_not_fit` (budget fits exactly the top 2 of 3 hits → 2 hits, in rank order); `budget_too_small_returns_empty_not_abstained` (budget 1 → empty, `abstained == false`); `t20_compact_is_cheaper_than_json` (ten memories: total `ApproxTokens` of compact lines is strictly less than of JSON items).
- [ ] **Step 2:** Run `cargo test -p clio-memory`. Expected: FAIL.
- [ ] **Step 3:** Implement; `Engine` holds `Arc<dyn TokenCounter>` defaulting to `ApproxTokens` (replace the stub from Task 3).
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit `feat(memory): token counter, compact and JSON formats, recall budget`.

---

### Task 6: Fixed blocks

**Files:** Modify `src/model.rs`, `src/error.rs`, `src/store.rs`, `src/sqlite/{mod,schema}.rs`, `src/engine.rs`; Test: `tests/context.rs`.

**Produces:**
```rust
pub struct BlockId(pub i64);
pub struct Block { pub id: BlockId, pub scope: ScopeId, pub name: String, pub budget_tokens: usize }
// Engine (person-only: NotAllowed otherwise; secret detector on the name)
pub fn create_block(&self, scope: ScopeId, name: &str, budget_tokens: usize, actor: ActorId) -> Result<BlockId>;
pub fn block_set(&self, block: BlockId, memories: &[MemoryId], actor: ActorId) -> Result<()>;   // replaces the list; each memory must be in the block's scope and Active (else InvalidState); order = priority
pub fn block_get(&self, block: BlockId) -> Result<Vec<MemoryId>>;
pub fn blocks(&self, scope: ScopeId) -> Result<Vec<Block>>;                                      // ordered by id
// Store: create_block, block, blocks, set_block_memories, block_memories
```
Tables: `blocks(id INTEGER PRIMARY KEY, scope INTEGER NOT NULL REFERENCES scopes(id), name TEXT NOT NULL, budget_tokens INTEGER NOT NULL, UNIQUE(scope, name))`, `block_memories(block INTEGER NOT NULL REFERENCES blocks(id), position INTEGER NOT NULL, memory INTEGER NOT NULL, PRIMARY KEY(block, position))`. Blocks are configuration, not logged.

- [ ] **Step 1: Failing tests** (`tests/context.rs`): `blocks_roundtrip_in_order`; `block_set_replaces_the_list`; `s25_agent_cannot_edit_blocks` (S-25 comment; `NotAllowed` for `create_block` and `block_set`); `quarantined_memory_cannot_enter_a_block` (`InvalidState`; list unchanged); `memory_from_another_scope_is_rejected`; `block_name_with_secret_is_rejected` (S-18 comment); `duplicate_block_name_in_scope_is_an_error`.
- [ ] **Step 2:** Run `cargo test -p clio-memory --test context`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit `feat(memory): fixed blocks`.

---

### Task 7: `context()`

**Files:** Modify `src/model.rs`, `src/engine.rs`; Test: `tests/context.rs`.

**Produces:**
```rust
pub struct ContextOpts { pub query: Option<String>, pub recall: RecallOpts, pub recall_budget: usize, pub format: Format }
// Default: query None, recall RecallOpts::default(), recall_budget 400, format Compact
pub fn context(&self, scope: ScopeId, opts: &ContextOpts) -> Result<String>;
```
Compact output: for each block with at least one line, `## <name>` then its lines; then, if `query` is set and recall returned hits not already shown in a block, `## relevante` then its lines; sections separated by a blank line; nothing when there is nothing. A block's header counts against its own `budget_tokens`; its lines are added in position order, stopping at the first that does not fit; a block whose header or first line does not fit is omitted whole. Recall uses `recall_budget` the same way and ignores `RecallOpts::token_budget`. A block entry is resolved by following `replaced_by` until an `Active` row (give up after 32 hops); entries ending at an archived/quarantined/missing row are omitted; the same memory is shown once. `Format::Json` renders `{"blocks":[{"name":...,"memories":[<row json>...]}],"recall":[<row json>...]}` with the same budget rules counted on each item's JSON; `historical` for the compact `sabido` field is `opts.recall.search.known_at.is_some()`.

- [ ] **Step 1: Failing tests:** `t13_context_never_exceeds_block_budget_nor_cuts_a_line` (several budgets 1..60: for each block, `ApproxTokens` of header+lines ≤ budget, every line is a complete `[m..]` line); `block_follows_updates` (block holds the id of "vive en Medellín"; after an UPDATE to "vive en Bogotá" the block renders Bogotá, once); `block_omits_archived_and_quarantined_successors` (entry archived → block omitted, no header); `separate_budgets` (a large identity block does not reduce what `recall_budget` allows, and a tiny `recall_budget` does not remove block lines); `recall_section_skips_memories_already_in_a_block`; `no_query_means_blocks_only`; `zero_budgets_give_empty_output_without_panic`; `json_format_is_valid_json_with_the_same_selection` (parse with `serde_json::from_str::<serde_json::Value>`; block and recall item counts equal the compact line counts for the same budgets); `context_is_empty_for_empty_scope`.
- [ ] **Step 2:** Run `cargo test -p clio-memory --test context`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit `feat(memory): context() with fixed blocks and recall`.

---

### Task 8: `model2vec` feature, performance check, docs, gates

**Files:** Modify `Cargo.toml`, `src/lib.rs`, `crates/clio-memory/README.md`; Create `src/model2vec.rs`; Test: `tests/embed.rs`, `tests/recall.rs`.

- [ ] **Step 1: Failing test** `vectors_live_in_the_same_0600_file` in `tests/embed.rs` (S-24 comment): after `embed_pending`, the database file is mode 0600, `dump_all` finds the `vectors` table inside it, and `rebuild_projection` leaves the vectors usable (`recall` by vector still finds the memory). Run `cargo test -p clio-memory --test embed vectors_live`. Expected: FAIL only if something is missing; if it passes on first run, note that it pins behavior from Tasks 2 and 3.
- [ ] **Step 2:** Add `model2vec-rs = { version = "0.3", optional = true }` and `[features] model2vec = ["dep:model2vec-rs"]` (keep `telemetry`). In `src/model2vec.rs` (compiled only with the feature) implement `pub struct Model2VecEmbedder` with `pub fn from_dir(path: &Path) -> Result<Self>` loading a local model directory (no network, no download), `model_id` = `"model2vec:<directory name>"`, `dim` read from the loaded model, `embed` mapping library errors to `Error::Embedding(String)`. Check the library's real API from its source in `~/.cargo/registry` before writing; if the loader needs a network-capable argument, pass the local path only. Run `cargo check -p clio-memory --features model2vec` and `cargo clippy -p clio-memory --features model2vec -- -D warnings`. Expected: both pass.
- [ ] **Step 3:** Add `#[ignore]` test `model2vec_real_model_embeds_spanish` (needs env var `CLIO_MODEL2VEC_DIR`; skips with a message when unset) and `#[ignore]` test `t25_recall_over_50k_memories_is_fast` in `tests/recall.rs`: create scope and actor through the engine, drop it, bulk-insert 50 000 rows directly with `rusqlite` in one transaction into `memories`, `memories_fts` (column list in `src/sqlite/schema.rs`) and `vectors` (random unit vectors, 256 dims, deterministic xorshift), reopen the engine with a `HashEmbedder` of dim 256, run `recall` five times and assert the median is under 150 ms. Run it with `cargo test -p clio-memory --release --test recall t25 -- --ignored --nocapture` and record the measured median in the commit message. If it fails the threshold, report the number instead of changing the threshold.
- [ ] **Step 4:** Update `crates/clio-memory/README.md` (new pieces: `embed_pending`, `recall`, blocks and `context`, the `model2vec` feature and that models are loaded from a local directory) and its status paragraph.
- [ ] **Step 5:** Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features`, `scripts/rule-coverage.sh`. Expected: all pass; rule coverage lists S-24 as referenced.
- [ ] **Step 6:** Commit `feat(memory): model2vec embedder, perf check and docs`.

---

## Self-review notes

- Coverage: acceptance 4, 5, 6, 12, 13, 17, 20, 25 map to Tasks 4, 5, 7, 8; S-24 to Task 8; S-18 and S-25 to Task 6; embedder/model/vector behavior to Tasks 2-3; formats and budgets to Tasks 5 and 7.
- Calls the plan makes beyond the design: block editing needs a person and an active same-scope memory (as designed); memories shown in a block are not repeated in the recall section; `RecallOpts::token_budget` is ignored inside `context`; the same-memory dedupe in blocks.
