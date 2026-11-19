# One continuous conversation (design, 2026-11-18)

Status: stage 1 approved by the owner on 2026-11-18 (cap 200,000, `/compact N`); stage 2 waits.

## What the owner asked for

One conversation that never has to be restarted: `clio` opens where the last chat ended, and Clio
keeps it affordable behind the scenes. Keep the prompt-cache hit rate high. `/new` stays.

## What Gentle Dot does (studied, not copied)

Source: `Gentleman-Programming/gentle-dot` (`packages/daemon/src/rotation.ts`, `conversations.ts`,
`docs/design.md` §7) and its engine `gentle-pi`, which runs on Pi like Clio.

- The person sees one chat. Behind it is a chain of Pi session files; the last one is open.
- Pi's own auto-compaction bounds the context: near the model's window it summarises the old part
  and keeps the last ~20,000 tokens.
- Rotation is theirs: when the open file passes 20 MB or holds 10 compactions, and the agent is idle,
  the daemon writes a new session file whose only entry is a hidden note (the latest compaction
  summary, which Pi already wrote, so no extra model call) and switches Pi to it. If Pi does not load
  it, it falls back to a plain new session. A failure leaves the chat where it was.
- History is read from the files of the chain, with an "earlier messages" divider.
- Detail lost by the summaries is covered by the memory (Engram).
- Cache: they have no cache code. The 93–97 % they show is the provider's prompt cache as Pi uses
  it, measured as cache-read tokens over all prompt tokens. It is high because the system prompt
  and tools never change, history only grows at the end, and compaction and rotation are rare.

## What Clio has today (checked in the code and on the server)

- Cache: the last 12 sessions on the server read 94.6–97.3 % from cache, except the first session
  after each reinstall (the prompt changed). Nothing to build; the design must not lower it.
- Pi 1.4.0 has auto-compaction on (reserve 16,384, keep 20,000) and the RPC commands `compact`,
  `new_session {parentSession}`, `switch_session`, `get_messages`; `crates/clio-brain/src/pi/rpc.rs`
  already wraps them.
- The `claude` connector declares a 1,000,000-token window for `claude-sonnet-5-5` (and the other
  measured models), so today Pi would only compact near one million tokens.
- After a compaction Clio already sends the identity again and asks the model for the session
  summary, which goes to clio-memory.
- `clio` opens a draft new session every time. A reopened session shows `get_messages`, which after
  a compaction is only the summary and the kept tail: the earlier messages are no longer on screen.
- A session is a run: one run folder (tool host, action log, undo, marks) and one Pi session.
- Per-session limits and marks assume short sessions: memory saves are capped per session (30 clean,
  5 once the session read outside content, 10 changes); the `tainted` and `suspect` marks last for
  the life of the session.

## Design

### 1. `clio` opens the last conversation

`clio` reopens the most recently used Clio session instead of a draft; with none, a draft as today.
`/new` (and `n` in the drawer) starts a new conversation; the drawer and `/sessions` stay as they
are. `clio --new` opens a draft directly. No new state file: "most recent" is what the drawer
already sorts by. Config `session.continuous` (default `true`) turns it off.
A conversation is held by one `clio` at a time (an advisory lock in its run folder): when the last
one is open in another terminal, `clio` opens a draft with a note instead of an older one, and the
drawer refuses it. Pi can write its session files, so Clio seals each (size and SHA-256 in the run
folder) when it opens and closes; a reopened file that does not match marks the session, with a note.

### 2. A cap on the context, checked between turns

Config `session.context_cap_tokens`, default `200000`. When a turn settles and nothing is waiting
(no run, no question, no approval), if the context Pi reports is over the cap, Clio sends `compact`.
The cap is never applied in the middle of a turn, so a long job is not cut; Pi's own compaction near
the window stays as the backstop. At most one automatic compaction per settled turn; if the context
is still over the cap afterwards, a note says so once (not again until the context is back under
the cap) and Clio waits for the next turn. Quitting does not wait for an automatic compaction (Pi is
stopped as on any close; the compaction is not recorded), and `Esc` stops one with Pi's `abort`.

The owner sets the cap from the chat: `/compact` alone compacts now, as today; `/compact N`, with N
a whole number of thousands of tokens (`/compact 300`), sets the cap and saves it in `config.json`;
any other text stays the instructions for the summary. `/compress` is an alias. `/context` shows
the cap.

