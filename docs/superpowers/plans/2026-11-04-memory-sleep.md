# Memory Sleep Phase Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A nightly, unattended sleep for the shared memory database:
- check it and copy it;
- quarantine secrets and likely attacks;
- fold exact repeats and, in Clio, merge near-duplicates with the person's configured model;
- purge what was discarded long ago;
- tell the agent the next morning.

**Architecture:**
- clio-memory gets an engine layer (new System-only operations whose safety rules live in the engine) and a `sleep` module that runs one night. The module takes an optional `Merger` trait object for the model step.
- `clio-memory sleep` runs it without a merger.
- Clio implements `Merger` with its configured model (`config.model`, whatever connector) and Jev, and runs it as `clio memory sleep`.
- A user timer (systemd/launchd) wakes it every 30 min from 23:00 to 05:30.

**Tech Stack:** Rust 2024, rusqlite (WAL), existing `clio_memory::Engine`, `fs2`-style flock already in `platform/unix.rs`, serde_json, Clio's `agent::Model` / `CommandModel` and `MemoryJudge`.

**Spec:** `docs/specs/2026-11-04-memory-sleep-design.md` (clio repo)

## Global Constraints

- The sleep uses no language model in clio-memory. In Clio, the merge uses `config.model` (provider, command and model as configured). No provider is named in code.
- Sleep operations are signed by the existing System actor, with reasons starting `sueño:`.
- Never touch what is the person's: `Engine::held_by_person(row)` or pinned (`pins`).
- Purge only memories discarded (their quarantine came from a `Forget` or `Flag` op, or from the sleep). The plazo is ≥ 30 days in quarantine, or ≥ 7 if the sleep sent it for a secret. Never a person's memory unless the person discarded it. Only when that night's copy succeeded.
- Per-night caps:
  - quarantines ≤ max(10, 10 % of active), secrets first;
  - purges ≤ 50;
  - merge groups ≤ 20, each of 2–4 memories.
  - Merge stops if more than half of its groups fail validation.
- Thresholds (Granite):
  - repeat ≥ 0.97;
  - merge candidates in [0.87, 0.97);
  - merged text similarity to each original ≥ 0.80;
  - attack quarantine at ≥ 2 points, or for hidden characters alone.
- Gates:
  - the hour window comes from the timer;
  - the command needs no complete sleep in the last 20 h, no non-sleep operation and no recorded use in the last 30 min, and the `<db>.sleep.lock` flock free;
  - `--now` skips quiet;
  - `--dry-run` writes nothing.
- The writer is held only in short sections (< 1 s): one memory, or one small vector batch. Analysis uses a read-only connection. Between sections, stop if another actor wrote since the sleep started.
- Reports and state live in `<data>/sleep/` (600 files, 700 dir) as JSON:
  - `state.json`: cursor `OpId`, `rules_version`, last complete;
  - `YYYY-MM-DD.json`, keeping the last 60;
  - copies `memory-YYYY-MM-DD.db`, keeping the last 3.
  - No schema change; `user_version` stays.
- The sleep's quarantines do not count against the author's quarantine limit.
- The public clio-memory repo carries no personal data. Commits use the noreply email. Split key-like test literals with `concat!`.
- Never `cargo +nightly`. Verify with `--all-features`. fmt, clippy `-D warnings` and gitleaks stay clean.
- Merges to main and tags happen only with Iván's yes.

## Review Focus

1. **A night interrupted mid-way** (another writer appears, crash, kill). Expected: everything done so far is consistent and undoable, the cursor does not advance, and the next slot continues.
2. **A memory text that tries to steer the merge model** (forged `</datos>`, "ignore previous", a URL to copy). Expected: the merged text never carries anything absent from the originals; the group is left «para revisar».
3. **Clio and Claude Code writing during the sleep.** Expected: their writes succeed within their ~2 s retry.
4. **Two installations** (clio-memory's timer and Clio's) or two machines' slots on the same night. Expected: one sleep only.
5. **The person's own memory with a secret, or a near-duplicate of an agent memory.** Expected: the person's memory is never quarantined, merged or purged; it is only reported.

---

### Task 1: Engine operations for the sleep (clio-memory)

**Files:**
- Modify: `src/engine.rs` (new pub fns near `forget` :1673 and `purge` :1703), `src/store.rs` and `src/sqlite/mod.rs` (`count_quarantined_by` :1257, a new `operations_after`)
- Test: `tests/sleep_engine.rs`

