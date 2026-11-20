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

- **When a chat opens**, Clio reads `~/.agents/skills` (every folder with a `SKILL.md`, found as
  Pi finds them) and compares each skill, by a digest of its files, with the copy she registered
  from that folder.
  - **Unchanged since the person's last yes:** nothing to do; her registered copy is already
    loaded.
  - **New or changed:** the judge reads it (the paced judging of an added skill), and one box
    lists all of them: for each, what it is for, its files and sizes, whether it is new or what
    changed (files added, removed, changed), and the judge's word. One yes adds or updates every
    one the judge read clean. One the judge flagged, or could not read, is listed apart with why
    and is not part of that yes: `clio skills add ~/.agents/skills/<name>` adds it on its own.
  - **Gone from the folder:** Clio turns her copy off and says so in a quiet note; `clio skills
    on` brings it back.
- What Clio loads is always her own registered copy (`skills/live`), never the folder itself: what
  the person approved is what the model reads, and a later edit of the folder changes nothing
  until the next yes.
- The record keeps the origin (`~/.agents/skills/<name>`) and the digest. A name already taken
  by a skill of another origin (shipped, Clio's own, from GitHub) keeps its owner; the shared one
  is refused with a line saying so.
- The box is the chat's approval box, with its timing; no permission window and no bypass
  answers it (as for any skill from outside). `no` adds nothing and is not asked again for the
  same content: the note says how to add it later.
- `clio skills sync` does the same at a terminal, asking `[y/N]` once.
- Only a person's chat looks at the folder. A `clio run`, a round, a review and a subagent use
  what is registered.
- Limits: a person-added skill's (100 files, 1 MiB each), and the judged text of one look is
  capped as an add's is; past it, the rest is listed as "not read yet: add it by name".
- `skills.shared` in the config (default `true`) turns the look off.

## The folder is the person's

In host mode the model's commands can write in the home. A fooled model must not be able to
plant a skill there for the next session, so:

- a command or a file tool that writes under `~/.agents/` is `control`: it always asks the
  person, with or without a permission window or bypass, and never carries the "machine" mark;
- and even then nothing loads without the box above.

## Not included

- `.agents/skills` of a project (it arrives with a repository, so it may be someone else's
  text): later, as its own decision.
- `AGENTS.md`.
- Writing to the shared folder from Clio (her own skills stay hers).

## Version

0.5.0.
