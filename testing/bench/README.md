# testing/bench

Crate `clio-bench`: the bench of D15. A task has an input, a check that something other than the model decides, and limits (steps, cost, time). The runner repeats each task, stores every run in clio-trace (a root span with task, run and success; a child span with tokens, cost and prompt version) and returns success, cost and time.

```sh
# reproducible: answers from a tape, time held still, no network
cargo run -p clio-bench -- replay --tasks testing/bench/tasks --tape testing/bench/tapes/replay.jsonl \
  --prices testing/bench/prices.json --repeats 3 \
  --baseline testing/bench/baseline.json --thresholds testing/bench/thresholds.json
```

Modes: `replay` (tape), `record` (asks `--cmd PROGRAM [ARG...]` and fills the tape), `real` (asks the command every time). The command reads a `Request` as JSON on stdin and writes a `Response` as JSON on stdout, so keys and HTTP stay out of this crate. Exit 0 passes the gate, 1 means worse than the baseline allows, 2 is a setup error.

- `tasks/trivial/`: 20 trivial requests that must be answered in one step.
- `tasks/real/`: the 10 real tasks of the person (pending, see its README).
- `tapes/replay.jsonl`: **synthetic** answers made with `synthetic-tape` (the text each check looks for). They are plumbing for the replay gate, not model answers; replace them with a real recording.
- `prices.json`: made-up prices for the model `fake/replay-1`. Fill in the real ones before a real run.
- `baseline.json`: the report of the synthetic replay, as the fixed reference. Save a new one with `--save-baseline`.
- `thresholds.json`: provisional limits (5 points of success, 20 % of cost). To be set with the first real measurements.

See [`docs/specs/`](../../docs/specs/) and [`docs/security.md`](../../docs/security.md).
