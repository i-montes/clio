# clio-node

Library of the per-node daemon (the daemon binary comes later): everything a node needs to run an
already approved action without being able to hurt anything outside a folder.

- `Sandbox`: a folder where tools read and write; every path is canonicalized and must stay inside
  (`..`, absolute paths and symlinks pointing out are rejected). `snapshot()` / `restore()` bring the
  folder back exactly (hard links, no copy when the filesystem allows it; writes replace files
  atomically so a snapshot is never touched).
- `jail` (`Jail`, `Job`, `Limits`): runs ONE command (explicit argv, never a shell) inside `bwrap`:
  `--unshare-all` plus user and cgroup required, `--disable-userns`, `--cap-drop ALL`,
  `--die-with-parent`, `--new-session`, `--clearenv` with a fixed `PATH`/`HOME`/`LANG`, read-only
  `/usr` (and `/bin`, `/lib`, `/lib64`, the loader cache), own `/proc`, `/dev`, `/tmp` (size
  limit with bwrap 0.9+), and only the working folder at `/work` read-write. `prlimit` sets CPU,
  address space, processes, file size and open files inside the jail; the wall-clock limit kills
  the jail's whole pid namespace; output is capped to its tail and a flood kills the command. The
  launcher itself runs with an EMPTY environment (bwrap's own environment would be readable as
  `/proc/1/environ`). `Jail::detect` and `Jail::probe` fail closed when `bwrap`, `prlimit` or
  user namespaces are missing. No seccomp filter, no disk quota: see `docs/threat-model.md`.
- `Jail::self_test` (`selftest.rs`, `selftest.py`): a program inside the jail tries forbidden
  things (read a decoy in HOME, connect to the host, write in `/usr`, kill a host process, read
  the caller's environment...); the host checks from outside that nothing happened.
- `run_tests_tool` / `register_run_tests`: the `run_tests` tool, `{"runner": "unittest"|"pytest",
  "target": "<optional>"}`, effect `LocalReversible`. Registered only where the jail can exist.
  `cargo_test` is not offered (a rustup `cargo` lives in the person's home).
- `Sandbox::set_copy_snapshots(true)`: snapshots copy files instead of hard-linking them. Required
  once programs run in the folder: an in-place edit would change a hard-linked snapshot too.
- `EffectLog`: JSON-lines log that makes an action (by id) apply its effect once.
- `LeaseTable`: leases on a resource (repo, path) with an injectable `Clock`.
- `Registry` and the basic tools: no tool without a declared `Effect`; only `Read` and
  `LocalReversible` in this phase; few visible tools, the rest found by `search`.
- `Executor`: runs an action only with an `Approval` valid for the hash of that exact action.

Tests: `tests/node.rs`, and `tests/jail.rs`, which ATTACKS the jail (attack cases 24 to 33). Without unprivileged user namespaces those tests print `SKIPPED` and return; set `CLIO_REQUIRE_JAIL=1` to make that a failure. Run with `-- --nocapture` to see the messages.

Spec: [`docs/specs/`](../../docs/specs/). Nothing is public yet.
