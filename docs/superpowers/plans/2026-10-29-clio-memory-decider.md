# clio-memory decider cascade Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add piece 3: a `Decider` trait and cascade in `propose` (rules first, then registered deciders in active or shadow mode), a decision log written in the same transaction as the operation, neighbors for deciders, and an opt-in `NearDuplicateDecider`.

**Architecture:** `propose` keeps its rules; only a plain "add active" verdict goes on to the registered deciders. A decider can only answer `Add` or `Review` (review = quarantine), so no decision can drop text. Every consulted decider leaves a row in `decisions`, written by the same store transaction as the operation. Decider failures (error, panic, invalid confidence) are recorded and skipped.

**Tech Stack:** Rust 2024; existing dependencies only.

**Spec:** `docs/specs/2026-10-29-clio-memory-decider-design.md` (binding), plus `docs/specs/clio-memory-spec-v0.2.md` and the earlier designs.

## Global Constraints

- Branch `feat/clio-memory`; commits local only. Gates: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features`, `scripts/rule-coverage.sh`, `cargo deny check`, `cargo audit`, gitleaks via `mise exec gitleaks@latest -- gitleaks detect --source . --no-banner`.
- `unsafe_code = "forbid"`, `clippy::unwrap_used = "deny"` (test helper modules start with `#![allow(clippy::unwrap_used)]`). No new dependencies. Storage stays private.
- Schema additions go into the version-1 DDL without bumping `VERSION`.
- Rules are never skipped or overridden by deciders; decider choices are limited to `Add` and `Review`.
- Every S-xx a test proves is written as a comment above that test.

## Review Focus

- A decider that errors, panics, returns NaN or an out-of-range confidence, or returns nothing never blocks, corrupts or loses a proposal; the text is always stored (active or quarantined). (Task 3)
- Decisions of a write that fails are not left behind, and decisions of a successful write point at its operation. (Task 3)
- A shadow decider never changes the outcome, even when it says `Review`, and still runs when an active decider already decided. (Task 3)
- Neighbors are scoped (never another scope), sorted best first, and a failing or absent embedder gives an empty list without failing the write. (Task 4)
- Rule outcomes that are not a plain active add (duplicate, untrusted origin, topic update) never consult a decider. (Task 3)

---

## File Structure

```
crates/clio-memory/src/decider.rs        Decider trait, NearDuplicateDecider (public)
crates/clio-memory/src/model.rs          Candidate, Neighbor, Question, Choice, Decision, DeciderMode, DecisionStatus, DecisionRecord
crates/clio-memory/src/store.rs          NewDecision, append_with_decisions, decisions
crates/clio-memory/src/sqlite/{mod,schema}.rs   decisions table
crates/clio-memory/src/engine.rs         with_decider, cascade, neighbors, decisions()
crates/clio-memory/src/rules.rs          rename Decision -> Verdict
crates/clio-memory/src/embed.rs          panic_message helper
crates/clio-memory/tests/{decider.rs,common/mod.rs}
docs/specs/clio-memory-spec-v0.2.md, crates/clio-memory/README.md
```

---

### Task 1: Update the spec for the product decision
**Files:** `docs/specs/clio-memory-spec-v0.2.md`.
- [ ] In section 2 step 6 and the section 3 table/text, replace the conflict marking with the decision "similar but different memories stay distinct; replacing is explicit by `topic_key`"; state that deciders choose only add or review; in section 10 replace test 14 with "two similar memories with different content coexist and `recall` returns both"; in the roadmap and objectives remove conflicts from v1 and keep the decision log and shadow mode.
- [ ] Verify with `grep -n -i "conflicto" docs/specs/clio-memory-spec-v0.2.md` that every remaining mention is the decision note or the unused-state note.
- [ ] Commit `docs: spec v0.2 follows the decider design (no conflicts in v1)`.

