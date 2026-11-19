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
   last round, what waits for the person's approval and is about to expire, and what the previous
   round reported.
3. **It checks each watched thing** with what it has: commands, its skills, its services' results.
   It reads; it changes nothing by itself.
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

- Nobody is there to answer, so anything that needs the person's permission is set aside as usual:
  it waits in the one approval box and is asked when a chat next opens. The round's notice says what
  waits and why ("the disk is at 86 %: I can clear the build cache (48 GB); it waits for your yes").
- An open permission window covers the round like any other conversation (the person said yes
  within the last hours); what it does under it is in the window's record and summary.
- A question to the person (`ask_person`) answers "decide yourself", as in any unattended run.
- It may save memories as the agent (the normal budgets) and report on its skills and services
  (`skill_report`, the manage tools' free acts). It cannot change the person's blocks, register a
  service or install a server without the person, as always.

## Limits

- **The judge must be on.** A round reads outside content with nobody watching: with the judge off
  (or the vault locked, since the model's key is there) no round runs, and one notice says why,
  once.
- If the judge flags an injection the round ends at once, the usual `suspect` rules apply (the
  permission window closes), and a notice says so.
- Its own budget: steps, time and cost per round (`round.max_steps`, `round.max_seconds`,
  `round.max_cost_micro_usd`), and the day's budget it shares with everything else.
- Its model is the session's default unless `round.model` says otherwise.
- The round never opens the person's conversation, never answers a box, never opens or extends a
  permission window.

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
- `run-due` starts a due round with `systemd-run --user --scope` (the services' detached start), or as a separate process group when there is none, and never waits; `clio round now` runs one in the foreground, ignoring the interval and the quiet hours but not the lock, the readiness checks or the limits.
- The round is given the whole "told" map (key, state, notice), not only the previous round's notices, so it reuses keys.
- Its data goes to the model through the findings path (`queue_findings`: wrapped as data, judged, the session marked); the ask comes from the brain in the envelope.
- The block is `key:` / `state:` / `notice:` per entry (`- key:` first), or the single line `nothing`; keys are lowercase letters, digits and `-` (40 at most), at most 20 entries, a notice is capped at 600 characters, the answer at 16 KiB; anything else is "could not report".
- No repeat: a problem is told when new or its state changed, again after 24 h as "still: …"; a `resolved` is told only of a key told before, then forgotten; an `info` once per text; a change of wording of the same problem is not told.
- Notices are taken only when a chat opens (not by `clio run`), shown once, and ride once with the next message wrapped as a subagent summary, judged, marking the session.
- A round's set-aside items carry "set aside by the round at HH:MM" as their reason and are offered in the next chat's one box; an item of a round the judge flagged is never answered by a window.
- "Not ready" is one notice per distinct reason, until a round runs again.
- A round does not serve MCP servers (it has the host tools only); its cost is priced at the default model's price even when `round.model` differs.
- The doctor asks systemd whether the timer is active only when its unit files exist.
