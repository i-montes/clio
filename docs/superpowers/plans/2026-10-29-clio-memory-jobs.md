# clio-memory job queue Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add piece 4: a job queue in the same SQLite file (enqueued atomically with the operation that causes it), a synchronous runner, and the jobs `Embed`, `Touch`, `Decay`, `Backup` and `Vacuum`, with usage-aware ranking and retention-based forgetting.

**Architecture:** A `jobs` table plus `usage` and a scope `retention_days`. `append_all` writes the operation, its decisions and its jobs in one transaction. `Engine::run_jobs` claims jobs (priority, then age), runs each handler, and records success, retry with backoff or failure. Handlers reuse existing engine paths (`embed_pending`, archive logging, the online-backup API).

**Tech Stack:** Rust 2024; `rusqlite` gains its `backup` feature (no new crates).

**Spec:** `docs/specs/2026-10-29-clio-memory-jobs-design.md` (binding), plus `docs/specs/clio-memory-spec-v0.2.md` and the earlier designs.

## Global Constraints

- Branch `feat/clio-memory`; commits local only. Gates: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features`, `scripts/rule-coverage.sh`, `cargo deny check`, `cargo audit`, gitleaks via `mise exec gitleaks@latest -- gitleaks detect --source . --no-banner`.
- `unsafe_code = "forbid"`, `clippy::unwrap_used = "deny"` (test helper modules start with `#![allow(clippy::unwrap_used)]`). No new crates. Storage stays private.
- Schema additions go into the version-1 DDL without bumping `VERSION`.
- Job payloads contain ids, a scope id, a time or a path: never memory text.
- Forgetting only archives; review powers stay person-only (`System` has none).
- Every S-xx a test proves is written as a comment above that test.

## Review Focus

- An operation and its jobs are written together or not at all; a failed append leaves no job. (Task 1, Task 2)
- A memory added while an `Embed` job is `Running` is never left without a pending `Embed`. (Task 2)
- A retry returning to `Pending` must not violate the pending-only dedupe index; an expired lease at `max_attempts` ends in `Failed`. (Task 1, Task 3)
- `recall` never blocks or fails because of `Touch` (busy write lock, read-only engine, store error). (Task 4)
- `decay` never archives what a fixed block shows (literal ids and their current successors), never touches semantic/procedural memories, never undoes a person's revert, and is deterministic and idempotent. (Task 5)
- A backup never overwrites a file, never leaves a readable-by-others file, and survives a stale `.tmp`. (Task 6)

---

## File Structure

```
crates/clio-memory/Cargo.toml                 rusqlite feature "backup"
crates/clio-memory/src/model.rs               JobKind, JobState, JobRecord, QueueStats, RunOpts, RunReport, ActorKind::System
crates/clio-memory/src/store.rs               NewJob, queue/usage/retention/backup methods, append_all
crates/clio-memory/src/sqlite/{schema,mod}.rs jobs, usage tables, retention_days, implementations
crates/clio-memory/src/engine.rs              enqueue rules, run_jobs, handlers, decay, touch, backup, queue API
crates/clio-memory/src/recall.rs              usage factor in fuse
crates/clio-memory/tests/{jobs.rs,common/mod.rs}
crates/clio-memory/README.md
```

---

