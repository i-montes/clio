# testing/bench

Crate `clio-bench`: the bench of D15. A task has an input, a check that something other than the model decides, and limits (steps, cost, time). The runner repeats each task, stores every run in clio-trace (a root span with task, run and success; a child span with tokens, cost and prompt version) and returns success, cost and time.

```sh
# reproducible: answers from a tape, time held still, no network
cargo run -p clio-bench -- replay --tasks testing/bench/tasks --tape testing/bench/tapes/beam-free.jsonl \
  --provider opencode-go --model beam-free --prices testing/bench/prices.json --repeats 3 \
  --baseline testing/bench/baseline.json --thresholds testing/bench/thresholds.json
```

Modes: `replay` (tape), `record` (asks `--cmd PROGRAM [ARG...]` and fills the tape), `real` (asks the command every time). The command reads a `Request` as JSON on stdin and writes a `Response` as JSON on stdout, so keys and HTTP stay out of this crate. Exit 0 passes the gate, 1 means worse than the baseline allows, 2 is a setup error.

- `tasks/trivial/`: 20 trivial requests that must be answered in one step.
- `tasks/real/`: the 10 real tasks of the person (pending, see its README).
- `tapes/beam-free.jsonl`: the real answers of `opencode-go/beam-free` to the 20 trivial tasks, recorded on 2026-10-31 with `record`. Replay uses it with `--provider opencode-go --model beam-free`; to refresh it, delete it and run `record`.
- `prices.json`: per-million-token prices. `opencode-go/beam-free` is 0 because the model is free for a limited time; if that changes, update it, or the cost gate means nothing. `fake/replay-1` is made up.
- `baseline.json`: the report of the replay with the real tape. `baseline-real.json`: the report of a real run (3 repeats), used by the nightly. Save a new one with `--save-baseline`.
- `thresholds.json`: provisional limits (5 points of success, 20 % of cost). Task 074 sets them with measurements over more than trivial tasks.
- `llm_openai_compat.py`: the program behind `--cmd` for any OpenAI-compatible API; defaults to OpenCode Go and reads the key from `OPENCODE_API_KEY` or, if that is not set, from `~/.config/clio/opencode-api-key` (mode 0600, refused otherwise). In GitHub it is the Actions secret `OPENCODE_API_KEY`. Never put the key in a file of the repository. `beam-free` is free in exchange for nothing; do not use `iceman-contributor-free`, which lets the provider train on the prompts.

See [`docs/specs/`](../../docs/specs/) and [`docs/security.md`](../../docs/security.md).
