# Agent guide

Monorepo with npm workspaces: `apps/web`, `apps/api`, `packages/ui`.

Before you open a pull request:

- run `npm run check` at the root (typecheck, lint, tests for every workspace);
- update the package's `CHANGELOG.md` if the change is user-visible.

Do not modify anything under `vendor/` or `packages/legacy/`. Ask before adding a new workspace or dependency.
Use conventional commits (`feat:`, `fix:`, `chore:`). Prefer small pull requests.
