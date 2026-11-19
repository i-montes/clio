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

Stage 2, as built (2026-11-18):

- Folder: `~/.local/share/clio/skills/` (Clio's data, which the tool host already refuses to every file tool): `live/<name>/` the enabled skills' current version (what Pi loads), `versions/<name>/<n>/` every version unchanged, `records/<name>.json`, `removed/`; written only by the person's `clio` under a lock, each change built in a hidden folder and renamed in.
- Pi: `--skill <live>` with `--no-skills` still on (no discovery), only for chat sessions (not `clio run`, not a review); the jail binds `live/` read-only; the start check accepts a skill command only from that folder.
- Reload: Pi's RPC has no reload and `ctx.reload()` would reload the guard itself mid-session, so a new skill reaches Pi at its next start (a new session, or reopening one): never mid-turn, one cache rewrite per new session.
- Drafts: `skill_write` composes SKILL.md itself (frontmatter, body, then `## How to verify` with `verify`), checks it (Pi's name rules; SKILL.md ≤ 20,000 bytes, ≤ 10 other files of ≤ 32 KiB, ≤ 128 KiB in all, `verify` ≤ 4,000, description 20–1,024; paths of plain names ≤ 4 deep, ≤ 200 characters, none starting with a dot; nothing the display cleaning changes; the vault's scrubber), has the judge read the whole text (an injection refuses and marks `suspect`; no judge, no draft), refuses a `verify` that is only `true`, `:`, `exit 0` or an `echo`, caps drafts at 6 an hour per session (taken ones count), refuses for an hour a name whose draft was dropped, records the session's marks in the draft, and stores it in the run's `toolhost/skills/drafts/<id>.json` (0600) with its digest.
- Review: the brain picks up the draft after the turn (and, when a session is opened again, any it took whose outcome was never decided: a `taken/<id>.done` marks the decided ones), stages it in the review's folder, `<run>/reviews/<n>/stage/<name>/` (Clio's data: every level made new 0700, files new, no link followed, any failure `could_not_check`; in host mode only the reviewer's commands see it, read-only, through the same window as `live/`, `clio toolhost --show`; in copy mode it is the reviewer's work folder), and runs stage 1's review with every file whole in the request and a check that says a check which cannot fail is none; after the review the staged copy must still hash to the draft, or it is `could_not_check`.
- Outcome: the verdict decides, not the findings (an adversarial reviewer always finds an edge case, so findings alone would never let a skill register). `passes` registers it at once in a clean session; in a marked session, or on `could_not_check`, it waits as pending class `skill` (bound by hash, 24 h, counts against the day's questions); a change to the shipped `skill-creator` or a name the person removed always waits too; notes of a pass reach the model once an hour per name; `fails` goes back to the model as data, and only `fails` counts: the third in a row for one name drops it, tells the person and bans the name for an hour.
- Notes: a `passes` with findings registers; the chat line says how many notes and where the full text is, and they go to the model once, as a review's findings, with Clio's request `SKILL_NOTES_ASK` (write a new version only if a note is worth it); that turn is not reviewed.
- Verdict: `clio-reviewer@3` (selected for every review; `@1` and `@2` stay): `fails` only when the given check fails, the thing does not do what it says or it is unsafe to run unattended; `passes` when the check passes and it does what it says, with problems outside its claims as findings with evidence; `could_not_check` as before. Registration keeps the version's verdict and date (the date it last passed).
- Use: a registered, enabled skill's files read unwrapped and without marking; in host mode commands see a read-only `live/` (bwrap `--ro-bind` over the masked data folder, systemd `BindReadOnlyPaths`) and naming a file there is not `control` (a quote, `\`, `$`, a backtick or a glob after the path, or a `..` in the command, keeps it `control`); in copy mode the command jail does not see skills, so scripts run in host mode only. The set-aside classes see a script's command line, not what it does inside: the review checks that.
- Reports: `skill_report` appends to the run's `toolhost/skills/reports.jsonl` with the session's mark; the brain applies them after each turn (renaming the file first so each counts once); three `failed` in a row turn the skill off and the person is told; a `worked` resets the count.
- Person: `/skills` and `clio skills` (list, `off`, `on`, `undo`, `remove` with a question); a skill the person turned off stays off when a new version is registered.
- Prompts `clio-agent@23` and `clio-host@7`: the previous ones plus one Skills section; the shipped `skill-creator` is installed by `clio init` or the first chat when it is missing and was not removed (`removed/skill-creator-<ms>`), updated there while the installed one is an earlier shipped text left as it was, and brought back by `clio skills on skill-creator`. Approval (`clio approve`) prints every file whole and asks; registration writes only new folders and files and takes a failed version's folder away.

Stage 3, as built (2026-11-19):

1. What: a service is a program run on a schedule with no model; `service_write` drafts it like `skill_write` (same file checks, judge, drafts folder and hourly cap, shared with skills), stage 1's reviewer runs its `verify` from a staged copy (`clio-reviewer@3`, the request says what a service may do), and then it always waits for the person as pending class `service` (S-69): the Approval box shows what it is for, why it waits, schedule, command, hosts, secrets (name → host, as `$ENV`), write paths, limits, the review's verdict and notes, then every file whole (`service.json` first). `fails` goes back to the model (three in a row drop it, as a skill) and never reaches the person.
2. Schedule: `every` is a whole number of `m`, `h` or `d`, at least `5m` (at most a year); `at: "HH:MM"`, local time, only with `every: "1d"`; no cron. Due: never ran, or `every` passed since the last real run (30 s slack for the timer's drift); with `at`, the latest local `HH:MM` came after both the last run and the version's registration. Shortcut: today's UTC offset is used, so across a summer-time change a daily run may come an hour off once.
3. A run: as the person (never root), `bash -c` (no login files), `umask 077` (and `UMask=0077` in the timer's unit), `NoNewPrivileges`, the machine read-only but its state folder and its declared `writes`, nothing of the home, `/home`, `/root`, `/var`, `/srv`, `/opt`, `/mnt` or `/media` visible but its version folder, its declared `reads` (read-only, `reads: [absolute paths]`, under the home or one of those roots, never a root itself or an ancestor of the home, validated like `writes` but only against the home itself, Clio's folders, `~/.ssh` and `~/.gnupg`; in the digest and its own row of the box), its `writes` and the `clio` binary when it lives in the home; Clio's folders and `/run` hidden; a private `/tmp` and `/dev/shm` the size of its memory (and `/dev`, IPC); no network of its own. `writes`: absolute, plain (no `.`/`..`/`//`), under the home, not the home, not inside or above Clio's folders, `~/.ssh`, `~/.gnupg`, the shells' start files, `~/.config/{systemd,autostart,environment.d,fish}`, `~/.local/bin`, `~/.cargo/bin`, `~/.pam_environment`, nor any folder that loads code by itself (`PROTECTED_PATHS`), nor a path with a name under the home that starts with a dot, nor one whose tree (4 levels, 2,000 entries, past that refused) holds `.git`, a `PROTECTED_PATHS` name, a shell start file, `.envrc`, `node_modules` or `pyvenv.cfg`; it must exist and be named without a link; checked when written, when approved and before each run. Limits: default 50 % CPU, 512 MB, 300 s; at most 100 %, 2048 MB, 3600 s (memory at least 16 MB, time at least 10 s); 256 processes. They are a transient user scope's (`env XDG_RUNTIME_DIR=… systemd-run --user --scope --quiet --collect -p MemoryMax=…M -p MemorySwapMax=0 -p CPUQuota=…% -p TasksMax=256 -- bwrap …`, the jail unchanged inside) when a probe scope with the same properties starts, else per process (`prlimit --as --cpu --nproc=256`); `nice -n 19` always; the run records `cgroup` or `per-process`. Hosts: with any declared, the runner starts the allow-list proxy of those `host:port`s (443 by default; names only, no address literal, never `*.localhost`, `*.local`, `*.internal`, `*.arpa`) in the runtime folder, which resolves each name once per connection and connects only to an address it checked is global unicast and not the machine's own (`EgressProxy::start_service`; Pi's proxy is unchanged), shows its folder read-only to the run, and runs the command under `clio pi-net` with `HTTPS_PROXY`/`HTTP_PROXY` (and lowercase) at `http://127.0.0.1:18080`; direct connections fail (no network namespace of its own).
4. Secrets: `secrets: [{name, env, host}]`, the host one of the declared ones, `env` uppercase and not one the shell, the launcher or the proxy sets. The runner (the person's process, never the tool host) first asks the vault's gateway socket `check(name, "service", host)` for every declared host (the run could send it to any; a draft the vault already refuses goes back to the model, else the run waits), then `resolve(name, "service", host, "service:<service>")` for its own host, compared by name without the port: the vault releases it only when it is bound to `service:<domain>` (its subdomains too, as every destination). The value goes to the run on its standard input, NUL-terminated, read into the variable by a fixed prefix (`IFS= read -r -d '' ENV; export ENV`), never in argv, a file or the unit's environment; the buffer is zeroed once written. Locked vault, no vault or a secret not allowed to every host: the run is recorded `waiting`, not a failure, the schedule does not move, the same wait again is one entry with one notice, and the timer tries it again at most every 15 minutes. The person stores one with `request_secret` (destination `service:<host>`) or `clio vault put NAME --dest service:<host>`. All output passes a scrubber holding the resolved values and the key shapes, then the vault's scrubber when its socket exists: a vault that cannot filter withholds the output, but one that is not running (connection refused) with a service that has no secrets keeps it; with no socket at all, it is kept after the run's own scrubber only.
5. Results: each run records start, duration, version, outcome, exit code, why, the scrubbed tail (4,000 bytes, cleaned) and who started it, in `runs/<name>.json`, the last 50. 0 `ok`; 10 `attention` (a notice with its last three lines; not a failure, it clears the count); anything else, a signal or the time limit is `failed`; three failures in a row pause it (`off: paused: three failed runs in a row`) with a notice. A run due while one is going is skipped (a per-service `flock`). Missed while down: one run (both the schedule and the timer's `Persistent=true`).
6. Scheduling: `clio services run-due` runs what is due and exits: with `systemd-run`, each in a user scope of its own (`systemd-run --user --scope --quiet --collect -- <clio> services run-timer <name>`), not waited for once it left the timer's cgroup (so the oneshot ending stops none; `run-timer` takes the lock and checks it is still due); without, or when that scope does not start, one after another. `remove` moves `state/<name>/` and `runs/<name>.json` into its `removed/` folder. `clio services enable` writes `~/.config/systemd/user/clio-services.service` (oneshot, `ExecStart=<clio> services run-due`, refused for a path with spaces or quotes) and `clio-services.timer` (`OnCalendar=minutely`, `Persistent=true`), both 0600, prints them, and runs `systemctl --user daemon-reload` and `enable --now`; `disable` stops it and removes exactly those two. No sudo. Lingering off (no `/var/lib/systemd/linger/<user>`): it says so and prints `loginctl enable-linger <user>`, never runs it. `clio doctor` warns when a service is on and the timer is not active, or bwrap is missing. The runner uses the host launcher's code, but only its bwrap launcher (see Launchers below).
7. Notices: `<data>/services/notices.json` (the last 50, each with a `seen` flag): a pause, a changed file or an `attention`; the next chat that opens (any mode) shows the unseen ones as quiet notes and marks them seen; `/services` and `clio services` show the last five. No Telegram.
8. Model tools, host mode only (`HOST_TOOL_NAMES`): `service_write {name, description, schedule, command, verify, files?, network?, secrets?, writes?, limits?, reason}` (the manifest is composed by the tool host, `service.json`; the stage 2 `verify` and secret-shape rules; a draft id starts with `s`) and `service_status {name?}` (the list, or one with its last five runs and the last output: outside data, wrapped, judged and marking the session). Nothing else: no tool enables, disables, runs, approves, removes or edits one; a new version is a new `service_write` and waits for the person again. `clio services …` and anything naming `clio-services.timer|service` is `control`, and so is a write under `~/.config/systemd/` or `~/.local/share/systemd/`.
9. Person: `/services` and `clio services`: list, `show <name>` (status and every file whole), `on`/`off`, `undo`, `remove` (asks first; kept in `removed/`), `run <name>` (now, through the same runner), `logs <name>` (the last 50 with their tails), `enable`/`disable`.
10. `service-creator` ships in the binary beside `skill-creator` (`skills/service-creator/`: `SKILL.md` and `scripts/check-run.sh`, which runs a command twice with a fresh state folder and passes only when both end 0 or 10), installed and updated by the same rules; a change to it always waits for the person.
11. Prompts `clio-agent@24` and `clio-host@8`: the previous ones plus one Services section (copy mode: services exist only in a host chat).

Decisions taken while building it:

- Layout: `~/.local/share/clio/services/` with the skills folder's code (`Kind::Service`: same lock, versions, records, `removed/`, `undo`; no `live/`), plus `state/<name>/` (0700), `runs/<name>.json`, `notices.json`, `locks/<name>.lock`. A version's files are stored in path order and a run reads the folder back (no link followed) and checks its digest against the record: any change, extra or missing file pauses it with a notice before anything runs.
- Manifest: `service.json`, pretty JSON composed only by the tool host (`deny_unknown_fields`), read back only if it recomposes to the same bytes.
- A service draft reuses the skill draft's type (its manifest in `skill_md`, an `s` id) so the skills' code and tests are unchanged; the id decides how the person's `clio` reads it and the text must then pass that kind's checks.
- Launchers: **a service runs only under bwrap; the systemd launcher is refused with its reason.** A systemd unit has no private pid namespace before systemd 257 (`PrivatePIDs=`; this server has 255) and Yama's `ptrace_scope` 1 still lets a process read `/proc/<pid>/root` of another of the same user: a service could read Clio's data and reach the vault's sockets through the root of any other process of the person's. bwrap's `--unshare-pid` and its own `/proc` show only the run's processes. So `clio services run`/`run-due` use `HostJail::with(Bwrap)` (no bwrap: refused, saying why), and the runner refuses any other launcher before starting anything. The systemd arguments are still built and tested (`CPUQuota`, `MemoryMax`, `MemorySwapMax=0`, `RuntimeMaxSec`, `ReadOnlyPaths=/`, `NoNewPrivileges`) for the day it gains `PrivatePIDs=yes`. A run also masks `/run` whole (the person's D-Bus, from which `systemd-run --user` would start something outside the jail, Docker's socket, the system bus, the vault's sockets), with the proxy's folder as the only window. bwrap has no cgroup: each process gets `prlimit --as=<memory> --cpu=<seconds × cpu%>` and `nice -n 19`, and the runner kills the whole tree at the time limit. That is weaker (per process, address space rather than resident memory: a program that reserves a lot of address space, such as a JVM or node, may not start under a small limit), and it is said here, in `docs/security.md` and in the threat model; without `prlimit` or `nice` the run is refused.
- The runner's own trace: one action-log line per run (`tool: service_run`, the name and the outcome; never the output).

Stage 4, as built (2026-11-19):

1. Kind: npm servers over standard input and output only, at an exact version (`MAJOR.MINOR.PATCH[-pre]`), started with `node --disable-wasm-trap-handler <entry>` (the entry: the package's only `bin`, or the one named after it, else `main`, else `index.js`, a plain file inside it). No HTTP transport, OAuth, `uvx`, Docker, resources, prompts or sampling; an unknown field in a proposal (a command, a URL) is refused saying so, and the docs say it.
2. Proposal: `mcp_propose {name, description, package, version, args?, env?, network?, secrets?, reads?, writes?, limits?, reason}` (host mode only, never in a review): the tool host composes `mcp.json` and checks it (npm's name rules, no leading `.`, `_` or `-`, one scope at most; services' rules for hosts, secrets, paths and limits; memory ≥ 512 MB; no secret shape), `args` ≤ 20 of ≤ 200 characters and `env` ≤ 20 settings of ≤ 500 (names as a service's variables, never `NODE_*` or a secret's variable; no control or hidden characters), both covered by the manifest's digest; refuses another package under an installed name and a manifest identical to the installed one (the same version with other args, env or bounds is an update), refuses from a `suspect` session and past six proposals an hour per session (counted before npm runs), asks `McpPort::npm_view` (only the fields `NPM_FIELDS`; one npm at a time, a busy npm refuses at once) (the person's npm, jailed, registry only) for the facts, which must be that name and version with a `sha512-` integrity (cleaned, each ≤ 200 characters, ≤ 10 maintainers shown), and sets it aside as pending class `mcp` (hash-bound, 24 h, asked with the other pending items under `approvals.every_hours`), always. No judge and no reviewer: there is nothing of the model's to run, and the person reads the box.
3. Box and `clio approve`: install or update, what it is for, package@version, publisher and maintainers (names only), integrity, a deprecation warning, direct dependencies, license (≤ 40) and repository (each labelled "the package says"), arguments and settings, hosts, each secret with its variable, its binding `mcp:<host>` and every host it can reach, reads, writes, limits, and "This is someone else's code: it will hold these credentials and can send them to the hosts above." (without secrets: that it runs here, in a jail, with what is listed), then that approving downloads it and probes it, off until its tools are confirmed. The approval box's timing (`y` only after the end was on screen).
4. Install, in the person's `clio`, after approval: npm in a service's jail (its own `HOME` and `.npmrc` in `mcp/npm/home-*`, user and global config pointing at it, `mcp/npm-cache/`, the registry set, `ignore-scripts`, no audit or fund, the proxy's only host `registry.npmjs.org`, Node's and npm's folders shown read-only, npm the `npm-cli.js` next to the Node Clio runs Pi with) runs `npm install --ignore-scripts --save-exact package@version` into `servers/<name>/.stage-<pid>/`; the lockfile must give that package that version and the integrity shown, and every package a `https://registry.npmjs.org/` tarball with an integrity, no link; (entries `inBundle` under `node_modules/` are accepted; links never); then the entry point, the tree's digest (paths, kinds, permissions, contents, link targets; ≤ 50,000 entries), the probe from the stage with an empty temporary home and at least 60 s, and only then a rename to `servers/<name>/<version>/`. The same package and version already installed with the same integrity reuses its tree when its digest still matches. A failure tells the model only the step that failed, in fixed words; npm's and the server's output goes, cleaned, to the server's log. Before each start the tree is hashed again; a difference turns it off (`off: its installed files changed after it was approved`) with a notice and it does not start.
5. Probe: the server started once in the same jail with no network, no secrets, no reads or writes, `initialize` and `tools/list` only (time: the lesser of 60 s and its call limit); failure removes the tree and registers nothing. Registered off (`waiting: confirm its tools`); `confirm` shows the tools as the server describes them and turns it on. First modes: `free` when the server marks the tool `readOnlyHint`, else `ask`. At each start the tools are compared with the stored ones: a changed description or schema, or a new tool, is `ask` and flagged (gone ones dropped), and a call to it waits; only `confirm` clears a flag.
6. Run: `services::Runner::prepare` (stage 3's jail, proxy, vault release, scrubber, limits), generalised to a `Spec` both use, and `HostJail::spawn_service` (the same bwrap command, piped, its own process group); secrets bound to `mcp:<host>`, released only when allowed to every declared host, on standard input before the JSON-RPC stream; `HOME` its own `data/<name>/`; `/home`, `/root`, `/var`, `/srv`, `/opt`, `/mnt` and `/media` masked (services too), and of Node only the binary and npm's package folder shown (refused when one would show the home, an ancestor of it or Clio's folders). Client: the tool host's, JSON-RPC 2.0 one message a line, protocol `2025-06-18`, `initialize`, `notifications/initialized`, `tools/list` (≤ 10 pages, ≤ 100 tools; a description ≤ 2,000 characters, a schema ≤ 16 KiB, a name of `[A-Za-z0-9_.-]` ≤ 64, else dropped), `tools/call`; lines ≤ 2 MiB, a call ≤ 16 MiB and 10,000 messages, ≤ 100 bad lines or server requests, each server request answered `-32601`; any of those, a crash or silence past the limit kills it, and the error carries the last 400 characters of its stderr (scrubbed). One process per server per session, started at its first call, stopped after 10 minutes unused (a reaper thread) and when the session ends; a pooled process records the version and tree digest it started with, and before every call and in the reaper one whose server is off, unconfirmed, removed or changed is stopped. The pool's lock is never held over a start or a call: a slow server blocks only its own calls (wait, start and call share one deadline, under the guard's 400 s). The client's channel is bounded. Call limit: 60 s by default, 300 s at most; result ≤ 48 KiB.
7. Model: `mcp_tools {server?}` and `mcp_call {server, tool, arguments}`, fixed in the tool list. Arguments: an object ≤ 32 KiB, required fields present, top-level types as the schema says, no extra field with `additionalProperties: false`, nothing the scrubber would change. An `ask` tool, a flagged one, and any call in a `suspect` session wait as class `send` with `MCP server «s», tool «t», arguments {json}`; approval runs that text exactly, in a new process of the server (the approving `clio`'s tool host), if the server is still on at the same version with that tool. Every tool list, result and failure is outside text (scrubbed, judged later, `external`, marking the session); the chat marks the session before an approved call's result reaches the model.
8. Person: `/mcp` and `clio mcp`: `list`, `show`, `on` (refused until confirmed), `off`, `confirm` (a box of every tool whole, its description cleaned and its mode in plain words, with the approval timing and queued with the other questions; bound by a digest of the list, refused if it changed), `allow`/`ask <name> <tool>`, `remove` (asks; manifest, servers, state, data and logs go to `removed/`), `logs` (starts and calls, never arguments or results). `clio mcp …` is `control`. Notices (a changed tree) show when a chat opens. No `undo`: a new version is a new proposal.
9. `mcp-installer` ships in the binary beside `skill-creator` and `service-creator` (`skills/mcp-installer/`: `SKILL.md` and `scripts/check-package.sh`, which checks a name and an exact version the way the tool host does), installed and updated by the same rules.
10. Prompts `clio-agent@25` and `clio-host@9`: the previous ones plus one MCP servers section (copy mode: MCP servers exist only in a host chat).

Decisions taken while building it:

- Order: check, then `npm view` (metadata only; the name and version reach the registry before approval, accepted), then set aside; after approval install, lockfile check, entry point, digest, rename, probe, state, registration off. A failed step leaves nothing installed and the pending item can be approved again.
- npm confinement as in 4: the same jail as a service, with the registry as its only host; its scratch home is removed after each run.
- Node under per-process limits: `prlimit --as` stops V8 reserving Wasm guard regions, so `fetch` (undici's Wasm parser) fails; Node runs with `--disable-wasm-trap-handler`.
- The manifest has no command: what runs is fixed (Node and the package's entry point), so a proposal cannot name a program.
- An approved `ask` call runs in its own short-lived process of the server, not in the session's pool: approval happens in the person's `clio`, which has its own tool host.
- Not built: Python or Docker servers, remote servers, OAuth, resources, prompts, sampling, per-server tools in the model's list, signature or provenance checks of npm packages.
