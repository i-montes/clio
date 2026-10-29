# 0001: Repository layout and CI gates

## Decision

- One Cargo workspace (edition 2024) under `crates/`. Non-Rust pieces live in `adapters/`, `apps/`, `infra/` and `testing/`.
- `clio-store` and `clio-memory` depend on no other Clio crate so they can be published as open source on their own.
- Workspace lints: `unsafe_code = "forbid"` (S-41) and `clippy::unwrap_used = "deny"` (allowed in tests).
- The pre-commit secret scan (S-43) is a plain git hook in `.githooks/` calling gitleaks, enabled with `just setup`. No pre-commit framework, so no Python dependency.
- Rule coverage (S-04) is a shell script that compares rule ids in `docs/security.md` and `docs/specs/` against files under `tests/` directories. It only warns for now; it becomes blocking once the rules have tests.
- License: `MIT OR Apache-2.0` (workspace default; revisit before the first publish).
