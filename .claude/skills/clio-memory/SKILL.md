---
name: clio-memory
description: Use clio-memory as Claude's own memory while working on the Clio repo, to dogfood it. Recall at the start of a task and before claiming what was decided; remember decisions, gotchas and findings; report anything odd about clio-memory itself. Use whenever working in this repo, or when the user says "memoria", "recuerda", "qué sabías de", or asks to test the memory.
---

# clio-memory as Claude's memory (dogfooding)

The memory file is `~/.local/share/clio-dev/memory.db`, scope `clio-dev` (Spanish). It lives outside the repo, so nothing in it is committed. Everything goes through `scripts/memory-dev.sh`, which builds the CLI, adds the model and runs as the agent `claude`. The first call builds (slow); later ones take about 1.6 s.

## When to recall

- At the start of a task in this repo: `scripts/memory-dev.sh context clio-dev "<the task in a sentence>"`.
- Before stating what was decided or why something is the way it is: `scripts/memory-dev.sh recall clio-dev "<question>"`.
- `nothing relevant` is a real answer: it means abstention, not an error. Do not retry with other wording until something comes back.

## When to remember

```
scripts/memory-dev.sh remember clio-dev "<one self-contained fact>" [--kind K] [--topic T] [--source S]
```

- Save what the repo and git history do not say: a decision and its reason, a gotcha, a rule that bit us, what the user prefers, a result from an experiment.
- One fact per memory, written so it makes sense alone, in Spanish.
- `--topic T` makes a newer fact replace the older one with the same topic. Use it for facts that change (state of a feature); leave it out for facts that accumulate.
- `--kind`: `semantic` (default, facts and decisions), `episodic` (something that happened), `working` (short-lived). Avoid `procedural`: an agent's procedural memory always goes to quarantine until the user promotes it.
- Output `Add` = saved, `Noop` = already known, `Quarantine` = held for the user. Rejected text (a secret was detected) is an error: never retry with the secret.
- Do not save code, file layouts, or anything in CLAUDE.md or the git log.

## Testing the memory itself

This is the point of dogfooding. When recall misses something you saved, brings up something irrelevant, a command is awkward, or an output is confusing, say so to the user in one line and save it as `--kind episodic --source dogfood` with the query and what came back. Do not fix clio-memory on the side unless asked.

## What not to do

- `promote`, `archive`, `revert`, `feedback wrong` are the user's calls: they act on what is in memory. Ask first. `feedback ID useful` after a recall that actually helped is fine and is how the memory learns.
- Do not edit the database file or delete it. Use `export` or `backup` (`scripts/memory-dev.sh backup PATH`) if a copy is needed.
- The CLI cannot create scopes (rule S-25). A new scope: `cargo run -p clio-memory --example init -- DB SCOPE [LANG]`, and only if the user asks.
- `scripts/memory-dev.sh stats` and `doctor` show the file's health; run `doctor` if something looks off.
