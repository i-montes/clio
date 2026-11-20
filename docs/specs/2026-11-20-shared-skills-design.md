# Skills from the shared folder (design, 2026-11-20)

Status: asked by the owner on 2026-11-20 ("not the ones loaded for Claude Code: the ones in the
generic place that every other agent reads when it starts… if there are any in that folder it
looks at them and loads them, passing through the judge"). It builds on
`2026-11-20-skills-add-design.md`.

## What it is

`~/.agents/skills/` is the Agent Skills location other agents read at start (Pi reads it too; Clio
starts Pi with discovery off). Clio now looks at that folder and loads what the person keeps
there, without loading anything the person has not said yes to.

## What happens

- **When a chat opens**, beside it (the chat never waits for it), Clio reads `~/.agents/skills`
  (every folder with a `SKILL.md`, found as Pi finds them) and compares each skill, by a digest
  of its files, with the copy she registered from that folder. The box is offered once the chat
  is idle.
  - **One skill at a time:** each folder at the top is read on its own, with a person-added
    skill's limits checked before anything is read (100 files, 1 MiB, 16 deep, no link
    followed); `.git`, `node_modules`, `venv`, `.venv` and `__pycache__` are skipped, the same
    as `clio skills add` skips them. One that cannot be read (or is past a limit) is said once
    with its real reason and counts as there, never as gone; it never stops the others.
  - **Unchanged since the person's last yes:** nothing to do; her registered copy is already
    loaded.
  - **New or changed:** the judge reads it (the paced judging of an added skill), and one box
    lists all of them: for each, what it is for, its files and sizes, and the judge's word. A new
    one says when its newest file was written; a changed one shows each changed file's size and
    time written, and a line diff of each changed text file against the copy of the last yes
    (unified, about 60 lines per skill, then "and N more lines"). One yes adds or updates every
    one the judge read clean. One the judge flagged, or could not read, is listed apart with why
    and is not part of that yes: `clio skills add ~/.agents/skills/<name>` adds it on its own.
  - **Gone from the folder:** Clio turns her copy off and says so in a quiet note; `clio skills
    on` brings it back. Back unchanged, it is on again by itself, but only if Clio turned it off
    for being gone: one the person turned off stays off. The model's `skill_manage on` for a
    skill off because it is gone always asks the person. Each of these is decided under the
    skills lock, against the record as it is then.
  - **The judge's budget:** a text already waiting in the box (judged clean) is recognised
    first: not judged again and not counted toward the cap. One the judge did not answer for is
    not judged again that day. A later yes forgets a remembered no; a "name taken" note is
    said once while that holder keeps the name, and is asked once the holder is gone.
- What Clio loads is always her own registered copy (`skills/live`), never the folder itself: what
  the person approved is what the model reads, and a later edit of the folder changes nothing
  until the next yes.
- The record keeps the origin (`~/.agents/skills/<name>`, each skill its own, also when
  `clio skills add ~/.agents/skills` or a group folder adds several) and the digest. A name already taken
  by a skill of another origin (shipped, Clio's own, from GitHub) keeps its owner; the shared one
  is refused with a line saying so.
- The box is the chat's approval box, with its timing; no permission window and no bypass
  answers it (as for any skill from outside). `no` adds nothing and is not asked again for the
  same content: the note says how to add it later.
- `clio skills sync` does the same at a terminal (with the same diffs and times), asking `[y/N]`
  once. A look takes a lock in Clio's data, so two processes never look at once; a chat whose
  look finds it held skips that look, and nothing of the box is offered while one runs.
- The folder's name never reaches the model as it is: what the model reads is its cleaned form.
- Only a person's chat looks at the folder. A `clio run`, a round, a review and a subagent use
  what is registered.
- Limits: a person-added skill's (100 files, 1 MiB each), and the judged text of one look is
  capped as an add's is; past it, the rest is listed as "not read yet: add it by name".
- `skills.shared` in the config (default `true`) turns the look off.

## The folder is the person's

In host mode the model's commands can write in the home. A fooled model must not be able to
plant a skill there for the next session (the folder is read by every other agent too), so:

- **the jail is the wall:** every host-mode command, background jobs included, sees `~/.agents`
  read-only (bwrap and Systemd alike). Only a command the person approved, as `control`, that
  names that folder runs with it writable;
- a host-mode session creates `~/.agents/skills` (0700, empty) when it starts, so the read-only
  view has something to cover. It is the standard folder; an empty one changes nothing for any
  other agent;
- the classifier decides what is asked: a command or a file tool (`fs_mkdir` too) that names
  `~/.agents/` in any of its cheap spellings (`~`, `$HOME`, `${HOME…}`, `~user`, the absolute
  path, `$PWD`, `$(pwd)`, `/proc/self/cwd`, `..`, a bare `.agents` when the command runs in the
  home) is `control`: it always asks the person, with or without a permission window or bypass,
  and never carries the "machine" mark. A spelling it misses (a copy into `~/`, `tar -C ~`, a
  script) is still stopped by the jail;
- and even then nothing loads without the box above.
- Under `sudo`, or in Systemd mode run by root, the person's root commands are outside this:
  that is inherent, as for every file of the home.

## Not included

- `.agents/skills` of a project (it arrives with a repository, so it may be someone else's
  text): later, as its own decision.
- `AGENTS.md`.
- Writing to the shared folder from Clio (her own skills stay hers).

## Version

0.5.0.
