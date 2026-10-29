# clio-memory core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build piece 1 of clio-memory: an append-only operation log with a rebuildable projection, `propose` with quota, secret detector and deterministic rules, quarantine, bitemporal reads, stemmed FTS5 search, `revert`, and a single-writer lock.

**Architecture:** A sync `Engine` over a `Store` trait with one SQLite implementation (`rusqlite`, WAL). Every operation stores full `before`/`after` row snapshots (`Change`), so the projection is a fold (upsert each `after`) and `revert` is generic (restore `before`). A `Mutex` serializes writes; reads open short-lived read-only connections. OS-specific code (writer lock, 0600 check) lives in `platform/linux.rs`.

**Tech Stack:** Rust 2024 (`unsafe_code = "forbid"`), `rusqlite` (feature `bundled`), `rust-stemmers`, `sha2`, `unicode-normalization`, `regex`, `serde` + `serde_json`, `thiserror`, `fs4`, dev: `tempfile`.

**Spec:** `docs/specs/2026-10-29-clio-memory-core-design.md` (this piece) and `docs/specs/clio-memory-spec-v0.2.md` (full engine).

## Global Constraints

- Crate `clio-memory` at `crates/clio-memory`; it must not depend on any other Clio crate (remove the `clio-store` dependency).
- `unsafe_code = "forbid"` and `clippy::unwrap_used = "deny"` (allowed in tests); `just check` and `just test` must pass; every new dependency must pass `cargo deny check` (licenses allowed: MIT, Apache-2.0; if one is rejected, stop and report instead of editing `deny.toml`).
- Sync API, no async runtime. Times are Unix seconds UTC read only from `Clock`. Ids are `i64`.
- OS-specific code only in `src/platform/linux.rs` behind `#[cfg(target_os = "linux")]`, with `compile_error!` for other targets (decision 0002).
- Every test that verifies a security rule names its id (`S-18`, `S-23`, `S-25`, `S-26`, `S-27`) in the test name or a comment so `scripts/rule-coverage.sh` sees it.
- Schema version `PRAGMA user_version = 1`. Open fails if the file's version is higher.
- Files are created with mode 0600; opening a file with any group/other permission bit fails.
- Commits are local only; never push.

## Review Focus

- FTS5 syntax in the user's query (`"`, `*`, `-`, `NEAR(`, `AND`, unbalanced quotes) must never error: it returns hits or an empty list. (Task 3, Task 7)
- The same text composed differently (`é` vs `e` + U+0301) and with extra whitespace is the same content: second propose is NOOP. (Task 2)
- Secrets in `topic_key`, `session`, `episode` or `source` are rejected like secrets in `content`, leaving nothing stored. (Task 8)
- `valid_to <= valid_from`, or an UPDATE whose `valid_from` is not after the replaced memory's `valid_from`, is rejected with `InvalidValidity`. (Task 8, Task 9)
- Promoting a quarantined candidate whose "would replace" target is no longer active activates it without replacing anything. (Task 9)

---

## File Structure

```
crates/clio-memory/
  Cargo.toml
  src/lib.rs            re-exports, platform compile_error gate
  src/error.rs          Error, Result
  src/clock.rs          Clock, SystemClock, FixedClock
  src/model.rs          ids, enums, MemoryRow, Change, Operation, Proposal, Outcome, SearchOpts, Hit, Quota
  src/normalize.rs      normalize(), content_hash()
  src/secrets.rs        find_secret()
  src/stem.rs           stem_text(), fts_query()
  src/rules.rs          effective_origin(), decide()
  src/store.rs          Store trait
  src/sqlite/mod.rs     SqliteStore (connections, Store impl)
  src/sqlite/schema.rs  DDL, version check
  src/sqlite/fold.rs    apply_change(), rebuild
  src/engine.rs         Engine (all public operations)
  src/platform/mod.rs   re-export; platform/linux.rs  writer lock + permissions
  tests/common/mod.rs   helpers
  tests/propose.rs  tests/temporal.rs  tests/revert.rs  tests/writer_lock.rs  tests/projection.rs
```

