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
  It is not offered, and its yes turns nothing on, with the judge off or in a conversation the
  judge marked `suspect`.
- `/bypass off` turns it off and shows the summary of what Clio approved by herself under it that
  the person had not been shown, and counts what earlier ones approved that was not shown either.
- `/bypass` says whether it is on, since when, and lists every bypass recorded, the newest first,
  each with what was approved under it (the newest 30 acts in all; `clio bypass` lists them all),
  with the date and time of each. Only the acts shown count as looked at.
- From a terminal: `clio bypass` (the state and the list) and `clio bypass off`. There is no
  `clio bypass on`: a command can be typed by anything; the box is the person.
- The status line shows `bypass on` while it is, in the colour of a warning. When a chat opens, one
  quiet line says how many acts Clio approved by herself under any bypass that the person has not
  looked at; when the judge turned it off, that line says so, once, with the count.

## What it answers

Everything a permission window answers, and also what a window never does: `control` acts on the
machine that make no new code run outside the jail (turning the machine off or restarting it; who
may use it: users, passwords, `sudoers`; a pattern of the person's own `host.json`), a new
service or a new version of one,
installing or updating an MCP server, `on` of what the person turned off, `remove` of a shipped
skill, service `undo`, a changed MCP tool, and what waited from before it was turned on.

Acting on units and services that already exist (start, stop, restart, enable, disable or status
of what is not Clio's) is not set aside at all, bypass or not, when it cannot load anything new:
`systemctl` on the system's units, `systemctl --user` with `stop`, `disable` or `status`. The
`--user` verbs that may load or start something new always ask (below).

A `clio run` is covered: it is the person's own request. A round is not covered because nobody
asked for that session; a `clio run` is the person's.

## What it never answers

These keep their box, with bypass on:

1. **What would change Clio's own guards.** An act on Clio's own folders (config, data, the vault,
   her programs, the bypass and window files), her own commands (`clio …`), and keys. Bypass is the
   person's yes to work on the machine, not to let the model rewrite who watches it: the judge is
   the only brake left under bypass, and a fooled model must not be able to turn it off.
2. **What creates or schedules new code outside the jail.** The jail hides Clio's folders from a
   command; what runs outside it does not have them hidden. So never: a transient unit
   (`systemd-run`), a call to the service manager over D-Bus (`busctl`, `gdbus`, `dbus-send`:
   `StartTransientUnit`, `StartUnit`), a scheduler (`crontab`, `at`, `batch`), a terminal
   multiplexer's server (`tmux`, `screen`), the person's own service manager loading or starting
   what may be new (`systemctl --user` with `start`, `restart`, `enable`, `link`, `edit`,
   `daemon-reload`, `set-property`, `set-environment`, `import-environment` and the like), and
   writing a unit file (theirs, `~/.config/systemd/`, `~/.local/share/systemd/`, or the
   system's), `environment.d`, a shell start-up file (`.bashrc`, `.bash_profile`, `.bash_login`,
   `.profile`, `.zshrc`, `.zprofile`, `.zshenv`) or Clio's programs in `~/.local/bin`, whatever
   the file says and however Clio is spelled in it, or not at all.
3. **A change to the person's blocks from a marked session** (`tainted`, `suspect`): those blocks
   are the person's words in every later conversation, of Clio and of the person's other agents.
4. **Anything in a session the judge marked `suspect`**, as today, **or in one whose judge did not
   answer** for text it read: the text still reaches the model, marked as data, a note says once
   that from then on nothing in that conversation runs under bypass or a window, and its items ask.
   Bypass stays on for the other conversations.
5. **The round**: it reads only, by code, and nothing covers it.
6. What needs the person by nature: a secret to type (`request_secret`), a question
   (`ask_person`).

What is refused outright today stays refused: bypass turns a wait into a yes, never a no into a yes.

## What turns it off

- The person (`/bypass off`, `clio bypass off`).
- The judge flagging an injection in any conversation: bypass turns off at once, as a window
  closes, and a notice says why when a chat next opens. The person turns it on again.
- The judge being off: bypass is not turned on, and an act that finds the judge off turns it off
  (the window's rule).

A restart of the machine does not turn it off, and neither does the wall clock: bypass has no end
to reach, and a clock set back before it was turned on neither turns it off nor keeps it from
closing. A close whose file cannot be written removes the file: bypass is then off.

## How it is kept

Its own file next to the window's (`<data>/host/bypass.json`: id, when, by which box, closed when
and why) and its own record (`<data>/host/bypass/<id>.jsonl`), written as a window's is: only by
the person's `clio`, never by anything the jail reaches; an act is recorded before it runs and its
outcome after. A missing or damaged file is "off", and so is a file whose record does not start
with this same bypass (a file written by hand, with no record or another's): such a file is off in
the status line, in the model's envelope and in the tool host, and nothing is recorded under it.
The one test both the tool host and the chat apply is a function beside `grant::may_answer`. While bypass is on no permission window opens (a
yes in a box is just that item's yes), and an open window is left to end by itself.

## The model

The prompts' approval passages say that with bypass on Clio does not wait: she acts, says what she
did, and still asks for what the list above keeps; and that she never asks the person to turn
bypass on. New prompt versions (`clio-agent`, `clio-host`), only those sentences changed. Whether bypass is on is told to the model with the session's other
state, not by the model's own reading of files.

## What reaches Clio's files from outside the jail, with bypass on or off

Hardening that bypass made pressing, for every session: what makes code run as the owner outside
the jail asks, with bypass off too (the `systemctl --user` verbs above, a command that writes the
unit folders, a shell start-up file, `environment.d` or Clio's programs, the write and edit tools
on the same paths, and `tmux` or `screen`). In host mode a command's jail shows Clio's programs in
`~/.local/bin` read-only and hides the terminal multiplexers' sockets (`/tmp/tmux-<uid>`,
`/run/screen/S-<user>`, the folders `$TMUX` and `$SCREENDIR` name), like Clio's folders. No file
tool and no command the list recognises reaches Clio's files; what remains is in the threat model.

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
