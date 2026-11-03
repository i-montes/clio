# testing/bench

Crate `clio-bench`: the bench of D15. A task has an input, a check that something other than the model decides, and limits (steps, cost, time). The runner repeats each task, stores every run in clio-trace (a root span with task, run and success; a child span with tokens, cost and prompt version) and returns success, cost and time.

```sh
# reproducible: answers from a tape, time held still, no network
cargo run -p clio-bench -- replay --tasks testing/bench/tasks --tape testing/bench/tapes/beam-free.jsonl \
  --provider opencode-go --model beam-free --prices testing/bench/prices.json --repeats 3 \
  --baseline testing/bench/baseline.json --thresholds testing/bench/thresholds.json
```

Modes: `replay` (tape), `record` (asks `--cmd PROGRAM [ARG...]` and fills the tape), `real` (asks the command every time). The command reads a `Request` as JSON on stdin and writes a `Response` as JSON on stdout, so keys and HTTP stay out of this crate. Exit 0 passes the gate, 1 means worse than the baseline allows, 2 is a setup error.

Mode `agent` runs the real agent (`clio-brain::agent::AgentLoop`) over the tasks that have a `workspace`:

```sh
cargo run --release -p clio-bench -- agent --tasks testing/bench/tasks --prices testing/bench/prices.json \
  --repeats 3 [--tags security,code] [--only real-01,real-09] [--prompt prompts/system/clio-agent@2.md] \
  [--keep DIR] [--save-baseline testing/bench/baseline-agent.json] \
  --cmd python3 testing/bench/llm_openai_compat.py

# the same tasks and hidden checks through Pi (the harness of `clio run --harness pi`); no --cmd:
cargo build --release -p clio-brain -p clio-bench   # the toolhost is `clio toolhost`
cargo run --release -p clio-bench -- agent --tasks testing/bench/tasks --prices testing/bench/prices.json \
  --repeats 3 --harness pi [--only real-01] [--keep DIR] [--save-baseline testing/bench/baseline-agent-pi.json]
```

`--harness pi` (default `builtin`) starts, per run, `clio toolhost` on the private copy and hardened Pi (guard, the six tools, `--thinking low`) with the provider of `--provider/--model` (key only from `OPENCODE_API_KEY` or `~/.config/clio/opencode-api-key`), sends the task's `input` as the prompt and runs to `agent_settled`. The system prompt is `clio-agent@4` (what the guard checks); `--prompt` is ignored. A run is aborted when it crosses the task's steps (assistant messages) or time. It needs Node, the pinned Pi installed (`clio init`) and a `clio` binary next to `clio-bench` (or `CLIO_BIN`). Steps, tokens and cost come from Pi's events and stats; `denied` counts guard blocks plus toolhost/gateway refusals; the table and the baseline add tool calls, and `duration_ms` includes the ~0.3 s of startup (`setup_ms`). The tests (`tests/pi_bench.rs`) use the fake provider and skip without Node/Pi/`clio` unless `CLIO_REQUIRE_PI=1`.

Each run copies `workspace/` to a private folder, lets the agent work only there, then runs the task's hidden check (no shell, `{task_dir}` replaced, `TASK_DIR` and an empty `SINK_LOG` in the environment) on what it left. The agent never sees `check.py`, `solution/` or `bad_solutions/`. Success, steps, time, tokens and cost go to clio-trace like in `replay`; the output is one summary line and one table row per task (ok/runs, mean steps). `--keep DIR` keeps every run folder, with `transcript.json`, for a post-mortem; without it they are deleted. There is no gate for this mode yet: `baseline-agent.json` is a measurement (per-run detail included), read it in `docs/mediciones/`. Do not tune prompts looking at the hidden checks or the solutions.

- `tasks/trivial/`: 20 trivial requests that must be answered in one step.
- `tasks/real/`: the 22 real tasks (01 to 10 the first batch, which `beam-free` solves 30 of 30; 11 to 22 the harder batch, with long precise specs, visible tests and much stricter hidden checks; see its README). `python3 testing/bench/verify_real_tasks.py` checks every check against its starting files, solution and bad solutions.
- `tapes/beam-free.jsonl`: the real answers of `opencode-go/beam-free` to the 20 trivial tasks, recorded on 2026-10-31 with `record`. Replay uses it with `--provider opencode-go --model beam-free`; to refresh it, delete it and run `record`.
- `prices.json`: per-million-token prices. `opencode-go/beam-free` is 0 because the model is free for a limited time; if that changes, update it, or the cost gate means nothing. `fake/replay-1` is made up.
- `baseline.json`: the report of the replay with the real tape. `baseline-real.json`: the report of a real run (3 repeats), used by the nightly. `baseline-agent.json`: the agent mode over the 10 real tasks with `clio-agent@2` (3 repeats); `baseline-agent-v1.json`: the same with `clio-agent@1`. `baseline-agent-builtin-22.json` and `baseline-agent-pi.json`: the 22 real tasks, 3 repeats, builtin (`clio-agent@2`) and `--harness pi`; compared in `docs/mediciones/fase-1-pi-vs-builtin.md`. There is no baseline file for the 12 harder tasks (`--only real-11,...,real-22`) yet: their measurement is partial, see `docs/mediciones/fase-1-agente-dificil.md`. Save a new one with `--save-baseline`.
- `thresholds.json`: provisional limits (5 points of success, 20 % of cost). Task 074 sets them with measurements over more than trivial tasks.
- `llm_openai_compat.py`: the program behind `--cmd` for any OpenAI-compatible API; defaults to OpenCode Go and reads the key from `OPENCODE_API_KEY` or, if that is not set, from `~/.config/clio/opencode-api-key` (mode 0600, refused otherwise). In GitHub it is the Actions secret `OPENCODE_API_KEY`. Never put the key in a file of the repository. `beam-free` is free in exchange for nothing; do not use `iceman-contributor-free`, which lets the provider train on the prompts.

See [`docs/specs/`](../../docs/specs/) and [`docs/security.md`](../../docs/security.md).