---

### Task 1: Scaffold, errors, clock, model types

**Files:** Modify `crates/clio-memory/Cargo.toml`; Create `src/{lib,error,clock,model}.rs`; Test: `src/clock.rs` (unit).

**Produces:** all types below, used by every later task.
```rust
pub struct ScopeId(pub i64); pub struct MemoryId(pub i64); pub struct OpId(pub i64); pub struct ActorId(pub i64);
pub enum MemoryType { Working, Episodic, Semantic, Procedural }
pub enum Origin { User, Agent, External }
pub enum State { Active, Quarantine, Archived, Replaced }
pub enum ActorKind { Agent, Session, Person, Importer }
pub enum OpKind { Add, Update, Noop, Promote, Archive, Revert }
pub struct MemoryRow { pub id: MemoryId, pub scope: ScopeId, pub kind: MemoryType, pub content: String, pub content_hash: String,
  pub topic_key: Option<String>, pub origin: Origin, pub actor: ActorId, pub session: Option<String>, pub episode: Option<String>,
  pub source: Option<String>, pub state: State, pub valid_from: i64, pub valid_to: Option<i64>, pub known_since: i64,
  pub replaced_by: Option<MemoryId>, pub supersedes: Option<MemoryId> }
pub struct Change { pub memory: MemoryId, pub before: Option<MemoryRow>, pub after: MemoryRow }
pub struct Operation { pub id: OpId, pub scope: ScopeId, pub kind: OpKind, pub memory: Option<MemoryId>, pub changes: Vec<Change>,
  pub actor: ActorId, pub reason: String, pub at: i64, pub reverts: Option<OpId> }
pub struct Proposal { pub scope: ScopeId, pub actor: ActorId, pub kind: MemoryType, pub content: String, pub topic_key: Option<String>,
  pub origin: Origin, pub session: Option<String>, pub episode: Option<String>, pub source: Option<String>,
  pub valid_from: Option<i64>, pub valid_to: Option<i64> }
pub struct Outcome { pub op: OpId, pub kind: OpKind, pub memory: MemoryId, pub state: State }
pub struct Quota { pub max: u32, pub window_secs: i64 }   // Default: 100 per 3600
pub struct SearchOpts { pub include_quarantine: bool, pub as_of: Option<i64>, pub known_at: Option<i64>, pub limit: usize } // Default limit 10
pub struct Hit { pub memory: MemoryRow, pub score: f64 }
pub struct RevertReport { pub reverted: Vec<OpId>, pub skipped: Vec<OpId> }
pub enum OpenMode { ReadWrite, ReadOnly }
pub trait Clock: Send + Sync { fn now(&self) -> i64; }   // SystemClock, FixedClock(AtomicI64) with set(i64)
pub enum Error { EmptyContent, QuotaExceeded, SecretDetected, InvalidValidity, UnknownScope, UnknownActor, NotFound, InvalidState,
  NotAllowed, NotRevertible(OpId), WriterLocked, ReadOnly, InsecurePermissions(PathBuf), SchemaTooNew(i64),
  Sqlite(rusqlite::Error), Io(std::io::Error), Json(serde_json::Error) }   // thiserror, From impls for the last three
```
Enums and structs derive `Debug, Clone, PartialEq` (+ `Copy, Eq` on enums/ids) and `Serialize, Deserialize` on `MemoryRow`, `Change`, enums and ids (they are the JSON snapshots stored in the log).

