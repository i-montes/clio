# AGENTS.md

This is a Rust workspace (edition 2024). The crates live in `crates/`, the CLI in `crates/app-cli`.

## Build and test

- Build: `cargo build --workspace`
- Before every commit run `just check` (fmt, clippy with `-D warnings`) and `just test`.
- Integration tests need Docker; if Docker is not there, run `cargo test --lib` and say so.

## Rules

- Never push to `main`; open a pull request from a branch named `feat/...` or `fix/...`.
- Do not add a dependency without asking first.
- Keep `unsafe` out of the code: the workspace forbids it.
- Commit messages: imperative mood, under 72 characters in the first line.
