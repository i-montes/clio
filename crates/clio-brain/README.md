# clio-brain

The brain that runs on the VPS. This crate is the library; the binary comes later.

- `control`: hard budgets per task and per day (S-47), anti-loop counter, retry budget,
  checkpoints, plan check, rule feasibility, retry evidence package, model output filter.
- `verify`: criteria close only with external signals; model opinion never does.
- `agent`: the minimum agent, end to end. `AgentLoop` runs triage over the person's message, a
  contract from level 2 up (saved as JSON in a run folder outside the workspace), then a loop of
  model turns. A turn is ONE JSON object in plain text, `{"tool":..,"args":{..}}` or
  `{"final":".."}` (`parse_turn` finds the first one inside fences or prose). Every tool call goes
  through `Gateway::decide_or_deny`, runs only on an allow in the `clio-node` executor inside the
  sandbox, and comes back scrubbed, filtered and wrapped as untrusted data (the session is tainted
  by anything read from a file). Every turn passes `control` (steps, time, cost and anti-loop cut
  hard at every level). It ends with a closing report: done, pending, next step. `Model` is a
  trait (closures work, for tests); `CommandModel` calls any program that speaks the
  `clio-bench` JSON (`Request` on stdin, `Response` on stdout), retries failures and empty answers.
  The agent has no tool that runs code, so it cannot run tests: nothing outside the model verifies
  its result, and the contract waits (`Blocked`) instead of closing as done.
- `vault_link`: the gateway side of the vault process. `VaultPolicy` is the gateway's
  `SecretPolicy`, answered by the vault's `check`. `SecretExecutor` is its `Executor`: it
  resolves `{{secreto:name}}` last, for the action's tool and domains only, runs the tool with the
  real values and sends every output through the vault's filter (dropped if the vault is down).
  `AgentLoop` still holds an in-process `Scrubber`; wiring it to the remote filter is not done.

- `tests/attacks.rs`: the attack battery against the whole agent (see
  [`docs/mediciones/fase-1-ataques.md`](../../docs/mediciones/fase-1-ataques.md)). S-16: the
  agent's gateway protects `PROTECTED_PATHS` (`extensions`, `skills`, `.pi`, `.claude`, `.git`).
- `models`: pinned model catalog, routing by level and privacy, sanitized payloads (S-44),
  rate limits and cost ledger in integer micro-USD.

Spec: [`docs/specs/`](../../docs/specs/). Nothing is public yet.
