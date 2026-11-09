# 0011: The memory file is not encrypted by the engine; the host's disk and the backups are

Status: accepted by default (2026-11-09), with the person away; it can be changed (`docs/decisiones-pendientes.md`). Closes task 303.

## Context

Task 303 asks whether `memory.db` should be encrypted at rest by the engine (SQLCipher) or not at all. S-23 asks for the file to be 0600 under its own user and for backups and exports to be encrypted; S-24 says vectors are as sensitive as the text; S-40 asks for an encrypted disk and encrypted backups on the host.

What already holds:

- The file is created 0600 in a 0700 folder, and the engine refuses to open it otherwise (`clio-memory/tests/writer_lock.rs`, S-23). Vectors live in the same file with the same permissions (`clio-memory/tests/embed.rs`, S-24).
- On the person's PC, the base of ADR 0006, the home disk is LUKS; `infra/local/check-host.sh` checks it (S-40).
- Backups go through restic (`docs/respaldos.md`), which encrypts them. The memory is copied hot (`clio-memory backup`), never the file in use.
- The same file is shared by every agent that uses clio-memory over MCP (Claude Code, OpenCode, Codex…), and by the nightly sleep. Each opens it as the person's user.

## Decision

**No SQLCipher.** The engine writes a plain SQLite file; confidentiality at rest comes from the host's disk encryption and from the backups' encryption.

- **What SQLCipher would protect against, and what it would not.** It protects a copy of the file that leaves the machine without its key: a stolen disk without LUKS, or a backup made outside restic. It does not protect against anything running as the person's user while the memory is open, which is the threat that matters here (`docs/threat-model.md`, "La memoria como superficie"): the key would have to be available to every MCP process and to the sleep, so it would sit in the same reach as the file.
- **What it would cost.** A different SQLite build for every platform clio-memory ships (Linux x86_64 and aarch64, macOS arm64 and Intel), a key to provision to every agent the installer connects, and a key the sleep must have at night without anyone there. Losing the key loses the memory.
- **Where it stops being enough.** The decision holds only where the host's disk is encrypted. It is reopened if the memory lives on a host without disk encryption (a VPS, or a cloud VM like the server used for development on 2026-11-09, whose disk is not LUKS), or if the memory is synchronized between machines (Engram decision 4, "not for now").
- **Exports.** `clio-memory export` writes plain JSON Lines for the person to read and move. Encrypting exports and the sleep's local copies is task 302, done with the off-machine backups (093, 094), not with this ADR.

## Consequences

- `check-host.sh` is the check that this decision depends on: a host where it fails the LUKS line should not hold a real memory.
- Nothing changes in code. The rule-coverage of S-23 and S-24 stays where it is.
- If the decision is reopened, the natural path is a key from the vault (`clio-vault`) handed to the engine at open, which is the same problem as giving the key to every MCP client; that is the cost to weigh then.
