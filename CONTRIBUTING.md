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

## Every incident adds a case to the battery

If something went wrong in production or in a report, the PR that fixes it adds the case in `testing/attacks/cases/` and a line in the attack battery of `docs/security.md`. A test fails if the two lists do not match.

> The `main` branch is protected: every change goes in through a PR, with the 10 CI checks green and a review by the code owners (`.github/CODEOWNERS`). Work always happens on a branch and a PR. The protection does not apply to admins, so the owner can merge with `gh pr merge --admin` once CI is green; pushing straight to `main` skips the protection and is not done.
