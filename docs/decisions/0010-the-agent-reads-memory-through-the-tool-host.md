# 0010: The agent reads memory through the tool host, with two read-only tools

Status: accepted (2026-11-03). Stage 1 (reading) and stage 2 (`memory_propose`, into quarantine only) are built. Design: `docs/specs/2026-11-03-memory-tools-design.md`.

## Context

The chat with Pi, the main path, did not use memory: the guard's `memoryLayer()` was empty, and `memory_link` served only `clio run` (the own loop) and the memory screen. The person asked for "a tool that connects clio agent with clio memory", made "the best way for the security and integrity of the data". Reading cannot hurt the data; writing can drown the person's review (S-25), and a hostile text that reaches memory persists (`docs/threat-model.md`, "La memoria como superficie").

## Decision

Two tools, `memory_search` and `memory_get`, **served by the tool host** (ADR 0007), registered by the guard, read only.

- **The tool host answers, not Pi and not the guard.** Pi runs in a jail that does not see Clio's data (ADR 0009). The host already talks to the vault and decides each call with the gateway; the guard only registers the tools and forwards the request, as it does with `read`. Without a `MemoryPort` in its `Config` the operations are not declared and the gateway denies them (S-02): the loose `clio-toolhost` binary has none.
- **The library, not the `clio memory` CLI.** `cli/memory.rs` is a thin layer over `MemoryLink`; the tools use that same layer (`LinkPort`, in `clio-brain`) inside the service. A process per call would load the vector model (tens of MB, seconds) every time, need arguments escaped and a text output read back. The contract of the tools would not change if the inside did.
- **Opened read only** (`MemoryLink::open_read`): a bug cannot write, the writer lock is never taken, so `clio memory remember` is never blocked. Consequence: reading leaves no usage signals; the statistics only show what `clio run` and the person retrieve.
- **Only active memories of the scope `clio`.** Missing, quarantined, archived and another scope's give the same error text, so none leaks its existence.
- **What comes back is data.** Cleaned before the cut (a secret is never split), one line per memory of at most 600 characters, at most 4 KiB per answer, wrapped with source `memory`. The session is tainted only when a memory is not the person's (origin `agent` or `external`): tainting on every read would be ignored, never tainting would make S-05 mean nothing.
- **A hard budget per session** (S-47, rule `S-47/memory`): 20 calls and 24 KiB of returned text; with under 1 KiB left it counts as spent. It lives in a file of the run folder, so a restarted service does not give a fresh one, and it counts every call the gateway let through, also the failed ones. The log (`memory.jsonl`) holds facts only: ids, counts, a short hash of the query, never a text.
- **The prompt** is `clio-agent@5`: it says the results are data that may be stale or hostile, that `agente` and `externo` are not what the person said, and that the budget is not for browsing.

## What it costs

- The model's provider sees what a tool returns, as it already sees the context (S-44). The budget limits how much can leave, not whether. Whatever must not leave the machine should not be in memory.
- Pi's user is the service's user: a taken-over Pi can make the calls the model can, with the same gateway and the same budget in front. It cannot open `memory.db`: the jail does not see it.
- An active memory of origin `agent` may hold an old injection that a clean run wrote without knowing. The defence is the wrapper and the taint, not a text filter.
- An order written inside the copy is carried out even if it comes from a memory (a write in the working copy is reversible, and the gateway does not refuse it in a tainted session). The defence is that it arrives as data, the session is marked, and the person reviews the copy with `diff` and `apply`. Only orders that point outside the folder or into a protected one are refused (S-13, S-16); attack 56 orders `../PWNED.txt` for that reason.
- There is no sensitivity class: everything active can be searched. The budget is per session, so a new session starts with a new one.
- The first call may be slow when the vector model is on disk (it loads then, once); the call has a 10 s deadline and fails with a short message, never hangs.

## Stage 2: `memory_propose`

Built after the cap on quarantine existed (50 pending per actor, one candidate per untrusted text). The tool takes `text` (1 to 400 characters after cleaning) and `kind` (`semantic` or `procedural`); the actor (`agente`), the origin (external) and the scope (`clio`) are not arguments, the port fixes them, so the call can only leave a candidate in quarantine and the person is the only one who promotes (S-61). The text goes through the vault's cleaner first and a text that had a secret is not proposed; the engine adds its own detector, the per-hour quota of the actor `agente` and the cap on quarantine; a procedure also waits for approval (S-26). A session may propose five (a file in the run folder, counted before the port is asked, so failed calls spend it and a restart does not reset it). A proposal does not mark the session as tainted: it holds what the model said, not what a memory said. The memory is opened for writing for the call and closed after, because the engine holds the writer lock while it lives; if another writer has it, the open waits two seconds and then fails with a short message. The close of a run (`RECORDAR:` / `PROCEDIMIENTO:` lines of the builtin loop) keeps working and shares the quota and the cap.

The secret check looks at the text as typed and as it will be proposed (one line): a secret with spaces cannot get through by swapping a space for a newline or a no-break space. A call that runs out of its 10 seconds tells the model it failed while the write may still complete in the background; the candidate is in quarantine either way and the call was counted.

Rulings: proposals are made during the session, not only at its close (a Pi chat has no single "close"); a text with a secret is refused instead of proposed with a hole in it; a refused call counts against the five; the vector index is not touched (a candidate gets its vector when the person's own commands embed what is pending).

## Tests

- `crates/clio-toolhost/tests/memory.rs`: the port, the budget (survives a restart, counts failed calls, hard cap), the facts log, tainting by origin.
- `crates/clio-brain/tests/memory_port.rs`: `LinkPort` over a real database, read only, one error for the four cases.
- `crates/clio-brain/tests/pi_stack/memory.rs`: the real Pi inside its jail, attacks 56 to 61 (61: what the model proposes waits in quarantine, from a tainted session too, and the sixth is refused). An order inside an active memory of the agent arrives as data and taints the session (56); repeated searches end at the cap, by calls and by bytes (57); a value of the vault inside a memory arrives struck out and is in no file of the run folder (58); quarantined, archived and foreign memories are not readable and their errors are identical (59); arguments that do not exist are refused and nothing that promotes, archives or purges can be asked, of the model or of the socket (60).
