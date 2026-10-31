# clio-contract

Task contract for levels 2 and 3: fields, a state machine that rejects invalid transitions,
and a durable JSON record (atomic write) another session can resume from.

- Only the person changes the objective, scope, role and policy (`Actor`; S-07). The agent proposes amendments, the person accepts.
- The level comes from triage and only goes up.
- Every success criterion declares how it is checked; otherwise the draft is not approved.
- Level 3 approval must carry the contract digest (S-14); the record is saved before the first write (`start`).
- A task or criterion is done only with attached evidence; closing always yields a fixed report (verified, unfinished, next step).
- Budget is enforced in code (S-47): exhausting it closes the contract with a report.
- Each state change emits an `Event` for the trace; `Telemetry` counts closes, amendments, first-try criteria, reopened tasks and time per phase.

Spec: [`docs/specs/clio-contract-spec-v0.1.md`](../../docs/specs/clio-contract-spec-v0.1.md).
