# Clio

Autonomous personal agent. Design: [`docs/specs/`](docs/specs/). Security rules (S-01 to S-68): [`docs/security.md`](docs/security.md); threat model: [`docs/threat-model.md`](docs/threat-model.md); reporting: [`SECURITY.md`](SECURITY.md). Decisions: [`docs/decisions/`](docs/decisions/). Contributing: [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Architecture

![Clio architecture: the guard is at the door](docs/clio-architecture.svg)

The guard is at the door ([ADR 0006](docs/decisions/0006-local-first-vps-optional.md), [ADR 0012](docs/decisions/0012-host-mode.md)). Clio has full use of the terminal of the machine it lives on, and the security layer is built around that instead of walling the agent in:

1. Pi, the agent harness, and the model stay in a `bwrap` jail ([ADR 0009](docs/decisions/0009-pi-runs-inside-a-jail.md)). They never see the vault, your files or Clio's data. The only credential inside is the model key, and it reaches only the model provider.
2. Every effect goes through the tool host (`clio-toolhost`), which decides each operation in code.
3. A judge (Temis, the owner's own service, reached through the Jev adapter) reads text that comes from outside before the model does, and withholds what tries to give the agent orders.
4. Four classes of action wait for your approval: delete, spend, send and control.
5. Secrets live in the vault. The model sees names, never values.
6. Everything is logged, and file changes can be undone.

Pi keeps its own sessions; Clio lists and opens them ([ADR 0007](docs/decisions/0007-pi-is-the-agent-harness.md)).

## Setup

Requires the Rust toolchain from `rust-toolchain.toml`, plus `just`, `gitleaks`, `cargo-deny` and `cargo-audit`.

```sh
just setup   # enables the secret-scanning pre-commit hook (S-43)
just check   # fmt, clippy, rule coverage (S-04: every rule needs a test or a reasoned line in `scripts/rule-coverage.exempt`)
just test
just audit   # cargo-deny and cargo-audit (S-41)
```

## Using Clio on your machine

Install with one line (Linux x86_64 and aarch64; macOS is not supported yet):

```sh
curl -fsSL https://github.com/i-montes/clio/releases/latest/download/install.sh | sh
```

It puts `clio` and `clio-vault` of the latest release in `~/.local/bin` (and that folder on the `PATH` of your next terminal), verified against the release's `SHA256SUMS`, and starts `clio setup`: a guided installer in Spanish or English that asks everything first, then installs the rest (bubblewrap, a private Node 22, Pi, clio-memory and its model, the vault, the model's key, the services timer, the shipped skills) and opens the chat. Running `clio setup` again repairs, updates or removes an installation; `clio update` brings everything up to date without questions. What it downloads and how each is checked: [`docs/security.md`](docs/security.md#what-the-installer-downloads).

By hand, from a checkout:

1. Install: `just install` (or `cargo build --release -p clio-brain -p clio-vault` and copy `target/release/clio*` to `~/.local/bin`). Then `clio init`.
2. Start the vault once per boot: `clio vault start`. The model key lives there. On a server, `loginctl enable-linger $USER` keeps the vault's runtime folder across logouts.
3. Pick a model connector: `clio connect opencode`, `clio connect claude` or `clio connect anthropic-api`. `claude` uses your Claude subscription through the official Claude Code, which `clio pi install` brings (about 400 MB, once); the token comes from `claude setup-token`, it uses your plan's quota like Claude Code, and the bridge package warns that Anthropic could change how it bills the Agent SDK. `anthropic-api` takes an Anthropic API key, billed per token.
4. Type `clio` to open the chat, or `clio run "instruction"` for one job without the interface.

By default Clio works in host mode ([ADR 0012](docs/decisions/0012-host-mode.md)): the agent acts on your machine, what deletes, spends, sends or touches Clio's own setup waits for your approval (`clio pending`, `clio approve <id>`, `clio reject <id>`, or the question at the end of a chat turn), and everything is logged (`clio actions`). With `--copy`, `"mode": "copy"` in `config.json`, or `clio run --workspace DIR`, it works on a copy of one project instead, and you decide what goes back with `clio diff <id>` and `clio apply <id>`. On Linux, in both modes, Pi runs inside its jail and does not start if the jail cannot be built ([ADR 0009](docs/decisions/0009-pi-runs-inside-a-jail.md)).

Clio ships with who she is. Her primordial memories (the blocks `identidad`, `reglas`, `preferencias` and `vigilancia`) go to the model with authority in every session. In the first chat she introduces herself and asks who you are. You change them by talking to her: in a session that read nothing from outside she writes the change at once; in one that did, the change waits for your approval. `clio memory identity` shows them and `clio memory remember "text" --block B` writes one by hand.

Clio writes her own skills (how to do a kind of task, with a check and, if needed, scripts) with `skill_write`: the judge reads each one, a second agent reviews it, and it is registered only by your own `clio`, at once in a clean session and with your approval otherwise. `/skills` or `clio skills` lists them and turns one off, back a version or away (S-68). `clio skills add owner/repo` (or a folder) installs skills written elsewhere, each only on your yes, after the judge read it.

Work that repeats can become a service: a program Clio writes with `service_write` that then runs by itself on a schedule, with no model, as you, the machine read-only except its own folder and the paths it declared, the network only to the hosts it declared, its secrets from the vault only for those hosts. Every service and every new version waits for your approval after the judge and the reviewer. `clio services enable` sets up a systemd user timer that runs them; `/services` or `clio services` lists them, shows their runs and turns one off (S-69).

Every few hours (`round.every_hours`, never in the quiet hours) the services timer also starts a round: Clio looks by herself at what you asked her to watch (your `vigilancia` block) and at services and MCP servers that raised a notice, and leaves a short notice only when something needs you, shown the next time a chat opens. A round only reads, by code (its commands run read-only without `sudo`, what would change something is refused, what reaches out waits for your yes), is never covered by a permission window, needs the judge on and has its own budget; what it thinks should be done it proposes in its notice. `clio round` or `/round` lists the last ones (S-71).

Clio can also use MCP servers: npm packages at an exact version that run on your machine over standard input and output (no remote servers, OAuth, Python or Docker ones). Clio proposes one with `mcp_propose`; it always waits for your approval, showing npm's facts, the hosts it may reach and the secrets it would hold. Approved, it is installed with no install scripts, started once without network or secrets to list its tools, and stays off until you confirm them (`/mcp confirm` or `clio mcp confirm`). It runs in a service's jail with only its own secrets; Clio calls it through two fixed tools, and a tool that may change something asks you each time (S-70).

Guide: [`docs/usage.md`](docs/usage.md).

## Layout

- The memory engine is [clio-memory](https://github.com/i-montes/clio-memory), in its own repository: Clio depends on it pinned by the commit of a release tag (`crates/clio-brain/Cargo.toml`); to move up, put the new tag's commit there (the memory file's schema only changes backward-compatibly). The same file, `~/.local/share/clio/memory.db`, is shared with any agent that uses clio-memory, such as Claude Code (`clio-memory setup claude-code`). When the `clio-memory` binary is on the `PATH`, Clio takes its vectors from the clio-memory daemon (one process per data folder, holding the only copy of the model) instead of loading the model itself; without it, it loads the model as before. The Pi tool host and the sleep still load the model themselves. To move the pin, install the `clio-memory` binary of the new tag first (`clio memory update`): Clio leaves a daemon older than its library alone (so the agents keep theirs) and loads the model itself. `clio` calls `clio_memory::private_runtime()` at start, so ONNX Runtime runs with its telemetry off.
- `crates/clio-brain` (the `clio` binary), `clio-vault`, `clio-gateway`, `clio-node`, `clio-toolhost`, `clio-contract`, `clio-triage`, `clio-trace`, `clio-core`: the working parts of Clio.
- `crates/clio-network`, `clio-policy`, `clio-diagnosis`, `apps/pwa`, `testing/fake-nodes`: placeholders for later phases.
- `adapters/models/openai_compat.py`: the model adapter `clio run` calls (OpenAI-compatible API, key in an envelope on stdin, or from `OPENCODE_API_KEY` when called with a bare request; never from a file), `adapters/models/claude_code.py` for the Claude connector (same contract; it runs the Claude Code binary, `CLAUDE_CODE_OAUTH_TOKEN`) and `adapters/models/anthropic_api.py` for the `anthropic-api` connector (`ANTHROPIC_API_KEY`).
- `adapters/pi`: the pinned Pi harness and its `clio-guard` extension. `infra/local`, `infra/vps`: host checks, backups, optional VPS draft. `testing/*`: attacks, bench, canaries, fakes, chaos, synthetic.

Platform-specific code (Linux only for now) is isolated and marked by name, never mixed into shared code. See [`docs/decisions/0002-platform-compatibility.md`](docs/decisions/0002-platform-compatibility.md).
