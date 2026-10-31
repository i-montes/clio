# clio-brain

The brain that runs on the VPS. This crate is the library; the binary comes later.

- `control`: hard budgets per task and per day (S-47), anti-loop counter, retry budget,
  checkpoints, plan check, rule feasibility, retry evidence package, model output filter.
- `verify`: criteria close only with external signals; model opinion never does.
- `models`: pinned model catalog, routing by level and privacy, sanitized payloads (S-44),
  rate limits and cost ledger in integer micro-USD.

Spec: [`docs/specs/`](../../docs/specs/). Nothing is public yet.