**Interfaces:**
- Produces:
  - `Engine::sleep_quarantine(&self, id: MemoryId, reason: &str) -> Result<OpId>`. An op of kind `Forget`, actor System, `reason` must start with `sueño:`. Refuses (`Error::NotAllowed`) when `held_by_person(row)` or pinned, and (`InvalidState`) when not Active.
  - `Engine::sleep_purge(&self, id: MemoryId, reason: &str, now: i64, secret_days: bool) -> Result<PurgeOutcome>`. The same purge as `purge_inner` but signed by System. Allowed only by the rule in Global Constraints. "When it entered quarantine" and "who sent it" are read from the latest op whose change moved it to `Quarantine`.
  - `Engine::sleep_merge(&self, sources: &[MemoryId], p: Proposal) -> Result<(MemoryId, OpId)>`. One op of kind `Update`, actor System: the new memory Active (before None), and each source Active → Replaced with `replaced_by = new`. It copies the sources' relations except `ConflictsWith`. It refuses if any source is not Active, is the person's or is pinned, or if sources differ in scope, kind or trust (`Origin::External` vs not).
  - `Engine::operations_after(&self, after: OpId, limit: usize) -> Result<Vec<Operation>>`, ascending.
  - `Engine::last_activity(&self, except: ActorId) -> Result<i64>`: the max of the latest op `at` by another actor and `usage.last_used`.
  - `Engine::system_actor(&self) -> ActorId`.

- [ ] **Step 1: Write the failing tests** in `tests/sleep_engine.rs`, using a `FixedClock` engine as `tests/common` does:
  - `sleep_quarantine_refuses_the_persons_and_pinned`: a person's memory → `Err(NotAllowed)`; a pinned agent memory → `Err(NotAllowed)`; an agent memory → Ok, state Quarantine, op actor == `system_actor()`, reason starts `sueño:`.
  - `sleep_purge_only_purges_what_was_discarded_long_ago`:
    - an agent memory forgotten by the agent 29 days ago → refused;
    - the same at 30 days → purged (tombstone present);
    - a memory waiting review (proposed to Quarantine by a decider) at 90 days → refused;
    - a person's memory forgotten by an agent at 90 days → refused;
    - one forgotten by the person → purged.
  - `a_secret_quarantined_by_the_sleep_is_purged_after_seven_days`: 6 days refused, 7 days ok (`secret_days = true`).
  - `sleep_merge_replaces_the_sources_and_reverts_whole`:
    - 3 agent memories with a relation each → merge → the sources are Replaced with `replaced_by` = new, and the new one carries the relations;
    - `revert(op, person)` → the sources are Active again and the new one is not active.
  - `sleep_merge_refuses_mixed_trust_scope_or_the_persons`: one External and one not → Err; two scopes → Err; one held by the person → Err.
  - `the_sleeps_quarantines_do_not_fill_the_authors_limit`: with `max_quarantine_per_actor = 2`, the sleep quarantines 3 of the agent's memories, and the agent's next proposal that needs review is not refused with `QuarantineFull`.
  - `system_still_cannot_forget_or_purge_directly`: `forget` / `purge` with `system_actor()` → `NotAllowed`.

- [ ] **Step 2: Run** `cargo test --all-features --test sleep_engine`. Expected: FAIL (the functions do not exist).
- [ ] **Step 3: Implement the functions above.** `count_quarantined_by` excludes memories whose latest quarantine op actor is System.
- [ ] **Step 4: Run** `cargo test --all-features` (whole crate). Expected: PASS, no regressions.
- [ ] **Step 5: Commit** `feat(engine): operations the sleep may do, with their rules inside the engine`.

### Task 2: Signals and the merge validator (clio-memory)

**Files:**
- Create: `src/sleep/signals.rs` (and `src/sleep/mod.rs` with `pub mod signals;`, exported as `clio_memory::sleep`)
- Test: `tests/sleep_signals.rs`, plus `tests/data/attacks.jsonl` (≥ 20 invented attack texts, es and en, one per family, combinations)