### Task 1: Types, schema and queue storage
**Files:** `Cargo.toml`, `src/model.rs`, `src/store.rs`, `src/sqlite/{schema,mod}.rs`, `src/rules.rs` (if the new `ActorKind` variant needs a match arm); Test: `src/sqlite/tests.rs`.
**Produces:**
```rust
// model.rs (public; serde derives on the enums)
pub enum JobKind { Embed, Touch, Decay, Backup, Vacuum }          // priority 10, 5, 3, 2, 1; Backup and Vacuum are heavy
pub enum JobState { Pending, Running, Done, Failed }
pub struct JobRecord { pub id: i64, pub kind: JobKind, pub payload: String, pub priority: i32, pub state: JobState, pub attempts: u32,
  pub max_attempts: u32, pub run_after: i64, pub lease_until: Option<i64>, pub dedupe_key: Option<String>, pub error: Option<String>,
  pub created_at: i64, pub finished_at: Option<i64> }
pub struct QueueStats { pub pending: usize, pub running: usize, pub failed: usize, pub oldest_pending_age_secs: Option<i64> }
pub struct RunOpts { pub max_jobs: usize, pub allow_heavy: bool }       // Default: 100, false
pub struct RunReport { pub done: usize, pub retried: usize, pub failed: usize, pub skipped_heavy: usize, pub recovered: usize }
// ActorKind gains `System` (internal; never creatable through Engine::register_actor)
// store.rs
pub struct NewJob { pub kind: JobKind, pub payload: String, pub dedupe_key: Option<String>, pub run_after: i64, pub at: i64 }   // priority derives from kind
fn append_all(&self, op: NewOperation, decisions: Vec<NewDecision>, jobs: Vec<NewJob>) -> Result<OpId>;   // required; append and append_with_decisions become provided methods over it
fn enqueue(&self, job: NewJob) -> Result<bool>;                      // false when ignored by the pending-only dedupe index
fn claim_job(&self, now: i64, only: Option<JobKind>, allow_heavy: bool, lease_secs: i64) -> Result<Option<JobRecord>>;   // best priority, then lowest id, run_after <= now; marks Running, attempts + 1, lease_until
fn finish_job(&self, id: i64, now: i64) -> Result<()>;
fn fail_job(&self, id: i64, now: i64, error: &str, retry_at: Option<i64>) -> Result<()>;   // Some -> Pending at that time (if another Pending job holds its dedupe key it is marked Done with a note instead); None -> Failed
fn recover_leases(&self, now: i64) -> Result<usize>;                 // Running with lease_until < now: back to Pending, or Failed("lease expired") if attempts >= max_attempts
fn jobs(&self, state: Option<JobState>, limit: usize) -> Result<Vec<JobRecord>>;   // newest first
fn queue_stats(&self, now: i64) -> Result<QueueStats>;
fn retry_job(&self, id: i64, now: i64) -> Result<()>;                // Failed -> Pending, attempts reset; InvalidState otherwise
fn prune_jobs(&self, finished_before: i64) -> Result<usize>;         // deletes Done/Failed older than the cutoff
fn count_ready_heavy(&self, now: i64) -> Result<usize>;
```
Tables: `jobs(id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, payload TEXT NOT NULL, priority INTEGER NOT NULL, state TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, max_attempts INTEGER NOT NULL DEFAULT 5, run_after INTEGER NOT NULL, lease_until INTEGER, dedupe_key TEXT, error TEXT, created_at INTEGER NOT NULL, finished_at INTEGER)`, `CREATE UNIQUE INDEX jobs_dedupe ON jobs(dedupe_key) WHERE state = 'Pending' AND dedupe_key IS NOT NULL`, `CREATE INDEX jobs_ready ON jobs(state, run_after, priority)`. In `Cargo.toml`: `rusqlite = { ..., features = ["bundled", "backup"] }`.
- [ ] **Failing tests** in `src/sqlite/tests.rs` (build jobs with a helper): `dedupe_ignores_a_second_pending_job_but_not_after_it_runs` (enqueue `embed` twice → second returns false; after `claim_job` the key is free again and a new `embed` enqueues); `claim_orders_by_priority_then_age_and_respects_run_after`; `claim_counts_the_attempt_and_sets_the_lease`; `heavy_jobs_wait_for_allow_heavy` (Backup/Vacuum not claimed with `false`, and `count_ready_heavy` sees them); `only_filters_by_kind`; `fail_with_retry_goes_back_to_pending_at_the_given_time`; `a_retry_that_collides_with_a_pending_twin_is_dropped_as_done`; `fail_without_retry_goes_to_failed_with_the_error`; `expired_leases_are_recovered_or_failed_at_max_attempts`; `append_all_writes_jobs_with_the_operation` and `failed_append_all_leaves_no_jobs` (unknown scope); `queue_stats_and_oldest_age`; `retry_job_only_from_failed`; `prune_deletes_only_old_finished_jobs`; `rebuild_projection_keeps_jobs`.
- [ ] Run `cargo test -p clio-memory --lib`. Expected: FAIL.
- [ ] Implement types, `NewJob` priority mapping, schema and the store methods; add `ActorKind::System`.
- [ ] Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] Commit `feat(memory): job queue storage`.

