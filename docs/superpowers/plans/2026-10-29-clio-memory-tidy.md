# clio-memory minor findings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the minor findings left by the reviews: size limit on stored (normalized) text, validated scope language, a script that reports a missing directory, a clear error for read-only opens in a non-writable directory, no panic escaping the model2vec embedder, `embed_pending` failing fast on a read-only engine, and `abstained` meaning "nothing relevant" even with `limit: 0`.

**Architecture:** Seven small independent changes, each with a failing test first. No new dependencies.

**Tech Stack:** Rust 2024, `rusqlite`, bash.

**Spec:** the review findings in this conversation; designs in `docs/specs/`. Accepted as they are (not in this plan): no ceiling on `with_limits`, unstemmed text in the t25 fixture, `scope_language` queried twice per `recall`, no diacritic folding in coverage for languages without a stop list, `NewOperation` duplicating `Operation`, quota accounting of empty/invalid proposals.

## Global Constraints

- Branch `feat/clio-memory`; commits local only. Gates: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features`, `scripts/rule-coverage.sh`, `cargo deny check`, `cargo audit`, gitleaks via `mise exec gitleaks@latest -- gitleaks detect --source . --no-banner`.
- `unsafe_code = "forbid"`, `clippy::unwrap_used = "deny"` (test helpers start with `#![allow(clippy::unwrap_used)]`).
- Limits are bytes. The storage layer stays private.

## Review Focus

- Text whose NFC form is longer than its input must be judged on the stored form. (Task 1)
- A read-only open in a non-writable directory must not leave files behind or fall back to a stale `immutable` read. (Task 3)
- A panic inside the embedder must come back as `Error::Embedding` and not poison the write lock. (Task 4)

---

### Task 1: Limit applies to the stored text
**Files:** `src/engine.rs`, `tests/propose.rs`, `crates/clio-memory/README.md`.
- [ ] Failing test `limit_applies_to_the_normalized_text`: limit 30, content `"\u{FB2C}".repeat(10)` (30 bytes raw, 60 bytes after NFC) → `Err(TooLong)`; and `"a  b"`-style whitespace-heavy text that is over the limit raw but within it after normalization is still rejected by the raw check (documented early reject).
- [ ] Run `cargo test -p clio-memory --test propose limit_applies`. Expected: FAIL.
- [ ] After `normalize`, return `TooLong` if the normalized content exceeds `max_content_bytes`. README: say the limit caps stored text again.
- [ ] Run the suite; commit `fix(memory): size limit also applies to the normalized text`.

### Task 2: Validate the scope language
**Files:** `src/engine.rs`, `tests/propose.rs`.
- [ ] Failing test `scope_language_is_checked`: a 257-byte language → `TooLong`; `"AKIA" + 16 uppercase` language → `SecretDetected`; nothing stored (`dump_all`).
- [ ] Run. Expected: FAIL.
- [ ] Pass `language` through `check_name` in `create_scope`.
- [ ] Run; commit `fix(memory): scope language goes through the name checks`.

### Task 3: Clear error for a read-only open in a non-writable directory
**Files:** `src/error.rs`, `src/sqlite/mod.rs`, `tests/writer_lock.rs`.
**Produces:** `Error::UnwritableDirectory(PathBuf)` ("opening a WAL database read-only needs a writable directory"). The read-only probe maps `SQLITE_READONLY` and `SQLITE_CANTOPEN` failures to it (with the database's directory); no `immutable` fallback.
- [ ] Failing test `read_only_open_in_a_non_writable_directory_is_a_clear_error`: create a WAL database with a normal engine, drop it, `chmod 0500` its directory; if a file can still be created there (running as root) restore permissions and return early; otherwise `Engine::open(ReadOnly)` → `Err(UnwritableDirectory(dir))` and no new file appears; restore `0700` at the end so the tempdir can be removed.
- [ ] Run. Expected: FAIL (raw SQLite error).
- [ ] Implement the mapping.
- [ ] Run; commit `fix(memory): clear error when a read-only open needs a writable directory`.

### Task 4: No panic escapes the embedder
**Files:** `src/embed.rs`, `src/model2vec.rs`.
**Produces:** `pub(crate) fn catch_panic<T>(f: impl FnOnce() -> T) -> Result<T>` using `std::panic::catch_unwind(AssertUnwindSafe(f))`, mapping a panic to `Error::Embedding("the embedding model panicked: ...")`. `Model2VecEmbedder::embed` calls the model through it.
- [ ] Failing unit tests in `embed.rs`: `catch_panic_turns_a_panic_into_an_error` (closure that panics → `Err(Embedding(_))`), `catch_panic_passes_values_through`.
- [ ] Run `cargo test -p clio-memory --lib catch_panic`. Expected: FAIL.
- [ ] Implement and use it in `model2vec.rs` (feature-gated); check `cargo clippy -p clio-memory --features model2vec -- -D warnings`.
- [ ] Run; commit `fix(memory): a panic inside the embedding model becomes an error`.

### Task 5: `embed_pending` fails fast on a read-only engine
**Files:** `src/engine.rs`, `tests/embed.rs`.
- [ ] Failing test `read_only_engine_does_not_embed`: a writer engine with a memory; a `ReadOnly` engine on the same file with a `CountingEmbedder` → `embed_pending(10)` is `Err(ReadOnly)` and the embedder was called 0 times.
- [ ] Run. Expected: FAIL (it embeds first).
- [ ] `Engine` keeps its `OpenMode`; `embed_pending` returns `ReadOnly` first.
- [ ] Run; commit `fix(memory): embed_pending checks read-only before computing`.

### Task 6: `abstained` with `limit: 0`
**Files:** `src/engine.rs`, `tests/recall.rs`.
- [ ] Failing test `limit_zero_is_not_an_abstention`: one memory "vive en Medellín", query "Medellín", `limit: 0` → `hits` empty and `abstained == false`.
- [ ] Run. Expected: FAIL.
- [ ] Compute `abstained` from the fused list before applying the limit.
- [ ] Run; commit `fix(memory): a result emptied by limit 0 is not an abstention`.

### Task 7: `rule-coverage.sh` reports a missing directory
**Files:** `scripts/rule-coverage.sh`.
- [ ] Failing command: in a scratch tree without `testing/`, run the script and expect a line `warning: directory not found: testing` on stderr; today the `find` error is hidden (`2>&1 | grep`).
- [ ] Search only the directories that exist and print the warning for the missing ones; still exit 0 and still print the summary.
- [ ] Re-run the scratch command and `scripts/rule-coverage.sh` (6/53 or more); commit `fix(scripts): rule-coverage warns about missing directories`.

### Task 8: Final gates
- [ ] Run every gate in Global Constraints and read each result; run `cargo test -p clio-memory --release --test recall t25 -- --ignored --nocapture` and the real-model probes (`CLIO_MODEL2VEC_DIR=$HOME/.cache/clio-models/potion-multilingual-128M cargo test -p clio-memory --release --features model2vec -- --ignored real_model model2vec_real`).
