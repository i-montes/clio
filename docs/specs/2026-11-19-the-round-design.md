# The round (design, 2026-11-19)

Status: decided by the owner's direction ("an autonomous but safe agent"; Telegram left out for
now). Built on `feat/round` (decisions below). It refines section 4 of `2026-11-17-clio-asistente-autonomo-design.md`.

## What it is for

Today Clio acts only when the person writes. The person's `vigilancia` block lists what Clio must
watch ("warn me if / passes 80 %"), and services run by themselves, but nothing looks at either
unless the person opens the chat and asks. The round is Clio waking up by herself, looking, and
leaving a notice only when something needs the person.

## What happens in a round

1. **A timer wakes it.** The same user timer that runs the services (`clio services run-due`, every
   minute) also starts a round when one is due: every `round.every_hours` (default 8; `0` = never),
   never between `round.quiet` hours (default `23:00-07:00`, local time). One round at a time; a
   round still running when the next is due makes the next one skip.
2. **It is one unattended session of the agent**, in host mode, with its own conversation (never the
   person's), its own prompt (`clio-round@1`) and the normal tools. It is given, as data: the
   person's watch list, the services and MCP servers that raised a notice or are paused since the
   last round, and what the previous rounds reported. (Not what waits for the person's approval:
   a round could issue someone else's item again.)
3. **It checks each watched thing** with what it has: commands, its skills, its services' results.
   It reads; it changes nothing, by code: its tool host only reads.
4. **Silence by default.** For each thing the answer is "nothing to tell" or one notice, in the
   person's language, saying what, how it knows, and what it proposes. A round with nothing to tell
   leaves no notice.
5. **It does not repeat.** A notice already given is given again only when the thing changed or was
   resolved ("the disk is back under 80 %").
6. **Where the notices go.** With no Telegram they are kept and shown as quiet notes the next time
   a chat opens (as services' notices are), and given to that conversation's model with the
   person's next message, as outside text, so Clio can answer about them. `clio round` prints the
   last rounds and their notices.

## What a round may do

A round reads and proposes. It changes nothing, and this is held by its tool host
(`--unattended`), not by its prompt:

- Its commands run only under bwrap, with the machine read-only, a `/tmp` of its own and no new
  privileges (no `sudo`); `/run` stays visible so `systemctl status` and the like work.
- `write`, `edit`, new folders, background commands, every act on what Clio created but `status`
  and `logs`, `skill_write`, `skill_report`, `service_write`, `mcp_propose`, a change to the
  person's blocks and `request_secret` are refused outright ("a round only reads: propose it in
  your notice").
- What reaches out (the network, a program that does by itself, `sudo`) waits for the person as
  `send`; a `systemctl` or `service` that does more than read waits as `control`; the listed
  classes wait as always. They are asked when a chat next opens, in the one box, named as the
  round's, and always ask: **no permission window covers a round**, in its tool host or in the
  chat. An approved item runs in a normal session.
- A question to the person (`ask_person`) answers that nobody is here; the round puts the
  question in a notice.
- It may save memories as the agent (the normal budgets).
- What it thinks should be done, it proposes in its notice, with the exact command ("the disk is
  at 86 %; I propose clearing the build cache: `docker buildx prune`").

## Limits

- **The judge must be on.** A round reads outside content with nobody watching: with the judge off
  (or the vault locked, since the model's key is there) no round runs, and one notice says why,
  once.
- If the judge flags an injection, or does not answer, the round ends at once (the text is
  withheld: a round reads only judged text), and a notice says so. Its own first message is judged
  too: a flag or no answer, and it does not start.
- Its own budget: steps, time and cost per round (`round.max_steps`, `round.max_seconds`,
  `round.max_cost_micro_usd`), and the day's budget it shares with everything else.
- Its model is the session's default unless `round.model` says otherwise.
- The round never opens the person's conversation, never answers a box, never opens or extends a
  permission window, and no window covers it.

## What the person does

Nothing. No command is needed: the round runs when the services timer is on (`clio services
enable`, already on for the owner). `/round` and `clio round` show the last rounds; `clio round
now` runs one; `round.every_hours: 0` turns it off.

## Not included

- Telegram or any push channel: the notices wait for the next chat.
- Rounds that act without permission outside a window.
- Event triggers (a webhook, a file change): services cover frequent checks.

## Decisions taken when building it

- The round's session is its own run folder (`meta.instruction` "Clio's round") with its Pi session inside it (`runs/<id>/pi`): not in the sessions drawer; `clio round` and `/round` list rounds, `clio actions --run <id>` shows what one did.
- Its state (last start, the "told" map, the last 50 rounds, why rounds are blocked) is `<data>/round/state.json`; its notices use the services' notices file format in `<data>/round/notices.json`; its lock is `<data>/round/round.lock`.
- `run-due` starts a due round with `systemd-run --user --scope -p RuntimeMaxSec=<round.max_seconds + 300>` (the services' detached start) and never waits; with no `systemd-run`, no round runs and one notice says why. `clio round now` runs one in the foreground, in the same read-only tool host, ignoring the interval and the quiet hours but not the lock, the readiness checks or the limits.
- Read-only by code: one tool-host mode, `--unattended`, set only for a round. It reuses the review's read-only job without its Unix-socket filter and with `/run` visible, and forces bwrap, as a review does, since the systemd launcher shares the person's pid namespace. A `systemctl`/`service` that does more than read waits as `control`, because the service manager's sockets are visible.
- No permission window covers a round: the tool host never answers its items under one, the chat refuses to, and the box marks them "this one always asks you".
- The round is given the "told" map (key, state, notice; the newest 50, an `info` forgotten after 7 days), not only the previous round's notices, so it reuses keys; it is not given what waits for approval.
- What needs attention is "since the last round's start", read before this round's start is written.
- Its data goes to the model wrapped as data and judged, marking the session; a flag or no answer means the round does not start. In the round, a judge that does not answer withholds the text and marks the session `unjudged`, which ends it.
- The block is `key:` / `state:` / `notice:` per entry (`- key:` first), or the single line `nothing`; keys are lowercase letters, digits and `-` (40 at most), at most 20 entries, a notice is capped at 600 characters, the answer at 16 KiB; anything else is "could not report". Clio's own notices are capped at 300 characters.
- No repeat: a problem is told when new or its state changed, again after 24 h as "still: …"; a `resolved` is told only of a key told before, then forgotten; an `info` once per text; a change of wording of the same problem is not told. A time ahead of now (a clock set back) counts as long ago, for "still" and for when a round is due.
- Notices are taken only when a chat opens (not by `clio run`), shown once, and ride once with the next message wrapped as a subagent summary, judged, marking the session.
- A round's set-aside items are named "set aside by the round at HH:MM" only where the person sees them (the box, `clio pending`), never in their stored reason, which the window rules read.
- "Not ready" is one notice per distinct reason, until a round runs again.
- A round does not serve MCP servers (it has the host tools only); its cost is priced at the default model's price even when `round.model` differs.
- The doctor asks systemd whether the timer is active only when its unit files exist.
- `clio round` is `control`: the model never starts one.