### Task 2: Atomic enqueue and the `Embed` job
**Files:** `src/engine.rs`, `src/model.rs`; Test: `tests/jobs.rs`, `tests/common/mod.rs`.
**Produces:**
```rust
impl Engine {
  pub fn run_jobs(&self, opts: &RunOpts) -> Result<RunReport>;    // ReadOnly on a read-only engine
}
```
Rules: any operation (propose Add or Update, promote, revert) whose changes leave at least one row not `Archived` enqueues one `Embed` job (`dedupe_key = "embed"`, payload `{}`) in the same `append_all`, only when the engine has an embedder. `run_jobs`: recover expired leases, then claim and run up to `max_jobs` jobs (lease 300 s). The `Embed` handler loops batches until nothing is pending (an internal `embed_pending_inner(limit) -> (stored, examined)`; stop when `examined == 0`; `embed_pending` keeps its behavior). Successful jobs are `finish_job`ed; the failure path comes in Task 3 (until then a handler error marks the job `Failed`).
- [ ] **Failing tests** in `tests/jobs.rs`: `t09_ten_updates_make_one_embed_job` (HashEmbedder; ten proposals with the same `topic_key` → `queue_stats().pending == 1`); `t08_operation_and_job_are_atomic` (a normal propose leaves its operation and exactly one pending `Embed`; a propose that fails inside the store, e.g. through a scope that does not exist, leaves neither); `run_jobs_embeds_everything_pending` (after the run, `embed_pending(10)` returns 0 and `recall` finds a memory by vector); `a_memory_added_while_embed_runs_is_not_lost` (claim the `Embed` through a second engine handle is not possible, so simulate: mark the pending `Embed` as `Running` with an unexpired lease via `claim` through the store-level test helper is out of reach from `tests/`; instead assert the property at the engine level: propose A, `run_jobs`, propose B → a new pending `Embed` exists and a second `run_jobs` embeds B); `no_embedder_means_no_embed_jobs`; `archive_enqueues_nothing`; `read_only_engine_cannot_run_jobs`; `run_jobs_respects_max_jobs`; `embed_job_drains_more_than_one_batch` (40 memories, batch size 32).
- [ ] Run `cargo test -p clio-memory --test jobs`. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] Commit `feat(memory): atomic embed jobs and the job runner`.

### Task 3: Retries, error queue, queue API
**Files:** `src/engine.rs`; Test: `tests/jobs.rs`.
**Produces:** `Engine::{jobs(&self, state: Option<JobState>, limit: usize) -> Result<Vec<JobRecord>>, queue_stats(&self) -> Result<QueueStats>, retry_job(&self, id: i64, actor: ActorId) -> Result<()>}` (`retry_job` person-only). Failure policy in `run_jobs`: a permanent error (`NoEmbedder`, `UnknownScope`, `UnknownActor`, `NotFound`, `ReadOnly`, `Json`, and `Io` of kind `AlreadyExists`, `NotFound` or `PermissionDenied`) fails the job at once; any other error retries with `run_after = now + min(60 · 2^(attempts − 1), 3600)` until `attempts >= max_attempts`, then `Failed`. After a run, `Done`/`Failed` jobs older than 7 days are pruned.
- [ ] **Failing tests:** `a_transient_failure_retries_with_growing_backoff` (an embedder that fails for the first calls: first failure at clock 1000 → `run_after` 1060, `Pending`, error stored; a run at 1059 does nothing; at 1060 it retries and a second failure schedules 120 s later); `retries_end_in_the_error_queue_after_max_attempts` (5 failures → `Failed`, error text kept, `queue_stats().failed == 1`); `a_permanent_failure_does_not_retry` (`NoEmbedder` path: enqueue via an engine that had an embedder, run on one without → `Failed` after one attempt); `an_expired_lease_is_recovered` (needs a way to leave a job `Running`: a handler that panics is caught as an error, so use the store-level test for the lease; at engine level assert `recovered` counts a job left `Running` by a prior engine instance reopened after its lease expired, created by dropping the engine mid-run is not reproducible, so this case stays covered by the Task 1 store tests and the report field is asserted `0` here); `retry_job_requires_a_person_and_a_failed_job`; `old_finished_jobs_are_pruned`; `jobs_lists_newest_first_and_by_state`.
- [ ] Run. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] Commit `feat(memory): retries, error queue and queue inspection`.

