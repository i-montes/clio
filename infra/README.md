# infra

- `local/`: lo que hace falta en este PC, que es la base (ADR 0006): `check-host.sh` (disco cifrado, cortafuegos, SSH, nada de Clio fuera de loopback) y los scripts de respaldos con restic (`docs/respaldos.md`).
- `models/`: instaladores del modelo de embeddings (`just setup-model-linux`, `just setup-model-macos`).
- `vps/`: borrador de un servidor opcional; no bloquea nada.

Ver [`docs/specs/`](../docs/specs/) y [`docs/security.md`](../docs/security.md).
