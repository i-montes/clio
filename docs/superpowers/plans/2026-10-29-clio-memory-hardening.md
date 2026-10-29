# clio-memory hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the findings of the repo audit and the earlier core review so work can continue on the decision cascade: a configurable size limit on stored text, a linear top-k in the vector scan, a read-only open that never creates files, same-second updates that do not fail, a script that cannot fail silently, CI actions pinned by commit, and a note that keeps administrative methods out of any agent-facing interface.

**Architecture:** Small, independent changes to the existing crate and repo files. Each task is a failing test (or a failing command) first, then the change. No new dependencies.

**Tech Stack:** Rust 2024, `rusqlite`, bash, GitHub Actions YAML, `gh` CLI (available through `mise`) for commit hashes.

**Spec:** the audit and review findings in this conversation; designs in `docs/specs/2026-10-29-clio-memory-core-design.md`, `docs/specs/2026-10-29-clio-memory-recall-design.md`, `docs/specs/clio-memory-spec-v0.2.md`. Not in scope: the fate of the `clio-store` crate (the user has not decided) and finding 3 as code (it becomes a documented requirement for the daemon, Task 7).

## Global Constraints

- Branch `feat/clio-memory`; commits local only, never push. Gates available now: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features`, `scripts/rule-coverage.sh`, `cargo deny check`, `cargo audit`, `just check`, `just test`, `just audit`, and gitleaks through `mise exec gitleaks@latest -- gitleaks detect --source . --no-banner`.
- `unsafe_code = "forbid"`, `clippy::unwrap_used = "deny"` (test helper modules start with `#![allow(clippy::unwrap_used)]`). No new dependencies.
- Storage layer stays private; everything goes through `Engine`.
- Size limits are measured in bytes (`str::len`), not characters.
- A proposal rejected for size still counts against the actor's quota (like secret rejections) and leaves nothing in any table.
- Every rule id (S-xx) a test proves is written as a comment above that test.

## Review Focus

- Text at the exact limit is accepted and one byte over is rejected, with multibyte characters counted by bytes (`é` is 2 bytes). (Task 1)
- A proposal rejected for size stores nothing and still uses up quota. (Task 1)
- `nearest` with `k` larger than the number of vectors, `k = 0`, and ties in score returns a correct, deterministic order. (Task 2)
- `ReadOnly` open of a missing, empty or too-new file never creates or changes a file. (Task 3)
- A chain of updates to one topic inside the same second keeps validity intervals contiguous and non-overlapping, and an explicit `valid_from` that is not later than the old one is still rejected. (Task 4)

---

## File Structure

```
crates/clio-memory/src/{model.rs,error.rs,engine.rs}   Limits, Error::TooLong, size checks, update bump
crates/clio-memory/src/sqlite/{mod.rs,tests.rs}        nearest top-k, read-only open
crates/clio-memory/src/platform/linux.rs               check_db_file
crates/clio-memory/tests/{propose.rs,temporal.rs,recall.rs,writer_lock.rs}
scripts/rule-coverage.sh
.github/workflows/*.yml, .github/dependabot.yml
docs/specs/clio-memory-spec-v0.2.md, docs/specs/2026-10-29-clio-memory-core-design.md
```

---

### Task 1: Configurable size limits

**Files:** Modify `src/model.rs`, `src/error.rs`, `src/engine.rs`, `src/lib.rs`; Test: `tests/propose.rs`.

**Produces:**
```rust
// model.rs
pub struct Limits { pub max_content_bytes: usize, pub max_field_bytes: usize }   // Default: 16 * 1024 and 256
// error.rs
Error::TooLong      // "text is longer than the configured limit"
// engine.rs
impl Engine { pub fn with_limits(self, limits: Limits) -> Self }
```
`propose` order becomes: empty → validity → scope → `bump_quota` → **size check** → secret check → rest. The size check rejects `content` over `max_content_bytes` and each of `topic_key`, `session`, `episode`, `source` over `max_field_bytes` with `TooLong`. `create_scope`, `register_actor` and `create_block` reject a `name` over `max_field_bytes` with `TooLong` (before the secret check).