- [ ] **Step 1:** In `Cargo.toml` remove `clio-store`; run `cargo add -p clio-memory rusqlite --features bundled`, `rust-stemmers sha2 unicode-normalization regex thiserror fs4`, `serde --features derive`, `serde_json`, and `cargo add -p clio-memory --dev tempfile`. Keep the `telemetry = []` feature.
- [ ] **Step 2: Failing test** in `src/clock.rs`: `fixed_clock_returns_set_value` (`FixedClock::new(100).now() == 100`; after `set(250)`, `now() == 250`).
- [ ] **Step 3:** Create the files with the types above; `lib.rs` declares modules and re-exports everything public. `FixedClock` uses `AtomicI64`.
- [ ] **Step 4:** Run `cargo test -p clio-memory && cargo clippy -p clio-memory --all-targets -- -D warnings && cargo deny check`. Expected: pass. If `cargo deny` rejects a license, stop and report.
- [ ] **Step 5:** Commit `feat(memory): scaffold crate, model types, clock`.

---

### Task 2: Normalize, hash, secret detector

**Files:** Create `src/normalize.rs`, `src/secrets.rs` (unit tests inline).

**Produces:** `pub fn normalize(s: &str) -> String` (NFC, trim, whitespace runs → one space); `pub fn content_hash(normalized: &str) -> String` (SHA-256 hex); `pub fn find_secret(s: &str) -> bool`.

- [ ] **Step 1: Failing tests** (`normalize.rs`): `composed_and_decomposed_hash_equal` (`"caf\u{e9}"` vs `"cafe\u{301}"` give equal `content_hash(normalize(..))`); `whitespace_collapsed` (`"  a \n\t b "` → `"a b"`). (`secrets.rs`): `flags_known_formats` for each of: `sk-` + 24 alphanumerics, `ghp_` + 36 alphanumerics, `AKIA` + 16 uppercase/digits, `xoxb-1234567890-abcdefghij`, `-----BEGIN RSA PRIVATE KEY-----`, a three-part `eyJ…​.eyJ…​.sig` JWT; `flags_long_random_token` (40 random mixed-case alphanumerics); `ignores_ordinary_text` (a Spanish sentence, an `https://example.com/a/b?c=d` URL, a UUID, a 64-char hex SHA-256, `"vive en Medellín desde 2026-03"`).
- [ ] **Step 2:** Run `cargo test -p clio-memory normalize secrets`. Expected: FAIL (functions missing).
- [ ] **Step 3:** Implement. `find_secret` = any of the regexes above, or a whitespace-delimited token of ≥32 chars over `[A-Za-z0-9+/=_-]` whose Shannon entropy is ≥ 4.2 bits/char (hex never reaches it). Compile regexes once with `std::sync::LazyLock`.
- [ ] **Step 4:** Run the tests again. Expected: PASS.
- [ ] **Step 5:** Commit `feat(memory): normalization, hashing and secret detector`.

---

### Task 3: Stemming and FTS query building

**Files:** Create `src/stem.rs`.

**Produces:** `pub fn stem_text(lang: &str, text: &str) -> String` and `pub fn fts_query(lang: &str, q: &str) -> Option<String>`.

- [ ] **Step 1: Failing tests:** `spanish_stems_inflections` (`stem_text("es","corriendo corren")` yields two identical tokens); `unknown_language_only_lowercases_and_splits` (`stem_text("xx","Hola, Mundo")` == `"hola mundo"`); `query_quotes_and_ors` (`fts_query("es","vive en Medellín")` is `Some` with every token wrapped in double quotes and joined by ` OR `); `hostile_query_is_safe` (`fts_query("es", r#"a" OR NEAR( * -x"#)` is `Some` and contains no unquoted operator characters); `empty_query_is_none` (`fts_query("es","  ")` and `fts_query("es","***")` are `None`).
- [ ] **Step 2:** Run `cargo test -p clio-memory stem`. Expected: FAIL.
- [ ] **Step 3:** Implement: lowercase, split on non-alphanumeric (Unicode), stem each token with the `rust_stemmers::Algorithm` for `"es"`, `"en"`, `"fr"`, `"de"`, `"pt"`, `"it"` (other codes: no stemming). `fts_query` quotes each token (tokens contain only alphanumerics, so no escaping is needed) and joins with ` OR `.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit `feat(memory): stemming and safe FTS queries`.

---

### Task 4: Platform: writer lock and 0600 check (S-23)

