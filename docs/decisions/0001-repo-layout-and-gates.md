# 0001: Repository layout and CI gates

## Decision

- One Cargo workspace (edition 2024) under `crates/`. Non-Rust pieces live in `adapters/`, `apps/`, `infra/` and `testing/`.
- `clio-memory` depends on no other Clio crate so it can be published as open source on its own. (`clio-store` was dropped; see 0003.)
- Workspace lints: `unsafe_code = "forbid"` (S-41) and `clippy::unwrap_used = "deny"` (allowed in tests).
- The pre-commit secret scan (S-43) is a plain git hook in `.githooks/` calling gitleaks, enabled with `just setup`. No pre-commit framework, so no Python dependency.
- Coverage (S-04) is a shell script that compares rule ids in `docs/security.md` and `docs/specs/` against files under `tests/` directories, and each spec's numbered acceptance tests against tests named `t<n>_…` or tagged `spec-<crate> #<n>`. It only warns for now; it becomes blocking once the rules have tests.
- License: `MIT OR Apache-2.0` (workspace default; revisit before the first publish).