### Task 4: Usage (`Touch`) and its effect on `recall`
**Files:** `src/sqlite/schema.rs`, `src/store.rs`, `src/sqlite/mod.rs`, `src/engine.rs`, `src/recall.rs`; Test: `tests/jobs.rs`, inline in `recall.rs`.
**Produces:** table `usage(memory INTEGER PRIMARY KEY, uses INTEGER NOT NULL, last_used INTEGER NOT NULL)`; store `record_usage(&self, memories: &[MemoryId], at: i64) -> Result<()>` (`uses + 1`, `last_used = max(old, at)`) and `usage_counts(&self, ids: &[MemoryId]) -> Result<HashMap<MemoryId, u32>>`. `recall` (not for `as_of`/`known_at` queries, only on a read-write engine, only when there are hits) enqueues a `Touch` job with payload `{"ids":[...],"at":<now>}` using `try_lock` on the write mutex and ignoring any error. The `Touch` handler calls `record_usage`. `fuse` takes an extra `uses: impl Fn(MemoryId) -> u32` and multiplies the score by `1 + RECENCY_WEIGHT·recency + USAGE_WEIGHT·min(1, ln(1+uses)/ln(21))` with `USAGE_WEIGHT = 0.05`.
- [ ] **Failing tests:** unit in `recall.rs`: `usage_breaks_ties_in_favor_of_the_used_memory`, `usage_factor_is_capped` (1 000 uses scores the same as 20), `usage_weight_is_below_the_recency_weight`. Integration: `recall_enqueues_a_touch_and_the_runner_applies_it` (after `run_jobs`, the memory's usage is 1; two recalls then a run give 2); `touch_uses_the_time_of_the_query_not_of_the_run`; `historical_recall_does_not_count_as_use`; `read_only_engine_does_not_enqueue_touch`; `recall_still_answers_when_the_write_lock_is_busy` (hold the engine's write lock from another thread through a slow decider registered for a `propose` in flight, then call `recall` from the test thread: it returns without waiting; usage is not recorded for it); `a_decider_that_calls_recall_does_not_deadlock`; `touch_payload_has_no_text` (payload parses to ids and `at` only).
- [ ] Run. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] Commit `feat(memory): usage tracking and its effect on recall`.

