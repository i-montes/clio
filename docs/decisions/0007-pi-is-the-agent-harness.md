# 0007: Pi is the agent harness

## Context

Clio had its own stand-in loop (`clio-brain::agent::AgentLoop`) and, in the plan, its own sessions. On 2026-10-31 the person decided that **Pi is the harness**: Pi runs the agent loop, the providers, the sessions and the compaction, for local and remote sessions, and everything else in Clio connects to it instead of rebuilding it. The person's earlier project Aria (a service that owns the sessions, with a terminal and a dock as windows onto it, Pi pinned by version, access from another computer over ssh) is the reference for style and for how to connect to Pi. Its code is not copied.

Facts checked against Pi 1.4.0 (published 2026-10-21, MIT, package `@earendil-works/pi-coding-agent`, Node 22.19 or newer):
- `pi --mode rpc` speaks JSON lines on stdin/stdout: `prompt`, `abort`, `steer`, session commands, `get_state`, `get_session_stats`, `compact`, and a stream of events (`message_update` with token usage and cost, `tool_execution_start/end`, `compaction_*`, `auto_retry_*`, `agent_settled`).
- Sessions are JSONL trees under a directory we choose (`PI_CODING_AGENT_SESSION_DIR` or `--session-dir`); `--continue`, `--resume`, `--session`, `--fork`.
- TypeScript extensions can block or rewrite every tool call (`tool_call`), rewrite results (`tool_result`), shape the model context (`context`, `before_agent_start`), react to session events and register their own tools.
- Startup flags to confine it: `--no-extensions` (explicit `-e` paths still load), `--no-mcp`, `--no-skills`, `--no-prompt-templates`, `--no-themes`, `--no-context-files`, `--no-approve`, `--offline`, `--no-builtin-tools`.
- Pi's own security guide says it runs with the user's permissions and asks for no approval: safety comes from limiting what it can reach, not from watching the transcript.

## Decision

1. **Pi (pinned 1.4.0, lockfile, no install scripts) runs the loop, providers, sessions and compaction**, one `pi --mode rpc` process per open session. Its config and sessions live in Clio's own directories (`PI_CODING_AGENT_DIR=~/.local/share/clio/pi/agent`, sessions in `~/.local/share/clio/pi/sessions`), never in `~/.pi`.
2. **Clio does not rebuild Pi's tools; it confines them.** Pi's own `read`, `write`, `edit`, `bash` (and `grep`, `find`, `ls` where Pi exports them) are the tools the model uses. Pi starts with `--no-builtin-tools --no-extensions -e <path>`; the only extension is Clio's `clio-guard`, which registers tools with the same names built with Pi's `create*Tool(cwd, { operations })`, so each primitive operation (read a file, write, make a folder, run a command) goes to the Rust `clio-toolhost` (gateway decision, snapshot copy, `bwrap` jail for `bash`, vault scrub, taint). Pi keeps truncation, diffs and rendering; only where and under which decision an operation runs changes. If the extension fails to load there are no tools at all (fail closed). This follows the design page: what Pi already has is not built.
3. **Clio is the service around Pi:** a local service owns the Pi processes and the sessions; the terminal is a window onto it (closing it stops nothing); a remote session is the same service on another host reached over ssh (`tailscale ssh` works), speaking Pi's RPC over stdio. Remote output is untrusted and sanitized like any other.
4. **Memory, the system prompt version and the external-data wrapping come in through the extension hooks** (`before_agent_start`, `context`, `session_start`, `agent_end`).
5. **The own `AgentLoop` stays as the `builtin` harness** for tests and the bench until the Pi harness passes the same tasks and the same attacks; then Pi becomes the default.
6. **Accepted residual risk:** the Pi process and the extension run as the person's user; only the tools are confined. Hardening to do later: run Pi itself inside `bwrap` with network limited to the model provider.

## Consequences

Tasks 098 to 102 change shape: 098 is the Rust side that launches and talks to Pi, 099 is reversed (Pi keeps the sessions; Clio lists and opens them), 100 and 101 are the launch flags plus the guard. The sessions of the abandoned own design are not built. The old `@mariozechner/*` package names are replaced by `@earendil-works/*`.
