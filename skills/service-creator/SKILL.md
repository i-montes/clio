---
name: service-creator
description: "How to write one of Clio's services with service_write: a program that runs by itself on a schedule, with no model. Use when the person wants something done every few minutes, hours or days, or when the same check keeps being asked for."
---

# Writing a service for Clio

A service is a program that runs by itself on a schedule (every 15 minutes, every day at 03:00),
on this machine, with no model in the loop. It costs no tokens and does the same thing every time.
You write it with `service_write`; a reviewer checks it; then it always waits for the person's
approval, because it is code that runs unattended on a machine with their real sites. Services
exist only in a chat on the person's own machine (host mode).

## Service, skill or nothing?

- A service: the work repeats on a clock and needs no judgement (check a site answers, rotate a
  log, back up a folder, fetch a feed and keep the new items).
- A skill: the work needs judgement each time, or comes when the person asks. A skill is steps
  you follow; a service is a program the timer runs.
- Nothing: work done once. Do it now with `bash`.

## The parts of service_write

- `name`, `description`: as a skill's. The description says what it does, in the person's words.
- `schedule`: `{"every": "15m"}` (minutes `m`, hours `h` or days `d`, at least `5m`), or
  `{"every": "1d", "at": "03:00"}` (local time). No cron syntax.
- `files`: the program, as scripts (`run.sh`, `check.py`). They are the service's folder, which
  is read-only when it runs, and its working folder.
- `command`: one line run with bash from that folder: `bash run.sh`.
- `verify`: commands, run from the service's folder, that succeed only when it works. Copy this
  skill's `scripts/check-run.sh` into `files` as `check-run.sh`: `bash check-run.sh . 'bash
  run.sh'` runs the command twice with a fresh state folder; add a check of what it wrote.
- `network`: only the hosts it must reach (`api.example.com`, or `host:port` when not 443).
  Leave it out when it needs none. It reaches them only through a proxy (`HTTPS_PROXY` is set for
  it; curl and most clients use it by themselves), and only with `https://`: the proxy opens
  tunnels, so a plain `http://` request is refused. Anything else, and any direct connection,
  fails. A name of the machine or its own network (`*.localhost`, `*.local`, `*.internal`) is
  refused, and so is any name that points at a private, loopback or link-local address when the
  run connects.
- `secrets`: `[{"name": "gh-token", "env": "GH_TOKEN", "host": "api.github.com"}]`: the run gets
  the value in that variable, and nowhere else. The host must be in `network`. The run could send
  it to any host in `network`, so the vault must let it go to every one of them: give a service
  with a secret only the hosts that secret is for. Ask the person to store it with
  request_secret, destination `service:<host>` (one for each host); until it is stored (or while
  the vault is locked) the runs wait, and are tried again every 15 minutes; they do not fail.
- `reads`: absolute paths under the home it must read (folders or files). Nothing else of the
  home is visible to it: not the person's files, not their sockets, not their keys; only its own
  folder, its state folder and the paths in `reads` and `writes`. Each must exist already; never
  the home itself, Clio's folders, ~/.ssh or ~/.gnupg.
- `writes`: absolute paths under the home it must write, besides its state folder. Each must
  exist already, and none may have a name that starts with a dot under the home (settings,
  keys); its tree may hold no repository (`.git`), nothing that loads code by itself
  (`node_modules`, a virtualenv, `.envrc`), and no more than 2,000 entries within 4 levels: when
  it does, declare a narrower folder. Never Clio's folders, ~/.ssh, ~/.gnupg, shell start files,
  ~/.config/systemd or ~/.local/bin: refused. This is checked again before each run.
- `limits`: `{"cpu_percent": 50, "memory_mb": 512, "seconds": 300}` are the defaults; lower them
  when you can; at most 100, 2048 and 3600. Its `/tmp` counts as memory and is no larger than
  `memory_mb`, and it may have at most 256 processes. Where the person's systemd can hold them,
  they are limits on the whole run; else on each of its processes.
- `reason`: why this service, in one line.

## A good service

- Safe to run twice, and to run late: it checks before it changes, and a run that finds nothing to
  do ends at once. A run that is still going when the next is due makes the next one skip; runs
  missed while the machine was down are made up once, not once per miss.
- Its exit code says how it went: `0` ok; `10` the person should look (the last three lines of
  what it printed, standard output then standard error, reach them as a notice: print what and
  why last, short); anything else failed. Three failures in a row pause it and the person is told.
- State only in `$CLIO_STATE_DIR` (its own folder, kept between runs): what it saw last, a lock,
  a counter. The rest of the machine is read-only for it, but for the paths in `writes`. What it
  writes is private to the person (it runs with `umask 077`).
- Short, plain output: the last lines of each run are kept and shown. Never print a secret: it is
  struck out anyway.
- Every host declared, every write path declared: what is not declared fails when it runs.
- No secret in any file, not even an example that looks like one.
- A check that runs locally, without the network: the reviewer has none. For a service that calls
  a host, keep the network call in one small function and check the rest (parsing a saved answer,
  what it does with it).

## After service_write

A reviewer runs the check; a failed review comes back to you with its findings (three in a row
and the draft is dropped). Then the person sees the whole service in an approval box and says yes
or no. Once registered, `service_status` shows how its runs went. To change it, write a new
version with the same name: it is reviewed and approved again. You cannot run, turn off or remove
a service: the person does that with `/services`.

## How to verify

d=$(mktemp -d) && printf 'date +%%s > "$CLIO_STATE_DIR/last"\n' > "$d/run.sh" && bash scripts/check-run.sh "$d" 'bash run.sh' | grep -q '^ok' && ! bash scripts/check-run.sh "$d" 'exit 3' >/dev/null 2>&1 && rm -rf "$d"