### Task 5: `Decay`
**Files:** `src/sqlite/{schema,mod}.rs`, `src/store.rs`, `src/engine.rs`; Test: `tests/jobs.rs`.
**Produces:** column `scopes.retention_days INTEGER` (null = never forget); `Engine::{set_retention(&self, scope: ScopeId, days: Option<u32>, actor: ActorId) -> Result<()> (person-only), enqueue_decay(&self, scope: ScopeId) -> Result<bool>}`; store `set_retention`, `retention(scope) -> Result<Option<u32>>`, `decay_candidates(scope, cutoff) -> Result<Vec<MemoryRow>>` (Active, kind Working or Episodic, scope, with `max(usage.last_used, known_since, latest Revert/Promote operation time on the memory) < cutoff`, ordered by id), and `system_actor() -> Result<ActorId>` (get-or-create a `System` actor named `clio-memory:system`). `Engine::register_actor` rejects `ActorKind::System` with `NotAllowed`. The `Decay` handler: applies pending `Touch` jobs first (claim only `Touch`), computes the cutoff from `retention_days`, drops candidates protected by a fixed block (the block's literal ids and what `current()` resolves them to), and archives the rest with ARCHIVE operations by the system actor, reason `decay: sin uso desde hace N días`, under the write lock.
- [ ] **Failing tests:** `t18_decay_is_deterministic_and_idempotent` (fixed clock; two engines with the same history give identical archived sets; running the job twice leaves the second run with no changes and no new operations); `decay_only_touches_working_and_episodic` (semantic and procedural old memories stay active); `decay_needs_a_retention_policy` (`None` → nothing); `recent_use_protects_a_memory` (a `Touch` pending at decay time is applied first); `blocks_protect_what_they_show` (a block lists an old episodic memory, and another block entry resolves through an update to a successor: neither is archived); `a_persons_revert_counts_as_activity` (decay archives, a person reverts, the next decay run does not archive it again until it is old again); `decay_uses_the_system_actor_and_archives_only` (operations have kind Archive by an actor of kind `System`; nothing is deleted; `System` cannot `promote` or `revert`); `register_actor_rejects_system`; `set_retention_is_person_only`; `decay_job_is_deduplicated_per_scope`; `decayed_memories_leave_reads_and_keep_their_history`.
- [ ] Run. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] Commit `feat(memory): retention-based forgetting`.

### Task 6: `Backup` and `Vacuum`
**Files:** `src/store.rs`, `src/sqlite/mod.rs`, `src/engine.rs`; Test: `tests/jobs.rs`.
**Produces:** `Engine::{enqueue_backup(&self, path: &Path) -> Result<bool>, enqueue_vacuum(&self) -> Result<bool>}` (dedupe keys `backup:<path>` and `vacuum`; both heavy); store `backup_to(&self, path: &Path) -> Result<()>` and `vacuum(&self) -> Result<()>`. Backup: delete a stale `<path>.tmp`, create it exclusively with mode 0600, run the online backup into it, publish with `std::fs::hard_link(tmp, path)` (fails with `AlreadyExists` if `path` exists: a permanent failure) and remove the `.tmp`.
- [ ] **Failing tests:** `backup_runs_only_with_allow_heavy` (job stays pending with `skipped_heavy == 1` otherwise); `backup_copies_the_data_with_0600` (S-23 comment; the copy opens read-only with `Engine::open(.., ReadOnly)` and returns the same memories; mode 0600; no `.tmp` left); `backup_does_not_overwrite` (existing destination → the job is `Failed` at the first attempt, the destination's bytes unchanged); `a_stale_tmp_is_replaced` (a 0644 junk `<path>.tmp` is removed and the result is 0600); `vacuum_runs_and_keeps_the_data`; `backup_and_vacuum_are_deduplicated`.
- [ ] Run. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] Commit `feat(memory): backup and vacuum jobs`.

### Task 7: Docs and final gates
- [ ] README: a "Background work" section (the queue, the jobs, `run_jobs`, no scheduler, `retention_days`, backup is 0600 and not encrypted).
- [ ] Run every gate in Global Constraints, the ignored perf test (`cargo test -p clio-memory --release --test recall t25 -- --ignored --nocapture`; recall now also enqueues a `Touch`, so its median must stay under 150 ms) and the real-model probes; read each result.
- [ ] Commit `docs(memory): document the job queue`.

---

## Self-review notes

- Coverage: design objectives and acceptance tests 8, 9, 18 map to Tasks 2, 5; retries, leases, dedupe, heavy gating, usage, decay protections and backup rules have named tests.
- Calls the plan makes: the lease-recovery path is verified at the store level (Task 1) because the engine cannot be made to die mid-job from a test; the "memory added while `Embed` runs" property is checked through the observable sequence (propose, run, propose, run) and the pending-only dedupe test in Task 1.
