# clio-memory final pieces (5, 6, 7) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the v1 of clio-memory: feedback and confidence (piece 5); `stats`, `doctor`, export/import and a `clio-memory` CLI (piece 6); an optional `telemetry` feature with `tracing` spans and a stored `trace_id` per operation (piece 7).

**Architecture:** Feedback lives next to `usage` as derived data; only the "flag for review" transition is a logged operation (`OpKind::Flag`). Export is JSON Lines read from one consistent snapshot; restore replays it into an empty file with the same ids, then refolds the projection. The CLI is a binary of the same crate over the public `Engine` API only, with a hand-written argument parser. Telemetry is `tracing` behind a cargo feature, with no content in any field.

**Tech Stack:** Rust 2024; one new optional dependency (`tracing`, feature `telemetry`); no `clap`, no OpenTelemetry crates.

**Spec:** `docs/specs/2026-10-29-clio-memory-final-pieces-design.md` (binding), plus `docs/specs/clio-memory-spec-v0.2.md` and the earlier designs.

## Global Constraints

- Branch `feat/clio-memory`; commits local only. Chain every gate and every commit with `&&` (a failed step must stop the chain). Gates: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features`, `scripts/rule-coverage.sh`, `cargo deny check`, `cargo audit`, gitleaks via `mise exec gitleaks@latest -- gitleaks detect --source . --no-banner`. Also build and test with the default features alone and with `--features telemetry`.
- `unsafe_code = "forbid"`, `clippy::unwrap_used = "deny"` (test helper modules start with `#![allow(clippy::unwrap_used)]`). The storage layer stays private; the CLI uses the public `Engine` API only.
- Schema additions go into the version-1 DDL without bumping `VERSION`.
- Nothing logged, traced, exported or printed by default may contain a secret; spans carry no memory text, topic keys or names.
- The CLI never exposes `register_actor`, `create_scope` or `set_retention`.
- Every S-xx a test proves is written as a comment above that test.

## Review Focus

- A feedback signal can never delete or replace anything; `Flag` only quarantines and is reversible; three unspent "wrong" signals flag exactly once and `promote`/`Flag` spend them. (Task 1)
- Export then import into a fresh file gives identical memories, history, blocks, decisions, usage and feedback; import refuses a non-empty file, a different major version, a different schema version, a newer minor, and any file containing a secret, leaving the target file unchanged. (Tasks 5, 6)
- `import_memories` goes through `propose` (quota, secrets, rules, quarantine), skips bad lines with their line number and never aborts half-way silently. (Task 7)
- Read-only CLI commands work while another process holds the writer lock and never write (no `Touch`, no files created). (Tasks 8, 9)
- With `telemetry` off there is no `tracing` code, dependency or table; with it on, no span or event field contains memory text; every operation's `trace_id` equals the id on its span. (Tasks 10, 11)

---

## File Structure

```
crates/clio-memory/Cargo.toml                  tracing (optional), [[bin]] clio-memory
crates/clio-memory/src/model.rs                Signal, FeedbackSummary, OpKind::Flag, ScopeInfo, ActorInfo, Stats, Check, DoctorReport, ExportSummary, ImportSummary, ImportOpts, Operation.trace_id
crates/clio-memory/src/store.rs                feedback, stats, export-snapshot, restore, trace_id methods
crates/clio-memory/src/sqlite/{schema,mod}.rs  feedback table, operations.trace_id, implementations
crates/clio-memory/src/engine.rs               feedback, confidence, scopes, actors, stats, doctor, export, import, import_memories, telemetry hooks
crates/clio-memory/src/transfer.rs             export/import line types and parsing
crates/clio-memory/src/telemetry.rs            (feature) span helpers, trace id generation, provider type
crates/clio-memory/src/bin/clio-memory.rs      CLI entry point; src/cli/{args,commands}.rs helpers
crates/clio-memory/tests/{feedback.rs,transfer.rs,cli.rs,telemetry.rs}
docs/specs/clio-memory-spec-v0.2.md, crates/clio-memory/README.md
```

---

