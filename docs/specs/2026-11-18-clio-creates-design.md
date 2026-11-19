# Clio creates: a reviewer, skills, services and MCP servers (design, 2026-11-18)

Status: agreed with the owner on 2026-11-18 ("de acuerdo con todo"). Nothing here is built.
It refines sections 4, 5 and 6 of `2026-11-17-clio-asistente-autonomo-design.md` and comes before
Telegram (section 3 there).

## What the owner asked for

An autonomous agent must be able to make what it needs: its own skills, its own tools, the MCP
servers it wants, and services for the work that repeats. The owner's method for trusting what it
makes is the one he uses for code: say what it must do and how that is checked, build it, and have
someone else try to break it.

## One mechanism, four products

Everything Clio creates goes through the same four steps:

1. **Spec.** What it is for and how to verify it works: a check that can be run.
2. **Build.** Clio writes it.
3. **Review.** A second agent that did not write it runs the check and tries to break it.
4. **Register.** It becomes active, the chat says so in one line, and it can be undone.

Two guards sit across all four, and neither is the reviewer:

- **Temis reads the text** of whatever is being registered (it is text that will be followed as
  instructions, or code that will run). An injection is withheld, as with any outside text.
- **A marked session asks.** In a session marked `tainted` or `suspect`, registering waits for the
  person's approval with the whole text on screen — the rule the person's blocks already follow. In
  a clean session Clio registers by itself and tells the person.

The reviewer checks quality. It is another model reading the same text, so what fools the author can
fool it: it does not replace the two guards.

## Stage 1. The reviewer (a second agent)

Clio cannot start a second agent today. This stage adds it, and everything later uses it.