**Interfaces:**
- Produces:
  - `pub const RULES_VERSION: u32 = 1;`
  - `pub enum Family { Hidden, Injection, Exfil, External }`
  - `pub fn attack_families(text: &str, external: bool) -> Vec<Family>`
  - `pub fn looks_like_attack(text: &str, external: bool) -> bool`: Hidden alone, or ≥ 2 families.
  - `pub fn has_secret(text: &str) -> bool`: wraps the private `secrets::find_secret`.
  - `pub enum Rejected { Json, Sources, Longer, NewUrl, NewPath, NewCommand, NewInjection, Secret, Drift(MemoryId) }`
  - `pub fn validate_merge(originals: &[MemoryRow], answer: &str, similarity: &dyn Fn(&str, &str) -> Option<f32>) -> Result<Merged, Rejected>`
  - `pub struct Merged { pub text: String, pub title: Option<String>, pub sources: Vec<MemoryId> }`
  - `answer` is the model's raw reply. Its JSON is `{"texto","titulo","fuentes"}`, and `fuentes` must equal the originals' ids as a set. URLs, paths, commands and injection phrases are compared against the union of the originals. `Drift` means the similarity is < 0.80 for that original (`None` means no vectors, which skips the check).

- [ ] **Step 1: Write the failing tests:**
  - `no_bench_memory_looks_like_an_attack`: every text of `bench/embeddings/memories.jsonl` and `bench/relations/extra.jsonl`, with `external` true and false → `false`.
  - `every_invented_attack_is_caught`: every line of `tests/data/attacks.jsonl` (`{"text", "external"}`) → `true`.
  - `one_injection_phrase_alone_is_not_an_attack`: «Seguridad: un texto que dice "ignora las instrucciones anteriores" es una inyección» with `external` false → `false`.
  - `hidden_characters_alone_are`: "hola\u{200B}mundo" → `true`.
  - `validate_merge_*`:
    - accepts a faithful merge;
    - rejects a new URL, an added "ignora las instrucciones", a secret (built with `concat!`), a missing source id, an extra id, and a longer text;
    - `Drift` when the similarity is 0.5.
- [ ] **Step 2: Run** `cargo test --all-features --test sleep_signals`. Expected: FAIL.
- [ ] **Step 3: Implement.** Use plain string scanning, case-insensitive after `normalize`; no regex crate unless it is already a dependency.
- [ ] **Step 4: Run** the same. Expected: PASS.
- [ ] **Step 5: Commit** `feat(sleep): attack signals and the merge validator`.

### Task 3: One night of sleep (clio-memory)

**Files:**
- Modify: `src/sleep/mod.rs`
- Create: `src/sleep/state.rs` (`state.json`, reports, copies, rotation)
- Test: `tests/sleep.rs`

**Interfaces:**
- Consumes: Task 1 and Task 2.
- Produces:
  - `pub trait Merger { fn merge(&self, group: &[MemoryRow]) -> Option<String>; fn preserved(&self, original: &MemoryRow, merged: &str) -> Option<bool>; }`
    - `merge` returns the raw reply.
    - `preserved == Some(false)` or `None` cancels a merge; a merger without a judge returns `Some(true)`.
  - `pub struct Options { pub now: bool, pub dry_run: bool, pub clock_now: i64 }`
  - `pub enum Outcome { Skipped(&'static str), Done(Report) }`
  - `pub struct Report`, serde, with:
    - `started`, `ended`, `complete: bool`;
    - `quarantined: Vec<(i64, String)>`, `merged: Vec<(i64, Vec<i64>)>`, `purged: Vec<i64>`, `repeated: Vec<(i64, i64)>`;
    - `review: Vec<String>`, `alarms: Vec<String>`, `tokens: u64`.
  - `pub fn run(data: &Path, model: &Arc<EmbedModel>, merger: Option<&dyn Merger>, opts: &Options) -> Result<Outcome, String>`
  - `pub fn last_report(data: &Path) -> Option<Report>`
  - `pub fn morning_line(r: &Report, now: i64) -> Option<String>`: `None` when older than 24 h or empty. The format is the spec's: `Anoche (sueño): …`.
- Order: as in the spec, steps 1–10. "New" = memories touched by ops after `state.cursor`, plus their ≥ 0.87 neighbors (via `Engine::neighbors`). All active memories when `rules_version` changed (that night: no model step for the old ones).