### Task 1: Feedback, confidence and `Flag`
**Files:** `src/model.rs`, `src/store.rs`, `src/sqlite/{schema,mod}.rs`, `src/engine.rs`; Test: `tests/feedback.rs`.
**Produces:**
```rust
pub enum Signal { Useful, Wrong }
pub struct FeedbackSummary { pub useful: u32, pub wrong: u32, pub confidence: f32 }   // wrong = unspent signals; confidence = clamp(0.5 + 0.1*useful - 0.2*wrong, 0, 1)
// OpKind gains Flag. Engine:
pub fn feedback(&self, memory: MemoryId, signal: Signal, actor: ActorId) -> Result<()>;   // not System; counts against quota; memory must be Active (InvalidState otherwise)
pub fn feedback_summary(&self, memory: MemoryId) -> Result<FeedbackSummary>;
pub fn confidence(&self, memory: MemoryId) -> Result<f32>;
// Store: record_feedback(&self, memory, actor, signal, at) -> Result<()>; feedback_counts(&self, memory) -> Result<(u32 useful, u32 unspent_wrong)>; spend_wrong(&self, memory) -> Result<()>
```
Table `feedback(id INTEGER PRIMARY KEY AUTOINCREMENT, memory INTEGER NOT NULL, actor INTEGER NOT NULL REFERENCES actors(id), signal TEXT NOT NULL, at INTEGER NOT NULL, spent INTEGER NOT NULL DEFAULT 0)` plus an index on `memory`. On the third unspent `Wrong`, in one engine-locked step: append an `OpKind::Flag` operation (Active -> Quarantine, reason `equivocado marcado 3 veces`, signed by the signalling actor, no extra quota) and spend the wrong signals; `promote` also spends them.
- [ ] **Failing tests** (`tests/feedback.rs`): `t19_three_wrong_signals_send_a_memory_to_review` (two signals: still Active; third: Quarantine, history ends with a `Flag` by the third actor, reason text, `list(Some(Quarantine))` shows it, `search` hides it); `a_person_can_resolve_a_flag_with_promote_or_archive_and_revert_restores_it`; `promote_spends_wrong_signals` (after promote, two more wrongs do not flag; the third does); `a_reverted_flag_needs_three_new_signals_to_flag_again`; `useful_raises_and_wrong_lowers_confidence` (0.5 start; 1 useful 0.6; 2 wrong from 0.5 gives 0.1; clamps at 0 and 1); `feedback_needs_an_active_memory` (quarantined or archived: `InvalidState`); `system_actor_cannot_give_feedback`; `feedback_counts_against_the_quota` (quota max 2: third call `QuotaExceeded`); `feedback_never_changes_content_or_deletes` (content, kind, topic and history length unchanged below the third signal; no operation kinds besides `Flag`); `signals_survive_rebuild_projection`.
- [ ] Run `cargo test -p clio-memory --test feedback`. Expected: FAIL.
- [ ] Implement (schema, store, engine; `Flag` handled by the existing generic revert and by `fold`).
- [ ] Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] Commit `feat(memory): feedback signals, confidence and flag for review`.

### Task 2: Confidence in `recall`, protection from forgetting
**Files:** `src/model.rs`, `src/recall.rs`, `src/engine.rs`, `src/sqlite/mod.rs`; Test: `src/recall.rs`, `tests/feedback.rs`.
`RecallHit` gains `pub confidence: f32`. `fuse` takes one more closure `confidence: impl Fn(MemoryId) -> f32` and multiplies the score by `1 + CONFIDENCE_WEIGHT * (2*confidence - 1)` with `CONFIDENCE_WEIGHT = 0.1` (so 0.9 to 1.1; a compile-time assertion keeps it below the recency weight). `decay_candidates` treats the time of the latest `Useful` signal of a memory as activity.
- [ ] **Failing tests:** unit in `recall.rs`: `confidence_breaks_ties`, `confidence_factor_is_bounded` (0 → 0.9×, 1 → 1.1×, default 0.5 → 1×), existing `fuse` calls updated. Integration: `recall_prefers_the_trusted_memory_on_a_tie` and `recall_hit_carries_its_confidence`; `useful_feedback_protects_from_decay` (episodic memory, retention 10 days, `Useful` at day 11 before decay: not archived; an unmarked twin is).
- [ ] Run. Expected: FAIL.
- [ ] Implement.
- [ ] Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] Commit `feat(memory): confidence in recall and useful feedback protects from decay`.

