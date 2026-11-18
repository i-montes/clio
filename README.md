# Clio

Autonomous personal agent. Design: [`docs/specs/`](docs/specs/). Security rules (S-01 to S-60): [`docs/security.md`](docs/security.md); threat model: [`docs/threat-model.md`](docs/threat-model.md); reporting: [`SECURITY.md`](SECURITY.md). Decisions: [`docs/decisions/`](docs/decisions/). Contributing: [`CONTRIBUTING.md`](CONTRIBUTING.md).

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

`just install` (or `cargo build --release -p clio-brain -p clio-vault` and copy `target/release/clio*` to `~/.local/bin`), then `clio init`, `clio vault start` (once per boot; the model key lives in the vault), `clio connect opencode`, `clio connect claude` (your Claude subscription, through the official Claude Code that `clio pi install` brings, about 400 MB once; the token comes from `claude setup-token`; it uses your plan's quota like Claude Code, and the bridge package warns that Anthropic could change how it bills the Agent SDK) or `clio connect anthropic-api` (an Anthropic API key, billed per token), `clio run --workspace DIR "instruction"`, `clio diff <id>`, `clio apply <id>`. The agent works on a copy of your project and you decide what goes back. In the chat, Pi runs inside a `bwrap` jail with its own network namespace and will not start if the jail cannot be built ([ADR 0009](docs/decisions/0009-pi-runs-inside-a-jail.md)). On a server, `loginctl enable-linger $USER` keeps the vault's runtime folder across logouts. Clio ships with who she is: her primordial memories (the blocks `identidad`, `reglas`, `preferencias` and `vigilancia` of the memory) go to the model with authority in every session, the first chat has her introduce herself and ask who you are, and you change them by talking to her in a clean session (one that read nothing from outside); `clio memory identity` shows them and `clio memory remember "text" --block B` writes one by hand. Guide (in Spanish): [`docs/uso-local.md`](docs/uso-local.md).

## Layout

- The memory engine is [clio-memory](https://github.com/i-montes/clio-memory), in its own repository: Clio depends on it pinned by the commit of a release tag (`crates/clio-brain/Cargo.toml`); to move up, put the new tag's commit there (the memory file's schema only changes backward-compatibly). The same file, `~/.local/share/clio/memory.db`, is shared with any agent that uses clio-memory, such as Claude Code (`clio-memory setup claude-code`). When the `clio-memory` binary is on the `PATH`, Clio takes its vectors from the clio-memory daemon (one process per data folder, holding the only copy of the model) instead of loading the model itself; without it, it loads the model as before. The Pi tool host and the sleep still load the model themselves. To move the pin, install the `clio-memory` binary of the new tag first (`clio memory update`): Clio leaves a daemon older than its library alone (so the agents keep theirs) and loads the model itself. `clio` calls `clio_memory::private_runtime()` at start, so ONNX Runtime runs with its telemetry off.
- `crates/clio-brain` (the `clio` binary), `clio-vault`, `clio-gateway`, `clio-node`, `clio-toolhost`, `clio-contract`, `clio-triage`, `clio-trace`, `clio-core`: the working parts of Clio.
- `crates/clio-network`, `clio-policy`, `clio-diagnosis`, `apps/pwa`, `testing/fake-nodes`: placeholders for later phases.
- `adapters/models/openai_compat.py`: the model adapter `clio run` calls (OpenAI-compatible API, key in an envelope on stdin, or from `OPENCODE_API_KEY` when called with a bare request; never from a file), `adapters/models/claude_code.py` for the Claude connector (same contract; it runs the Claude Code binary, `CLAUDE_CODE_OAUTH_TOKEN`) and `adapters/models/anthropic_api.py` for the `anthropic-api` connector (`ANTHROPIC_API_KEY`).
- `adapters/pi`: the pinned Pi harness and its `clio-guard` extension. `infra/local`, `infra/vps`: host checks, backups, optional VPS draft. `testing/*`: attacks, bench, canaries, fakes, chaos, synthetic.

Platform-specific code (Linux only for now) is isolated and marked by name, never mixed into shared code. See [`docs/decisions/0002-platform-compatibility.md`](docs/decisions/0002-platform-compatibility.md).
