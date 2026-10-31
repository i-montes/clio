# testing/synthetic

Crate `clio-synthetic`: made-up facts and files for tests and benchmarks, from fictional names and a seed. Never real data: emails end in `.invalid`, and a test checks that nothing looks like a secret. Depends on no other Clio crate.

- `facts(seed, n)`: `n` unique facts with a stable `topic` each, for a memory.
- `write_files(seed, dir, n)`: `n` small text files for a node to read.

See [`docs/specs/`](../../docs/specs/) and [`docs/security.md`](../../docs/security.md).