### Task 2: Types, trait, decision table and store support
**Files:** `src/model.rs`, `src/decider.rs`, `src/store.rs`, `src/sqlite/{schema,mod}.rs`, `src/rules.rs`, `src/lib.rs`; Test: `src/sqlite/tests.rs`.
**Produces:**
```rust
// model.rs (public; serde derives on the enums)
pub struct Candidate { pub scope: ScopeId, pub kind: MemoryType, pub content: String, pub topic_key: Option<String>, pub origin: Origin, pub actor_kind: ActorKind }
pub struct Neighbor { pub memory: MemoryRow, pub similarity: f32 }
pub struct Question { pub candidate: Candidate, pub neighbors: Vec<Neighbor> }
pub enum Choice { Add, Review }
pub struct Decision { pub choice: Choice, pub confidence: f32, pub reason: Option<String> }   // provider = the decider's name()
pub enum DeciderMode { Active, Shadow }
pub enum DecisionStatus { Decided, None, BelowThreshold, Invalid, Error }
pub struct DecisionRecord { pub id: i64, pub scope: ScopeId, pub op: Option<OpId>, pub provider: String, pub mode: DeciderMode,
  pub status: DecisionStatus, pub choice: Option<Choice>, pub confidence: Option<f32>, pub reason: Option<String>, pub acted: bool, pub at: i64 }
// decider.rs (public)
pub trait Decider: Send + Sync { fn name(&self) -> &str; fn decide(&self, q: &Question) -> Result<Option<Decision>>; }
// store.rs
pub struct NewDecision { pub scope: ScopeId, pub provider: String, pub mode: DeciderMode, pub status: DecisionStatus,
  pub choice: Option<Choice>, pub confidence: Option<f32>, pub reason: Option<String>, pub acted: bool, pub at: i64 }
fn append_with_decisions(&self, op: NewOperation, decisions: Vec<NewDecision>) -> Result<OpId>;   // one transaction; every decision gets op = the new id
fn append(&self, op: NewOperation) -> Result<OpId> { self.append_with_decisions(op, Vec::new()) }   // provided method
fn decisions(&self, scope: ScopeId, from: i64, to: i64) -> Result<Vec<DecisionRecord>>;             // at in [from, to), oldest first
```
Table `decisions(id INTEGER PRIMARY KEY AUTOINCREMENT, scope INTEGER NOT NULL REFERENCES scopes(id), op INTEGER REFERENCES operations(id), provider TEXT NOT NULL, mode TEXT NOT NULL, status TEXT NOT NULL, choice TEXT, confidence REAL, reason TEXT, acted INTEGER NOT NULL, at INTEGER NOT NULL)` plus an index on `(scope, at)`. `rules::Decision` is renamed `rules::Verdict` (private) to avoid the name clash.
- [ ] **Failing tests** in `src/sqlite/tests.rs`: `decisions_are_written_with_their_operation` (append with two decisions: both rows have `op` equal to the returned id and the right fields); `failed_append_leaves_no_decisions` (an append whose operation violates a constraint, e.g. unknown scope, with a decision: `Err`, and `decisions(scope, 0, i64::MAX)` is empty); `decisions_range_is_half_open` (rows at 100 and 200: `[100,200)` has one, `[100,201)` two, `[101,200)` none); `rebuild_projection_keeps_decisions`.
- [ ] Run `cargo test -p clio-memory --lib decisions`. Expected: FAIL.
- [ ] Implement the types, trait, table and store methods; export the new public items from `lib.rs`.
- [ ] Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] Commit `feat(memory): decider types and decision log storage`.

