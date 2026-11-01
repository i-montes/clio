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
  Without the jail the agent has no tool that runs code: nothing outside the model verifies its
  result and the contract waits (`Blocked`). With `AgentConfig::run_tests` (set only when the jail
  is PROVEN: `cli/jail.rs` runs `Jail::probe` and the self-test) it also has `run_tests`, governed
  by the gateway like any tool (declared args `runner` and `target`, effect `LocalReversible`).
  Its output is external: scrubbed, wrapped as `UNTRUSTED-DATA`, it taints the session; the exit
  code is outside the markers. If the last `run_tests` came after the last write and passed, the
  contract closes `ClosedOk` (through `verify::ExternalSignal::TestResult`); no tests, a failure
  or a later write leave it `Blocked`. The CLI uses prompt `clio-agent@3` when the jail is proven
  and `clio-agent@2` otherwise; `CLIO_JAIL=off` switches it off. `clio doctor` prints one PASS or
  FAIL per escape attempt (`--json`: `jail`). Tests: `tests/run_tests.rs` (attack 34 is in
  `tests/attacks.rs`); they print `SKIPPED` without user namespaces (`CLIO_REQUIRE_JAIL=1` fails).
- `pi`: the Rust side of the Pi harness (ADR 0007), a library the TUI does not use yet. `install` (pinned version and lockfile embedded from `adapters/pi/`, Node 22.19+ check, `npm ci --ignore-scripts`), `launch` (`start(&Launch)`: always `--no-builtin-tools --no-extensions` [+ `-e` guard] `--no-mcp --no-skills --no-prompt-templates --no-themes --no-context-files --no-approve --offline`, empty environment plus a short list, `models.json` 0600 without the key, a 0700 tree, `umask 077`, startup checks that kill Pi if the model, the session, the commands or the declared tools are not what was asked), `rpc` (`RpcClient`: LF-only JSONL, ids, tolerant `Event`, deadlines on every wait, `Cancel`, `Drop` kills the process group, sanitized capped stderr, `UsageMeter` for the status bar), `sessions` (`list_sessions`: header and the least of each `.jsonl`), `text` (sanitizing). `clio pi doctor|install|sessions`. Tests: `tests/pi_rpc.rs` (a fake Pi in Node), `tests/pi_real.rs` (the real Pi against `testing/fake-provider/`), `tests/pi_sessions.rs`.
- `vault_link`: the gateway side of the vault process. `VaultPolicy` is the gateway's
  `SecretPolicy`, answered by the vault's `check`. `SecretExecutor` is its `Executor`: it
  resolves `{{secreto:name}}` last, for the action's tool and domains only, runs the tool with the
  real values and sends every output through the vault's filter (dropped if the vault is down).
  `AgentLoop` still holds an in-process `Scrubber`; wiring it to the remote filter is not done.

- `tests/attacks.rs`: the attack battery against the whole agent (see
  [`docs/mediciones/fase-1-ataques.md`](../../docs/mediciones/fase-1-ataques.md)). S-16: the
  agent's gateway protects `PROTECTED_PATHS` (`extensions`, `skills`, `.pi`, `.claude`, `.git`).
- `memory_link`: the agent's side of `clio-memory` and the ONLY writer on behalf of a run. Opens
  `<data>/memory.db` (0600, scope `clio`, never elsewhere) with four actors, each with its own
  quota: `persona` (the only one that writes `user` origin, promotes and archives), `agente`,
  `sesion` and `importador`. `recall_for` builds the fixed blocks plus what is relevant, each under
  its own token budget, and abstains when nothing is; the agent shows it to the model as data in a
  new `memoria` context layer (`AgentLoop::memory`, `ContextReport::memory`). `close_run` writes
  ONE batch after the loop: the run's own episodic record (active only if the run never touched
  outside content, quarantine otherwise) and up to five `RECORDAR:` / `PROCEDIMIENTO:` lines of
  the final message, always quarantined as external (procedures wait for a person on top of that).
  A refusal (secret, quota, size) is a line in `CloseReport`, never an error. The embedding model
  (`EmbedModel`, optional `model2vec` feature, on by default) loads once per process and only on
  demand; without it everything works by text. CLI `clio memory` (`src/cli/memory.rs`: `search`,
  `list`, `show`, `remember`, `promote`, `archive`, `feedback`, `export`, `stats`; no `purge`
  until PURGE exists) and the Memory screen (`m`, `src/cli/tui/memory.rs`). Tests:
  `tests/memory_agent.rs` (cycle, quarantine, procedures, quotas, abstention, secrets),
  `tests/memory_cli.rs` (every subcommand, two real runs with a scripted adapter, pty), attacks
  10, 11 and 35 to 37 in `tests/attacks.rs`.
- `models`: pinned model catalog, routing by level and privacy, sanitized payloads (S-44),
  rate limits and cost ledger in integer micro-USD.

Spec: [`docs/specs/`](../../docs/specs/). Nothing is public yet.
