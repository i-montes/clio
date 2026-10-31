# clio-node

Library of the per-node daemon (the daemon binary comes later): everything a node needs to run an
already approved action without being able to hurt anything outside a folder.

- `Sandbox`: a folder where tools read and write; every path is canonicalized and must stay inside
  (`..`, absolute paths and symlinks pointing out are rejected). `snapshot()` / `restore()` bring the
  folder back exactly (hard links, no copy when the filesystem allows it; writes replace files
  atomically so a snapshot is never touched).
- `EffectLog`: JSON-lines log that makes an action (by id) apply its effect once.
- `LeaseTable`: leases on a resource (repo, path) with an injectable `Clock`.
- `Registry` and the basic tools: no tool without a declared `Effect`; only `Read` and
  `LocalReversible` in this phase; few visible tools, the rest found by `search`.
- `Executor`: runs an action only with an `Approval` valid for the hash of that exact action.

Spec: [`docs/specs/`](../../docs/specs/). Nothing is public yet.