### Task 3: `scopes`, `actors`, `stats`
**Files:** `src/model.rs`, `src/store.rs`, `src/sqlite/mod.rs`, `src/engine.rs`; Test: `tests/transfer.rs` (shared helpers in `tests/common`).
**Produces:** `ScopeInfo { id, name, language, retention_days }`, `ActorInfo { id, name, kind }`, `ScopeStats { scope, name, active, quarantine, archived, replaced }`, `Stats { file_bytes: u64, wal_bytes: u64, scopes: Vec<ScopeStats>, operations: u64, decisions: u64, vectors: u64, pending_embeddings: usize, queue: QueueStats }`; `Engine::{scopes(), actors(), stats()}` (all reads, work on read-only engines; file sizes from the filesystem, WAL = `<db>-wal` or 0). `SqliteStore` keeps its path for this.
- [ ] **Failing tests:** `scopes_and_actors_list_what_exists` (names, languages, retention, kinds; the system actor shows once decay ran); `stats_counts_memories_by_state_and_scope`; `stats_sees_queue_vectors_and_pending_embeddings` (with an embedder and a job); `stats_reports_file_and_wal_sizes` (`file_bytes > 0`; wal is 0 or the real `-wal` length); `stats_work_on_a_read_only_engine`.
- [ ] Run. Expected: FAIL. Implement. Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] Commit `feat(memory): scopes, actors and stats`.

### Task 4: `doctor`
**Files:** `src/model.rs`, `src/store.rs`, `src/sqlite/mod.rs`, `src/engine.rs`; Test: `tests/transfer.rs`.
**Produces:** `Check { name: String, ok: bool, detail: String }`, `DoctorReport { checks: Vec<Check> }` with `all_ok()`; `Engine::doctor() -> Result<DoctorReport>` (read-only safe). Checks, each with a stable name: `permissions` (mode 0600 of the file), `schema_version`, `projection_matches_log` (fold the operations in memory and compare with `memories`; never writes), `no_orphan_vectors`, `no_orphan_usage_or_feedback`, `block_entries_exist`, `failed_jobs` (ok when none), `pending_embeddings` (informational, ok). Store gets read methods to support them (`fold_log_preview`, orphan counts).
- [ ] **Failing tests:** `a_healthy_file_passes_every_check`; `doctor_detects_a_corrupt_projection` (a raw SQL `UPDATE memories SET content = 'x'` makes `projection_matches_log` fail and names the memory id); `doctor_detects_orphans` (raw inserts into `vectors`, `usage`, `feedback`, `block_memories` for missing memories); `doctor_flags_failed_jobs`; `doctor_flags_loose_permissions` (create the file 0644 through a raw connection, open `ReadOnly` is refused earlier, so assert through a 0600 file then `chmod 0644` after opening); `doctor_never_writes` (file bytes and mtime unchanged on a read-only engine).
- [ ] Run. Expected: FAIL. Implement. Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] Commit `feat(memory): doctor`.

### Task 5: Export
**Files:** `src/transfer.rs`, `src/model.rs`, `src/store.rs`, `src/sqlite/mod.rs`, `src/engine.rs`, `src/lib.rs`; Test: `tests/transfer.rs`.
**Produces:** `pub const EXPORT_FORMAT_VERSION: &str = "1.0.0";` `ExportSummary { scopes, actors, operations, decisions, blocks, usage, feedback: usize }`; `Engine::export(&self, out: &mut dyn std::io::Write) -> Result<ExportSummary>` (works read-only). Lines are `serde_json` objects with a `"type"` of `header`, `scope`, `actor`, `operation`, `decision`, `block`, `block_memory`, `usage`, `feedback`, in that order (the header carries `format`, `format_version`, `schema_version`, `exported_at`); operations carry their full `changes` snapshots and `trace_id`; no vectors, jobs or quotas. One consistent snapshot (a single read transaction on one connection). A `Store::export_snapshot(&self, f: &mut dyn FnMut(ExportRecord) -> Result<()>) -> Result<()>` streams records.
- [ ] **Failing tests:** `export_writes_a_versioned_header_then_typed_lines_in_order`; `export_contains_everything_needed_to_rebuild` (operation lines parse back into `Operation`s equal to `history`); `export_omits_vectors_jobs_and_quotas`; `export_has_no_secret_and_is_snapshot_consistent` (a writer thread proposing during the export never produces a line referencing an operation that is not exported: every `memory` id in a line appears in an exported operation); `export_works_on_a_read_only_engine`.
- [ ] Run. Expected: FAIL. Implement. Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] Commit `feat(memory): JSONL export`.

