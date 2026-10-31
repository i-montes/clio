# adapters/pi

Pi is the agent harness of Clio (ADR 0007). This folder pins it and, in `extensions/`, holds the extension that guards it.

- `package.json` pins `@earendil-works/pi-coding-agent` to an exact version (1.4.0, MIT; it brings its own agent core and model API). `package-lock.json` is committed.
- `.npmrc` sets `ignore-scripts=true`: nothing runs at install time (S-42). Two packages in the tree declare install scripts (`@google/genai`, `protobufjs`); they never run here.
- `overrides` forces patched versions of two transitive dependencies that `npm audit` flags in 1.4.0: `undici` (high, a memory issue in an EventSource parser Clio does not use) and `minimatch` (moderate, glob matching). Remove an override when Pi ships the fix. Checked: Pi 1.4.0 runs normally with them.
- The CI installs only from the lockfile (`npm ci`), fails if it would change, and runs `npm audit`; the nightly audits again. `CODEOWNERS` makes the owner review every change here.

**How Clio launches Pi** (the Rust side does it; here for reference): `pi --mode rpc --no-builtin-tools --no-extensions -e <clio-guard> --no-mcp --no-skills --no-prompt-templates --no-themes --no-context-files --no-approve --offline`, with `PI_CODING_AGENT_DIR`, `PI_CODING_AGENT_SESSION_DIR`, `PI_OFFLINE=1`, `PI_TELEMETRY=0` and `PI_SKIP_VERSION_CHECK=1`, a clean environment, and the working folder set to the session's copy of the project.

**Updating Pi (task 102):** change the exact version, run `npm install --package-lock-only --ignore-scripts`, read the diff of the lockfile and the release notes, rerun `npm audit`, run the bench and the attack battery, and only then merge. Never `^` or `latest`.