- [ ] **Step 1: Failing tests** in `tests/propose.rs`: `content_at_the_limit_is_accepted_and_one_byte_over_is_not` (limits `{ max_content_bytes: 10, .. }`; `"a".repeat(10)` accepted, `"a".repeat(11)` → `TooLong`); `limit_counts_bytes_not_characters` (limit 10, `"é".repeat(5)` accepted, `"é".repeat(6)` → `TooLong`); `oversized_proposal_leaves_nothing_and_uses_quota` (actor quota max 2; an oversized proposal → `TooLong`, then a valid one succeeds, then the third → `QuotaExceeded`; `dump_all` does not contain a marker string placed in the oversized content; `list` has exactly one row); `metadata_fields_have_their_own_limit` (each of `topic_key`, `session`, `episode`, `source` at 257 bytes → `TooLong`, at 256 accepted); `names_are_limited_too` (`create_scope`, `register_actor` and `create_block` with a 257-byte name → `TooLong`); `limits_can_be_raised` (`with_limits(Limits { max_content_bytes: 64 * 1024, ..Limits::default() })` accepts 20 000 bytes of procedural text that the default rejects); `default_limits_are_16_kib_and_256` (assert the `Default` values).
- [ ] **Step 2:** Run `cargo test -p clio-memory --test propose`. Expected: FAIL (no `Limits`).
- [ ] **Step 3:** Implement the signatures above; export `Limits` from `lib.rs`; `Engine` holds a `Limits` initialised to `Limits::default()`.
- [ ] **Step 4:** Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] **Step 5:** Commit `fix(memory): configurable size limits on stored text`.

---

### Task 2: Linear top-k in the vector scan

**Files:** Modify `src/sqlite/mod.rs`; Test: `src/sqlite/tests.rs`, `tests/recall.rs`.

**Produces:** `nearest` keeps the best `k` with a bounded structure (`BinaryHeap`, O(n log k)) instead of re-sorting a `Vec` on every row; result order is unchanged: best score first, ties by lower id. `k = 0` returns empty without reading rows; `k` larger than the row count returns every matching row sorted.

- [ ] **Step 1: Failing tests** in `src/sqlite/tests.rs`: `nearest_returns_everything_sorted_when_k_exceeds_rows` (5 memories with distinct hand-made scores, `k = 1_000_000` → all 5 in descending score; no allocation of `k` slots: the call must not abort); `nearest_ties_break_by_lower_id` (three memories with identical vectors → ids ascending); `nearest_with_k_zero_is_empty`. In `tests/recall.rs`, extend the ignored `t25_recall_over_50k_memories_is_fast` with a second measurement, `limit: 10_000` (pool 50 000), asserting its median is under 300 ms; print both medians. Run the extended perf test now to watch it fail: `cargo test -p clio-memory --release --test recall t25 -- --ignored --nocapture`. Expected: the 10 000-limit run takes seconds and the assertion fails.
- [ ] **Step 2:** Run `cargo test -p clio-memory --lib nearest`. Expected: the two new correctness tests may already pass (they pin behavior to keep); the perf test is the red one.
- [ ] **Step 3:** Replace the per-row `push`/`sort_by`/`truncate` with a bounded heap keyed by score (total order on `f32`) then id; sort the final heap once. Keep the existing `nearest_scans_vectors_in_order` plan test green.
- [ ] **Step 4:** Run `cargo test -p clio-memory --lib nearest` and the perf test again. Expected: PASS and both medians under their thresholds (record the numbers in the commit message).
- [ ] **Step 5:** Commit `fix(memory): bounded heap for top-k in the vector scan`.

---

### Task 3: Read-only open never creates files

**Files:** Modify `src/platform/linux.rs`, `src/sqlite/mod.rs`; Test: `tests/writer_lock.rs`, `src/platform/linux.rs`.

