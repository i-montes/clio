# Bypass (design, 2026-11-20)

Status: asked by the owner on 2026-11-20 ("a `/bypass on` command that allows a bypass mode with
no end: it would no longer need approval to work"). It extends the permission window
(`2026-11-18-clio-creates-design.md`, "the permission window", and `crates/clio-toolhost/src/grant.rs`).

## What it is

With bypass on, Clio answers by herself what would wait for the person's yes, records it, and goes
on. It has no end: it lasts until the person turns it off, across restarts of the machine. It is
the person's standing yes; the permission window (a yes worth 4 hours) stays as it is for when
bypass is off.

## What the person does

- `/bypass on` in the chat. One box says what it means ("Clio will approve by herself everything
  that asks you, with no end, until `/bypass off`. It turns itself off if the judge flags an
  injection.") and takes the yes with the approval timing (`y` then Enter, nothing accepted in the
  first second, a paste or a click never answers). Only this box turns it on.
- `/bypass off` turns it off and shows the summary of what Clio approved by herself since it was
  turned on or since the last summary shown.
- `/bypass` says whether it is on, since when, and lists what was approved under it.
- From a terminal: `clio bypass` (the state and the list) and `clio bypass off`. There is no
  `clio bypass on`: a command can be typed by anything; the box is the person.
- The status line shows `bypass on` while it is, in the colour of a warning. When a chat opens with
  bypass on, one quiet line says how many acts Clio approved by herself since the person last
  looked.

## What it answers

Everything a permission window answers, and also what a window never does: `control` acts on the
machine (the service manager, units, schedulers, `sudo`), a new service or a new version of one,
installing or updating an MCP server, `on` of what the person turned off, `remove` of a shipped
skill, service `undo`, a changed MCP tool, and what waited from before it was turned on.

## What it never answers

These keep their box, with bypass on:

1. **What would change Clio's own guards.** An act on Clio's own folders (config, data, the vault,
   her programs, the bypass and window files), her own commands (`clio …`), and keys. Bypass is the
   person's yes to work on the machine, not to let the model rewrite who watches it: the judge is
   the only brake left under bypass, and a fooled model must not be able to turn it off.
2. **A change to the person's blocks from a marked session** (`tainted`, `suspect`): those blocks
   are the person's words in every later conversation, of Clio and of the person's other agents.
3. **Anything in a session the judge marked `suspect`**, as today.
4. **The round**: it reads only, by code, and nothing covers it.
5. What needs the person by nature: a secret to type (`request_secret`), a question
   (`ask_person`).

What is refused outright today stays refused: bypass turns a wait into a yes, never a no into a yes.

## What turns it off

- The person (`/bypass off`, `clio bypass off`).
- The judge flagging an injection in any conversation: bypass turns off at once, as a window
  closes, and a notice says why when a chat next opens. The person turns it on again.
- The judge being off: bypass is not turned on, and an act that finds the judge off turns it off
  (the window's rule).

A restart of the machine does not turn it off.

## How it is kept

Its own file next to the window's (`<data>/host/bypass.json`: id, when, by which box, closed when
and why) and its own record (`<data>/host/bypass/<id>.jsonl`), written as a window's is: only by
the person's `clio`, never by anything the jail reaches; an act is recorded before it runs and its
outcome after. A missing or damaged file is "off". The one test both the tool host and the chat
apply is a function beside `grant::may_answer`. While bypass is on no permission window opens (a
yes in a box is just that item's yes), and an open window is left to end by itself.

## The model

The prompts' approval passages say that with bypass on Clio does not wait: she acts, says what she
did, and still asks for what the list above keeps. New prompt versions (`clio-agent`, `clio-host`),
only those sentences changed. Whether bypass is on is told to the model with the session's other
state, not by the model's own reading of files.

## Security rule

One new rule in `docs/security.md` after S-71, with its coverage, saying what this section says in
one paragraph, and the threat-model list gains "the person turned approvals off" with the brake
that remains (the judge) and what a fooled model can do under it (anything on the machine that
does not need Clio's own folders: delete, send, spend, install).

## Not included

- A bypass per conversation or with a time limit: the window is the time-limited form.
- Bypass for the round.
- A way for the model to ask for bypass.

## Version

0.3.0.
