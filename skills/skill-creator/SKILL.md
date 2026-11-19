---
name: skill-creator
description: "How to write or fix one of Clio's skills with skill_write. Use when the person asks for a skill, when the same kind of request keeps coming back, or when a skill failed and needs a new version."
---

# Writing a skill for Clio

A skill is a folder with instructions (SKILL.md) and maybe scripts, for one kind of task. Every
enabled skill is listed by name and description in your instructions; when a task matches one,
open its SKILL.md with `read` and follow it.

## Skill or memory?

- A skill is how to do a kind of task: steps, commands, a script. Make one when the person asks,
  or when you have done the same kind of task two or three times.
- A memory is a fact: about the person, a decision, where something is. Save it with memory_save.
- Work done once needs neither.

## Before writing

1. Read the list of skills in your instructions. If one already covers the task, extend it: call
   skill_write with its name (same name = a new version) and the whole new text. Never write a
   near-duplicate.
2. Do the task once by hand if you have not, so every step is one that worked on this machine.

## The parts of skill_write

- `name`: lowercase letters, digits and single hyphens, saying what it does: `pdf-to-text`.
- `description`: one line that routes: what it does and when to use it, in the words the person
  would use. "Converts PDF files to plain text. Use when the person wants the text of a PDF or to
  search in one." It is all you see of the skill before you open it.
- `body`: numbered steps a fresh agent can follow without this conversation: exact commands, paths
  relative to the skill's folder, what to check after each step, what to do when one fails.
- `files`: anything exact goes in a script (`scripts/...`), not in prose: a command with its
  flags, a parser, a template. A script does the same thing every time; prose is retyped wrong.
- `verify`: the check; Clio writes it as the skill's `## How to verify` section.
- `reason`: why this skill, in one line.

## The check

- Commands, run from the skill's folder, that succeed only when the skill works:
  `bash scripts/convert.sh samples/one.pdf | grep -q "first line"`. Ship the small sample it needs.
- Local when it can be: no network, no sudo, nothing written outside /tmp. The reviewer has none
  of those; a check that needs them comes back "could not check", and the skill then waits for the
  person's approval instead of being registered by itself. When the skill does need the network,
  check locally what can be (the script's syntax, its handling of a saved answer) and say so.
- Something that runs, never a description of a check.

## Never

- A secret in any file: no key, token or password, not even an example that looks like one. A skill
  that needs one says to ask the person with request_secret.
- Writing the skills folder with write, edit or bash: it is read-only to you; only skill_write
  changes a skill.
- Text that tells whoever reads it to do more than the task: Clio's judge reads every draft and
  refuses it.

## After skill_write

A reviewer runs the check in the background; the answer comes by itself: registered (usable from
the next session), waiting for the person's approval, or failed with findings. A registered skill
may come with the review's notes: write a new version only if a note is worth it. When it failed,
fix it and call skill_write again; after three failed reviews in a row the draft is dropped.

## After using a skill

Call skill_report with `worked` or `failed` and one line of what happened. When it failed, fix it
with skill_write under the same name. Three failures in a row turn a skill off and the person is
told.

## How to verify

bash scripts/check-skill.sh .