**Produces:** `platform::check_db_file(path: &Path) -> Result<()>`: `NotFound` if the path does not exist, `InsecurePermissions(path)` if the mode has any group/other bit, otherwise `Ok`; it never creates anything. `SqliteStore::open` in `ReadOnly` mode calls it instead of `prepare_db_file`, then reads `PRAGMA user_version` through a short read-only connection: `0` (empty or uninitialised file) → `NotFound`, greater than `schema::VERSION` → `SchemaTooNew`.

- [ ] **Step 1: Failing tests** in `tests/writer_lock.rs`: `read_only_open_of_a_missing_file_fails_and_creates_nothing` (`Err(NotFound)`; the path and its `.lock` do not exist afterwards); `read_only_open_of_an_empty_file_is_not_found` (zero-byte 0600 file → `NotFound`, file unchanged); `read_only_open_of_a_newer_schema_is_rejected` (file with `user_version = 99`, mode 0600 → `SchemaTooNew(99)`); `read_only_open_still_rejects_loose_permissions` (0644 file → `InsecurePermissions`); the existing `read_only_engine_reads_but_cannot_write` must stay green. In `platform/linux.rs`: `check_db_file_never_creates` (missing path → `NotFound`, still missing).
- [ ] **Step 2:** Run `cargo test -p clio-memory --test writer_lock`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] **Step 5:** Commit `fix(memory): read-only open never creates or accepts uninitialised files`.

---

### Task 4: Same-second updates and the equal-start promote case

**Files:** Modify `src/engine.rs`; Test: `tests/temporal.rs`.

**Produces:** in `propose`, when the decision is an UPDATE **and the caller did not set `valid_from`**, the new memory's `valid_from` becomes `max(now, old.valid_from + 1)`; `known_since` stays `now`. If the caller set `valid_to` and it is not later than the bumped `valid_from`, the proposal fails with `InvalidValidity` before anything is written. An explicit `valid_from` that is not later than the old one is still `InvalidValidity` (unchanged). `promote` behavior is unchanged.

- [ ] **Step 1: Failing tests** in `tests/temporal.rs`: `two_updates_in_the_same_second_both_apply` (clock fixed at 100; three proposals with the same `topic_key` and different content: all succeed, the replaced rows have `valid_to` equal to the next row's `valid_from`, the three `valid_from` values are 100, 101, 102, `known_since` is 100 for all, exactly one is Active); `explicit_valid_from_not_after_the_old_one_is_still_rejected` (explicit `valid_from = 100` against an existing 100 → `InvalidValidity`, nothing written); `bumped_start_past_an_explicit_end_is_rejected` (old at 100; update with no `valid_from` but `valid_to = 100` at clock 100 → `InvalidValidity`, nothing written); `promote_with_equal_start_is_rejected` (external candidate and a current owner with the same `valid_from` → `promote` returns `InvalidValidity`, candidate still in quarantine, owner still Active).
- [ ] **Step 2:** Run `cargo test -p clio-memory --test temporal`. Expected: FAIL (the first two; the last may already pass and pins existing behavior).
- [ ] **Step 3:** Implement in `propose`, tracking whether `p.valid_from` was `None` before defaulting it.
- [ ] **Step 4:** Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] **Step 5:** Commit `fix(memory): updates in the same second start one second after the previous one`.

---

### Task 5: `rule-coverage.sh` cannot fail silently

**Files:** Modify `scripts/rule-coverage.sh`.

- [ ] **Step 1: Failing command.** Build a scratch tree with no test naming any rule and run the script there: `T=$(mktemp -d); mkdir -p $T/scripts $T/docs/specs $T/crates/x/tests $T/testing; cp scripts/rule-coverage.sh $T/scripts/; echo "S-01 rule" > $T/docs/security.md; echo "fn t(){}" > $T/crates/x/tests/a.rs; (cd $T && scripts/rule-coverage.sh; echo "exit=$?")`. Expected today: no summary and `exit=1`.
- [ ] **Step 2:** Make the `tested=` pipeline tolerate "no matches" (the script header promises it always exits 0); keep the rest unchanged.
- [ ] **Step 3:** Re-run the scratch command. Expected: summary `rule coverage: 0/1 rules referenced by a test`, the missing rule listed, `exit=0`. Also run `scripts/rule-coverage.sh` in the repo: still `6/53` or more.
- [ ] **Step 4:** Commit `fix(scripts): rule-coverage always exits 0 and always prints its summary`.

