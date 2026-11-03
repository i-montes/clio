# testing/attacks

Crate `clio-attacks`: the attack battery of [`docs/security.md`](../../docs/security.md). One JSON file per case in `cases/`: `id`, `title`, the `rules` (S-xx) it exercises, `status` and what a safe system does (`expect`).

- `pending`: the system under test does not exist yet. The runner lists it, never passes it.
- `active`: it must pass (`Target::attack` reports the payload was refused) or the gate blocks.

Process rule: **every new incident adds a case here and a line to the battery in `docs/security.md`**. A test fails if the two lists differ in size, and another one fails if a case names a rule that does not exist.

Three targets run the battery: the gateway alone (`crates/clio-gateway/tests/attacks.rs`: 01, 02, 04, 05, 07, 13, 14, 17) and the whole agent (`crates/clio-brain/tests/attacks.rs`: 01, 02, 05, 06, 07, 09, 13, 14, 17, 19 to 23, 34) and the jail (`crates/clio-node/tests/jail.rs`: 24 to 33, hostile programs that run INSIDE the `bwrap` jail; they need unprivileged user namespaces and print `SKIPPED` without them, and 34 is skipped the same way), with a gullible scripted model that obeys what files tell it and the decoys of `testing/canaries` searched in every output. The agent harness lives in `clio-brain` because it needs the gateway, node and vault; this crate stays a leaf. A case may only be `active` if a target has a scenario for it that really attempts the attack. The `pending` ones and why, and the measured results: [`docs/mediciones/fase-1-ataques.md`](../../docs/mediciones/fase-1-ataques.md).
