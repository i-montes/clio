# CLAUDE.md

Next.js 15 app with the App Router and TypeScript.

## Commands

- Use pnpm, never npm or yarn: `pnpm install`, `pnpm dev`, `pnpm build`.
- Run `pnpm lint` and `pnpm test` before you commit. Fix lint errors instead of disabling the rule.
- Database migrations: `pnpm db:migrate`. Never edit a migration that is already on `main`.

## Style

- Server components by default; add `"use client"` only where state or effects need it.
- Tailwind for styles; no CSS modules.
- Name files in kebab-case. Export one component per file.

When you are unsure about a product decision, ask me instead of guessing.