---

### Task 6: Pin GitHub Actions by commit

**Files:** Modify `.github/workflows/*.yml`; Create `.github/dependabot.yml`.

- [ ] **Step 1: Failing command.** `grep -hE '^\s*-?\s*uses:' .github/workflows/*.yml | grep -vE '@[0-9a-f]{40}'` lists every unpinned action. Expected today: all of them (`actions/checkout`, `Swatinem/rust-cache`, `EmbarkStudios/cargo-deny-action`, `taiki-e/install-action`, `gitleaks/gitleaks-action`).
- [ ] **Step 2:** For each distinct `owner/repo@vN`, get the commit the tag points to with `gh api repos/<owner>/<repo>/commits/<tag> --jq .sha` (run `gh` through `mise exec gh@latest --` if it is not on `PATH`); rewrite each `uses:` as `owner/repo@<40-hex> # <tag>`. Keep the existing `with:` blocks untouched.
- [ ] **Step 3:** Add `.github/dependabot.yml` with one `github-actions` ecosystem entry at directory `/`, weekly schedule.
- [ ] **Step 4:** Re-run the grep. Expected: no output. Validate the YAML with `python3 -c "import yaml,glob;[yaml.safe_load(open(f)) for f in glob.glob('.github/**/*.yml', recursive=True)]"` (skip if PyYAML is missing and say so). The workflows themselves cannot run without pushing: state that in the commit message.
- [ ] **Step 5:** Commit `ci: pin GitHub Actions by commit and add Dependabot for actions`.

---

### Task 7: Document what must never be reachable by an agent

**Files:** Modify `docs/specs/clio-memory-spec-v0.2.md`, `docs/specs/2026-10-29-clio-memory-core-design.md`.

- [ ] **Step 1:** In spec section 8, extend the MCP sentence so that, besides `promote`, `purge` and `revert`, it also says that `register_actor`, `create_scope`, `archive` and the block operations (`create_block`, `block_set`) are never exposed to an agent: a caller that can register a person actor can use every review power.
- [ ] **Step 2:** In the core design's "Riesgos conocidos", add that `register_actor` has no authority check at the crate level by design, and that the daemon (v2) must not expose it; the decision belongs to the daemon's design.
- [ ] **Step 3:** Verify: `grep -n "register_actor" docs/specs/*.md` shows both mentions.
- [ ] **Step 4:** Commit `docs: administrative methods stay out of agent-facing interfaces`.

---

### Task 8: Final gates

- [ ] **Step 1:** Run, and read the output of, each: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo test --workspace --all-features`; `scripts/rule-coverage.sh`; `cargo deny check`; `cargo audit`; `mise exec gitleaks@latest -- gitleaks detect --source . --no-banner`; `just check`; `just test`; `just audit`.
- [ ] **Step 2:** Run the ignored perf test: `cargo test -p clio-memory --release --test recall t25 -- --ignored --nocapture`. Expected: both medians under their thresholds; record them.
- [ ] **Step 3:** Update `crates/clio-memory/README.md` with the size limits (`Limits`, defaults) in one sentence.
- [ ] **Step 4:** Commit `docs(memory): mention size limits`.

---

## Self-review notes

- Coverage: audit findings 1 (Task 1), 2 (Task 2), 3 (Task 7, as a documented requirement), 4 (Task 6), 5 (Task 5), 6 (Task 4), 7 (Task 3). Finding 8 (`clio-store`) is left out on purpose, pending the user's decision.
- Decisions the plan makes for the user: limits are configurable with defaults of 16 KiB and 256 bytes; an oversized proposal consumes quota; a same-second update starts one second after the previous version (a one-second fiction in `valid_from`, never in `known_since`); finding 3 is handled by documentation, not code.
