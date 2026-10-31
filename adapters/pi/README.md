# adapters/pi

Pi is the agent runtime Clio builds on (D16). This folder only **pins** it for now: the adapter between Pi and Clio is not written yet (task 098).

- `package.json` pins `@mariozechner/pi-agent-core` and `@mariozechner/pi-ai` to an exact version (0.73.1, MIT). `package-lock.json` is committed.
- `.npmrc` sets `ignore-scripts=true`: nothing runs at install time (S-42). Two packages in the tree declare install scripts (`@google/genai`, `protobufjs`); they never run here.
- `overrides` forces `basic-ftp` to a patched version: 0.73.1 pulls an older one with a high-severity advisory (a slow parser, reachable only through FTP URLs, which Clio does not use). Remove the override when Pi ships the fix.
- The CI installs only from the lockfile (`npm ci`), fails if it would change, and runs `npm audit`; the nightly audits again. `CODEOWNERS` makes the owner review every change in this folder.

**Updating Pi (task 102):** change the exact versions, run `npm install --package-lock-only --ignore-scripts`, read the diff of the lockfile and the release notes, run the bench (`docs/mediciones/`), and only then merge. Never `^` or `latest`.

The minimal loop in `crates/clio-brain` (`AgentLoop` behind a `Model` trait) is a stand-in with the same shape, so Pi can replace it behind an adapter without changing the rest.