**Files:** Create `src/platform/mod.rs`, `src/platform/linux.rs`; Modify `src/lib.rs`.

**Produces:** `platform::prepare_db_file(path: &Path) -> Result<()>` (create with mode 0600 if missing; if existing mode has any `0o077` bit, `Err(Error::InsecurePermissions(path))`); `platform::try_lock_writer(db_path: &Path) -> Result<WriterLock>` (exclusive non-blocking `flock` on `<db_path>.lock`, created 0600; held until the value is dropped; `Err(Error::WriterLocked)` if taken). `mod.rs` re-exports `linux::*` under `#[cfg(target_os = "linux")]` and has `#[cfg(not(target_os = "linux"))] compile_error!("clio-memory has no platform module for this OS; see docs/decisions/0002-platform-compatibility.md");`.

- [ ] **Step 1: Failing tests** in `linux.rs`: `s23_new_file_gets_0600`; `s23_existing_0644_file_is_rejected` (create with 0644, `prepare_db_file` returns `InsecurePermissions`); `second_lock_fails_until_first_dropped` (`try_lock_writer` twice on the same path: second is `WriterLocked`; after dropping the first, a third succeeds). Use `tempfile::tempdir`.
- [ ] **Step 2:** Run `cargo test -p clio-memory platform`. Expected: FAIL.
- [ ] **Step 3:** Implement with `std::os::unix::fs::{OpenOptionsExt, PermissionsExt}` and `fs4::fs_std::FileExt::try_lock_exclusive` (safe wrapper; no `unsafe`).
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit `feat(memory): linux platform module with writer lock and permission check`.

---

### Task 5: Rules (pure decision function)

**Files:** Create `src/rules.rs` (unit tests inline).

**Produces:**
```rust
pub fn effective_origin(actor: ActorKind, declared: Origin) -> Origin   // Agent|Session declaring User -> Agent
pub struct Existing<'a> { pub by_hash: Option<&'a MemoryRow>, pub by_topic: Option<&'a MemoryRow> } // both already filtered to Active, same scope (by_topic also same type)
pub enum Decision { Noop(MemoryId), Add { state: State, supersedes: Option<MemoryId> }, Update { old: MemoryId } }
pub fn decide(kind: MemoryType, effective: Origin, existing: Existing) -> Decision
```
Order: hash match → `Noop`; quarantine needed (`effective == External`, or `kind == Procedural && effective != User`) → `Add{state: Quarantine, supersedes: by_topic.map(id)}`; topic match → `Update{old}`; else `Add{state: Active, supersedes: None}`.

- [ ] **Step 1: Failing tests:** `agent_cannot_claim_user_origin`; `person_keeps_declared_origin`; `hash_match_is_noop_even_for_external`; `external_with_topic_match_goes_to_quarantine_not_update` (S-25: result is `Add{Quarantine, supersedes: Some(old)}`); `agent_procedural_is_quarantined` (S-26); `user_procedural_is_active`; `topic_match_updates`; `no_match_adds_active`.
- [ ] **Step 2:** Run `cargo test -p clio-memory rules`. Expected: FAIL.
- [ ] **Step 3:** Implement as specified.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit `feat(memory): deterministic write rules`.

---

### Task 6: Store trait, SQLite schema, append, fold, rebuild

**Files:** Create `src/store.rs`, `src/sqlite/{mod,schema,fold}.rs`; Test: `tests/projection.rs`, `tests/common/mod.rs`.

