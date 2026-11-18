# infra

- `local/`: what this PC needs, which is the base (ADR 0006): `check-host.sh` (encrypted disk, firewall, SSH, nothing of Clio outside loopback) and the backup scripts with restic (`docs/backups.md`).
- `models/`: installers of the embedding model (`just setup-model-linux`, `just setup-model-macos`).
- `vps/`: a draft of an optional server; it blocks nothing.

See [`docs/specs/`](../docs/specs/) and [`docs/security.md`](../docs/security.md).
