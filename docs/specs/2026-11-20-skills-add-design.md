# Skills the person installs (design, 2026-11-20)

Status: asked by the owner on 2026-11-20. Companion of `2026-11-20-subagents-design.md` (the
`subagent` tool, built on its own branch): a skill that dispatches subagents needs both.

## What it is

Owner, 2026-11-20: "I can install the skills I want; I work mainly with those [superpowers' SDD,
ponytail], so I expect Clio to be able to use them." Clio ships none of them; it installs any.

- **From where.** A folder that holds a skill (`SKILL.md`), a folder that holds several
  (`skills/*/SKILL.md`, the layout of a Claude Code plugin), or a GitHub repository
  (`owner/repo`, optionally `@ref`, or its https URL), fetched as that ref's archive over https
  and unpacked with the checks the installer's downloads already have. The record keeps where it
  came from and the commit.
- **Who.** `clio skills add <source> [name…]` at a terminal, and in the chat the person just asks:
  the model proposes with `skill_add {source, names?, reason}`, which sets it aside as class
  `skill`, and the box shows each skill (name, description, files, sizes, scripts, origin and
  commit, what the judge said). The person's yes installs it. Adding a skill from outside always
  asks: no permission window and no bypass answers it, since a skill is text the model will read
  as Clio's own.
- **Checks.** Every file is read by the judge before the box; a flag is shown in the box in plain
  words and the default answer is no. No reviewer run: the person chose this text. Names are
  Clio's (lowercase letters, digits, `-`); a name already taken by another origin is refused with
  a line saying so; the same origin again is a new version (`clio skills undo` goes back).
  A person-added skill may be larger than a model-written one (SDD has 15 files): up to 100 files
  and 1 MiB; `skill_write`'s limits stay. Only skills are taken from a plugin: its hooks, commands
  and agents are not installed, and the summary says so.
- **Skills written for another harness.** They name tools Clio does not have. The system prompt
  gains a short map, in a new prompt version: a Task or Agent tool is `subagent`; a todo tool is
  `todo_write`; a Skill tool means reading that skill's `SKILL.md`; Read, Write, Edit, Bash, Grep
  and Glob are Clio's `read`, `write`, `edit`, `bash` and `find`; `plugin:name` is the skill
  `name`. No skill's text is rewritten.
- A child reads the installed skills too, which is what SDD's implementer and reviewers need.


## Verified for real before merging

In an isolated home: `clio skills add obra/superpowers` and `clio skills add DietrichGebert/ponytail`
install their skills (the box, the judge's line, the record with the commit), `clio skills` lists
them, a second add of the same origin is a new version and `undo` goes back, a name taken by
another origin is refused, and a folder with a planted injection shows the judge's flag with "no"
as the default.

## Not included

- Shipping third-party skills inside Clio.
- Repositories outside GitHub (clone it and add the folder).
- A plugin's hooks, commands and agents.
- Updating installed skills by themselves.

## Version

0.4.0.
