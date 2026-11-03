# 0008: Triage, contract and verification wrap the Pi run, in the brain

## Context

Pi is the default harness (ADR 0007), but triage, contract, verification, retries with fresh context and the memory write at close exist only in the builtin `AgentLoop`. On the Pi path only the budget and the anti-loop apply (S-47, `pi/budget.rs`). The design v2 asks for the rest at levels 2 and 3. On 2026-11-02 the person decided that it must live on the path that follows the security rules and the service protocols.

Three places were possible: **A** around `run_prompt` in `clio-brain` (the Clio service); **B** inside the `clio-guard` extension, in Pi's process; **C** leave Pi as "level 1 forever" and defer.

## Decision

**A.** Triage, contract and verification wrap the Pi run in the brain; the guard stays what it is, the enforcement point of each tool call.

- **S-01 and S-11:** control must hold even if Pi or the model misbehave, so it lives in code outside the model and outside Pi's loop. An extension runs inside Pi's process with Pi's privileges and reloads hot (the "extensions can do everything" gap of `docs/security.md`): the wrong place for a limit that must not move.
- **S-07:** the level and the contract come only from the person's message, so they are computed in the brain before the prompt reaches Pi, and nothing Pi reads can lower the level or change the goal.
- **Service protocol (ADR 0007, point 3):** the Clio service owns the Pi processes, the sessions and the task record; the terminal is a window onto it and a remote session is the same service on another host. The control layer belongs to the service, not to a window and not to Pi.
- **«Another process, not the one that wrote» (design v2):** verification becomes true by construction: Pi writes, the brain verifies.
- **Memory (S-25 to S-27):** the write at close goes through `memory_link`, the only writer.

## Shape

1. **Before the prompt:** triage (`clio-triage`) on the person's message gives the level, which only goes up. From level 2: the contract (`clio-contract`), saved outside the working copy, in the run folder. The budget is already per prompt.
2. **During:** `RunGuard` (S-47) and, per call, the guard and the gateway, as now.
3. **After Pi settles:** verification with external signals (tests through the toolhost jail, the contract's invariants). If it fails: retry with a fresh Pi context plus an `EvidencePackage` (never the old transcript), counted by the retries cap; the diagnosis comes with phase 3.
4. **Close:** report with what was verified, what is pending and the next step; memory write through `memory_link`.
5. **Approvals** stay in the terminal until the PWA (phase 4); the absent mode does not change.

## Open

- The plan check of the gateway (step 7) needs a plan the Pi run does not have; level 2 and 3 runs would pass the planner's plan to the toolhost.
- Where the verification signals come from when the task is not code.
- The contract record is a JSON file in the run folder (the default of task 137).

## Consequences

The builtin harness stays for the bench until the Pi path passes the same tasks and the same attacks with these pieces on. Order of work: triage and level on the Pi path (with an S-07 test that an injected "this is level 0" changes nothing), then contract, then verification, then retries with evidence, then the memory write. Each piece brings its security tests; those that touch the gateway, the vault or the node are reviewed by the person (S-53).
