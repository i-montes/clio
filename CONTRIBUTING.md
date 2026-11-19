# Contributing

Before you open a PR: `just setup` (turns on the hook that refuses secrets), `just check` and `just test`.

## Language

Code, comments, the interface, prompts and docs are in English. The model answers in whatever language the person writes in. Dated historical records (`docs/plans/`, `docs/specs/`, `docs/superpowers/`, `docs/mediciones/`, `docs/lista-maestra.md`, `docs/fase-1-cierre.md`) stay as they were written.

## Every PR names the security rules it touches

In the description, the **Rules** section lists the S-xx of [`docs/security.md`](docs/security.md) the change touches and the tests that cover them. If it touches none, write "none". A rule without a test is not met, and CI checks every rule (S-04): each rule in `docs/security.md` needs a test that runs (a `// S-xx` comment right above a `#[test]`) or a line in `scripts/rule-coverage.exempt` that says, with a reason, where else it is checked (a CI job, a host script, a part that does not exist yet). A new rule with neither fails CI; so does an exemption that already has a test, so it does not go stale.

## A person reviews these paths, not an agent

The gateway (`crates/clio-gateway`), the vault (`crates/clio-vault`), node signatures (`crates/clio-node`) and the PWA (`apps/pwa`). `.github/CODEOWNERS` says so, and no agent approves or merges changes to them (S-53). The same goes for the `Cargo.toml` files, `deny.toml` and `.github/`, because `unsafe_code = "forbid"` lives there (S-41).

## Dependencies and secrets

No new dependency without passing `cargo-deny`. Nothing that looks like a secret goes into the repository, not even as an example: use made-up values the scanner does not recognize.

## Cutting a release

1. On a branch, set the new version in the workspace `Cargo.toml` (`[workspace.package] version`, and the `clio-core` requirement in `crates/clio-vault/Cargo.toml` if it names it), run `cargo check` so `Cargo.lock` follows, and merge it through a PR.
2. Tag that commit on `main` and push the tag: `git tag v0.2.0 && git push origin v0.2.0`.
3. `release.yml` checks that the tag is the version in `Cargo.toml` (`clio --version` prints it), runs every gate, and waits in the `release-review` environment until the owner approves (read the sensitive paths first, as the workflow says). Then it builds `clio` and `clio-vault` for Linux x86_64 and aarch64 (glibc 2.28, with `cargo zigbuild`) and macOS arm64 and x86_64, smoke-tests them (on Ubuntu 22.04 and Debian 12 too), attests them, and publishes them with `VERSION`, `SHA256SUMS` and `install.sh`.
4. The one-line install and `clio update` take the new release at once (they read `releases/latest/download/VERSION`). A release is not edited after it is published: a fix is a new version.

A manual run of the workflow (`workflow_dispatch`) builds and tests everything but publishes nothing.

## Every incident adds a case to the battery

If something went wrong in production or in a report, the PR that fixes it adds the case in `testing/attacks/cases/` and a line in the attack battery of `docs/security.md`. A test fails if the two lists do not match.

> The `main` branch is protected: every change goes in through a PR, with the 10 CI checks green and a review by the code owners (`.github/CODEOWNERS`). Work always happens on a branch and a PR. The protection does not apply to admins, so the owner can merge with `gh pr merge --admin` once CI is green; pushing straight to `main` skips the protection and is not done.
