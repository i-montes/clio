# Skills installed whole: instructions and resources (design, 2026-11-22)

Status: asked by the owner on 2026-11-22 ("I want installing a skill to work as it does in Claude
Code, Codex, or any other agent"). It changes `2026-11-20-skills-add-design.md` and
`2026-11-20-shared-skills-design.md`; the rules are S-05, S-68 and S-74 (no new rule).

## The problem

`clio skills add https://github.com/Leonxlnx/taste-skill` failed: the archive is 28.9 MiB (over
the 20 MiB cap, and the error could not tell that from "no such repository" or "no network"),
the skill is 1 631 files and 34.9 MiB (over the 100 files and 1 MiB an added skill could hold),
and its images and templates would have been skipped as "not text". Clio read every file of a
skill as her own text, so every file had to be judged and shown; other agents copy the folder:
`SKILL.md` is the instructions, `references/` documents read on demand, `scripts/` programs,
`assets/` files those use.

## What changes

A skill is its instructions; the rest of its folder are resources.

- **The whole folder is installed**, byte for byte, in Clio's skills store as before (one writer,
  every version kept, `clio skills undo`). The digest of a version covers every file: the
  instructions as before, then, when there are resources, a separator no path can spell
  (`\0resources`) and each resource's path and SHA-256. A skill without resources has exactly
  the digest it had before.
- **The rule.** An added skill's file is an *instruction* when it is `SKILL.md`, or its name ends
  in `.md`, `.markdown` or `.txt`, it is UTF-8 text with no character the box would not show as
  written, and it fits: instructions together hold at most 100 files and 1 MiB (`SKILL.md`
  first, then by path). Everything else is a *resource*: scripts, images, data, templates, and
  documents past that limit (stored rather than refusing the skill). Decided by Clio from the
  name and the bytes, never from the skill's front matter or anything it says. The measured
  skill has 28 instruction files (470 KiB) and 1 603 resources.
- **The record says which is which.** Each version of an added skill keeps `instructions`, the
  list of its instruction paths. The tool host serves a file of a registered, enabled skill as
  Clio's own text (unwrapped, not marking the session, S-05) only when its path is in that list;
  every other file of it (a resource, or anything not listed) is outside text: wrapped as data,
  it marks the session (`foreign`) and goes through the judge path, as a foreign file does. A
  version written before this change has no list: every file of it keeps the treatment it was
  approved under (every file was judged and shown then). Listing a skill's folder stays
  unmarked: the judge read every name (below).
- **Instructions keep their treatment.** Checked (front matter, names, characters, the vault's
  filter), read by the judge at install with the resources' names, shown to the person, then
  read as Clio's own text.
