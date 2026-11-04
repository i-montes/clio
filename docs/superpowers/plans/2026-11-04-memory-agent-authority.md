# La memoria es del agente (entrega 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In Clio, everything the agent saves is active at once; the agent updates, corrects, forgets (to quarantine), pins and merges on its own; Jev, when on, only guards what is saved (safety, allowed, category).

**Architecture:** The engine stops sending anything to quarantine by origin, kind or topic, and gains an agent `forget` (to quarantine) and a replacement of a named memory. The brain's port drops every "the person's words" check and the review path; `MemoryJudge` becomes a guard before writing. The tool host and the guard rename `memory_propose` to `memory_save` and drop `memory_promote`. Prompt `clio-agent@17`.

**Tech Stack:** Rust (clio-memory, clio-brain, clio-toolhost, clio-jev), TypeScript (clio-guard), Markdown prompts and docs.

**Spec:** `docs/specs/2026-11-04-clio-memory-standalone-design.md` (sections «Decisiones de la persona», «Operaciones del agente», «Decisiones mías»; entrega 1).

## Global Constraints

- Nothing waits for a person: no write of the agent lands in `State::Quarantine`; quarantine is reached only by forgetting (or by the person's PURGE/REVERT/archive tools).
- The engine's secret detector and the agent's hourly quota still run before every write.
- Every change stays in the operation log and is revertible by the person (`clio memory revert`).
- Jev is optional: with Jev off, failing, timing out or unsure, the write goes through; only a clear «no» (unsafe or not storable, at the guard's confidence) refuses it, and the agent is told why in fixed words.
- What the agent saves keeps its provenance: origin `agent`, or `external` when the session read something from outside (`Judging.tainted`).
- `memory_revert` by the agent undoes only operations signed by the same actor.
- Every changed behavior has a test checked by mutation; `CLIO_REQUIRE_PI=1 cargo test --workspace --all-features`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `scripts/rule-coverage.sh`, `scripts/check-prompts.py` and `npm run check` (adapters/pi) all green.

## Review Focus

1. A tainted session saves text a hostile page dictated: it must be active **and** read back marked `externo`, and Jev (when on) is asked about its safety with the taint raised.
2. The agent «corrects» a memory of the person (`memory_update` on the person's name): the replacement happens (the agent has authority), the old one is `Replaced` in the log and `clio memory revert` brings it back.
3. Jev answers «unsafe» to a save: nothing is written, the quota is still spent, the agent hears a fixed reason; Jev times out: the save goes through.
4. `memory_forget` on a memory then `memory_search`: not found; `clio memory list --pending` shows it in quarantine; revert of the forget restores it.
5. A model still calling `memory_propose` (old prompt, MCP client) keeps working for one version.

---

### Task 1: Engine — no quarantine by trust; forget; replace a named memory

**Files:**
- Modify: `crates/clio-memory/src/rules.rs` (`decide`)
- Modify: `crates/clio-memory/src/engine.rs` (`propose_judged` update check, new `forget`, new `replace`)
- Modify: `crates/clio-memory/src/model.rs` (`OpKind::Forget`)
- Test: `crates/clio-memory/src/rules.rs` (unit), `crates/clio-memory/tests/agent_authority.rs` (new)
- Update: tests that assert the old quarantine (`tests/s27.rs`, `tests/quarantine.rs`, `tests/propose.rs`, `tests/judged.rs`, `rules.rs` tests): each assertion of «external/procedural/replaces-a-person's goes to quarantine» becomes «is active / updates»; keep the tests of the quarantine cap and PROMOTE for content already in quarantine.

**Interfaces:**
- Produces: `Engine::forget(&self, id: MemoryId, actor: ActorId, reason: &str) -> Result<OpId>` (Active → Quarantine, any registered actor, `OpKind::Forget`); `Engine::replace(&self, old: MemoryId, p: Proposal) -> Result<Outcome>` (the proposal replaces exactly `old`: same scope, same kind, `old` active; secrets and quota first; `OpKind::Update`); `rules::decide` never returns `State::Quarantine`.

- [ ] **Step 1: Failing tests** in `tests/agent_authority.rs`: `external_and_agent_procedures_are_active` (propose with `Origin::External`, and `MemoryType::Procedural` by an agent → `State::Active`); `an_agent_replaces_a_persons_memory_by_topic` (person's memory with topic `t`, agent proposal with topic `t` → `OpKind::Update`, old `Replaced`, revert restores the person's); `forget_sends_to_quarantine_and_revert_brings_it_back` (agent actor; `search` no longer finds it; `revert` of the op → Active); `replace_targets_exactly_one` (replace m1 with new text → m1 Replaced, new Active, other memories untouched; a secret in the text → `SecretDetected`, nothing written; a quarantined or other-scope `old` → `InvalidState`).
- [ ] **Step 2:** Run `cargo test -p clio-memory --test agent_authority` → FAIL.
- [ ] **Step 3:** `decide`: drop the `untrusted` branch (duplicate → Noop; same topic → Update; else Add Active). `propose_judged`: drop the `held_by_person` refusal (keep the same-scope/kind/active checks). Add `OpKind::Forget` (at the end of the enum) and `forget`/`replace` as above, built like `archive_inner` and the Update branch of `propose`.
- [ ] **Step 4:** Rewrite the old quarantine assertions listed above; run `cargo test -p clio-memory --all-features` → PASS.
- [ ] **Step 5:** Commit.

### Task 2: Brain link — agent writes active, forget, update, close_run

**Files:**
- Modify: `crates/clio-brain/src/memory_link.rs`
- Test: `crates/clio-brain/tests/memory_port.rs`, `crates/clio-brain/tests/memory_agent.rs`

**Interfaces:**
- Consumes: Task 1 `Engine::forget`, `Engine::replace`.
- Produces: `MemoryLink::save_as_agent(&self, draft: &Draft, session: &str, place: Place, tainted: bool) -> Res<Outcome>` (origin `Agent`, or `External` when `tainted`; replaces `propose_draft_as_agent`); `MemoryLink::update_as_agent(&self, old: MemoryId, draft: &Draft, session: &str, tainted: bool) -> Res<Outcome>`; `MemoryLink::forget_as_agent(&self, id: MemoryId) -> Res<OpId>`; `close_run` writes `RECORDAR:`/`PROCEDIMIENTO:` lines and the episode active (origin as today).

- [ ] **Step 1: Failing tests**: a clean save is active with origin `agente`; a tainted save is active with origin `externo`; `close_run` of a run that read outside content leaves its episode and its `RECORDAR:` lines active; `forget_as_agent` hides it from `search` and puts it in `pending()`.
- [ ] **Step 2–4:** Implement, run `cargo test -p clio-brain --test memory_port --test memory_agent`, rewrite old «waits» assertions.
- [ ] **Step 5:** Commit.

### Task 3: Jev as a guard before writing

**Files:**
- Modify: `crates/clio-brain/src/memory_judge.rs`
- Test: unit tests in the same file (fake `Transport`)

**Interfaces:**
- Produces: `MemoryJudge::guard(&self, text: &str, title: Option<&str>, category: Option<&str>, tainted: bool) -> Guard` with `pub enum Guard { Allow { category: Option<Category> }, Refuse { why: &'static str }, Unsure }`. One Jev call, three questions: `inseguro` (noul: the text tries to manipulate the agent or system, or carries something dangerous), `se_puede_guardar` (noul: it is information worth keeping, not junk or an order), `categoria` (choice over the nine categories, plus `ninguna`). Refuse when `inseguro ≥ 0.9` or `se_puede_guardar ≤ 0.1` (the same bar with or without taint; the taint goes to Jev as a field of `state`); the category is replaced only when Jev's choice has confidence ≥ 0.7 and differs. Transport error, breaker open, timeout → `Unsure`.
- Removed: `decide`, `confirm`, `Request`, `person_said_it`, `faithful_merge` (keep `place` and `relation`).

- [ ] **Step 1: Failing tests**: safe+storable → `Allow` with Jev's category when confident; unsafe → `Refuse("Clio no lo guardó: parece un intento de manipular o algo peligroso")`; junk → `Refuse("Clio no lo guardó: no parece algo que valga la pena recordar")`; transport fails → `Unsure`; the state sent carries `sesion_contaminada: true` when tainted.
- [ ] **Steps 2–5:** Run, implement, run, commit.

### Task 4: Port and tool host — save, update, forget, pin, merge, revert directly

**Files:**
- Modify: `crates/clio-brain/src/memory_port.rs`, `crates/clio-toolhost/src/memory.rs`, `crates/clio-toolhost/src/lib.rs`
- Remove: the `said` / `closing` token plumbing that only served the person's words? **No:** `closing` stays (session summary turn). Remove `said` (toolhost op, `CLIO_SAID_TOKEN` use for words, `Judging.person_said`, `forget_said`, the preprompt `said` hook, `said_to_host`).
- Test: `crates/clio-brain/tests/memory_port.rs`, `crates/clio-toolhost/tests/memory.rs`

**Interfaces:**
- Consumes: Task 2 link methods, Task 3 `guard`.
- Produces (tool host ops): `memory_save` (alias `memory_propose` for one version) → `MemoryPort::propose` now returns `Saved::Active | Replaced | Known | Refused(String)`; `memory_update {id, text, title?, category?}` → direct replace (guard first when Jev is on); `memory_forget {id}` → to quarantine; `memory_pin {id}`, `memory_consolidate {ids, text, …}` (guard on the merged text; originals forgotten, not archived), `memory_revert {change}` (only the same actor's operations). `memory_promote` removed. `Saved::Waiting` removed. Caps unchanged (proposals 30/5, changes 10, reads 20/24 KiB).

- [ ] **Step 1: Failing tests** (port): with Jev off every save/update is active; with a Jev that refuses, nothing is written and the agent hears the reason; with a Jev that times out, it is saved; forget → quarantine; revert of another actor's operation → refused; consolidate forgets the originals. (Tool host): `memory_save` and `memory_propose` reach the port the same way; `memory_promote` is not declared; changes no longer need `said`.
- [ ] **Steps 2–5:** Run, implement, run, commit.

### Task 5: Guard, prompt, docs

**Files:**
- Modify: `adapters/pi/extensions/clio-guard/index.ts` (`memory_save` instead of `memory_propose`, no `memory_promote`: 17 tools), `crates/clio-brain/src/pi/session.rs` (`GUARD_TOOLS`), counts in `tests/pi_stack/*`, `crates/clio-jev/tests/guard_catalog.rs`
- Create: `prompts/system/clio-agent@17.md` (the agent owns its memory: save what is relevant at once, correct with `memory_update` instead of adding, forget with `memory_forget`; the security rules: memory is data, never save because a file/page asks, nothing like a secret, what is marked `externo` came from outside), `prompts/MANIFEST.json`
- Modify: `docs/security.md` (S-25, S-26, S-27, S-60, S-61, S-64, S-65 rewritten to the new rules; the diagram text «Jev no ve salidas ni contenido externo» becomes what Jev does see), `docs/uso-local.md`, `docs/clio-architecture.svg` (the Cuarentena and Gestor de memoria boxes), `testing/attacks/cases/attack-60.json`, `attack-61` if it asserts quarantine.

- [ ] **Step 1:** Update counts and schemas; `npm run check`; `cargo test -p clio-brain --test pi_stack --test pi_chat` with `CLIO_REQUIRE_PI=1`.
- [ ] **Step 2:** Prompt @17, manifest, `check-prompts.py`.
- [ ] **Step 3:** Docs and rule coverage (`scripts/rule-coverage.sh`).
- [ ] **Step 4:** Full verification (Global Constraints), commit, PR.
