# testing/fake-llm

Crate `clio-fake-llm`: a deterministic stand-in for an LLM, used as a dev-dependency.

- `Replay`: answers from a JSON Lines tape keyed by the hash of the request. A request with no recording fails with its hash; nothing reaches the network.
- `Record`: wraps a real `Llm`, records what it answers (after a secret check) and replays it afterwards. Meant for nightly runs and for recording by hand.
- `Scripted`: fixed answers in order, for unit tests with no tape.

Every field of `Request` is part of the hash: adding one means re-recording the tapes.

See [`docs/specs/`](../../docs/specs/) and [`docs/security.md`](../../docs/security.md).