### Task 6: Import (restore)
**Files:** `src/transfer.rs`, `src/store.rs`, `src/sqlite/mod.rs`, `src/engine.rs`; Test: `tests/transfer.rs`.
**Produces:** `Engine::import(&self, input: &mut dyn std::io::BufRead) -> Result<ExportSummary>` (writes; requires a read-write engine). Preconditions and checks, all before any insert: target has no scopes and no operations and at most the `clio-memory:system` actor (replaced); header `format == "clio-memory-export"`, same major version, not a newer minor, `schema_version` equal to the crate's; every line parses with a known `type` (no unknown types); no operation snapshot contains something `find_secret` flags. Then, in one store transaction: insert scopes, actors, operations (same ids), decisions, blocks, block memories, usage, feedback; then refold the projection and `operation_memories`; enqueue `Embed` when there is an embedder. A failure anywhere leaves the file as it was. Add `Error::BadImport(String)` for rejected files.
- [ ] **Failing tests:** `export_then_import_round_trips` (recall, list, history, blocks, decisions, usage and feedback of the restored file equal the original's; `doctor` is all ok on the restored file); `import_refuses_a_file_with_data` (`InvalidState`, nothing changed); `import_accepts_a_target_that_only_has_the_system_actor`; `import_rejects_a_different_major_a_newer_minor_and_a_different_schema_version`; `import_rejects_an_unknown_line_type_and_a_bad_line_with_its_line_number`; `import_rejects_a_secret_and_leaves_the_target_untouched` (S-18, byte-for-byte unchanged file); `import_is_atomic` (a file whose last line is bad changes nothing); `import_enqueues_embedding_with_an_embedder`.
- [ ] Run. Expected: FAIL. Implement. Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] Commit `feat(memory): restore from an export`.

### Task 7: `import_memories`
**Files:** `src/transfer.rs`, `src/model.rs`, `src/engine.rs`; Test: `tests/transfer.rs`.
**Produces:** `ImportOpts { origin: Origin }` (Default `External`), `ImportSummary { read, added, noop, quarantined: usize, rejected: Vec<(usize, String)> }` (at most 100 rejections kept), `Engine::import_memories(&self, input: &mut dyn BufRead, scope: ScopeId, actor: ActorId, opts: &ImportOpts) -> Result<ImportSummary>`. The actor must be an `Importer` or a `Person` (`NotAllowed` otherwise). Each non-empty line is JSON `{ "content", "kind"?, "topic_key"?, "valid_from"?, "valid_to"?, "source"? }`, sent through `propose` with `origin = opts.origin`; `kind` defaults to `semantic`, an unknown kind rejects the line; invalid JSON, missing `content`, `TooLong`, `SecretDetected`, `QuotaExceeded` and the like reject only that line with its 1-based line number.
- [ ] **Failing tests:** `import_memories_goes_through_propose_and_quarantines_by_default`; `a_bad_line_is_rejected_with_its_number_and_the_rest_import` (invalid JSON, no content, unknown kind, a 20 000-byte content, a secret); `quota_is_respected_and_counted_as_rejected` (actor quota 3, five lines: three added, two rejected); `an_agent_cannot_be_the_importer`; `origin_user_is_only_honored_for_a_person_or_importer` (an `Importer` or `Person` with `ImportOpts { origin: User }` gets active memories; with the default they are quarantined); `duplicates_are_noops_and_counted`; `import_memories_enqueues_embedding_with_an_embedder`.
- [ ] Run. Expected: FAIL. Implement. Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] Commit `feat(memory): import loose memories through propose`.