**Produces** (`Store: Send + Sync`; `SqliteStore::open(path: &Path, mode: OpenMode) -> Result<SqliteStore>`):
```rust
fn create_scope(&self, name: &str, language: &str) -> Result<ScopeId>;
fn scope_language(&self, scope: ScopeId) -> Result<String>;          // UnknownScope
fn register_actor(&self, name: &str, kind: ActorKind, quota: Quota) -> Result<ActorId>;
fn actor_kind(&self, actor: ActorId) -> Result<ActorKind>;           // UnknownActor
fn bump_quota(&self, actor: ActorId, now: i64) -> Result<()>;        // counts one proposal in the actor's window; Err(QuotaExceeded) when over max; commits on its own
fn find_active_by_hash(&self, scope: ScopeId, hash: &str) -> Result<Option<MemoryRow>>;
fn find_active_by_topic(&self, scope: ScopeId, kind: MemoryType, topic: &str) -> Result<Option<MemoryRow>>;
fn next_memory_id(&self) -> Result<MemoryId>;
fn append(&self, op: NewOperation) -> Result<OpId>;                  // one transaction: insert operation, fold its changes into memories + FTS
fn get(&self, id: MemoryId) -> Result<MemoryRow>;                    // NotFound
fn list(&self, scope: ScopeId, state: Option<State>, kind: Option<MemoryType>) -> Result<Vec<MemoryRow>>;  // ordered by id
fn history(&self, id: MemoryId) -> Result<Vec<Operation>>;           // ops whose changes touch the memory, ordered by id
fn operation(&self, id: OpId) -> Result<Operation>;
fn operations_from(&self, from: OpId) -> Result<Vec<Operation>>;     // id >= from, ascending
fn operations_by_actor(&self, actor: ActorId) -> Result<Vec<Operation>>;
fn rebuild_projection(&self) -> Result<()>;                          // clears memories + FTS, refolds all operations in id order, in one transaction
```
`NewOperation` is `Operation` without `id`. Tables: `scopes`, `actors`, `quota_counter(actor, window_start, count)`, `operations(id, scope, kind, memory, changes TEXT json, actor, reason, at, reverts)`, `memories` (one column per `MemoryRow` field), `memories_fts` (FTS5, one `text` column, `tokenize='unicode61 remove_diacritics 2'`, rowid = memory id, text = `stem_text(scope language, content)`). Reads open a short-lived read-only connection per call (`// shortcut: one connection per read, pool if recall latency needs it`); writes go through one `Mutex<Connection>` (WAL, `foreign_keys=ON`); `ReadOnly` mode holds no write connection and write methods return `Error::ReadOnly`. `fold::apply_change` upserts `after` and refreshes its FTS row.

- [ ] **Step 1: Failing tests** in `tests/projection.rs` (helper in `tests/common/mod.rs` builds a `SqliteStore` in a tempdir): `t01_rebuild_projection_gives_identical_state` — via the store, create a scope and actor, `append` an Add, an Update (two changes: old → Replaced with `valid_to` and `replaced_by`, new → Active) and an Archive; capture `list(scope, None, None)`, call `rebuild_projection`, assert `list` is equal; also assert a `search` hit count stays equal once Task 7 exists (leave that assertion for Task 7). `append_is_atomic` — `append` with a change whose `after` violates a constraint (e.g. unknown scope id) returns `Err` and `operation(OpId(1))` is `NotFound`. `schema_too_new_is_rejected` — set `PRAGMA user_version=99` on a file, `open` returns `SchemaTooNew(99)`. `bump_quota_limits_per_window` — quota max 2, window 3600: third call at the same `now` is `QuotaExceeded`; a call at `now + 3600` succeeds.
- [ ] **Step 2:** Run `cargo test -p clio-memory --test projection`. Expected: FAIL.
- [ ] **Step 3:** Implement schema creation + version check on open (`PRAGMA user_version`), the `Store` trait and `SqliteStore`. `changes` is stored as JSON of `Vec<Change>`.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit `feat(memory): store trait, sqlite schema, append and rebuild`.

---

### Task 7: Search with temporal filters

**Files:** Modify `src/store.rs`, `src/sqlite/mod.rs`; Test: `tests/temporal.rs`, `tests/projection.rs`.