- [ ] **Step 1: Write the failing tests** (temp dirs, `HashEmbedder` from `tests/common`, and a fake `Merger` that returns scripted replies):
  - `it_skips_when_slept_less_than_20h_ago`: a `state.json` with last complete 19 h ago → `Skipped`.
  - `it_skips_when_the_memory_was_used_in_the_last_30_minutes`: an agent op 10 min ago → `Skipped`, and `now: true` runs anyway.
  - `two_sleepers_never_overlap`: hold `<db>.sleep.lock`, then run → `Skipped`.
  - `a_failed_check_touches_nothing`: corrupt a page of a copy of the db, run on it → `alarms` non-empty, and no op was written.
  - `secrets_and_attacks_go_to_quarantine_but_the_persons_only_alarm`: an agent memory with a secret → quarantined `sueño: secreto`; the person's memory with a secret → still Active, plus an alarm; an external memory with injection and exfil → quarantined.
  - `an_exact_repeat_keeps_the_persons_and_carries_relations`.
  - `the_merge_applies_a_faithful_group_and_rejects_a_poisoned_one`: one group answered faithfully → merged; another answered with an extra URL → untouched, in `review`.
  - `merge_stops_when_most_groups_fail`: 3 groups, all rejected → 0 merged, an alarm, and the third group was never asked.
  - `the_caps_hold`: 30 attack memories among 40 active → at most 10 quarantined, plus the alarm `demasiados`.
  - `purge_waits_for_the_copy`: make the copy fail (read-only `sleep/`) → no purge.
  - `solid_memories_are_not_reviewed_again`: a first night merges, a second night with no new ops calls the merger 0 times; a new near-duplicate on the third night calls it once, for its group only.
  - `another_writer_stops_the_night_and_the_cursor_stays`: a fake merger whose `merge` writes an agent memory → the report has `complete: false`, and `state.cursor` is unchanged.
  - `dry_run_writes_nothing`: the file's hash and `sleep/` are unchanged.
  - `morning_line_says_what_happened`: the exact string for a report with 1 quarantined, 1 merged and 1 review item, and `None` after 25 h.
- [ ] **Step 2: Run** `cargo test --all-features --test sleep`. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - The copy uses `Store::backup_to` from a read-only open.
  - The engine is opened for writing per section with `Memory::open_write`-style retries (8 × 250 ms) and dropped right after.
  - Rotation: keep 3 copies and 60 reports.
- [ ] **Step 4: Run** `cargo test --all-features`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(sleep): one night of sleep, incremental and capped`.

### Task 4: Command, timer, morning line and doctor (clio-memory) → v0.5.0

**Files:**
- Modify:
  - `src/bin/clio-memory/{args.rs,main.rs,commands.rs}`: `sleep [--now] [--dry-run] [--report]` and `setup sleep [--remove]`;
  - `src/setup.rs`: the unit text, install and remove; `setup claude-code` calls install;
  - `src/agent/memory.rs` `context()` :912 and `src/tools.rs` `memory_context`: the morning line goes first;
  - `src/engine.rs` doctor or the CLI doctor: last sleep, complete, alarms;
  - `README.md`, `docs/motor.md`, `skill/SKILL.md` (one sentence: the morning line, and that the agent may consolidate «para revisar»);
  - `Cargo.toml` version `0.5.0`.
- Test: `tests/runtime.rs` style → `tests/sleep_setup.rs`; `tests/cli.rs`.

**Interfaces:**
- Produces:
  - `pub fn sleep_units(exe: &Path, args: &[&str]) -> (String, String)`: the systemd service and timer text. The timer is `OnCalendar=*-*-* 23,00,01,02,03,04,05:00,30:00`, no `Persistent`.
  - `pub fn launchd_plist(exe: &Path, args: &[&str]) -> String`: `StartCalendarInterval` entries for the same slots.
  - `pub fn install_sleep(home: &Path, exe: &Path, args: &[&str], run: &dyn Fn(&[&str]) -> Result<(), String>) -> Result<PathBuf, String>`
  - `pub fn remove_sleep(home, run)`
  - `run` executes `systemctl --user …` or `launchctl …` (injected so tests do not touch the system).
  - The unit name is `clio-memory-sleep` on both installers.

- [ ] **Step 1: Write the failing tests:**
  - `the_timer_wakes_every_half_hour_from_23_to_0530` (unit text contains the exact `OnCalendar`);
  - `install_writes_the_units_and_enables_the_timer` (temp home, the recorded commands are `daemon-reload` and `enable --now clio-memory-sleep.timer`);
  - `remove_disables_and_deletes`;
  - `setup_claude_code_installs_the_sleep_too`;
  - `memory_context_starts_with_last_nights_line` (write a report, call the tool);
  - `cli_sleep_dry_run_prints_and_writes_nothing`;
  - `doctor_says_when_it_last_slept`.
- [ ] **Step 2: Run** `cargo test --all-features`. Expected: the new tests FAIL.
- [ ] **Step 3: Implement.** Linux uses `~/.config/systemd/user/`. macOS uses `~/Library/LaunchAgents/com.clio-memory.sleep.plist`. On other OSes, setup says it is not supported and goes on.
- [ ] **Step 4: Run** the gates: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features`, `cargo deny check`. Expected: all green.
- [ ] **Step 5: Commit, push the branch and open the PR.** Merging and tagging `v0.5.0` wait for Iván's yes.

