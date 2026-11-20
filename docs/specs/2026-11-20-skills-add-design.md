# Skills the person installs (design, 2026-11-20)

Status: asked by the owner on 2026-11-20. Companion of `2026-11-20-subagents-design.md` (the
`subagent` tool, built on its own branch): a skill that dispatches subagents needs both.

## What it is

Owner, 2026-11-20: "I can install the skills I want; I work mainly with those [superpowers' SDD,
ponytail], so I expect Clio to be able to use them." Clio ships none of them; it installs any.

- **From where.** A folder that holds a skill (`SKILL.md`), a folder that holds several
  (`skills/*/SKILL.md`, `.claude/skills/*/`, `plugins/*/skills/*/`: the layouts of a Claude Code
  plugin), or a GitHub repository (`owner/repo`, optionally `@branch-or-tag`, or its https URL).
  A ref is a branch or a tag of that repository, never a commit hash (GitHub serves a fork's
  commit through the parent's name): the archive is asked for as `refs/heads/<ref>`, then
  `refs/tags/<ref>`, and with no ref the default branch; a hex ref that is neither is refused
  ("name a branch or a tag"). It is fetched over https only, at most 20 MiB, and read in memory
  with the checks the installer's downloads have (nothing of it runs or is unpacked to disk). The
  record keeps where it came from and the commit named in the archive's own header.
- **Who.** `clio skills add <source> [name…]` at a terminal (a folder only this way: the model
  can write folders), and in the chat the person just asks: the model proposes a GitHub repository
  with `skill_add {source, names?, reason}`, which sets each skill aside as class `skill`. The box
  shows each skill (name, description, files, sizes, scripts, origin, ref and commit, what the
  judge said, what of a plugin is not installed), where to read exactly what would be installed
  (`https://github.com/<owner>/<repo>/tree/<commit>/<folder>`) and, when the conversation read
  outside text, that it did; the yes is to that repository at that commit. The person's yes
  installs it. Adding a skill from outside always asks: no permission window and no bypass
  answers it, since a skill is text the model will read as Clio's own. `clio skills add` prints
  the same and asks only at a terminal (piped input adds nothing unless `--yes`); `--show` prints
  every file of the skills and adds nothing.
- **Checks.** Every file is read by the judge before the box (long text in batches, a minute in
  all; past it, not judged); a flag is shown in the box in plain words and the default answer is
  no, and so is a skill the judge could not read. No reviewer run: the person chose this text.
  Every file goes through the vault's filter, as `skill_write`'s do: a secret's shape refuses the
  skill. The front matter must read the same to Clio and to Pi's YAML parser: plain `key: value`
  lines, `name` and `description` plain, quoted on one line, or a `>`/`|` block of one indent
  (as ponytail writes them); anything else refuses the skill, and a `name` given must be exactly
  the name Clio registers. Names are Clio's (lowercase letters, digits, `-`); a name already
  taken by another origin is refused with a line saying so; the same origin again is a new version
  (`clio skills undo` goes back). A person-added skill may be larger than a model-written one
  (SDD has 15 files): up to 100 files and 1 MiB; `skill_write`'s limits stay. Only skills are
  taken from a plugin, or from a repository that is one skill: its hooks, commands and agents are
  not installed, and the summary says so. A folder is read from a handle on its top folder, no
  link followed even if one is swapped in while it is read.
- **Skills written for another harness.** They name tools Clio does not have. The system prompt
  gains a short map, in a new prompt version: a Task or Agent tool is `subagent`; a todo tool is
  `todo_write`; a Skill tool means reading that skill's `SKILL.md`; Read, Write, Edit and Bash are
  Clio's `read`, `write`, `edit` and `bash`; Grep (searching inside files) is `bash` with `grep`
  or `rg`; Glob (finding files by name) is `find`; `plugin:name` is the skill `name`. No skill's
  text is rewritten.
- A child reads the installed skills too, which is what SDD's implementer and reviewers need.


## Verified for real before merging

In an isolated home: `clio skills add obra/superpowers` and `clio skills add DietrichGebert/ponytail`
install their skills (the box, the judge's line, the record with the commit), `clio skills` lists
them, a second add of the same origin is a new version and `undo` goes back, a name taken by
another origin is refused, and a folder with a planted injection shows the judge's flag with "no"
as the default; a tag and a branch ref install, a commit hash is refused, `--show` adds nothing
and piped input is refused.

## Not included

- Shipping third-party skills inside Clio.
- Repositories outside GitHub (clone it and add the folder).
- A plugin's hooks, commands and agents.
- Updating installed skills by themselves.

## Version

0.4.0.
