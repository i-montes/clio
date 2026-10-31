# clio-memory

A generic memory engine for agents on a single SQLite file. No GPU, no external services, and no dependency on any other Clio crate.

- **One file, one writer.** The first process to open the file for writing takes a lock; others open it read-only, and a write from them returns a clear error. A read-only open never creates, initialises or changes the database file or the lock; SQLite itself may create `-wal` and `-shm` files next to a WAL database, so the directory must be writable.
- **The log is the truth.** Every change is an operation in an append-only log; the memory table and the text index are rebuilt from it with `rebuild_projection`.
- **One way in.** Content only enters through `propose`: quota per actor, secret detector, then deterministic rules. External content and agent-proposed procedures wait in quarantine until a person calls `promote`.
- **Bounded input.** `Limits` caps the input and the stored (NFC-normalized) text in bytes (16 KiB for content, 256 for `topic_key`, `session`, `episode`, `source` and names); raise it with `Engine::with_limits` for long procedures. A proposal over the limit is rejected, leaves nothing behind and still counts against the actor's quota.
- **Nothing is deleted.** `revert` and `archive` append operations; a fact replaced by a newer one keeps its validity interval, so you can ask what was true (`as_of`) or what was known (`known_at`) at a date.

```rust
use std::sync::Arc;
use clio_memory::*;

let engine = Engine::open("memory.db".as_ref(), OpenMode::ReadWrite, Arc::new(SystemClock))?;
let scope = engine.create_scope("main", "es")?;
let actor = engine.register_actor("ana", ActorKind::Person, Quota::default())?;
let out = engine.propose(Proposal {
    scope, actor, kind: MemoryType::Semantic, content: "Vive en Medellín".into(),
    topic_key: Some("residencia".into()), origin: Origin::User,
    session: None, episode: None, source: None, valid_from: None, valid_to: None,
})?;
let hits = engine.search(scope, "Medellín", SearchOpts::default())?;
engine.revert(out.op, actor)?;
# Ok::<(), Error>(())
```

## Finding things

- **`embed_pending(limit)`** computes vectors for memories that have none for the current model (`Engine::with_embedder`). `propose` never calls the model; until a memory has a vector it is found by text only. Switching models re-embeds everything on the next call.
- **`recall(scope, query, opts)`** mixes full text and vectors (reciprocal rank fusion, a small recency boost, and a small nudge from feedback) and answers "nothing relevant" (`abstained`) instead of padding the list. Each hit carries its text, vector and recency scores and its confidence. Vector search is exact and written in safe Rust: about 70 ms for a whole `recall` over 50 000 memories of 256 dimensions on a Ryzen 7 5700X.
- **`context(scope, opts)`** builds what goes into the prompt: fixed blocks (identity, rules, preferences; only a person edits them, only active memories enter, and they follow updates) and then what `recall` finds, each under its own token budget, as one short line per fact or as JSON.
- Optional feature `model2vec` adds `Model2VecEmbedder::from_dir`, which reads a local Model2Vec model directory. The crate never downloads models and never opens network connections. `just setup-model-macos` or `just setup-model-linux` installs the default model (potion-multilingual-128M, pinned and checked by sha256) in `~/.cache/clio-models/`.

## Deciding what to store

`propose` first applies fixed rules (exact duplicate, untrusted origin, same `topic_key`). Memories that look alike but say different things stay as different memories; replacing is always explicit, by `topic_key`. Whatever the rules leave as a plain "add" can be put to **deciders** registered with `Engine::with_decider` (a classifier, Jev, an LLM): each is `Active` or `Shadow` (consulted and logged, never acted on, to compare it before trusting it), sees the proposal plus its most similar memories, and can only answer *add* or *review* (the proposal goes to quarantine for a person). A decider can never drop, replace, archive or delete: with a real model a contradiction can score closer to a memory than a paraphrase does ("No vive en Medellín" 0.90 vs "Reside en Medellín" 0.81 against "Vive en Medellín"). Errors, panics and invalid answers are recorded and skipped. Every answer goes to a decision log (`Engine::decisions`) written in the same transaction as the operation. `NearDuplicateDecider` (off by default) asks for review when the closest neighbor is almost the same text.

## Background work