### Task 3: The cascade in `propose`
**Files:** `src/engine.rs`, `src/embed.rs`; Test: `tests/decider.rs`, `tests/common/mod.rs`.
**Produces:**
```rust
impl Engine {
  pub fn with_decider(self, d: Arc<dyn Decider>, mode: DeciderMode, min_confidence: f32) -> Self;  // registration order = consultation order
  pub fn decisions(&self, scope: ScopeId, from: i64, to: i64) -> Result<Vec<DecisionRecord>>;
}
// embed.rs: pub(crate) fn panic_message<T>(f: impl FnOnce() -> T) -> std::result::Result<T, String>   (catch_panic is rebuilt on it)
```
Cascade (only when the rules' verdict is a plain active add and at least one decider is registered): consult registrations in order; an active decider is skipped once an earlier active one applied a decision, a shadow one always runs; for each consulted decider record one `NewDecision`: error or panic → `Error` (reason = message); `Ok(None)` → `None`; confidence NaN or outside `[0, 1]` → `Invalid`; active with confidence below `min_confidence` → `BelowThreshold`; otherwise `Decided` (`acted` only for the first applied active one). `Review` applied → candidate state `Quarantine`, operation reason `review requested by <name>`; `Add` applied → no change. Decisions go to the store with the operation in `append_with_decisions`. `min_confidence` default for tests is 0.8. The provider name stored is `Decider::name()`.
Test helpers in `tests/common/mod.rs`: `ScriptedDecider { name, behavior, calls: Arc<AtomicUsize>, seen: Arc<Mutex<Vec<Question>>> }` with `Behavior::{Say(Choice, f32), Nothing, Fail, Panic}`.
- [ ] **Failing tests** in `tests/decider.rs`: `without_deciders_nothing_is_logged` (default behavior unchanged, `decisions` empty); `active_review_quarantines_and_is_logged` (Review at 0.9: outcome `Quarantine`, history reason contains `review requested by`, one record `Decided`/`Review`/`acted`, `record.op == Some(outcome.op)`; a person can `promote` it afterwards); `active_add_changes_nothing_but_is_logged`; `below_threshold_is_logged_not_applied` (0.5 vs default 0.8 → Active, status `BelowThreshold`, `acted == false`); `min_confidence_is_per_registration` (registered with 0.4, 0.5 applies); `shadow_logs_and_never_acts` (Shadow `Review` → Active add, record mode `Shadow`, `acted == false`); `first_active_wins_later_actives_are_skipped_shadows_still_run` (active A `Say(Add)`, active B `Say(Review)`, shadow S: outcome Active; B's `calls == 0`; S's `calls == 1`; records for A and S only); `active_nothing_falls_through_to_the_next` (A `Nothing`, B `Say(Review)` → quarantined, records `None` then `Decided`); `rules_veto_and_end_the_cascade` (S-25: an external proposal is quarantined by the rules and the decider's `calls == 0`; an exact duplicate is a NOOP with `calls == 0`; a same-topic update with `calls == 0`; no records); `decider_error_and_panic_do_not_block_the_write` (Fail and Panic: proposal stored active, records `Error` with a reason); `invalid_confidence_is_recorded_and_ignored` (NaN, 1.5, -0.1 → `Invalid`, proposal active); `a_decider_can_never_drop_the_text` (for every behavior the proposed content is present in `list(scope, None, None)`, and across all operations in `history` no kind other than Add/Update/Noop/Promote/Archive/Revert appears: there is no way to delete); `decisions_range_by_time` (clock 100 and 200, half-open interval through `Engine::decisions`); `rebuild_projection_keeps_decisions`; `decisions_survive_reopen`.
- [ ] Run `cargo test -p clio-memory --test decider`. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] Commit `feat(memory): decider cascade with active and shadow modes`.

### Task 4: Neighbors for deciders
**Files:** `src/engine.rs`; Test: `tests/decider.rs`.
Neighbors are computed only if at least one decider is registered and an embedder is set: embed the candidate content, `unit_vector` it (any failure → empty list), `nearest` with default visibility (active memories of the candidate's scope), `k = 5`, then load each memory; similarity is the cosine. They are computed under the write lock, before the decider calls.
- [ ] **Failing tests:** `neighbors_arrive_best_first_within_the_scope` (HashEmbedder; three embedded memories in scope A, one similar memory in scope B; the decider's `seen` question has the scope-A memories only, similarity descending, at most 5, similarity of an identical-token memory near 1); `no_embedder_means_no_neighbors` and `memories_without_vectors_are_not_neighbors`; `a_failing_embedder_gives_no_neighbors_and_the_write_continues` (a `BadEmbedder`/`PoisonEmbedder` for the candidate text: empty neighbors, outcome Add active, record present); `neighbors_are_not_computed_without_deciders` (a `CountingEmbedder` stays at 0 calls from `propose` when no decider is registered).
- [ ] Run. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] Commit `feat(memory): neighbors for deciders`.

### Task 5: `NearDuplicateDecider`
**Files:** `src/decider.rs`; Test: inline unit tests and `tests/decider.rs`; real-model probe in `tests/decider.rs`.
**Produces:** `pub struct NearDuplicateDecider { pub threshold: f32 }` with `Default` 0.97 and `Decider::name() == "near-duplicate"`: if the first neighbor is of the same kind as the candidate and its similarity is at least `threshold`, return `Review` with confidence `similarity.clamp(0.0, 1.0)` and reason `parece duplicado de m<id>`; otherwise `None`.
- [ ] **Failing tests:** unit: `flags_a_close_neighbor_of_the_same_kind`, `ignores_other_kinds_and_far_neighbors`, `similarity_slightly_above_one_is_clamped`, `default_threshold_is_097`. Integration: `near_duplicate_sends_a_close_match_to_review` (HashEmbedder, memory "tiene un coche rojo", candidate "tiene un coche rojo grande" with threshold 0.8: quarantined with the reason naming the neighbor; with the default 0.97 it is added normally); `a_person_resolves_the_review` (archive the candidate → hidden; or promote → active). Add `#[ignore]` `real_model_paraphrase_vs_contradiction_probe` (needs `CLIO_MODEL2VEC_DIR`, feature `model2vec`): prints the cosine between "Vive en Medellín" and "Reside en Medellín", "Vive en Bogotá", "No vive en Medellín"; no assertion on separation.
- [ ] Run. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test -p clio-memory`; run the probe with the model and record the numbers in the commit message. Commit `feat(memory): opt-in near-duplicate decider`.

### Task 6: Docs and final gates
- [ ] README: a short "Deciding what to store" section (deciders, modes, review = quarantine, decision log, near-duplicate is opt-in).
- [ ] Run every gate in Global Constraints and the ignored perf test (`cargo test -p clio-memory --release --test recall t25 -- --ignored --nocapture`); read each result.
- [ ] Commit `docs(memory): document deciders`.

---

## Self-review notes

- Coverage: design objectives (trait, cascade, modes, log, neighbors, near-duplicate), acceptance 7 and 14, and the review-focus items map to Tasks 2-5; the spec edit is Task 1.
- Calls the plan makes: `Decision` has no `provider` field (the decider's `name()` is authoritative, so a decider cannot spoof another's name); `append` becomes a provided method over `append_with_decisions`; shadow deciders run even after an active decision.
