# testing/attacks

Crate `clio-attacks`: the attack battery of [`docs/security.md`](../../docs/security.md). One JSON file per case in `cases/`: `id`, `title`, the `rules` (S-xx) it exercises, `status` and what a safe system does (`expect`).

- `pending`: the system under test does not exist yet. The runner lists it, never passes it.
- `active`: it must pass (`Target::attack` reports the payload was refused) or the gate blocks.

Process rule: **every new incident adds a case here and a line to the battery in `docs/security.md`**. A test fails if the two lists differ in size, and another one fails if a case names a rule that does not exist.

When the gateway is ready, implement `Target` for it, turn each case `active`, and run the battery from the gate.
