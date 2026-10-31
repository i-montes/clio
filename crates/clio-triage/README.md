# clio-triage

Rule-based triage. One pass over the person's message gives a level (0 direct, 1 simple, 2 full,
3 long), an effort, an ambiguity flag, a possible-irreversible flag, recommended tools and a
model class. No network: the decider is a trait and the rules decider is the default, so triage
works without an LLM.

- The input type (`TriageInput`) only takes a `PersonMessage`, so external content cannot reach it (S-07).
- Order: forced by the person (`/rápido`, `/plan`), hard rules (possible irreversible => level 2 or more), verb rules, then level 1 when in doubt.
- A `Session` only goes up; each rise is logged, and reaching level 3 asks for contract approval.
- `Telemetry` keeps counters: requests per level, rises, failures from a too-low level, cost per level.

Spec: [`docs/specs/clio-triage-spec-v0.1.md`](../../docs/specs/clio-triage-spec-v0.1.md).