- **Resources** are stored as they came and never read as Clio's own text. Their checks: no link,
  paths inside the skill with plain names (letters, digits, `. _ -`, none starting with a dot),
  at most 8 folders deep for an added skill (the measured skill goes 5 deep; `skill_write`
  keeps 4), not given twice, the vault's secret-shape filter on every resource that is text, and
  the limits below. A script (`#!` first) is stored 0700, the rest 0600, as before. When one is
  run it is a command like any other: same classification as the line typed, same jail, same
  walls (S-75/S-76 hide the person's credentials from it). Nothing of a skill is ever run by
  installing it.

## Limits, and why

| What | Limit | Why |
|---|---|---|
| Instructions of a skill | 100 files, 1 MiB | What the judge reads and the person reads in the box (as before); 2 MiB of judged text an add stays. The measured skill uses 28 files, 470 KiB. |
| A whole skill | 4 000 files, 64 MiB | About 2.5× the files and 1.8× the bytes of the largest real skill measured (1 631 files, 34.9 MiB); held once in memory while read, written once per version kept, staged until the yes. |
| Kept of an archive, or read of a folder | 20 000 entries, 128 MiB | What is held in memory while the skills are found: the skills' folders only (the rest of a repository is skipped unread, its top-level files by name only), so a large repository around a skill does not count. |
| The download | 256 MiB compressed | On disk in Clio's staging folder, removed at once; never read whole into memory. |
| The archive unpacked | 2 GiB, 100 000 entries | Streamed through `gzip -dc` twice (once for the names, once for the skills' bytes), so only time is spent on what is not kept. |
| A resource read with a file tool | 2 MiB | As any file read (`MAX_RAW_BYTES`). |

Every limit's error says which limit, what was found and by how much it is over.

## The archive and its errors

curl's answer is read: a 404 is "no such repository, branch or tag (or it is private)", an
abort for size is "larger than 256 MiB, the most Clio downloads" (GitHub does not send a size
before the archive, so the size is given when Clio has it), and anything else is "GitHub could
not be reached (curl's reason)". A network failure stops the add at once; a branch that is
missing is tried as a tag, as before.

## Spellings

`owner/repo`, `owner/repo@ref`, `https://github.com/owner/repo` (with `www.`, without the
scheme, with `.git` or a trailing `/`), and a link to one skill's folder,
`https://github.com/owner/repo/tree/<ref>/<folder>` (or `…/blob/<ref>/<folder>/SKILL.md`): the
ref is the segment after `tree/` (a branch with a `/` in it is given as `owner/repo@branch`),
and only that folder is read. `git@github.com:…`, `http://`, another host, or a link to a file
other than `SKILL.md` fail with a sentence that says what to type.

## Staging

What is shown is what is registered: the instructions travel in the item (as before); the
resources are written, each once and named by its SHA-256, to `<staging>/skill-<digest>/` (the
tool host's run folder for `skill_add`, Clio's data for `clio skills add` and the shared folder's
look). The person's `clio` registers only bytes whose SHA-256 and size are the ones the item
lists, and only if the digest over everything is the item's; a missing or changed file refuses
the yes ("add it again"). The staged folder is removed after registration, and any staged folder
older than two days (an item waits one) is removed when skills are next found.

## The person's box

Instructions are listed as before, with sizes. Resources are summarised: how many and how large,
by folder and by kind (images, svg, scripts, web pages, styles, data, documents, fonts, other),
and the scripts named (those outside `assets/` first, twelve, then "and N more"). It says plainly
that the judge read the instructions and the resources' names, not the resources, that Clio
reads a resource only as outside text, and that a script runs as any command does, in the jail.
One yes per skill, as before. `clio skills add … --show` prints every instruction whole and every
resource by name and size. The same box serves `clio skills add <github>` and `<folder>`, the
model's `skill_add` and the shared folder.

## The shared folder (S-74)

Read with the skill limits above (4 000 files, 64 MiB). A changed skill shows each changed
instruction with its line diff, and each added, removed or changed resource by name and size
(30 lines, then "and N more resources changed"), never a diff of their bytes. A skill registered
before this change whose folder is unchanged (no file the old reading would have skipped) is
known by its old digest and is not asked again.

## Decisions taken without the owner

- The tier is by kind (markdown and plain text) and size, not by folder: a `README.md` under
  `assets/` is an instruction (judged and shown); a `.sh` under `references/` is a resource.
- The judge reads the resources' names (not their bytes), so a listing of a skill's folder stays
  Clio's own text and no new mark comes from ordinary use.
- An item set aside before this change, whose files include a non-document, is refused at the yes
  ("add it again"), because a resource is never registered as an instruction.
- No executable bit is taken from the archive: `#!` decides, as before.
- The model's tool description is unchanged (it may pass a folder link; Clio reads it).
- The vault's filter refuses a resource only when it masks something in it. Found in the real
  run: taste-skill's `scripts/start.mjs` holds a BOM inside a regular expression; the display
  cleaning removes it, and the old check (any change refuses) took that for a secret. An
  instruction still has to come through unchanged, since it is shown and read as Clio's own.

## Tests (each fails without the code it guards)

1. Installs whole: `a_large_real_skill_installs_whole_from_an_archive_and_from_a_folder` (brain:
   archive and folder, byte for byte, digest, a changed image is another digest),
   `a_large_real_skill_from_github_is_set_aside_whole` (the model's `skill_add`),
   `a_changed_resource_asks_again_and_a_skill_taken_before_resources_does_not` (shared folder).
2. A resource is outside text: `a_skills_resource_reads_as_outside_text_and_its_instructions_as_clios_own`
   (attack 69: wrapped, marked, judged and withheld; listed instructions unmarked; a record
   without the list reads as before); the same words in an instruction are refused at the add
   (`skill_add_sets_each_skill_aside_judged_and_writes_nothing`).
3. Nothing turns a resource into an instruction: `an_added_skill_keeps_its_origin_and_its_name`
   (a script among the instructions, a kept resource gone or changed: nothing added),
   `only_a_listed_instruction_is_clios_own_text`, `instructions_and_resources_are_told_apart_by_clios_rule`.
4. Limits and errors: `an_added_skills_limits_say_what_was_over_and_by_how_much`,
   `a_folder_past_its_limit_says_by_how_much`, `the_limits_are_on_what_is_kept_not_on_the_repository_around_it`,
   `a_missing_a_too_large_and_an_unreachable_repository_each_say_so`.
5. What was there keeps working: the old digest (`the_digest_covers_every_resource_and_keeps_the_old_one_without_them`,
   the shared-folder test above), the legacy record (test of 2).
6. Only the person's yes: the existing S-68 and S-74 tests, unchanged, and the box rows
   (`the_terminal_add_shows_the_judge_and_adds_only_on_a_yes`).

## Not included

Running skills differently, a registry, `npx`-style installers, a repository's `install.sh`,
plugins' hooks, commands and agents (still named, never installed), any change to how the judge
decides.

## Version

Not bumped here.