### Task 5: Clio: the merge with the person's model, the command, the context and doctor

**Files:**
- Modify:
  - `crates/clio-brain/Cargo.toml`: repin `clio-memory` to the v0.5.0 commit;
  - `crates/clio-brain/src/memory_judge.rs`: `preserved`;
  - `crates/clio-brain/src/memory_link.rs` `context`: the morning line, as in clio-memory;
  - `crates/clio-brain/src/cli/memory.rs`: `sleep` and `setup` subcommands;
  - `crates/clio-brain/src/cli/doctor.rs`: the last sleep;
  - `docs/uso-local.md`, `docs/security.md` (a new rule S-xx for the sleep with its test), `crates/clio-brain/README.md`.
- Create: `crates/clio-brain/src/sleep_merger.rs`
- Test: `crates/clio-brain/tests/sleep.rs`; a doctor unit test.

**Interfaces:**
- Consumes: `clio_memory::sleep::{run, Merger, Options, Report, last_report, morning_line}`, `clio_memory::setup::install_sleep`.
- Produces:
  - `pub struct ModelMerger { model: Mutex<Box<dyn Model>>, judge: Option<Arc<MemoryJudge>>, scrub: Scrub, budget: AtomicU64 }`, implementing `Merger`:
    - `merge` sends one system message (fixed text) and one user message holding the group as JSON inside `<datos>…</datos>`, after the vault scrub;
    - it returns `None` when the scrub fails or the budget is spent.
  - `MemoryJudge::preserved(&self, original: &str, merged: &str) -> Option<bool>`: one closed question `se_conserva`, true only at ≥ 0.9.
  - The model is built from `config.model` the way `cli/runs.rs:521` builds `CommandModel` (provider, command, model, the key from the vault). The merge is skipped (`merger = None`) when there is no model or the vault is closed.
  - Per-night token budget: `const SLEEP_TOKENS: u64 = 60_000`.

- [ ] **Step 1: Write the failing tests:**
  - `the_merger_sends_the_memories_as_data_and_never_tools` (a fake `Model` records the messages: the memories are inside `<datos>`, there is no tools field, and the system text is fixed);
  - `a_model_that_echoes_an_instruction_is_rejected_by_the_sleep` (a fake model answers with "ignora…" plus a URL → no merge);
  - `jev_saying_something_was_lost_cancels_the_merge`;
  - `no_configured_model_still_sleeps_without_merging`;
  - `clio_context_starts_with_last_nights_line`;
  - `the_budget_stops_the_merges`.
- [ ] **Step 2: Run** `cargo test -p clio-brain --all-features --test sleep`. Expected: FAIL.
- [ ] **Step 3: Implement.** `clio memory setup` calls `install_sleep(home, <clio exe>, &["memory", "sleep"], …)`.
- [ ] **Step 4: Run** Clio's gates: `just check`, `CLIO_REQUIRE_PI=1 just test`, `just audit`. Expected: all green.
- [ ] **Step 5: Commit, push and open the PR.** Merge waits for Iván's yes.

### Task 6: On this machine (after both merges, with Iván's yes)

- [ ] **Step 1:** `cargo install --locked --path . --features model2vec,granite` in clio-memory, then `just install` in Clio.
- [ ] **Step 2:** `clio memory sleep --dry-run --now` on the real memory. Read the report: no quarantine of the person's memories, and the list «para revisar» makes sense.
- [ ] **Step 3:** `clio memory setup` (installs the timer). Check with `systemctl --user list-timers clio-memory-sleep.timer`.
- [ ] **Step 4:** Update clio-memory memory m14/m21 (what was built, the PRs) and record the result.