Work that does not belong inside `propose` or a read lives in a job queue in the same SQLite file. A job is queued in the same transaction as the operation that causes it, so it is never lost and never orphaned, and identical pending jobs are merged (ten updates in a row make one `Embed` job). `Engine::run_jobs(&RunOpts)` runs what is due: it takes over jobs whose worker died (expired lease), retries failures with a growing wait (60 s, doubling, up to an hour; five attempts, then the error queue), and sends failures that retrying cannot fix straight to the error queue. There is no scheduler: the caller (a daemon, the CLI) decides when to call it, and passes `allow_heavy` when the machine can afford a backup or a `VACUUM`. Jobs: `Embed` (computes the missing vectors, only when an embedder is set), `Touch` (a `recall` on a read-write engine records which memories it returned; pending ones are applied together in one batch; frequent use gives a small, capped boost, always below the recency boost), `Decay` (archives unused *working* and *episodic* memories after the scope's `retention_days`, never what a fixed block shows and never semantic or procedural memories; it signs with an internal system actor that has no review powers), `Backup` (hot copy with SQLite's backup API, private 0600, never overwrites, **not encrypted**) and `Vacuum`. `Engine::jobs` and `Engine::queue_stats` show the queue; finished jobs are pruned after 7 days and the error queue after 90.

## Feedback and confidence

`Engine::feedback(memory, Signal::Useful | Signal::Wrong, actor)` records what an agent or a person thought of a memory that `recall` brought up. Confidence starts at 0.5, each *useful* adds 0.1 and each unspent *wrong* takes 0.2 (clamped to 0..1; `Engine::confidence`, `Engine::feedback_summary`). It scales a hit's score by 0.9 to 1.1 (never more than the recency boost, so it breaks ties and does not reorder everything), `RecallHit::confidence` shows it, and a *useful* signal counts as activity, so the memory is not forgotten for lack of use. A signal can never delete, replace or edit anything. The third unspent *wrong* writes one `Flag` operation, signed by the actor who gave it, that sends the memory to quarantine for a person to review (`list`, `promote`, `archive`); `promote` and the flag itself spend the wrong signals, so three new ones are needed to flag it again, and `revert` undoes the flag. Signals count against the actor's quota and need an active memory.

## Looking after the file

`Engine::scopes`, `actors` and `stats` (file and WAL size, memories by state and scope, operations, decisions, vectors, pending embeddings, queue) only read, so they work on a read-only engine. `Engine::doctor` checks the file without writing: permissions, schema version, that `memories` is exactly what folding the log gives (and names the ids that differ), orphan vectors, usage, feedback and block entries, and the error queue. A failed check is a finding in the report, not an `Err`.

## Export and import

`Engine::export(&mut impl Write)` writes JSON Lines from one consistent snapshot: a header (`format`, `format_version`, `schema_version`, `exported_at`) and then `scope`, `actor`, `operation` (with full before/after snapshots and the `trace_id`), `decision`, `block`, `block_memory`, `usage` and `feedback` lines. Vectors (recomputed), jobs and quota counters are left out; actors keep their quota limits. `export_to_file` creates the file new with mode 0600 and never overwrites. Exports are **not encrypted**. The format has a semver (`EXPORT_FORMAT_VERSION`): a reader takes the same major version and nothing newer than its own minor.

`Engine::import(&mut impl BufRead)` **restores** an export into an empty file (no scopes or operations; the internal system actor is replaced), keeping every id, in one transaction: either everything comes back or the file is untouched. It refuses another major version, a newer minor, a different schema version (that would need a migration that does not exist yet), unknown or malformed lines (with the line number), lines over 8 MiB, and anything that looks like a secret. **Trust model:** a restore writes operations as they are, so a doctored file could bring back memories that never went through quarantine. Use it for exports this program wrote. To load memories from anywhere else use `Engine::import_memories(reader, scope, actor, &ImportOpts)`: one JSON object per line (`content`, optionally `kind`, `topic_key`, `valid_from`, `valid_to`, `source`), each sent through `propose` (quota, secret detector, rules, deciders) with origin `External` by default, so they wait in quarantine. The actor must be an importer or a person. A line that cannot be accepted is rejected and counted with its line number (the first 100 are kept) and the rest go on.

## Command line

