# The round (design, 2026-11-19)

Status: decided by the owner's direction ("an autonomous but safe agent"; Telegram left out for
now). Nothing here is built. It refines section 4 of `2026-11-17-clio-asistente-autonomo-design.md`.

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