- **What it is.** A second Pi session started by the brain, jailed like the first, with its own
  prompt (`clio-reviewer@1`), a clean context (it gets the spec, the thing to review and how to
  verify it; never the author's conversation) and the same tool host, under the same rules as the
  session that asked for it, marks included. It has no memory-writing tools and cannot start another
  agent.
- **What it does.** It runs the check from the spec, then looks for ways the thing fails: inputs it
  does not handle, steps that assume what is not true on this machine, anything it does beyond what
  it says. It answers in a fixed shape: verdict (passes / fails / could not check), what it ran and
  what happened, findings with the evidence for each.
- **Who starts it.**
  - Always, before a skill, a tool, a service or an MCP server is registered (stages 2 to 4).
  - When Temis says so: after a turn that changed something, the brain asks the judge one closed
    question (does this work need an independent review?) with the request and the list of what was
    done. On yes, the reviewer looks at that work and its findings go to the model with the next
    message, as Clio's own note.
  - When the person asks (`/review`).
- **What its answer is to the model.** Data, wrapped like any other result that did not come from
  the person.
- **Limits.** One review per turn, a cap per hour, its own budget of steps and time; a review that
  cannot run does not block ordinary work, but it does block a registration.
- **Model.** Opus (owner, 2026-11-18): the reviewer is a subagent that runs on Opus whatever the
  session's model is, since the same model shares its blind spots. `review.model` in `config.json`
  holds it.

What the reviewer does not solve: a skill that was right and went stale. That is handled where the
skill is used (stage 2).

## Stage 2. Skills

Pi already implements skills (the Agent Skills format: a folder with `SKILL.md`, a name, a
description that says when to use it, optional scripts). Clio turns that on instead of inventing a
format.

- **Where they live.** A folder of Clio's own, with history so a change can be undone. Pi reads it
  read-only from inside the jail. The model reads a skill with the tools it has; it writes one only
  through a new tool, `skill_write`, served by the tool host — never with the plain file tools.
- **What a skill of Clio's must have,** beyond the format: a section **"How to verify"** with the
  check the reviewer runs, and the date it last passed.
- **Creating one.** `skill_write` runs the four steps: Temis reads it, the reviewer runs the check
  in a clean context, and it is registered (or waits for approval in a marked session). The chat
  says what was created and what it is for. `/skills` lists them; the person can turn one off or
  undo a change.
- **When.** When the person asks, or when Clio notices the same kind of request coming back.
- **Going stale.** When following a skill fails, Clio records it against the skill and corrects it
  (a new version, reviewed again). A skill that keeps failing is turned off and the person is told.
- **Too many.** Before creating one, Clio looks for one that already covers it and extends that one.
- **The skill that makes skills.** The first skill Clio ships with: how to write a good one for this
  system (the description that routes, the verify section, the two guards). Written for Clio, not
  copied from another product.
- **Cost to the prompt cache.** Pi puts every skill's name and description in the system prompt, so
  each new skill rewrites the cache once, and Pi must reload to see it: a new skill serves from the
  next task, not inside the one that made it.

**Tools of its own** are the scripts of a skill, run through the tool host under the host-mode rules
(same classes set aside, same judge, same log). Promoting a much-used script to a named tool with
its own schema is left for later: it adds convenience, not capability.

## Stage 3. Services

Work that repeats should be a program that runs by itself, not the model doing it again each time:
it costs no tokens, it does the same thing every day, and the model comes in only when it fails.

- **What a service is.** A program, a schedule (a system timer), what it is allowed to reach, a log,
  and a health check. Clio writes it; it is reviewed and registered like a skill.
- **Secrets.** A service that calls an API needs a key the model never sees. This stage builds the
  missing piece: a registered program uses a secret from the vault, bound to one destination, the
  way `request_secret` already binds it. Until then no tool of the model can use a stored secret.
- **On a production machine.** Each service runs with limits on processor and memory, as its own
  system unit, so it cannot starve the owner's sites.
- **No silent failure.** Each run records its result; the round (section 4 of the earlier design)
  reads those results and tells the person only when something changed or broke.
- **Creating a timer is a control action** today and asks for approval. A registered service is the
  approved shape of that: reviewed once, then it runs without asking.

## Stage 4. MCP servers

Clio may find, install and use MCP servers. Three conditions:

- **They run outside the jail, behind the tool host.** Inside the jail a server has no network. The
  tool host is the client: what a server returns passes the judge as outside text, its calls are
  logged, and its keys come from the vault without the model seeing them.
- **Pinned installs.** An exact version, no install scripts, as Pi itself is installed.
- **One approval per new server.** It is someone else's code holding the person's credentials: the
  first time, the person approves that server (what it is, where it comes from, what it may reach).
  After that Clio uses it by itself. Servers that write start read-only where the server allows it.
- **A skill for it.** How to choose, install, check and retire a server on this system, written the
  same way as the skill that makes skills.

## Order

1. The reviewer.
2. Skills, with the skill that makes skills.
3. Services, with the use of secrets.
4. MCP servers behind the tool host.

Then Telegram and the round, which will use all four.

## What this costs

- **Tokens.** A reviewed turn costs about twice. Registration is rare; the Temis-decided review is
  the one to watch, hence the cap per hour.
- **The cache.** Skills and tools live in the cached prefix; each change rewrites it once.
- **Shared blind spots.** Author and reviewer are models of the same family. A different model, a
  clean context and a check that is run rather than read reduce it; they do not remove it.
- **More to look after.** Skills, services and servers accumulate. Each has an owner (Clio), a check
  and a way to turn it off; the round reports the ones that fail.

## Decisions

- The reviewer is mandatory for anything registered, and Temis decides for the rest (owner).
- Registration is free in a clean session, with a line in the chat; it asks in a marked one.
- MCP servers are allowed and Clio installs them (owner). One approval per new server is proposed;
  the owner asked what it means and has not decided.
- The skill that makes skills is written for Clio, not copied.
- The reviewer is a subagent on Opus (owner).
- This comes before Telegram, which is still not needed (owner).

## Open for whoever builds

- How Pi reloads skills in a long-lived session (`/reload` over RPC, or a restart of Pi between
  turns) and what that does to the cache.
- Whether the reviewer shares the session's tool host or gets its own, given the tool host's
  per-session budgets and marks.
- The exact closed question for Temis and its threshold, measured the way the other questions were.

Stage 1, as built (2026-11-18):

- Tool host: the reviewer gets its own (`clio toolhost --review`), started with the session's work folder, host rules and judge, its run id `<run>.review-<n>`: a shared one would make the chat and the review wait for each other (one call at a time) and mark the conversation from the reviewer's reads. It starts with the session's marks and spends from the same day's budget.
- Reduced list: `read`, `bash`, `find`, `ls` (`CLIO_GUARD_REVIEW=1`), fixed whatever the mode so its prefix caches; in host mode `bash` without `background`. Not a gateway subagent (S-17 would deny its `bash`): it runs under the session's own rules.
- A set-aside action is refused in the review's tool host, logged, and nothing is stored as pending. The review's tool host also refuses every tool but the reading ones and `exec`, and the network and `sudo` outright; its `bash` writes no copy (copy read-only in copy mode, the machine read-only but `/tmp` in host mode); its findings go through the judge, and are not delivered if the mark fails; its rows show in `/actions`; its cost cap is `review.max_cost_micro_usd`; its prompt is `clio-reviewer@2`.
- Verdict format: the whole final message is one fenced `review` block with `verdict:`, `ran:` and `findings: none` or `- what:` / `  evidence:` items; a finding without evidence is dropped, at most 20 are kept; anything else is `could_not_check` with the raw text cleaned and capped.
- The judge's question (not measured against the real judge yet): `needs_review`, "The work listed in `done` was carried out on this machine to answer `person_request`. It needs an independent check by someone else: it changes code, configuration, a service or data, a mistake in it could go unnoticed, and it is not trivial to see at a glance that it is right.", threshold `review.threshold` (0.8).
- Model: `review.model`, by default `claude-opus-5-5` with the `claude` connector and the session's model with the others; when the connector's list (what Pi offered last, or the catalog) does not have it, the session's model, said in the result.
