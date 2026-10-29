# Clio

Autonomous personal agent. Design: [`docs/specs/`](docs/specs/). Security rules (S-01 to S-53): [`docs/security.md`](docs/security.md). Decisions: [`docs/decisions/`](docs/decisions/).

## Architecture

![Clio architecture](docs/clio-architecture.svg)

## Setup

Requires the Rust toolchain from `rust-toolchain.toml`, plus `just`, `gitleaks`, `cargo-deny` and `cargo-audit`.

```sh
just setup   # enables the secret-scanning pre-commit hook (S-43)
just check   # fmt, clippy, rule coverage (S-04, warn-only)
just test
just audit   # cargo-deny and cargo-audit (S-41)
```

## Layout

- `crates/clio-store`, `crates/clio-memory`: generic, publishable on their own; they depend on no other Clio crate.
- `crates/*`: the rest of Clio (placeholders for now).
- `adapters/pi`, `apps/pwa`, `infra`, `testing/*`: placeholders.