Why a cap below the window: every message sends the whole context again. Cached, a read costs a
tenth of normal input, so each message at 600,000 tokens weighs like 60,000; and when the cache has
expired (minutes to an hour without writing) the next message rewrites all of it. At 200,000 both
costs are a third. What leaves the context is still in the summary, in clio-memory and on screen.

Cost of a compaction to the cache: one rewrite of a small context (summary + ~20,000 tokens). The
system prompt and tools stay byte-identical, so they keep reading from cache.

Rejected: setting Pi's `reserveTokens` to "window minus cap" so Pi compacts by itself. It depends on
each model's window (a 200K model would compact on every turn) and it would also cut mid-turn.

### 3. History comes from the files

The chat shows the messages recorded in the conversation's session files, oldest first, not
`get_messages`. It loads the last page (about 100 turns) and an earlier page when the person scrolls
to the top. A quiet line marks where the earlier part was summarised. Clio only reads Pi's session
format (`type: "message"` entries); it never writes it. A file over 128 MiB is not read (Pi's copy
is shown instead), a line over 512 KiB is skipped, and a page holds at most 5,000 entries.

### 4. Rotation (second stage)

When the open Pi session file holds `session.rotate_compactions` compactions (default 10) or passes
20 MB, and the chat is idle: Clio sends `new_session` with `parentSession`, inside the same run, and
records the chain (ordered list of Pi session files) in the run folder. The handoff is the latest
compaction summary of the old file; it travels in Clio's own envelope (`<clio-context>`) with the
first message of the new session, the same way the identity already does. If `new_session` fails,
the chat stays in the old session and a note says so.

Difference from Gentle Dot: they write a Pi session file by hand with a hidden entry. Clio uses its
envelope, so it does not depend on Pi's file format for writing.

What rotation buys is small: it keeps the open file short so Pi opens it fast. It does not improve
what the model remembers (the handoff is the same summary). That is why it is the second stage.

### 5. Limits and marks in a conversation that does not end

This is the part Gentle Dot does not have, and it needs the owner's decision.

- **Memory budgets.** The per-session caps (30 saves, 5 once tainted, 10 changes) would run out
  for good. Decision: the same numbers count over a rolling hour instead of the whole session. Not
  "start again at each compaction": Pi reports compactions, and Pi is inside the jail, so a fooled
  Pi could reset its own budget; the tool host's clock cannot be moved from there.
- **`tainted`** (the session read outside content). In one endless chat it is almost always set:
  in copy mode any file read or command output sets it; in host mode reading a foreign folder, a
  command with the network, one that reaches out or reads logs; in both, a memory that is not the
  person's, a judged tool result (`suspect`) and a mark restored from the run folder. Until now a tainted session's
  `memory_block_edit` was refused ("open a new session"). The owner hit this on 2026-11-18: after a
  disk clean-up he asked Clio to keep watching the disk, and the change to his `vigilancia` block
  was refused because the session had read the output of `df` and `docker`. Decision: in a marked
  session (`tainted` or `suspect`) a change to the person's blocks is **set aside for the person's
  approval** instead of refused. The approval box shows the block, the action and the exact text;
  approved, it is written with the person's origin, exactly as shown, once; rejected or expired,
  nothing is written. A clean session writes as before, without asking. The protection is the
  same one S-05 wanted: text from outside cannot become the person's words unless the person reads
  those words and approves them.
- **`suspect`** (the judge withheld an injection). Unchanged: it lasts until `/new`, and its
  networked commands wait for approval. The approval box for a block change says so when set.
- **Memory read budget** (20 calls / 24 KiB per session) counts over a rolling hour too.

## Stages

1. Sections 1, 2, 3 and 5: the continuous chat, the cap, history from the files, rolling budgets;
   and `/usage` shows the cache hit rate (cache-read tokens over all prompt tokens).
2. Section 4: rotation and the chain of files.

## Not included

- Keeping the cache warm between messages (Pi's `cacheWarming: "idle"` and `PI_CACHE_RETENTION`):
  not verified that they apply to the `claude` connector, which runs Claude Code and caches on its
  own. Measure first with `/usage`.
- Telegram and unattended runs: they will join the same conversation later; out of scope here.

## Decisions

1. The cap is 200,000 tokens by default (owner, 2026-11-18), changed with `/compact N`.
2. The marks last until `/new`, but they no longer refuse a change to the person's blocks: it waits
   for the person's approval (owner's concern of 2026-11-18, section 5).
3. Stage 2 (rotation) comes after stage 1 has been used for a while.
