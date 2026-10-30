# testing/canaries

Crate `clio-canaries`: decoy secrets for the gate.

- `Canaries::from_seed` / `fresh`: fake, unique values (`sk-canary-…`), the same for the same seed.
- `scan`, `assert_clean`: look for a decoy in a trace, log or payload, in plain, base64 (any alignment) and URL-encoded form. A hit panics, which fails the test.
- `scan_dir`: the same over every file under a folder.

Plant a decoy where the real secret would go, run the scenario, then scan everything it wrote.

See [`docs/specs/`](../../docs/specs/) and [`docs/security.md`](../../docs/security.md).
