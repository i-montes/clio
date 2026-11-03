# Clio

Autonomous personal agent. Design: [`docs/specs/`](docs/specs/). Security rules (S-01 to S-59): [`docs/security.md`](docs/security.md); threat model: [`docs/threat-model.md`](docs/threat-model.md); reporting: [`SECURITY.md`](SECURITY.md). Decisions: [`docs/decisions/`](docs/decisions/). Contributing: [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Architecture

![Clio architecture, design v2](docs/clio-architecture.svg)

Design v2, aligned with [ADR 0006](docs/decisions/0006-local-first-vps-optional.md) (the host is this PC; a VPS is optional) and [ADR 0007](docs/decisions/0007-pi-is-the-agent-harness.md) (Pi keeps its sessions; Clio lists and opens them).

## Setup

Requires the Rust toolchain from `rust-toolchain.toml`, plus `just`, `gitleaks`, `cargo-deny` and `cargo-audit`.

```sh
just setup   # enables the secret-scanning pre-commit hook (S-43)
just check   # fmt, clippy, rule coverage (S-04: every rule needs a test or a reasoned line in `scripts/rule-coverage.exempt`)
just test
just audit   # cargo-deny and cargo-audit (S-41)
```

## Using Clio on your PC

`just install` (or `cargo build --release -p clio-brain -p clio-vault` and copy `target/release/clio*` to `~/.local/bin`), then `clio init`, `clio run --workspace DIR "instruction"`, `clio diff <id>`, `clio apply <id>`. The agent works on a copy of your project and you decide what goes back. In the chat, Pi runs inside a `bwrap` jail with its own network namespace and will not start if the jail cannot be built ([ADR 0009](docs/decisions/0009-pi-runs-inside-a-jail.md)). Guide (in Spanish): [`docs/uso-local.md`](docs/uso-local.md).

## Layout

- `crates/clio-memory`: generic, publishable on its own; depends on no other Clio crate.
- `crates/clio-brain` (the `clio` binary), `clio-vault`, `clio-gateway`, `clio-node`, `clio-toolhost`, `clio-contract`, `clio-triage`, `clio-trace`, `clio-core`: the working parts of Clio.
- `crates/clio-network`, `clio-policy`, `clio-diagnosis`, `apps/pwa`, `testing/fake-nodes`: placeholders for later phases.
- `adapters/models/openai_compat.py`: the model adapter `clio run` calls (OpenAI-compatible API, key from `OPENCODE_API_KEY` or `~/.config/clio/opencode-api-key`, which must not be readable by group or others).
- `adapters/pi`: the pinned Pi harness and its `clio-guard` extension. `infra/local`, `infra/vps`: host checks, backups, optional VPS draft. `testing/*`: attacks, bench, canaries, fakes, chaos, synthetic.

Platform-specific code (Linux only for now) is isolated and marked by name, never mixed into shared code. See [`docs/decisions/0002-platform-compatibility.md`](docs/decisions/0002-platform-compatibility.md).