**Produces:** `fn search(&self, scope: ScopeId, query: &str, opts: &SearchOpts) -> Result<Vec<Hit>>` on `Store`. Query goes through `fts_query` with the scope's language (`None` → empty result); score is `-bm25()` so higher is better; ordered by score descending, then id. Filters: no `as_of`/`known_at` → `state = Active` (plus `Quarantine` when `include_quarantine`); with `as_of = T` → states Active or Replaced with `valid_from <= T AND (valid_to IS NULL OR valid_to > T)`; with `known_at = K` → also `known_since <= K`. Always limited to the scope and to `opts.limit`.

- [ ] **Step 1: Failing tests** in `tests/temporal.rs` (store-level, rows appended directly): `search_finds_stemmed_match` (indexed "corriendo en el parque", query "corren" hits); `search_ignores_other_scopes`; `quarantine_hidden_unless_requested`; `t06_as_of_returns_fact_valid_then` (old memory valid [100,200), new valid from 200; `as_of=150` returns only the old one, `as_of=250` only the new); `t17_known_at_hides_later_learned_facts` (memory with `valid_from=10`, `known_since=300`; `known_at=100, as_of=150` returns nothing; `known_at=400` returns it); `hostile_queries_do_not_error` (the Review Focus strings return `Ok`). Add to `tests/projection.rs` the post-rebuild search-equality assertion deferred in Task 6.
- [ ] **Step 2:** Run `cargo test -p clio-memory --test temporal`. Expected: FAIL.
- [ ] **Step 3:** Implement with an FTS5 `MATCH` joined to `memories` on rowid.
- [ ] **Step 4:** Run `cargo test -p clio-memory`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(memory): stemmed FTS search with validity and knowledge filters`.

---

### Task 8: Engine: open, scopes, actors, propose (ADD, NOOP, quarantine)

**Files:** Create `src/engine.rs`; Modify `src/lib.rs`; Test: `tests/propose.rs`.

**Produces:**
```rust
impl Engine {
  pub fn open(path: &Path, mode: OpenMode, clock: Arc<dyn Clock>) -> Result<Engine>; // prepare_db_file, then try_lock_writer for ReadWrite
  pub fn create_scope(&self, name: &str, language: &str) -> Result<ScopeId>;
  pub fn register_actor(&self, name: &str, kind: ActorKind, quota: Quota) -> Result<ActorId>;
  pub fn propose(&self, p: Proposal) -> Result<Outcome>;
  pub fn get(&self, id: MemoryId) -> Result<MemoryRow>;
  pub fn list(&self, scope: ScopeId, state: Option<State>, kind: Option<MemoryType>) -> Result<Vec<MemoryRow>>;
  pub fn history(&self, id: MemoryId) -> Result<Vec<Operation>>;
  pub fn search(&self, scope: ScopeId, query: &str, opts: SearchOpts) -> Result<Vec<Hit>>;
  pub fn rebuild_projection(&self) -> Result<()>;
}
```
`propose` order: reject empty/whitespace `content` (`EmptyContent`) → reject `valid_to <= valid_from` (`InvalidValidity`) → `bump_quota` → `find_secret` over `content`, `topic_key`, `session`, `episode`, `source` (`SecretDetected`; the quota counter already ticked, nothing else is written) → normalize + hash → `effective_origin` (via `actor_kind`) → lookups → `rules::decide` → build the operation → `append`. All of `propose` runs under an `Engine`-level `Mutex<()>`. `Outcome` for a NOOP carries the existing memory id and `State::Active`, and the NOOP operation is logged with no changes. `valid_from` defaults to `clock.now()`; `known_since` is always `clock.now()`. Operation `reason` is a short fixed string per rule (`"hash match"`, `"external origin"`, `"procedural by non-user"`, `"new"`).

- [ ] **Step 1: Failing tests** in `tests/propose.rs` (helper: tempdir engine with `FixedClock`, scope `es`, a Person actor and an Agent actor): `t02_same_content_twice_is_add_then_noop` (kinds `[Add, Noop]`, one memory); `unicode_equivalent_content_is_noop`; `t03_secret_is_rejected_and_leaves_no_trace` — propose content containing `sk-` + 24 alphanumerics returns `SecretDetected`, then read every table's text via a raw `rusqlite` connection on the file (`sqlite_master` → `SELECT * FROM <table>`) and assert the secret string appears nowhere; `secret_in_metadata_fields_is_rejected` (S-18; `topic_key`, `session`, `episode`, `source` each); `t04_external_origin_hidden_until_promote_s25` (stays out of `search` and appears in `list(Some(Quarantine))`); `s26_agent_procedural_is_quarantined_person_procedural_is_active`; `agent_cannot_claim_user_origin` (stored `origin` is `Agent`); `t05_scopes_are_isolated` (same content and `topic_key` in scopes A and B: both ADD; search in A never returns B); `s27_quota_exceeded_is_rejected` (max 2: third propose is `QuotaExceeded`; a secret-rejected propose also consumed one); `empty_content_rejected`; `invalid_validity_rejected`.
- [ ] **Step 2:** Run `cargo test -p clio-memory --test propose`. Expected: FAIL.
- [ ] **Step 3:** Implement `Engine` as specified, delegating to `Store`. UPDATE is left returning `Error::InvalidState` until Task 9 (the `Decision::Update` arm).
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit `feat(memory): engine with propose, quarantine and quotas`.

---

### Task 9: Engine: UPDATE, promote, archive

**Files:** Modify `src/engine.rs`; Test: `tests/temporal.rs` (engine-level cases appended).

**Produces:**
```rust
pub fn promote(&self, id: MemoryId, actor: ActorId) -> Result<OpId>;   // actor must be ActorKind::Person (NotAllowed); memory must be Quarantine (InvalidState)
pub fn archive(&self, id: MemoryId, actor: ActorId) -> Result<OpId>;   // memory must be Active or Quarantine (InvalidState)
```
UPDATE (`Decision::Update{old}`): one operation, two changes — old → `Replaced` with `valid_to = new.valid_from` and `replaced_by = new`; new → `Active`. If `new.valid_from <= old.valid_from`, return `InvalidValidity` before writing. `promote` sets the memory `Active`; if it has `supersedes = Some(old)` and `old` is still `Active`, it adds the replacement changes as in UPDATE (same `InvalidValidity` check), otherwise it only activates and clears nothing else. Promote and archive are logged as `Promote` / `Archive` operations with `before`/`after` snapshots.

- [ ] **Step 1: Failing tests:** `update_replaces_and_closes_validity` (propose "vive en Medellín" with `topic_key="residencia"` at clock 100, then "vive en Bogotá" same topic at clock 200: old is `Replaced`, `valid_to=200`, `replaced_by` = new; new is `Active`; `search` for "Medellín" returns nothing now but returns the old one with `as_of=150`); `update_with_earlier_valid_from_is_rejected` (new `valid_from` ≤ old's → `InvalidValidity`, nothing written); `external_update_waits_in_quarantine` (S-25; external same-topic proposal: old stays `Active`, candidate is `Quarantine` with `supersedes`; after `promote` by a Person the old is `Replaced` and the candidate `Active`); `promote_requires_person` (Agent actor → `NotAllowed`); `promote_with_stale_target_only_activates` (archive the target before promote: candidate becomes `Active`, nothing replaced); `promote_non_quarantine_is_invalid_state`; `archive_hides_from_search`.
- [ ] **Step 2:** Run `cargo test -p clio-memory --test temporal`. Expected: FAIL.
- [ ] **Step 3:** Implement. Build all changes inside the same locked section and a single `append`.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit `feat(memory): update, promote and archive`.

---

### Task 10: Engine: revert (single, by actor, since)

**Files:** Modify `src/engine.rs`; Test: `tests/revert.rs`.

**Produces:**
```rust
pub fn revert(&self, op: OpId, actor: ActorId) -> Result<OpId>;
pub fn revert_actor(&self, target: ActorId, by: ActorId) -> Result<RevertReport>;
pub fn revert_since(&self, from: OpId, by: ActorId) -> Result<RevertReport>;
```
Rule for one operation: for each change, the new `after` is `before` (or, when `before` is `None`, the old `after` with `state = Archived`); the new operation is `Revert` with `reverts = Some(op)`, `changes` swapped accordingly (`before` = current row, `after` = restored row), `known_since` untouched. Allowed only if every change's current row equals that change's `after` (`NotRevertible(op)` otherwise, nothing written). A NOOP reverts to an empty REVERT operation. Bulk calls walk candidate operations newest → oldest, skip operations of kind `Revert` and `Noop`, revert the eligible ones and list the rest in `skipped`; `revert_actor` candidates are operations by `target`, `revert_since` candidates are all operations with id ≥ `from`.

- [ ] **Step 1: Failing tests:** `t10_revert_add_archives_and_logs` (Add → revert: memory `Archived`, history ends with a `Revert` op whose `reverts` is the Add); `revert_update_restores_old_and_archives_new` (old `Active` with `valid_to=None`, new `Archived`); `revert_promote_returns_to_quarantine`; `revert_of_revert_reapplies`; `revert_blocked_when_memory_changed_later` (Add by A, Update by B: reverting A's Add returns `NotRevertible`, history unchanged); `t15_revert_actor_undoes_all_of_one_actor` (actor X makes three Adds, actor Y one: after `revert_actor(X, person)` X's three are `Archived`, Y's untouched, report has three `reverted`, none `skipped`; running it again reverts nothing new); `revert_actor_skips_covered_operations` (X adds, Y updates the same topic: X's Add is in `skipped`, nothing else changed); `revert_since_walks_newest_first`; `revert_keeps_known_since`; `revert_then_rebuild_matches` (rebuild_projection after reverts gives identical `list`).
- [ ] **Step 2:** Run `cargo test -p clio-memory --test revert`. Expected: FAIL.
- [ ] **Step 3:** Implement as specified.
- [ ] **Step 4:** Run the tests. Expected: PASS.
- [ ] **Step 5:** Commit `feat(memory): revert by operation, actor and point`.

---

### Task 11: Single writer, crate docs, full gates

**Files:** Test: `tests/writer_lock.rs`; Create `crates/clio-memory/README.md`.

- [ ] **Step 1: Failing tests:** `t16_second_writer_gets_clear_error` (two `Engine::open(path, ReadWrite, ..)` on the same file: second is `WriterLocked`; after dropping the first a new one opens); `read_only_engine_reads_but_cannot_write` (`ReadOnly` open while a writer exists: `get`/`search` work, `propose` and `create_scope` return `ReadOnly`); `s23_open_rejects_0644_file` (existing 0644 file: `open` returns `InsecurePermissions`; new file ends up 0600); `engine_survives_reopen` (propose, drop, reopen, `get` returns the same row).
- [ ] **Step 2:** Run `cargo test -p clio-memory --test writer_lock`. Expected: FAIL only where behavior is missing; fix `Engine::open`/`ReadOnly` handling accordingly.
- [ ] **Step 3:** Write `crates/clio-memory/README.md` (what it is, the single-file/one-writer model, a 15-line usage example with `Engine::open`, `propose`, `search`, and a pointer to the two specs).
- [ ] **Step 4:** Run `just check`, `just test`, `just audit`. Expected: all pass; `scripts/rule-coverage.sh` lists S-18, S-23, S-25, S-26, S-27 as referenced by a test.
- [ ] **Step 5:** Commit `feat(memory): single-writer enforcement and crate readme`.

---

## Self-review notes

- Coverage: acceptance tests 1 (T6), 2, 3, 4, 5 (T8), 6, 17 (T7), 10, 15 (T10), 16 (T11); rules S-18, S-23, S-25, S-26, S-27 mapped to tests; PURGE, vectors, decider, queues deferred as in the design.
- Deviation from the design, for the user to confirm: writes are serialized by a `Mutex`, not a dedicated writer thread (same guarantee, less machinery). "Revert only if last on its memory" is implemented as "the memory still equals the state the operation left", which also lets bulk reverts chain correctly.