`clio-memory` is a binary of this crate with no extra dependencies. It opens the file read-only for everything that only reads (`scopes`, `stats`, `doctor`, `list`, `get`, `history`, `review`, `jobs`, `recall`, `context`, `export`), so it works while another program has the file open and never changes it (SQLite itself keeps `-wal` and `-shm` files next to a WAL database while it is read); commands that change something (`promote`, `archive`, `revert`, `feedback`, `retry-job`, `run-jobs`, `backup`, `import`, `import-memories`) take the writer lock and say so if someone else holds it. They act as a person called `cli:$USER` (or `--actor NAME`, which must be a person), created on first use. It has no way to register actors, create scopes or set retention: whoever can register a person can promote, so that stays in code. Text read from memory is shown with control characters replaced, so a stored escape sequence cannot reach your terminal. Exit codes: 0 fine, 1 error (or a failed `doctor` check, or rejected import lines), 2 wrong usage. `--json` prints one JSON document for most read commands. `run-jobs` will not start while `Embed` jobs are waiting and no `--model` is given (they would fail for good); `--model DIR` needs the `model2vec` feature.

```text
$ cargo run -p clio-memory --example demo -- demo.db     # a scope, two people and three memories
$ clio-memory --db demo.db stats
file: 139264 bytes (wal 0)
operations: 3 · decisions: 0 · vectors: 0 · pending embeddings: 0
queue: pending 0 · running 0 · failed 0
scope main: active 2 · quarantine 1 · archived 0 · replaced 0
$ clio-memory --db demo.db review main              # what came from outside waits here
m3 · Quarantine · Semantic · El periódico se fundó en 2010
$ clio-memory --db demo.db promote 3
promoted m3 (op 4)
$ clio-memory --db demo.db recall main dónde vive
m1 · Active · Semantic · Vive en Medellín · topic residencia · score 0.0180
$ clio-memory --db demo.db feedback 1 useful
recorded useful for m1 · confidence 0.60 · useful 1 · wrong 0
$ clio-memory --db demo.db export demo.jsonl
$ clio-memory --db restored.db import demo.jsonl     # into a new file
$ clio-memory --db restored.db doctor                # ok on every line
```

## Telemetry (optional)

The `telemetry` feature adds `tracing` (and nothing else) and turns on spans and events: `propose` with its steps (`secrets`, `normalize`, `rules`, `neighbors`, one `decider` each, `append`), `recall`, `context`, `promote`, `archive`, `revert`, `feedback` and every job; events for a second writer turned away, the embedding model in use, reverts, backups and jobs that end in the error queue. Fields hold ids, sizes, counts, outcomes (a fixed word, never an error message) and times, never memory text, topic keys, names or paths. The crate installs no subscriber and exports nothing: the program that embeds it chooses (OpenTelemetry included). Each operation stores the id of the trace that caused it (`operations.trace_id`, `Operation::trace_id`, also in exports): with the feature, one id per top-level call, taken from the provider you give to `Engine::with_trace_id_provider` (connect it to your OpenTelemetry context; it must not call back into the engine) or generated as 128 bits when there is none. The same id is the `trace_id` field of the span of that call. Without the feature there is no `tracing` code or dependency, the column stays empty and there is no telemetry table.

## Status

The v1 scope is complete: the core (log, quarantine, revert, text search), vectors with hybrid recall and `context()`, the decider cascade with its decision log, the job queue (embedding, usage, forgetting, backup), feedback and confidence, `stats`/`doctor`/export/import and the command line, and optional telemetry. Not in v1: the daemon (HTTP and MCP), `PURGE`, retention rollups, encryption of backups and exports, importers for other tools' formats. Design: [`docs/specs/2026-10-29-clio-memory-core-design.md`](../../docs/specs/2026-10-29-clio-memory-core-design.md), [`docs/specs/2026-10-29-clio-memory-recall-design.md`](../../docs/specs/2026-10-29-clio-memory-recall-design.md), [`docs/specs/2026-10-29-clio-memory-final-pieces-design.md`](../../docs/specs/2026-10-29-clio-memory-final-pieces-design.md) and the full spec [`docs/specs/clio-memory-spec-v0.2.md`](../../docs/specs/clio-memory-spec-v0.2.md).