### Task 8: CLI skeleton and read commands
**Files:** `Cargo.toml` (`[[bin]] name = "clio-memory" path = "src/bin/clio-memory.rs"`), `src/bin/clio-memory.rs`, `src/cli/{mod,args}.rs` included with `#[path]`/`mod` from the binary; Test: `tests/cli.rs` (runs `env!("CARGO_BIN_EXE_clio-memory")` through `std::process::Command` on a temp file prepared with the library).
**Produces:** usage `clio-memory --db PATH [--actor NAME] [--model DIR] [--json] <command> [args]`; exit codes 0 / 1 (error, or `doctor` with a failed check, or rejected import lines) / 2 (usage). Read commands open the file `ReadOnly`: `scopes`, `stats`, `doctor`, `list SCOPE [--state active|quarantine|archived|replaced]`, `get ID`, `history ID`, `review SCOPE` (quarantined), `jobs [--state S]`, `recall SCOPE TEXT`, `context SCOPE TEXT`. Scope arguments are names; ids are `m12`/`12`. Text output is one line per item (the compact line format for memories); `--json` prints one JSON document for `scopes`, `stats`, `doctor`, `list`, `get`, `history` and `jobs`. `--model DIR` (feature `model2vec`; without the feature the flag is an error) loads a local model for `recall` and `context`.
- [ ] **Failing tests** (`tests/cli.rs`): `no_command_or_unknown_command_exits_2_with_usage`; `scopes_stats_list_get_history_review_jobs_print_what_the_library_holds`; `json_flag_prints_valid_json`; `doctor_exits_0_when_healthy_and_1_when_a_check_fails` (corrupt a row with raw SQL); `recall_and_context_work_without_a_model`; `read_commands_work_while_another_process_holds_the_writer_lock` (keep an `Engine` open in the test process, run the CLI); `read_commands_never_write` (no `-lock`, no new jobs, file bytes unchanged after `recall`); `missing_database_is_a_clear_error_and_creates_nothing`; `the_cli_has_no_way_to_register_actors_create_scopes_or_set_retention` (those words are unknown commands, exit 2).
- [ ] Run `cargo test -p clio-memory --test cli`. Expected: FAIL.
- [ ] Implement.
- [ ] Run. Expected: PASS.
- [ ] Commit `feat(memory): CLI with read commands`.

### Task 9: CLI write commands
**Files:** `src/cli/*`, `src/bin/clio-memory.rs`; Test: `tests/cli.rs`.
Write commands open read-write and fail with a clear message when the writer lock is held (`WriterLocked`): `promote ID`, `archive ID`, `revert OP`, `feedback ID useful|wrong`, `retry-job ID`, `run-jobs [--heavy]`, `backup PATH` (enqueue then run heavy jobs synchronously), `export PATH` (read-only open; refuses to overwrite), `import PATH` (restore), `import-memories SCOPE PATH [--origin user|agent|external]`. Review commands act as the person actor `cli:<user>` (or `--actor NAME`; must be a person; created with quota 1 000 000 per hour when missing); `import-memories` acts as an importer actor `cli-import:<user>` (created the same way). `$USER` falls back to `unknown`.
- [ ] **Failing tests:** `promote_archive_revert_work_as_a_person`; `feedback_through_the_cli_can_flag_a_memory`; `run_jobs_embeds_with_a_model_flag_absent_as_a_noop_for_embed` (without `--model` and without an embedder, `run-jobs` leaves Embed jobs untouched because none were queued; it applies Touch/Decay), `backup_creates_a_private_copy_that_opens` (mode 0600, `doctor` on the copy is ok); `export_then_import_through_the_cli_round_trips` (into a new file); `export_refuses_to_overwrite`; `import_memories_reports_rejected_lines_and_exits_1`; `write_commands_fail_clearly_while_another_process_holds_the_writer_lock` (exit 1, message mentions the lock); `actor_flag_must_name_a_person` (an agent name: exit 1); `the_default_actor_is_created_once_and_reused`.
- [ ] Run. Expected: FAIL. Implement. Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] Commit `feat(memory): CLI write commands`.

### Task 10: Telemetry feature and `trace_id`
**Files:** `Cargo.toml` (`tracing = { version = "0.1", optional = true }`; `telemetry = ["dep:tracing"]`), `src/model.rs`, `src/store.rs`, `src/sqlite/{schema,mod}.rs`, `src/telemetry.rs` (feature), `src/engine.rs`, `src/transfer.rs`; Test: `tests/telemetry.rs` (`#![cfg(feature = "telemetry")]`) and inline.
**Produces:** column `operations.trace_id TEXT` (always in the schema, null without the feature); `Operation.trace_id: Option<String>` and `NewOperation.trace_id: Option<String>` (public structs: update every constructor in tests and engine); export/import carry it. With the feature: `type TraceIdProvider = Arc<dyn Fn() -> Option<String> + Send + Sync>`; `Engine::with_trace_id_provider(self, p: TraceIdProvider) -> Self`; each operation written by `propose`, `promote`, `archive`, `revert*`, `feedback` (Flag) and `decay` gets `trace_id = provider() or a random 32-hex-digit id` (from two `std::collections::hash_map::RandomState` hashes), created once per top-level call.
- [ ] **Failing tests:** `without_the_feature_trace_id_is_none_and_no_telemetry_table_exists` (default-features test in `tests/transfer.rs`: `operation.trace_id == None`; the exact list of tables equals the expected set, so any `telemetry%` table would fail it; S-18/test 21); with the feature: `every_operation_gets_a_32_hex_trace_id`, `a_provider_overrides_the_generated_id`, `a_provider_returning_none_falls_back_to_a_generated_id`, `trace_id_survives_export_and_import`, `all_operations_of_one_call_share_one_trace_id` (an Update writes one operation; `revert_actor` of several: each operation its own call; a `decay` run: one id per archive operation or one per run, pick one and assert it consistently).
- [ ] Run default and `--features telemetry`. Expected: FAIL.
- [ ] Implement; verify `cargo tree -p clio-memory -e normal` shows no `tracing` without the feature.
- [ ] Commit `feat(memory): optional telemetry feature and operation trace ids`.

