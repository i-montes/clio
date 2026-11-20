# Subagents, and the skills that use them (design, 2026-11-20)

Status: asked by the owner on 2026-11-20 ("the agent can launch subagents; we need something that
says how many it launches; make sure it can use the superpowers SDD skill and ponytail").

## What is true today (checked in the code)

- The model cannot launch a subagent. Pi leaves subagents out on purpose (its README; it only
  ships an example extension that spawns unguarded `pi` processes, unusable in Clio's jail). Under
  the `claude` connector the bridge runs Claude Code with `tools: []`, so Claude Code's own
  Task tool is not reachable either.
- Clio already runs a second agent in two places, both read-only: the reviewer (S-67) and the
  round (S-71). The reviewer's launcher (own run folder, own Pi folders, own tool host, own prompt
  and tool list, own limits, the session's marks and day budget, its report wrapped as outside
  text and judged) is the piece to generalise.
- The gateway's S-17 ("subagents only read and get no secrets") is set nowhere outside tests.
- Subagent-driven development needs one kind of child the code has never had: one that writes.

## The tool

`subagent {tasks: [{name, task, mode: "read" | "write", model?}], reason}`, in a chat session
only. Each task is a child agent: a fresh context, the task's text, and a final report. The call
returns when every child ended; its result is each child's report.

- **Readers** (`read`): up to `subagent.max_parallel` (4) at once, at most `subagent.max_per_call`
  (8) in a call. A reader is the reviewer's setup: `read`, `bash`, `find`, `ls` in a tool host
  that only reads.
- **A writer** (`write`): one in a call, and alone in it. It has `read`, `write`, `edit`, `bash`,
  `find`, `ls`, in the place the session works (the same working copy, or the same machine in host
  mode), under the session's own rules for files and commands.
- The tool runs alone in its step (never beside the parent's own commands or edits): the parent
  waits for it.
- A child cannot launch children. The reviewer and the round have no `subagent` tool.
- `model`: one of the models of the session's connector; default `subagent.model`, else the
  session's.

## What a child may do

Less than its parent, never more:

- **Nothing that would wait for the person.** A set-aside act (delete, spend, send, control, and
  the rest) is refused in a child, with or without a permission window or bypass: it says so in
  its report and the parent does it under the normal rules. A child never asks the person
  (`ask_person`), never requests a secret, and receives none.
- **No memory, no creating.** No memory tools, no change to the person's blocks, no `skill_write`,
  `service_write`, `mcp_propose`, MCP calls, managing, background jobs.
- **It reads Clio's registered skills** (as the parent does, read-only), so a child can follow one.
- **Marks go both ways.** A child starts with its parent's marks. Its report is outside text:
  cleaned, capped (50 KB), judged, wrapped as a subagent summary, and the parent is marked before
  the report reaches the model. A judge flag in a child ends that child and counts as a flag of
  the conversation (the window and bypass close).
- **What a writer changed is the run's.** `clio undo` and the action log cover a child's changes
  with the parent's, named as the child's.
- **Limits.** Per child: `subagent.max_steps` (100), `subagent.max_seconds` (1800),
  `subagent.max_cost_micro_usd` (1,000,000). Per conversation: `subagent.max_per_hour` (40). All
  of it counts in the session's usage and in the day's budget. `Esc` stops the call and its
  children.

Each child has its own run folder (`<run>/subagents/<n>`), Pi folders and tool host; none shares
the parent's tool host, whose powers are per process.

## What the person sees

- While a call runs: its row in the chat, one line per child (name, mode, model, state, time), and
  in the status line `N subagents running`.
- When it ends: one note, `3 subagents: 2 done, 1 failed · 4 min · US$ 0.42`.
- `/usage`: a line for the conversation, `subagents: 7 launched (5 read, 2 write) · steps · cost`.
- `/subagents`: the conversation's children, newest first, with their state and where each one's
  report is kept.

## Security rule

A new rule after the last one: what "What a child may do" says, in one paragraph, with tests on
both sides (a reader cannot write; a writer cannot do what waits; neither gets a secret, memory or
a child of its own; marks cross; the report is judged before the model sees it; limits hold). S-17
keeps its sentence for the gateway's own subagents and gains: "The chat's subagents are not gateway
subagents: what they may do is S-<new>."

## Verified for real before merging

With the real model, in a throwaway repository: `obra/superpowers` and `DietrichGebert/ponytail`
are installed with `clio skills add`, and Clio is asked to do a small plan with subagent-driven
development and ponytail; the run must show a writer child that commits, reader
children that review, the count in the status line and `/usage`, a refused set-aside act reported
by a child, and `clio undo` taking back a child's change.

## Not included

- Children that keep running after the call returns, or that talk to each other.
- More than one writer at a time (worktrees per child).
- Children in the round or the reviewer.
- Shipping third-party skills inside Clio.
- Repositories outside GitHub (clone it and add the folder).

## Version

0.5.0 (after the skills change, 0.4.0); if this merges first, 0.4.0.
