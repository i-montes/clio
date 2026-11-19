# clio-toolhost

The per-session service that runs the primitive operations of Pi's tools (ADR 0007: Pi brings `read`,
`write`, `edit`, `bash`, `find`, `ls`; Clio only changes where they run). It is the only thing that
touches the machine: Pi's `clio-guard` extension only sends it requests.

```
clio-toolhost --socket-dir D --work W --scratch S --run-dir R [--rules FILE] [--jail auto|off] [--vault-socket P] [--memory-data D [--memory-model M]]
```

- `clio toolhost ...` (hidden subcommand of the `clio` binary) takes the same arguments and is what the interface starts for a Pi session: no second binary to install.
- `D` must exist with mode 0700 and belong to the user; the socket is `D/toolhost.sock` (0600). Otherwise it does not start.
- `W` is the session's working copy; `S` (snapshots) and `R` (effects log) go outside `W`.
- It prints `READY <socket path>` and runs until its stdin is closed (so it does not outlive whoever started it).
- It needs `HOME`: the jail's self-test plants its decoy there and checks that it is not visible.
- `--jail auto` offers `exec` (Pi's `bash`) only if `bwrap` starts and the jail's self-test passes; otherwise `exec` is not declared and the gateway denies it (S-02): `bash` fails closed. `--jail off` does not offer it.
- `--rules` is a JSON of gateway rules: only the circuit breaker threshold and extra protected paths are taken from it. The working folder, the declared tools and the mandatory protected paths (`extensions`, `skills`, `.pi`, `.claude`, `.git`, `.agents`, `.vscode`, `.husky`, `.githooks`, `.cargo`, `.devcontainer`) are always the service's own.
- `--memory-data D [--memory-model M]` offers `memory_search`/`memory_get`/`memory_propose` over whatever the binary's factory (`cli::run_with`) makes of the two folders; `clio toolhost` creates `clio-brain`'s `LinkPort` (it reads read-only and proposes by opening the memory for writing only during the call). A bare `clio-toolhost` has no memory and rejects `--memory-data`. `M` (the vector model) is passed by the parent: this process runs with another `HOME`.
- `CLIO_TOOLHOST_SCRUB_JSON='{"name":"value"}'` teaches values to the scrubber; `--vault-socket` adds the vault's filter (if it does not answer, the output is withheld). The real link with the session's vault is still to do.

### Host mode (ADR 0012)

```
clio-toolhost --socket-dir D --scratch S --run-dir R --host-home H --host-state E --host-run ID [--host-control P]... [--host-rules FILE] [--jail auto|off]
```

- The tools act on the real machine (`/`) instead of a copy; `--work` is not needed. An absolute path is used as it is and a relative one is taken from `H`; commands start in `H`.
- `--host-control P` (repeatable): Clio's folders. The file tools deny them wherever the path really lands (canonicalized; links and `..` included), rule `host/control`, text `that belongs to Clio: only the person touches it`; a command sees them empty and read-only; `fs_glob` does not enter them.
- `--host-state E` (`Dirs.data/host`): the action log `actions.jsonl` (0600, one line before and one after each `exec`, `fs_write` and `fs_mkdir`, and one for what is denied or set aside; if the one before cannot be written, the action does not run) and what is set aside in `pending/<id>.json`.
- `--host-run ID`: the run's id, for the log and for what is set aside. `--host-rules FILE`: the person's `host.json` (it only adds: `foreign`, `ask`; a `questions_per_day` from before is ignored). In `ask`, the classes are `delete`, `spend`, `send` and `control`; a `host.json` written with `borrar`, `gastar` and `enviar` is still read.
- `exec` runs with `clio-node`'s host launcher (`systemd` with `sudo -n systemd-run` if there is passwordless `sudo`, otherwise `bwrap --bind / /`), only if its self-test passed at start; otherwise it is not offered. Time limit of 120 s if none is given, up to 3600.
- `fs_write` and `fs_mkdir` leave a copy and a line in `R/changes.jsonl` before changing anything; `host::undo(R)` undoes it.

| Operation | Arguments in host mode |
|---|---|
| `exec` | `command`, `timeout?` (s, up to 3600), `network?` (bool: without it the command has no network; with it the session is marked, source `web`) |

- **Mark**: in this mode, reading your files or running a command does not mark the session; reading under a foreign folder does (source `foreign`: `/var/log`, `/var/mail`, `/var/spool`, `uploads`, `Downloads`, `Descargas` and those of `host.json`), and so does a command with `network: true` (`web`) and a command that names a foreign folder or reads logs (`journalctl`, `docker logs`, `docker compose logs`, `kubectl logs`).
- **Set aside** (rule `host/aside`, `ok: false`): a command or write that the list in `src/ask.rs` classifies (`delete`, `spend`, `send`, `control`) does not run; it is saved and the answer is `set aside {id}: «{class}» needs the person's approval. Do not try it another way. Go on with the rest and say so at the end.` The same action again in the same session gets the same `id`. A session with 20 items waiting gets no more: the answer is `[toolhost] {tool} was not run: it needs the person's approval and could not be set aside (too many actions wait for the person: wait for their answers)`. An item more than 7 days past its time is removed when the store is read; one claimed for an answer that never wrote its result (`<id>.ran` with the item still `waiting`) is `pending::interrupted` and is never offered again. `ToolHost::approve(id, now)` runs it once, as it was saved (same hash), and not after it expires (24 h: `expired on {date}: it did not run`); `pending::reject` discards it. Pending items saved with the old class names (`borrar`, `gastar`, `enviar`) are still read.

## Protocol

One JSON per line, several requests per connection, a cap of 4 MiB per line and 30 s to read.

```
→ {"id","tool","args":{...},"session","call_id","tainted":false}
← {"id","ok","content","data","decision":{"outcome","rule","reason"},"tainted","external"}
```

`tool` is a primitive operation of Pi's tools (the `clio-guard` extension builds them with `operations`):

| Operation | Arguments | Result |
|---|---|---|
| `fs_read` | `path`, `raw?` | `content` (cleaned; with `raw` the bytes as they are, only for `edit`) |
| `fs_stat` | `path`, `mode?` (`exists`, `r`, `rw`) | `data.exists`, `data.is_dir`; with `r`/`rw` it fails if there is no access |
| `fs_list` | `path` | `data.entries` |
| `fs_glob` | `path`, `pattern`, `limit?` | `data.paths` (does not follow links; ignores `.git` and `node_modules`) |
| `fs_write` | `path`, `content` | atomic replacement, creates the folders |
| `fs_mkdir` | `path` | |
| `exec` | `command`, `timeout?` (s, cap 120) | `content` = output (stdout and stderr), `data.exit_code`, `data.signal`, `data.timed_out`; always in the jail, only if the self-test passed |
| `memory_search` | `query` (up to 200 characters), `limit?` (1 to 8, default 5) | `content` = one line per active memory, `[m{id}] {kind} · {persona\|agente\|externo} · {date}: {text}`, wrapped as data (`UNTRUSTED-DATA memory`); `data.ids`, `data.count`, `data.masked`. Only if the service received a `MemoryPort` |
| `memory_get` | `id` (integer > 0) | the same for one active memory. Without a `MemoryPort` all three are denied (S-02) |
| `memory_propose` | `text` (1 to 400 characters, after cleaning), `kind` (`semantic` or `procedural`) | `content` = a fixed line («proposed: …»). The service cleans the text with the vault (if it had a secret it is not proposed), puts it on one line and the port saves it as a candidate in quarantine, actor `agente`, origin external, scope `clio`; there are no arguments for that. Effect `LocalReversible`, no network egress. Five per session (`R/memory-proposals.json`, `S-47/memory`; they count even if they fail); it does not mark the session (S-61) |
| `filter` | `items: [{text, wrap?: "file"\|"tool-result"}]` | `data.items`: each text sanitized, cleaned and, if it asks for `wrap`, wrapped as external data. No effects: it does not go through the gateway |
| `judge` | `text`, `source` | outside text the brain hands the model itself (what an action the person approved printed): `content` is what the model may get, the text cleaned or the notice; `data` as `filter`'s. The judge reads all of the text, in 24 KiB windows that overlap by 2 KiB, asked at once; beyond 16 windows it counts as unjudged |
| `jobs`, `job_output`, `job_stop` | `id` (the last two) | host mode only: the background commands of the session (how many run, what one printed since the last read, whole lines only while more may come, stop one) |

While a command runs, `filter`, `judge`, `jobs`, `job_output` and `job_stop` are still answered (they take no lock the command holds); a request whose client left while it waited for the one before is refused and runs nothing. `R/jobs/` (the background commands' output) is removed when the session's jobs end. `request_secret` takes a destination `TOOL:HOST`: the tool is `[a-z_]+` and the host has at least two labels. A command that was set aside and then approved runs with a time limit of 3600 s if it asked for none, and what it printed is judged before the model sees it.

Paths are relative to the working copy. Every operation (except `filter`) goes through: `Gateway::decide_or_deny` (its own rules, deny by default, fails closed) → rule `S-12/bash` for `exec` (not empty, no NUL, up to 8 KiB) → `clio_node::Executor` on the copy (canonicalized paths, links that point outside rejected, protected paths) or the jail for `exec` → sanitizing and the vault's scrubber.

- **Taint (S-05)**: per session, it only goes up. Reading a file, listing, searching or running a command sets the mark; `tainted` in the request raises it (it does not lower it); `tainted` is in every answer.
- **Memory (S-60)**: read-only, through `Config`'s `MemoryPort`. It comes out clean (the scrubber before the cut), one line per memory (600 characters at most) and 4 KiB at most in total, wrapped as data and `external`; but it marks the session only if some memory is not the person's, unlike files and commands, which always mark. The port runs on another thread with a 10 s deadline and a net against panics; a failure is a short error for that call.
- **Memory budget (S-47, rule `S-47/memory`)**: 20 calls and 24 KiB of returned text per service (per `R` folder), with a hard cut. It comes after the gateway and before validating and calling the port; it counts every call the gateway let through (also invalid ones or ones with the port in error, not those the gateway denied). Each answer is limited to what is left (`min(4 KiB, 24 KiB - spent)`); with less than 1 KiB left or 20 calls spent, it is denied with `S-47/memory`, without touching the port. That denial does not count toward the gateway's circuit breaker (like `S-12/bash`). The spend is in `R/memory-budget.json` (`{"calls","bytes"}`, 0600, written with a temporary file and `rename`): it survives a restart of the service. If it cannot be read, understood or written, the call is denied: it is never left without a cap.
- **Fact log**: `R/memory.jsonl` (0600, append-only), one line per call that passed the budget (in `memory_propose`, `ids` is the candidate's id): `at` (ms), `tool`, `results`, `ids`, `bytes`, `tainted` (this call marked the session) and `query_hash` (8 hex of the SHA-256 of the cleaned query; `null` in `memory_get`). Never the text of the query nor of a memory. If it cannot be written, the call is denied.
- The gateway's **«ask»** is denied (`NobodyHome`); `with_asker` allows setting another one. After 10 denials in a row in a session, what was going to «ask» is therefore denied.
- **`exec` and protected folders**: those that do not exist are created empty during the command (the jail mounts them read-only) and removed afterwards.
- Files over 8 MiB are not read; outputs over 2 MiB are withheld whole (they are not cut before cleaning).
- An invalid line, a session with odd characters or an unknown field run nothing.

## Tests

`cargo test -p clio-toolhost`. `tests/toolhost.rs` and `tests/hardening.rs` test the service alone; `tests/host.rs`, host mode (only with `bwrap`, never `sudo`, in temporary folders; also case 62 of the battery); `tests/judge.rs`, the judge at the door (cases 63 and 64).
The tests with the real Pi are in `clio-brain` (`cargo test -p clio-brain --test pi_stack`, folder
`crates/clio-brain/tests/pi_stack/`): they launch Pi through `pi::launch::build`, inside its jail, with the
hardened flags and `clio toolhost`, against a fake OpenAI-compatible model
(`common/fake_model.rs`, 127.0.0.1 only). Without `node` or without `npm ci` they say `SKIPPED`; with
`CLIO_REQUIRE_PI=1` they fail. The child processes have `Drop` guards.