### Task 11: Spans and events
**Files:** `src/engine.rs`, `src/telemetry.rs`; Test: `tests/telemetry.rs`.
With the feature, spans (via `tracing::info_span!`, all behind `#[cfg(feature = "telemetry")]` helpers so the code compiles to nothing otherwise): `propose` (fields `scope`, `actor`, `kind`, `trace_id`, and on exit `outcome`, `state`), with child spans `normalize`, `secrets`, `rules`, `neighbors`, `decider` (field `provider`), `append`; `recall` (`scope`, `limit`, `text_hits`, `vector_hits`, `hits`, `abstained`), `context`, `promote`, `archive`, `revert`, `feedback`, `job` (`kind`, `id`, `attempts`, `outcome`). Events: writer lock rejected, embedder model used by `embed_pending`, revert, backup done/failed, job moved to `Failed`. Fields hold ids, sizes, counts, durations, outcomes: never text, `topic_key`, names or paths.
- [ ] **Failing tests** (a minimal `Subscriber` defined in the test that records every span and event with all its fields as strings): `propose_emits_its_spans_with_the_expected_children`; `recall_context_and_jobs_emit_spans`; `events_are_emitted_for_the_listed_situations`; `t22_no_span_or_event_field_contains_memory_text` (propose, recall, context, revert, feedback, jobs and decay over memories whose content, topic key and names carry a distinctive marker string; no recorded field value contains it); `t24_the_operation_trace_id_matches_its_propose_span` (the stored id equals the `trace_id` field of the span that wrote it); `no_spans_without_a_subscriber_cost_nothing_observable` (operations behave identically with and without a subscriber).
- [ ] Run with `--features telemetry`. Expected: FAIL. Implement. Run `cargo test -p clio-memory --features telemetry` and with default features. Expected: PASS.
- [ ] Commit `feat(memory): tracing spans and events behind the telemetry feature`.

### Task 12: Docs and final gates
- [ ] Update `docs/specs/clio-memory-spec-v0.2.md` (section 8: the new operations `feedback`, `stats`, `doctor`, `scopes`, `import_memories`; `register_actor`, `create_scope`, `set_retention` stay out of MCP) and the README (feedback and confidence, `stats` and `doctor`, export/import and its trust model, the `clio-memory` CLI with one example session, the `telemetry` feature and `with_trace_id_provider`). Remove "Still to come" from the status line: the v1 scope is complete.
- [ ] Run every gate in Global Constraints, with default features, `--features telemetry`, `--features model2vec` and `--all-features`; run the ignored perf test (`cargo test -p clio-memory --release --test recall t25 -- --ignored --nocapture`; recall median must stay under 150 ms) and the real-model probes (`CLIO_MODEL2VEC_DIR=$HOME/.cache/clio-models/potion-multilingual-128M cargo test -p clio-memory --release --features model2vec -- --ignored real_model model2vec_real`); run the CLI by hand once over a temp file (create data with a small example, then `stats`, `doctor`, `recall`, `export`, `import` into a new file, `doctor` on it) and paste the result in the commit message. Read each output.
- [ ] Commit `docs(memory): document feedback, CLI, export and telemetry`.

---

## Self-review notes

- Coverage: acceptance 19 (Task 1), 21, 22, 24 (Tasks 10, 11), 1 and 16 reinforced by the CLI (Tasks 8, 9), the export/import round trip (Tasks 5, 6), and every design objective has a named test; test 23 (retention rollups) is v2.
- Calls the plan makes: Task 10's "decay run" trace-id granularity is left to the implementer but must be tested consistently; the CLI test that needs a held writer lock keeps an `Engine` open in the test process (flock is per open file description, so the second open conflicts even in the same process tree).
