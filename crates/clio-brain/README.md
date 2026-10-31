# clio-brain

The brain. It runs on the user's own machine by default (a VPS is optional, ADR 0006). This crate is the library plus the `clio` binary (`src/main.rs`, `src/cli/`): `init`, `run`, `diff`, `apply`, `undo`, `runs`, `show`, `vault`, `doctor`. `run` copies the workspace to `~/.local/share/clio/runs/<id>/sandbox/` (leaving out `.git`, heavy folders and secret-looking files), runs the agent there, and never touches the original; `apply` puts changes back only after a person confirms, refusing changed originals, protected paths and symlinks, with a backup for `undo`. Usage guide (Spanish): [`docs/uso-local.md`](../../docs/uso-local.md). Tests: `tests/cli.rs` (scripted model, no network; it also drives the real binary in a pseudo-terminal through `tests/pty_driver.py`, stdlib Python 3).

`clio` with no arguments in a terminal opens the TUI (`src/cli/tui/`, `ratatui` + `crossterm`); without a terminal it prints the help and exits 0. The TUI keeps logic and drawing apart: `app` holds the state and the pure `update(App, Input) -> (App, Vec<Cmd>)` (effects come back as `Cmd`s that `mod.rs` runs on worker threads, results return as `Bg` inputs); `home`, `new_run`, `live`, `detail` and `doctor` are the screens (state, keys, `draw`); `theme` uses only basic ANSI colors; `sanitize` cleans every outside text (escape sequences, control and bidi characters) before it is drawn or printed. `apply`/`undo` in the TUI call `cli::apply` (`plan`, `commit`, `undo_run`): the safety checks live in one place. `cli/ui.rs` decides color and symbols for the plain commands (`NO_COLOR`, `TERM=dumb`, Unicode or ASCII); `runs`, `show` and `doctor` take `--json`. The TUI tests need no terminal (keys against `update`, drawing against `ratatui`'s `TestBackend`, hostile strings); the pseudo-terminal tests cover the alternate screen, restoring the terminal on `q`, `Ctrl-C` and on a panic, and a run with apply and undo.

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
  `AgentLoop::observer` (trait `Observer`, one method `event(&Event)`, empty by default) reports
  `Started`, `Step`, `ProtocolError`, `Tool` (with the gateway's outcome and rule) and `Final`;
  `AgentLoop::cancel` (an `AtomicBool`) ends the loop with `Ending::Cancelled` before the next model call.
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
