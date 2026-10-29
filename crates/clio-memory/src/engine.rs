use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use crate::clock::Clock;
use crate::decider::Decider;
use crate::embed::{Embedder, panic_message, unit_vector};
use crate::error::{Error, Result};
use crate::format::{compact_line, json_item};
use crate::model::{
    ActorId, ActorInfo, ActorKind, Block, BlockId, Candidate, Change, Choice, ContextOpts,
    DeciderMode, DecisionRecord, DecisionStatus, DoctorReport, ExportSummary, FeedbackSummary,
    Format, Hit, ImportOpts, ImportSummary, JobKind, JobRecord, JobState, Limits, MemoryId,
    MemoryRow, MemoryType, Neighbor, NewOperation, OpId, OpKind, OpenMode, Operation, Origin,
    Outcome, Proposal, Question, QueueStats, Quota, Recall, RecallHit, RecallOpts, RevertReport,
    RunOpts, RunReport, ScopeId, ScopeInfo, SearchOpts, Signal, State, Stats,
};
use crate::normalize::{content_hash, normalize};
use crate::platform;
use crate::recall::{confidence_of, fuse};
use crate::rules::{Existing, Verdict, decide, effective_origin};
use crate::secrets::find_secret;
use crate::sqlite::SqliteStore;
use crate::stem::{content_tokens, tokens};
use crate::store::{NewDecision, NewJob, Store};
use crate::token::{ApproxTokens, TokenCounter};
use crate::transfer::{Line, LooseMemory, parse_kind, read_line_bounded};

/// `(memory ids, time of the query)` from a `Touch` payload; `None` if it is malformed.
fn parse_touch(payload: &str) -> Option<(Vec<MemoryId>, i64)> {
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let at = v["at"].as_i64()?;
    let ids = v["ids"]
        .as_array()?
        .iter()
        .filter_map(|i| i.as_i64().map(MemoryId))
        .collect();
    Some((ids, at))
}

/// Failures retrying cannot fix go straight to the error queue.
fn is_permanent(e: &Error) -> bool {
    use std::io::ErrorKind::{AlreadyExists, NotFound, PermissionDenied};
    match e {
        Error::NoEmbedder
        | Error::UnknownScope
        | Error::UnknownActor
        | Error::NotFound
        | Error::ReadOnly
        | Error::BadJobPayload
        | Error::Json(_) => true,
        Error::Io(io) => matches!(io.kind(), AlreadyExists | NotFound | PermissionDenied),
        _ => false,
    }
}

/// `s` cut to at most `max` bytes at a character boundary.
fn clip(s: &str, max: usize) -> String {
    let mut end = max.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// The change that retires `old` in favor of `new`: `old` becomes `Replaced`, valid until `new`
/// starts. `new` may start exactly when `old` did (then `old` was valid for zero seconds), never
/// before.
fn replace(old: &MemoryRow, new: &MemoryRow) -> Result<Vec<Change>> {
    if new.valid_from < old.valid_from {
        return Err(Error::InvalidValidity);
    }
    let mut after = old.clone();
    after.state = State::Replaced;
    after.valid_to = Some(
        old.valid_to
            .map_or(new.valid_from, |t| t.min(new.valid_from)),
    );
    after.replaced_by = Some(new.id);
    Ok(vec![Change {
        memory: old.id,
        before: Some(old.clone()),
        after,
    }])
}

/// The memory engine: one write path (`propose`), reads, and reversible changes.
pub struct Engine {
    store: Box<dyn Store>,
    clock: Arc<dyn Clock>,
    /// Serializes every write so lookups and the append that follows cannot interleave.
    write: Mutex<()>,
    embedder: Option<Arc<dyn Embedder>>,
    tokens: Arc<dyn TokenCounter>,
    limits: Limits,
    mode: OpenMode,
    deciders: Vec<Registered>,
    /// The trace id of the top-level call holding the write lock (made when the first operation
    /// of the call is written, cleared when the call ends), so one call is one trace.
    #[cfg(feature = "telemetry")]
    trace: Mutex<Option<String>>,
    #[cfg(feature = "telemetry")]
    trace_provider: Option<crate::telemetry::TraceIdProvider>,
}

/// The write lock for one top-level call. With telemetry it also ends the call's trace.
struct WriteGuard<'a> {
    _lock: std::sync::MutexGuard<'a, ()>,
    #[cfg(feature = "telemetry")]
    trace: &'a Mutex<Option<String>>,
}

#[cfg(feature = "telemetry")]
impl Drop for WriteGuard<'_> {
    fn drop(&mut self) {
        // still holding the write lock: the next call starts with a clean slate
        *self.trace.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

struct Registered {
    decider: Arc<dyn Decider>,
    mode: DeciderMode,
    min_confidence: f32,
}

/// How long a claimed job may run before another run may take it over.
const LEASE_SECS: i64 = 300;

/// Done jobs are kept this long (to inspect what happened), then pruned.
const KEEP_DONE_SECS: i64 = 7 * 86_400;
/// The error queue (failed jobs) is kept longer: someone should get to look at it.
const KEEP_FAILED_SECS: i64 = 90 * 86_400;

/// Pending `Touch` jobs applied together.
const TOUCH_BATCH: usize = 1000;

/// Memories embedded per round when an `Embed` job drains the backlog.
const EMBED_RUN_BATCH: usize = 1000;

/// How many similar memories a decider is shown.
const NEIGHBORS: usize = 5;

/// Unspent "wrong" signals that send a memory to review.
const FLAG_AFTER_WRONG: u32 = 3;

/// Texts sent to the embedder per call.
const EMBED_BATCH: usize = 32;

impl Engine {
    /// `ReadWrite` takes the exclusive writer lock for the file (`WriterLocked` if another
    /// process holds it); `ReadOnly` can read but every write returns `ReadOnly`.
    pub fn open(path: &Path, mode: OpenMode, clock: Arc<dyn Clock>) -> Result<Self> {
        let store = SqliteStore::open(path, mode).inspect_err(|e| {
            if matches!(e, Error::WriterLocked) {
                event!(WARN, "writer_lock_rejected");
            }
        })?;
        Ok(Self {
            store: Box::new(store),
            clock,
            write: Mutex::new(()),
            embedder: None,
            tokens: Arc::new(ApproxTokens),
            limits: Limits::default(),
            mode,
            deciders: Vec::new(),
            #[cfg(feature = "telemetry")]
            trace: Mutex::new(None),
            #[cfg(feature = "telemetry")]
            trace_provider: None,
        })
    }

    /// Connects the engine to the caller's tracing context: every operation records the id
    /// `provider` returns (an OpenTelemetry trace id, say), or a generated 128-bit id when it
    /// returns `None` or something that is not a plain identifier. A provider that panics is
    /// treated as returning `None`. It is asked once per top-level call, only when an operation
    /// is actually written, while the write lock is held: it must not call the engine.
    #[cfg(feature = "telemetry")]
    pub fn with_trace_id_provider(mut self, provider: crate::telemetry::TraceIdProvider) -> Self {
        self.trace_provider = Some(provider);
        self
    }

    /// The trace id for operations written by the current call; `None` without telemetry.
    #[cfg(feature = "telemetry")]
    fn trace_id(&self) -> Option<String> {
        use crate::telemetry::{acceptable, random_trace_id};
        let mut slot = self.trace.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_none() {
            let supplied = self
                .trace_provider
                .as_ref()
                .and_then(|p| panic_message(|| p()).ok().flatten())
                .filter(|id| acceptable(id));
            let id = supplied.unwrap_or_else(random_trace_id);
            // the span of the call that is writing (its field is declared empty)
            // only on this crate's own spans: the host's current span is not ours to write on
            let current = tracing::Span::current();
            if current.metadata().is_some_and(|m| {
                m.target()
                    .starts_with(module_path!().split("::").next().unwrap_or(""))
            }) {
                current.record("trace_id", id.as_str());
            }
            *slot = Some(id);
        }
        slot.clone()
    }

    #[cfg(not(feature = "telemetry"))]
    fn trace_id(&self) -> Option<String> {
        None
    }

    /// The model used by `embed_pending` and, from the next task on, by `recall`.
    pub fn with_embedder(mut self, embedder: Arc<dyn Embedder>) -> Self {
        self.embedder = Some(embedder);
        self
    }

    /// Registers a decider. Deciders run while the engine holds its write lock, so a decider
    /// must not call any write method of the engine it is registered on (it would wait for
    /// itself forever); reads are fine. A slow decider delays every write.
    ///
    /// Deciders are consulted in registration order, only for proposals the rules leave as a
    /// plain "add". An `Active` one whose answer has at least `min_confidence`
    /// is applied (the first one wins and later active ones are skipped); a `Shadow` one is always
    /// consulted and only recorded. A decider can only answer add or review (quarantine), so no
    /// answer can drop a proposal.
    pub fn with_decider(
        mut self,
        decider: Arc<dyn Decider>,
        mode: DeciderMode,
        min_confidence: f32,
    ) -> Self {
        // Fail closed: NaN would make `confidence < min` false and so act on any answer.
        let min_confidence = if min_confidence.is_nan() {
            1.0
        } else {
            min_confidence.clamp(0.0, 1.0)
        };
        self.deciders.push(Registered {
            decider,
            mode,
            min_confidence,
        });
        self
    }

    /// What the registered deciders said, `at` in `[from, to)`, oldest first.
    pub fn decisions(&self, scope: ScopeId, from: i64, to: i64) -> Result<Vec<DecisionRecord>> {
        self.store.decisions(scope, from, to)
    }

    /// The active memories of `scope` most like `content`, best first. Empty without an embedder,
    /// when the embedder fails or cannot represent the text, when a store read fails, and for
    /// memories that have no vector yet: a decider gets less context, never a failed write.
    fn neighbors(&self, scope: ScopeId, content: &str) -> Vec<Neighbor> {
        let Some(embedder) = &self.embedder else {
            return Vec::new();
        };
        let vector = match panic_message(|| embedder.embed(&[content])) {
            Ok(Ok(mut vectors)) => vectors
                .pop()
                .and_then(|v| unit_vector(v, embedder.dim()).ok()),
            _ => None,
        };
        let Some(vector) = vector else {
            return Vec::new();
        };
        let hits = self
            .store
            .nearest(
                scope,
                embedder.model_id(),
                &vector,
                &SearchOpts::default(),
                NEIGHBORS,
            )
            .unwrap_or_default();
        hits.into_iter()
            .filter_map(|(id, similarity)| {
                self.store
                    .get(id)
                    .ok()
                    .map(|memory| Neighbor { memory, similarity })
            })
            .collect()
    }

    /// Text a decider hands back goes into the audit log: capped, and replaced when it looks like
    /// a secret (a decider may echo its prompt, which contains other memories).
    fn bounded(&self, text: &str) -> String {
        if find_secret(text) {
            return "[redacted: looked like a secret]".to_string();
        }
        clip(text, self.limits.max_field_bytes)
    }

    /// Asks the registered deciders and logs each answer. Errors, panics and invalid answers are
    /// recorded and skipped: a decider never blocks a write.
    fn consult(
        &self,
        question: &Question,
        now: i64,
    ) -> (Option<(Choice, String)>, Vec<NewDecision>) {
        let mut applied: Option<(Choice, String)> = None;
        let mut log = Vec::new();
        for reg in &self.deciders {
            if reg.mode == DeciderMode::Active && applied.is_some() {
                continue;
            }
            let name = clip(reg.decider.name(), self.limits.max_field_bytes);
            let mut rec = NewDecision {
                scope: question.candidate.scope,
                provider: name.clone(),
                mode: reg.mode,
                status: DecisionStatus::None,
                choice: None,
                confidence: None,
                reason: None,
                acted: false,
                at: now,
            };
            let _s = span!("decider", provider = name.as_str());
            match panic_message(|| reg.decider.decide(question)) {
                Err(msg) => {
                    rec.status = DecisionStatus::Error;
                    rec.reason = Some(self.bounded(&format!("panicked: {msg}")));
                }
                Ok(Err(e)) => {
                    rec.status = DecisionStatus::Error;
                    rec.reason = Some(self.bounded(&e.to_string()));
                }
                Ok(Ok(None)) => {}
                Ok(Ok(Some(d))) => {
                    rec.choice = Some(d.choice);
                    rec.confidence = Some(d.confidence);
                    rec.reason = d.reason.map(|r| self.bounded(&r));
                    if !(0.0..=1.0).contains(&d.confidence) {
                        rec.status = DecisionStatus::Invalid;
                    } else if reg.mode == DeciderMode::Active && d.confidence < reg.min_confidence {
                        rec.status = DecisionStatus::BelowThreshold;
                    } else {
                        rec.status = DecisionStatus::Decided;
                        if reg.mode == DeciderMode::Active {
                            rec.acted = true;
                            applied = Some((d.choice, name));
                        }
                    }
                }
            }
            log.push(rec);
        }
        (applied, log)
    }

    /// Replaces the default size limits (see `Limits`).
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    fn check_name(&self, name: &str) -> Result<()> {
        if name.len() > self.limits.max_field_bytes {
            return Err(Error::TooLong);
        }
        if find_secret(name) {
            return Err(Error::SecretDetected);
        }
        Ok(())
    }

    /// Replaces the default token estimate used for budgets.
    pub fn with_token_counter(mut self, counter: Arc<dyn TokenCounter>) -> Self {
        self.tokens = counter;
        self
    }

    /// Computes and stores vectors for up to `limit` memories (not archived) that have none for
    /// the current model; returns how many it stored. The embedder runs without holding the
    /// write lock.
    ///
    /// A batch with the wrong number of vectors or a wrong-sized vector fails with `BadVector`
    /// and stores nothing (the embedder is broken). A text the model cannot represent (an
    /// all-zero or non-finite vector, like an emoji-only text) is recorded as unembeddable and
    /// skipped so it cannot block the rest; if a whole batch is like that, a canary text tells a
    /// broken embedder (error) from genuinely unrepresentable texts (skipped).
    pub fn embed_pending(&self, limit: usize) -> Result<usize> {
        self.embed_pending_inner(limit).map(|(stored, _)| stored)
    }

    /// Returns `(stored, examined)`: how many vectors were stored and how many pending memories
    /// were looked at (including the ones recorded as unembeddable).
    fn embed_pending_inner(&self, limit: usize) -> Result<(usize, usize)> {
        if self.mode == OpenMode::ReadOnly {
            return Err(Error::ReadOnly); // before spending CPU on vectors that cannot be stored
        }
        let embedder = self.embedder.as_ref().ok_or(Error::NoEmbedder)?;
        let (model, dim) = (embedder.model_id(), embedder.dim());
        let pending = self.store.pending_embeddings(model, limit)?;
        let examined = pending.len();
        let mut stored = 0;
        for batch in pending.chunks(EMBED_BATCH) {
            let texts: Vec<&str> = batch.iter().map(|(_, t)| t.as_str()).collect();
            let vectors = embedder.embed(&texts)?;
            if vectors.len() != batch.len() {
                return Err(Error::BadVector);
            }
            let (mut good, mut skipped) = (Vec::new(), Vec::new());
            for ((id, _), v) in batch.iter().zip(vectors) {
                if v.len() != dim {
                    return Err(Error::BadVector);
                }
                match unit_vector(v, dim) {
                    Ok(unit) => good.push((*id, unit)),
                    Err(_) => skipped.push(*id),
                }
            }
            if good.is_empty() && !skipped.is_empty() && !self.embedder_works(embedder.as_ref())? {
                return Err(Error::BadVector);
            }
            let _g = self.write_guard();
            self.store.put_vectors(model, dim, &good)?;
            self.store.mark_unembeddable(model, &skipped)?;
            stored += good.len();
        }
        if examined > 0 {
            event!(
                INFO,
                "embedded",
                model = model,
                stored = stored,
                examined = examined
            );
        }
        Ok((stored, examined))
    }

    /// The `Embed` job to enqueue with an operation: any operation that leaves a memory that is
    /// not archived may have created one that needs a vector. One pending job covers them all
    /// (the dedupe key), so ten updates in a row make one job. Only with an embedder.
    fn embed_jobs(&self, changes: &[Change], now: i64) -> Vec<NewJob> {
        if self.embedder.is_none() || changes.iter().all(|c| c.after.state == State::Archived) {
            return Vec::new();
        }
        vec![NewJob {
            kind: JobKind::Embed,
            payload: "{}".into(),
            dedupe_key: Some("embed".into()),
            run_after: now,
            at: now,
        }]
    }

    /// Runs queued background work: first takes over jobs whose worker died (expired lease), then
    /// claims jobs best priority first up to `max_jobs`. Heavy jobs (backup, vacuum) only run with
    /// `allow_heavy`. Synchronous and driven by the caller: there is no scheduler.
    pub fn run_jobs(&self, opts: &RunOpts) -> Result<RunReport> {
        if self.mode == OpenMode::ReadOnly {
            return Err(Error::ReadOnly);
        }
        let mut report = RunReport {
            recovered: self.store.recover_leases(self.clock.now())?,
            ..RunReport::default()
        };
        for _ in 0..opts.max_jobs {
            let now = self.clock.now();
            let Some(job) = self
                .store
                .claim_job(now, opts.only, opts.allow_heavy, LEASE_SECS)?
            else {
                break;
            };
            let span = span!(
                "job",
                kind = format!("{:?}", job.kind).as_str(),
                id = job.id,
                attempts = job.attempts,
                trace_id = ::tracing::field::Empty,
                outcome = ::tracing::field::Empty,
            );
            if job.kind == JobKind::Touch {
                // every ready Touch goes in one batch: one transaction for the usage, one to close them
                let mut batch = vec![job];
                batch.extend(self.store.claim_jobs(
                    now,
                    JobKind::Touch,
                    LEASE_SECS,
                    TOUCH_BATCH,
                )?);
                let size = batch.len();
                let applied = {
                    let _g = self.write_guard();
                    self.apply_touch_batch(batch)
                };
                match applied {
                    Ok((ok, bad)) => {
                        report.done += ok;
                        report.failed += bad;
                        record!(span, "outcome", if bad == 0 { "done" } else { "failed" });
                    }
                    Err(_) => {
                        report.retried += size;
                        record!(span, "outcome", "retried");
                    }
                }
                continue;
            }
            match self.execute(&job) {
                Ok(()) => {
                    self.store.finish_job(job.id, self.clock.now())?;
                    report.done += 1;
                    record!(span, "outcome", "done");
                }
                Err(e) => {
                    let now = self.clock.now();
                    if is_permanent(&e) || job.attempts >= job.max_attempts {
                        self.store.fail_job(job.id, now, &e.to_string(), None)?;
                        report.failed += 1;
                        record!(span, "outcome", "failed");
                        event!(
                            WARN,
                            "job_failed",
                            kind = format!("{:?}", job.kind).as_str(),
                            id = job.id,
                            attempts = job.attempts,
                            permanent = is_permanent(&e)
                        );
                    } else {
                        // 60 s after the first failure, doubling each time, never more than an hour
                        let wait = 60i64
                            .saturating_mul(1i64 << (job.attempts.saturating_sub(1)).min(20))
                            .min(3600);
                        self.store
                            .fail_job(job.id, now, &e.to_string(), Some(now + wait))?;
                        report.retried += 1;
                        record!(span, "outcome", "retried");
                    }
                }
            }
        }
        if !opts.allow_heavy {
            report.skipped_heavy = self.store.count_ready_heavy(self.clock.now())?;
        }
        let now = self.clock.now();
        self.store
            .prune_jobs(now - KEEP_DONE_SECS, now - KEEP_FAILED_SECS)?;
        Ok(report)
    }

    /// Sets how long working and episodic memories of `scope` may go unused before the `Decay`
    /// job archives them (`None` = never forget). Person only. `Some(0)` archives everything unused up
    /// to this second on the next pass.
    pub fn set_retention(&self, scope: ScopeId, days: Option<u32>, actor: ActorId) -> Result<()> {
        let _g = self.write_guard();
        self.require_person(actor)?;
        self.store.set_retention(scope, days)
    }

    /// Queues the forgetting pass for a scope (one pending job per scope).
    pub fn enqueue_decay(&self, scope: ScopeId) -> Result<bool> {
        let _g = self.write_guard();
        self.store.scope_language(scope)?;
        let now = self.clock.now();
        self.store.enqueue(NewJob {
            kind: JobKind::Decay,
            payload: serde_json::json!({ "scope": scope.0 }).to_string(),
            dedupe_key: Some(format!("decay:{}", scope.0)),
            run_after: now,
            at: now,
        })
    }

    /// Queues a hot backup to a new file `path` (one pending job per path). Heavy: it only runs
    /// with `RunOpts::allow_heavy`. The copy is private (0600) but not encrypted. The path is made
    /// absolute now (not relative to wherever the job later runs) and must be valid UTF-8. The
    /// job never overwrites `path`; it does replace a stale file named `<path>.tmp`.
    pub fn enqueue_backup(&self, path: &Path) -> Result<bool> {
        let absolute = std::path::absolute(path)?;
        let text = absolute.to_str().ok_or(Error::InvalidState)?;
        let _g = self.write_guard();
        let now = self.clock.now();
        self.store.enqueue(NewJob {
            kind: JobKind::Backup,
            payload: serde_json::json!({ "path": text }).to_string(),
            dedupe_key: Some(format!("backup:{text}")),
            run_after: now,
            at: now,
        })
    }

    /// Queues a `VACUUM` (heavy). One pending job at a time.
    pub fn enqueue_vacuum(&self) -> Result<bool> {
        let _g = self.write_guard();
        let now = self.clock.now();
        self.store.enqueue(NewJob {
            kind: JobKind::Vacuum,
            payload: "{}".into(),
            dedupe_key: Some("vacuum".into()),
            run_after: now,
            at: now,
        })
    }

    /// Applies any `Touch` jobs still waiting, so a memory that was just recalled is not
    /// forgotten. The caller holds the write lock.
    fn apply_pending_touches(&self) -> Result<()> {
        loop {
            let batch =
                self.store
                    .claim_jobs(self.clock.now(), JobKind::Touch, LEASE_SECS, TOUCH_BATCH)?;
            if batch.is_empty() {
                return Ok(());
            }
            self.apply_touch_batch(batch)?;
        }
    }

    /// Archives the scope's unused working and episodic memories, oldest id first, with ARCHIVE
    /// operations signed by the system actor. Never touches what a fixed block shows (the ids it
    /// lists and what they resolve to after updates). Nothing is deleted.
    fn apply_decay(&self, job: &JobRecord) -> Result<()> {
        let v: serde_json::Value = serde_json::from_str(&job.payload)?;
        let scope = ScopeId(v["scope"].as_i64().ok_or(Error::BadJobPayload)?);
        let Some(days) = self.store.retention(scope)? else {
            return Ok(());
        };
        // The write lock is held from here on: a block edited while we decide what to protect
        // could otherwise end up listing a memory we are about to archive.
        let _g = self.write_guard();
        self.apply_pending_touches()?;

        let mut protected: std::collections::HashSet<MemoryId> = std::collections::HashSet::new();
        for block in self.store.blocks(scope)? {
            for entry in self.store.block_memories(block.id)? {
                protected.insert(entry);
                if let Some(shown) = self.current(entry)? {
                    protected.insert(shown.id);
                }
            }
        }

        let cutoff = self.clock.now() - i64::from(days) * 86_400;
        let system = self.store.system_actor()?;
        for row in self.store.decay_candidates(scope, cutoff)? {
            if protected.contains(&row.id) {
                continue;
            }
            let mut after = row.clone();
            after.state = State::Archived;
            let id = row.id;
            let changes = vec![Change {
                memory: id,
                before: Some(row),
                after,
            }];
            let reason = format!("decay: sin uso desde hace {days} días");
            self.log(scope, OpKind::Archive, id, changes, system, &reason)?;
        }
        Ok(())
    }

    /// Jobs, newest first; `None` lists every state.
    pub fn jobs(&self, state: Option<JobState>, limit: usize) -> Result<Vec<JobRecord>> {
        self.store.jobs(state, limit)
    }

    /// Checks the file without writing: permissions, schema, that `memories` is what the log
    /// folds to, orphan rows, the error queue. A failed check is a finding, not an `Err`.
    pub fn doctor(&self) -> Result<DoctorReport> {
        let model = self.embedder.as_ref().map(|e| e.model_id());
        Ok(DoctorReport {
            checks: self.store.doctor(model)?,
        })
    }

    /// Writes the whole memory as JSON Lines (see `EXPORT_FORMAT_VERSION`) from one consistent
    /// snapshot. Vectors, jobs and quota counters are left out: they are recomputed or
    /// operational. The output is not encrypted. Works on read-only engines.
    pub fn export(&self, out: &mut dyn std::io::Write) -> Result<ExportSummary> {
        let mut summary = ExportSummary::default();
        self.store
            .export_snapshot(self.clock.now(), &mut |record| {
                record.tally(&mut summary);
                serde_json::to_writer(&mut *out, &record)?;
                out.write_all(b"\n")?;
                Ok(())
            })?;
        out.flush()?;
        Ok(summary)
    }

    /// Restores an export (see `export`) into an empty file, with the original ids, in one
    /// transaction: either everything comes back or nothing changes. The file must have no
    /// scopes or operations (the internal system actor is allowed and replaced). Rejects other
    /// major format versions, newer minors, other schema versions, unknown or malformed lines
    /// (with the line number) and anything that looks like a secret. It is meant for exports
    /// this program wrote, not for files from elsewhere: use `import_memories` for those.
    pub fn import(&self, input: &mut dyn std::io::BufRead) -> Result<ExportSummary> {
        if self.mode == OpenMode::ReadOnly {
            return Err(Error::ReadOnly);
        }
        let _g = self.write_guard();
        let now = self.clock.now();
        let embed_job = self.embedder.as_ref().map(|_| NewJob {
            kind: JobKind::Embed,
            payload: "{}".into(),
            dedupe_key: Some("embed".into()),
            run_after: now,
            at: now,
        });
        let mut line_no = 0u64;
        let mut buf = Vec::new();
        self.store.restore(
            &mut || loop {
                match read_line_bounded(input, &mut buf)? {
                    Line::Eof => return Ok(None),
                    Line::TooLong => {
                        return Err(Error::BadImport(format!(
                            "line {}: the line is too long",
                            line_no + 1
                        )));
                    }
                    Line::Read => {}
                }
                line_no += 1;
                if buf.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                let record = serde_json::from_slice(&buf)
                    .map_err(|e| Error::BadImport(format!("line {line_no}: {e}")))?;
                return Ok(Some((line_no, record)));
            },
            embed_job,
        )
    }

    /// Loads loose memories, one JSON object per line (`content`, and optionally `kind`,
    /// `topic_key`, `valid_from`, `valid_to`, `source`), through `propose`: the actor's quota, the
    /// secret detector, the rules and the deciders all apply, and each line commits on its own.
    /// A line that cannot be accepted (bad JSON, no content, unknown kind, too long, a secret,
    /// over quota) is rejected and counted with its line number; the rest go on. `actor` must be
    /// an importer or a person. With the default `ImportOpts` everything lands in quarantine. If
    /// storage or the input fails mid-way the summary says where (`aborted`) instead of being lost.
    pub fn import_memories(
        &self,
        input: &mut dyn std::io::BufRead,
        scope: ScopeId,
        actor: ActorId,
        opts: &ImportOpts,
    ) -> Result<ImportSummary> {
        if self.mode == OpenMode::ReadOnly {
            return Err(Error::ReadOnly);
        }
        if !matches!(
            self.store.actor_kind(actor)?,
            ActorKind::Importer | ActorKind::Person
        ) {
            return Err(Error::NotAllowed);
        }
        self.store.scope_language(scope)?;
        const KEPT_REJECTIONS: usize = 100;
        let mut summary = ImportSummary::default();
        let mut buf = Vec::new();
        let mut line_no = 0usize;
        loop {
            let read = match read_line_bounded(input, &mut buf) {
                Ok(read) => read,
                Err(e) => {
                    summary.aborted = Some((line_no + 1, e.to_string()));
                    break;
                }
            };
            if matches!(read, Line::Eof) {
                break;
            }
            line_no += 1;
            if matches!(read, Line::Read) && buf.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            summary.read += 1;
            let result = match read {
                Line::TooLong => Err(Error::TooLong),
                _ => self.import_one(&buf, scope, actor, opts),
            };
            match result {
                Ok(out) => match (out.kind, out.state) {
                    (OpKind::Noop, _) => summary.noop += 1,
                    (_, State::Quarantine) => summary.quarantined += 1,
                    _ => summary.added += 1,
                },
                Err(
                    e @ (Error::BadImport(_)
                    | Error::EmptyContent
                    | Error::TooLong
                    | Error::QuotaExceeded
                    | Error::SecretDetected
                    | Error::InvalidValidity),
                ) => {
                    summary.rejected_total += 1;
                    if summary.rejected.len() < KEPT_REJECTIONS {
                        summary.rejected.push((line_no, e.to_string()));
                    }
                }
                Err(other) => {
                    summary.aborted = Some((line_no, other.to_string()));
                    break;
                }
            }
        }
        Ok(summary)
    }

    fn import_one(
        &self,
        line: &[u8],
        scope: ScopeId,
        actor: ActorId,
        opts: &ImportOpts,
    ) -> Result<Outcome> {
        let loose: LooseMemory =
            serde_json::from_slice(line).map_err(|e| Error::BadImport(e.to_string()))?;
        let kind = match loose.kind.as_deref() {
            None => MemoryType::Semantic,
            Some(name) => parse_kind(name)
                .ok_or_else(|| Error::BadImport(format!("unknown kind {name:?}")))?,
        };
        self.propose(Proposal {
            scope,
            actor,
            kind,
            content: loose.content,
            topic_key: loose.topic_key,
            origin: opts.origin,
            session: None,
            episode: None,
            source: loose.source,
            valid_from: loose.valid_from,
            valid_to: loose.valid_to,
        })
    }

    /// `export` into a new file with mode 0600. Refuses to touch a file that already exists
    /// (`Io` with `AlreadyExists`), and removes the partial file if the export fails.
    pub fn export_to_file(&self, path: &Path) -> Result<ExportSummary> {
        let file = platform::create_private_file(path)?;
        let written = (|| {
            let mut out = std::io::BufWriter::new(file);
            let summary = self.export(&mut out)?;
            out.into_inner()
                .map_err(std::io::IntoInnerError::into_error)?
                .sync_all()?;
            Ok(summary)
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(path);
        }
        written
    }

    /// How many pending jobs of `kind` are due to run now.
    pub fn ready_jobs(&self, kind: JobKind) -> Result<usize> {
        self.store.count_ready(kind, self.clock.now())
    }

    pub fn scopes(&self) -> Result<Vec<ScopeInfo>> {
        self.store.scopes()
    }

    pub fn actors(&self) -> Result<Vec<ActorInfo>> {
        self.store.actors()
    }

    /// Sizes, counts by state and the queue; reads only, so it works on read-only engines.
    pub fn stats(&self) -> Result<Stats> {
        let model = self.embedder.as_ref().map(|e| e.model_id());
        self.store.stats(model, self.clock.now())
    }

    pub fn queue_stats(&self) -> Result<QueueStats> {
        self.store.queue_stats(self.clock.now())
    }

    /// Puts a job from the error queue back to `Pending` with its attempts reset (person only).
    pub fn retry_job(&self, id: i64, actor: ActorId) -> Result<()> {
        let _g = self.write_guard();
        self.require_person(actor)?;
        self.store.retry_job(id, self.clock.now())
    }

    fn execute(&self, job: &JobRecord) -> Result<()> {
        match job.kind {
            JobKind::Embed => loop {
                let (_, examined) = self.embed_pending_inner(EMBED_RUN_BATCH)?;
                if examined == 0 {
                    return Ok(());
                }
            },
            JobKind::Touch => {
                let _g = self.write_guard();
                self.apply_touch_batch(vec![job.clone()]).map(|_| ())
            }
            JobKind::Decay => self.apply_decay(job),
            JobKind::Backup => {
                let v: serde_json::Value = serde_json::from_str(&job.payload)?;
                let path = v["path"].as_str().ok_or(Error::BadJobPayload)?;
                let _g = self.write_guard();
                #[cfg(feature = "telemetry")]
                let started = std::time::Instant::now();
                let result = self.store.backup_to(Path::new(path));
                event!(
                    INFO,
                    "backup",
                    outcome = if result.is_ok() { "done" } else { "failed" },
                    elapsed_ms = started.elapsed().as_millis() as u64
                );
                result
            }
            JobKind::Vacuum => {
                let _g = self.write_guard();
                self.store.vacuum()
            }
        }
    }

    /// True if the embedder gives a usable vector for an ordinary text.
    fn embedder_works(&self, embedder: &dyn Embedder) -> Result<bool> {
        let probe = embedder.embed(&["a"])?;
        Ok(probe.len() == 1
            && probe
                .into_iter()
                .next()
                .is_some_and(|v| unit_vector(v, embedder.dim()).is_ok()))
    }

    fn write_guard(&self) -> WriteGuard<'_> {
        WriteGuard {
            _lock: self.write.lock().unwrap_or_else(PoisonError::into_inner),
            #[cfg(feature = "telemetry")]
            trace: &self.trace,
        }
    }

    /// Promote, archive and revert are review powers: only a person may use them.
    fn require_person(&self, actor: ActorId) -> Result<()> {
        match self.store.actor_kind(actor)? {
            ActorKind::Person => Ok(()),
            _ => Err(Error::NotAllowed),
        }
    }

    pub fn create_scope(&self, name: &str, language: &str) -> Result<ScopeId> {
        let _g = self.write_guard();
        self.check_name(name)?;
        self.check_name(language)?;
        self.store.create_scope(name, language)
    }

    pub fn register_actor(&self, name: &str, kind: ActorKind, quota: Quota) -> Result<ActorId> {
        if kind == ActorKind::System || name.starts_with("clio-memory:") {
            return Err(Error::NotAllowed); // internal only: it signs background operations
        }
        let _g = self.write_guard();
        self.check_name(name)?;
        self.store.register_actor(name, kind, quota)
    }

    /// The only way content enters the memory.
    pub fn propose(&self, p: Proposal) -> Result<Outcome> {
        let span = span!(
            "propose",
            scope = p.scope.0,
            actor = p.actor.0,
            kind = format!("{:?}", p.kind).as_str(),
            trace_id = ::tracing::field::Empty,
            outcome = ::tracing::field::Empty,
            state = ::tracing::field::Empty,
        );
        let result = self.propose_inner(p);
        finish_propose!(span, result);
        result
    }

    fn propose_inner(&self, p: Proposal) -> Result<Outcome> {
        let _g = self.write_guard();
        if p.content.trim().is_empty() {
            return Err(Error::EmptyContent);
        }
        let now = self.clock.now();
        let start_defaulted = p.valid_from.is_none();
        let valid_from = p.valid_from.unwrap_or(now);
        if p.valid_to.is_some_and(|to| to <= valid_from) {
            return Err(Error::InvalidValidity);
        }
        self.store.scope_language(p.scope)?;
        // Counts before the size and secret checks so rejected proposals still use up quota.
        if self.store.actor_kind(p.actor)? == ActorKind::System {
            return Err(Error::NotAllowed); // the system actor signs background operations only
        }
        self.store.bump_quota(p.actor, now)?;
        let fields = [
            p.topic_key.as_ref(),
            p.session.as_ref(),
            p.episode.as_ref(),
            p.source.as_ref(),
        ];
        if p.content.len() > self.limits.max_content_bytes
            || fields
                .into_iter()
                .flatten()
                .any(|f| f.len() > self.limits.max_field_bytes)
        {
            return Err(Error::TooLong);
        }
        let texts = [
            Some(&p.content),
            p.topic_key.as_ref(),
            p.session.as_ref(),
            p.episode.as_ref(),
            p.source.as_ref(),
        ];
        let secret = {
            let _s = span!("secrets");
            texts.into_iter().flatten().any(|t| find_secret(t))
        };
        if secret {
            return Err(Error::SecretDetected);
        }

        let content = {
            let _s = span!("normalize");
            normalize(&p.content)
        };
        // The raw check above is an early reject; this one bounds what is actually stored (NFC can
        // make some characters up to three times longer).
        if content.len() > self.limits.max_content_bytes {
            return Err(Error::TooLong);
        }
        let hash = content_hash(&content);
        let actor_kind = self.store.actor_kind(p.actor)?;
        let origin = effective_origin(actor_kind, p.origin);
        let by_hash = self.store.find_active_by_hash(p.scope, &hash)?;
        let by_topic = match p.topic_key.as_deref() {
            Some(t) => self.store.find_active_by_topic(p.scope, p.kind, t)?,
            None => None,
        };
        let existing = Existing {
            by_hash: by_hash.as_ref(),
            by_topic: by_topic.as_ref(),
        };

        let id = self.store.next_memory_id()?;
        let mut row = MemoryRow {
            id,
            scope: p.scope,
            kind: p.kind,
            content,
            content_hash: hash,
            topic_key: p.topic_key,
            origin,
            actor: p.actor,
            session: p.session,
            episode: p.episode,
            source: p.source,
            state: State::Active,
            valid_from,
            valid_to: p.valid_to,
            known_since: now,
            replaced_by: None,
            supersedes: None,
        };
        let verdict = {
            let _s = span!("rules");
            decide(p.kind, origin, existing)
        };
        let mut decisions: Vec<NewDecision> = Vec::new();
        let mut review_by: Option<String> = None;
        if matches!(
            verdict,
            Verdict::Add {
                state: State::Active,
                supersedes: None
            }
        ) && !self.deciders.is_empty()
        {
            let question = Question {
                candidate: Candidate {
                    scope: p.scope,
                    kind: p.kind,
                    content: row.content.clone(),
                    topic_key: row.topic_key.clone(),
                    origin,
                    actor_kind,
                },
                neighbors: {
                    let _s = span!("neighbors");
                    self.neighbors(p.scope, &row.content)
                },
            };
            let (applied, log) = self.consult(&question, now);
            decisions = log;
            if let Some((Choice::Review, who)) = applied {
                review_by = Some(who);
            }
        }
        let (kind, memory, changes, reason) = match verdict {
            Verdict::Noop(existing_id) => {
                (OpKind::Noop, existing_id, vec![], "hash match".to_string())
            }
            Verdict::Add { state, supersedes } => {
                row.state = if review_by.is_some() {
                    State::Quarantine
                } else {
                    state
                };
                row.supersedes = supersedes;
                let reason = match (&review_by, state, origin) {
                    (Some(who), _, _) => format!("review requested by {who}"),
                    (None, State::Quarantine, Origin::External) => "external origin".to_string(),
                    (None, State::Quarantine, _) => "procedural by non-user".to_string(),
                    _ => "new".to_string(),
                };
                (
                    OpKind::Add,
                    id,
                    vec![Change {
                        memory: id,
                        before: None,
                        after: row,
                    }],
                    reason,
                )
            }
            Verdict::Update { .. } => {
                let old = by_topic.ok_or(Error::InvalidState)?;
                // Several facts about one topic can arrive within the same clock second (a batch
                // written when an episode closes). With no explicit start the new one starts now,
                // and if the old one also started this second it was valid for zero seconds
                // (`valid_to == valid_from`): no dates are invented. An explicit start must be
                // later than the old one's; a start before it (a future-dated owner) is rejected.
                if !start_defaulted && row.valid_from <= old.valid_from {
                    return Err(Error::InvalidValidity);
                }
                let mut changes = replace(&old, &row)?;
                changes.push(Change {
                    memory: id,
                    before: None,
                    after: row,
                });
                (OpKind::Update, id, changes, "same topic".to_string())
            }
        };
        let state = match kind {
            OpKind::Noop => State::Active,
            _ => changes.last().map_or(State::Active, |c| c.after.state),
        };
        let trace_id = self.trace_id();
        let _append = span!("append");
        let jobs = self.embed_jobs(&changes, now);
        let op = self.store.append_all(
            NewOperation {
                scope: p.scope,
                kind,
                memory: Some(memory),
                changes,
                actor: p.actor,
                reason,
                at: now,
                reverts: None,
                trace_id,
            },
            decisions,
            jobs,
        )?;
        Ok(Outcome {
            op,
            kind,
            memory,
            state,
        })
    }

    /// Moves a quarantined memory to active (a person must do it); if it was waiting to replace
    /// an active memory, that one is replaced in the same operation.
    pub fn promote(&self, id: MemoryId, actor: ActorId) -> Result<OpId> {
        let span = span!(
            "promote",
            memory = id.0,
            actor = actor.0,
            trace_id = ::tracing::field::Empty,
            outcome = ::tracing::field::Empty
        );
        outcome!(span, self.promote_inner(id, actor))
    }

    fn promote_inner(&self, id: MemoryId, actor: ActorId) -> Result<OpId> {
        let _g = self.write_guard();
        self.require_person(actor)?;
        let row = self.store.get(id)?;
        if row.state != State::Quarantine {
            return Err(Error::InvalidState);
        }
        let mut after = row.clone();
        after.state = State::Active;
        let owner = match row.topic_key.as_deref() {
            Some(topic) => self
                .store
                .find_active_by_topic(row.scope, row.kind, topic)?,
            None => None,
        };
        let mut changes = Vec::new();
        if let Some(cur) = owner {
            // At most one active memory per topic: a newer candidate replaces the current one;
            // an older one is kept only as history, closed where the current one starts.
            match after.valid_from.cmp(&cur.valid_from) {
                Ordering::Greater => changes = replace(&cur, &after)?,
                Ordering::Less => {
                    after.state = State::Replaced;
                    let end = after
                        .valid_to
                        .map_or(cur.valid_from, |t| t.min(cur.valid_from));
                    after.valid_to = Some(end);
                    after.replaced_by = Some(cur.id);
                }
                Ordering::Equal => return Err(Error::InvalidValidity),
            }
        }
        changes.push(Change {
            memory: id,
            before: Some(row.clone()),
            after,
        });
        // a person vindicated it: the "wrong" signals it had are spent, in the same transaction
        let now = self.clock.now();
        let jobs = self.embed_jobs(&changes, now);
        self.store.append_spending(
            NewOperation {
                scope: row.scope,
                kind: OpKind::Promote,
                memory: Some(id),
                changes,
                actor,
                reason: "promoted".into(),
                at: now,
                reverts: None,
                trace_id: self.trace_id(),
            },
            Vec::new(),
            jobs,
            id,
        )
    }

    /// Records that an actor found a memory useful or wrong. It counts against the actor's quota,
    /// the system actor cannot give it, and the memory must be active. The third unspent "wrong"
    /// signal (from anyone) sends the memory to quarantine with a `Flag` operation signed by the
    /// actor who gave it; a person resolves it with `promote` or `archive`, or reverts the
    /// `Flag`. Feedback never edits, replaces or deletes anything.
    pub fn feedback(&self, memory: MemoryId, signal: Signal, actor: ActorId) -> Result<()> {
        let span = span!(
            "feedback",
            memory = memory.0,
            actor = actor.0,
            signal = format!("{signal:?}").as_str(),
            trace_id = ::tracing::field::Empty,
            outcome = ::tracing::field::Empty
        );
        outcome!(span, self.feedback_inner(memory, signal, actor))
    }

    fn feedback_inner(&self, memory: MemoryId, signal: Signal, actor: ActorId) -> Result<()> {
        let _g = self.write_guard();
        if self.store.actor_kind(actor)? == ActorKind::System {
            return Err(Error::NotAllowed);
        }
        let row = self.store.get(memory)?;
        if row.state != State::Active {
            return Err(Error::InvalidState);
        }
        let now = self.clock.now();
        self.store.bump_quota(actor, now)?;
        // one transaction for the signal and, when it is the third unspent "wrong", the flag
        let flag = || {
            let mut after = row.clone();
            after.state = State::Quarantine;
            let changes = vec![Change {
                memory,
                before: Some(row.clone()),
                after,
            }];
            let jobs = self.embed_jobs(&changes, now);
            let op = NewOperation {
                scope: row.scope,
                kind: OpKind::Flag,
                memory: Some(memory),
                changes,
                actor,
                reason: "equivocado marcado 3 veces".into(),
                at: now,
                reverts: None,
                trace_id: self.trace_id(),
            };
            (op, jobs)
        };
        self.store
            .record_feedback_flagging(memory, actor, signal, now, FLAG_AFTER_WRONG, &flag)?;
        Ok(())
    }

    /// Useful and unspent wrong signals of a memory and the confidence they give.
    pub fn feedback_summary(&self, memory: MemoryId) -> Result<FeedbackSummary> {
        self.store.get(memory)?;
        let (useful, wrong) = self.store.feedback_counts(memory)?;
        let confidence = confidence_of(useful, wrong);
        Ok(FeedbackSummary {
            useful,
            wrong,
            confidence,
        })
    }

    pub fn confidence(&self, memory: MemoryId) -> Result<f32> {
        self.feedback_summary(memory).map(|s| s.confidence)
    }

    /// Takes an active or quarantined memory out of circulation without deleting anything.
    pub fn archive(&self, id: MemoryId, actor: ActorId) -> Result<OpId> {
        let span = span!(
            "archive",
            memory = id.0,
            actor = actor.0,
            trace_id = ::tracing::field::Empty,
            outcome = ::tracing::field::Empty
        );
        outcome!(span, self.archive_inner(id, actor))
    }

    fn archive_inner(&self, id: MemoryId, actor: ActorId) -> Result<OpId> {
        let _g = self.write_guard();
        self.require_person(actor)?;
        let row = self.store.get(id)?;
        if !matches!(row.state, State::Active | State::Quarantine) {
            return Err(Error::InvalidState);
        }
        let mut after = row.clone();
        after.state = State::Archived;
        let changes = vec![Change {
            memory: id,
            before: Some(row.clone()),
            after,
        }];
        self.log(row.scope, OpKind::Archive, id, changes, actor, "archived")
    }

    /// Undoes one operation by appending a REVERT that restores each change's `before` (an ADD
    /// has no `before`: its memory is archived). Only allowed while every memory the operation
    /// touched is still exactly as the operation left it.
    fn revert_locked(&self, op: &Operation, actor: ActorId) -> Result<OpId> {
        let mut changes = Vec::with_capacity(op.changes.len());
        for c in &op.changes {
            let current = self.store.get(c.memory)?;
            if current != c.after {
                return Err(Error::NotRevertible(op.id));
            }
            let restored = c.before.clone().unwrap_or_else(|| {
                let mut archived = c.after.clone();
                archived.state = State::Archived;
                archived
            });
            changes.push(Change {
                memory: c.memory,
                before: Some(current),
                after: restored,
            });
        }
        let now = self.clock.now();
        let jobs = self.embed_jobs(&changes, now);
        let new = self.store.append_all(
            NewOperation {
                scope: op.scope,
                kind: OpKind::Revert,
                memory: op.memory,
                changes,
                actor,
                reason: "revert".into(),
                at: now,
                reverts: Some(op.id),
                trace_id: self.trace_id(),
            },
            Vec::new(),
            jobs,
        )?;
        event!(INFO, "reverted", op = op.id.0, new_op = new.0);
        Ok(new)
    }

    pub fn revert(&self, op: OpId, actor: ActorId) -> Result<OpId> {
        let span = span!(
            "revert",
            op = op.0,
            actor = actor.0,
            trace_id = ::tracing::field::Empty,
            outcome = ::tracing::field::Empty
        );
        outcome!(span, self.revert_inner(op, actor))
    }

    fn revert_inner(&self, op: OpId, actor: ActorId) -> Result<OpId> {
        let _g = self.write_guard();
        self.require_person(actor)?;
        self.revert_locked(&self.store.operation(op)?, actor)
    }

    /// Reverts everything `target` did that is still revertible, newest first.
    pub fn revert_actor(&self, target: ActorId, by: ActorId) -> Result<RevertReport> {
        let span = span!(
            "revert_actor",
            actor = target.0,
            trace_id = ::tracing::field::Empty,
            outcome = ::tracing::field::Empty,
        );
        outcome!(span, self.revert_actor_inner(target, by))
    }

    fn revert_actor_inner(&self, target: ActorId, by: ActorId) -> Result<RevertReport> {
        let _g = self.write_guard();
        self.revert_many(self.store.operations_by_actor(target)?, by)
    }

    /// Reverts every revertible operation with id >= `from`, newest first.
    pub fn revert_since(&self, from: OpId, by: ActorId) -> Result<RevertReport> {
        let span = span!(
            "revert_since",
            from = from.0,
            trace_id = ::tracing::field::Empty,
            outcome = ::tracing::field::Empty,
        );
        outcome!(span, self.revert_since_inner(from, by))
    }

    fn revert_since_inner(&self, from: OpId, by: ActorId) -> Result<RevertReport> {
        let _g = self.write_guard();
        self.revert_many(self.store.operations_from(from)?, by)
    }

    fn revert_many(&self, mut ops: Vec<Operation>, by: ActorId) -> Result<RevertReport> {
        self.require_person(by)?;
        ops.reverse();
        let mut report = RevertReport::default();
        for op in ops
            .iter()
            .filter(|o| !matches!(o.kind, OpKind::Revert | OpKind::Noop))
        {
            match self.revert_locked(op, by) {
                Ok(_) => report.reverted.push(op.id),
                Err(Error::NotRevertible(_)) => report.skipped.push(op.id),
                Err(e) => return Err(e),
            }
        }
        Ok(report)
    }

    fn log(
        &self,
        scope: ScopeId,
        kind: OpKind,
        memory: MemoryId,
        changes: Vec<Change>,
        actor: ActorId,
        reason: &str,
    ) -> Result<OpId> {
        let now = self.clock.now();
        let jobs = self.embed_jobs(&changes, now);
        self.store.append_all(
            NewOperation {
                scope,
                kind,
                memory: Some(memory),
                changes,
                actor,
                reason: reason.into(),
                at: now,
                reverts: None,
                trace_id: self.trace_id(),
            },
            Vec::new(),
            jobs,
        )
    }

    /// Creates a fixed block (person only). Blocks go into the prompt without passing the
    /// relevance filters, so who may change them is restricted like `promote`.
    pub fn create_block(
        &self,
        scope: ScopeId,
        name: &str,
        budget_tokens: usize,
        actor: ActorId,
    ) -> Result<BlockId> {
        let _g = self.write_guard();
        self.require_person(actor)?;
        self.check_name(name)?;
        self.store.scope_language(scope)?;
        self.store.create_block(scope, name, budget_tokens)
    }

    /// Replaces a block's memories (person only). Every memory must be active and in the
    /// block's scope; order is priority. A rejected edit changes nothing.
    pub fn block_set(&self, block: BlockId, memories: &[MemoryId], actor: ActorId) -> Result<()> {
        let _g = self.write_guard();
        self.require_person(actor)?;
        let scope = self.store.block(block)?.scope;
        for id in memories {
            let row = self.store.get(*id)?;
            if row.scope != scope || row.state != State::Active {
                return Err(Error::InvalidState);
            }
        }
        self.store.set_block_memories(block, memories)
    }

    pub fn block_get(&self, block: BlockId) -> Result<Vec<MemoryId>> {
        self.store.block(block)?;
        self.store.block_memories(block)
    }

    pub fn blocks(&self, scope: ScopeId) -> Result<Vec<Block>> {
        self.store.blocks(scope)
    }

    pub fn get(&self, id: MemoryId) -> Result<MemoryRow> {
        self.store.get(id)
    }

    pub fn list(
        &self,
        scope: ScopeId,
        state: Option<State>,
        kind: Option<MemoryType>,
    ) -> Result<Vec<MemoryRow>> {
        self.store.list(scope, state, kind)
    }

    pub fn history(&self, id: MemoryId) -> Result<Vec<Operation>> {
        self.store.history(id)
    }

    pub fn search(&self, scope: ScopeId, query: &str, opts: SearchOpts) -> Result<Vec<Hit>> {
        self.store.search(scope, query, &opts)
    }

    /// Hybrid search: full text plus vectors (when an embedder is set), fused by reciprocal
    /// rank. A candidate only counts if it clears the relevance threshold of the list it came
    /// from; if none does the answer is "nothing relevant" (`abstained`) rather than the least
    /// bad matches. A query with no letters or digits abstains without touching the embedder.
    pub fn recall(&self, scope: ScopeId, query: &str, opts: &RecallOpts) -> Result<Recall> {
        let span = span!(
            "recall",
            scope = scope.0,
            limit = opts.search.limit,
            text_hits = ::tracing::field::Empty,
            vector_hits = ::tracing::field::Empty,
            hits = ::tracing::field::Empty,
            abstained = ::tracing::field::Empty,
            outcome = ::tracing::field::Empty,
        );
        outcome!(span, self.recall_inner(scope, query, opts))
    }

    fn recall_inner(&self, scope: ScopeId, query: &str, opts: &RecallOpts) -> Result<Recall> {
        if !query.chars().any(char::is_alphanumeric) {
            record_current!("hits", 0usize);
            record_current!("abstained", true);
            return Ok(Recall {
                hits: Vec::new(),
                abstained: true,
            });
        }
        let pool = opts.search.limit.saturating_mul(5).max(50);
        let pool_opts = SearchOpts {
            limit: pool,
            ..opts.search
        };

        let text_hits = self.store.search(scope, query, &pool_opts)?;
        let lang = self.store.scope_language(scope)?;
        // Relevance by text = how many of the query's content words the memory contains. Function
        // words ("de", "en la") do not count, so they neither pad results nor block real matches.
        let wanted: HashSet<String> = content_tokens(&lang, query).into_iter().collect();
        let covered = |content: &str| {
            let have: HashSet<String> = tokens(&lang, content).into_iter().collect();
            wanted.intersection(&have).count() as f64
                >= opts.min_text_coverage * wanted.len() as f64
        };
        let text: Vec<(MemoryId, f64)> = if wanted.is_empty() {
            Vec::new()
        } else {
            text_hits
                .iter()
                .filter(|h| covered(&h.memory.content))
                .map(|h| (h.memory.id, h.score))
                .collect()
        };
        let mut rows: HashMap<MemoryId, MemoryRow> = text_hits
            .into_iter()
            .map(|h| (h.memory.id, h.memory))
            .collect();

        let mut vector: Vec<(MemoryId, f32)> = Vec::new();
        if let Some(e) = &self.embedder {
            // A query the model cannot represent just means no vector list: text still answers.
            let q = e
                .embed(&[query])?
                .pop()
                .and_then(|v| unit_vector(v, e.dim()).ok());
            if let Some(q) = q {
                for (id, cos) in self
                    .store
                    .nearest(scope, e.model_id(), &q, &pool_opts, pool)?
                {
                    if cos >= opts.min_vector_score {
                        if let std::collections::hash_map::Entry::Vacant(v) = rows.entry(id) {
                            v.insert(self.store.get(id)?);
                        }
                        vector.push((id, cos));
                    }
                }
            }
        }

        record_current!("text_hits", text.len());
        record_current!("vector_hits", vector.len());
        let now = self.clock.now();
        let ranked: Vec<MemoryId> = text
            .iter()
            .map(|t| t.0)
            .chain(vector.iter().map(|v| v.0))
            .collect();
        let usage = self.store.usage_counts(&ranked).unwrap_or_default();
        let signals = self.store.feedback_counts_many(&ranked).unwrap_or_default();
        let fused = fuse(
            &text,
            &vector,
            now,
            |id| rows.get(&id).map_or(now, |r| r.known_since),
            |id| usage.get(&id).copied().unwrap_or(0),
            |id| signals.get(&id).map_or(0.5, |&(u, w)| confidence_of(u, w)),
        );
        // "abstained" means nothing was relevant; a cap (limit) emptying the list is not that.
        let abstained = fused.is_empty();
        let hits: Vec<RecallHit> = fused
            .into_iter()
            .take(opts.search.limit)
            .filter_map(|f| {
                rows.remove(&f.id).map(|memory| RecallHit {
                    memory,
                    score: f.score,
                    text: f.text,
                    vector: f.vector,
                    recency: f.recency,
                    confidence: f.confidence,
                })
            })
            .collect();
        let hits = self.within_budget(hits, opts);
        if opts.search.as_of.is_none() && opts.search.known_at.is_none() {
            self.record_use(&hits);
        }
        record_current!("hits", hits.len());
        record_current!("abstained", abstained);
        Ok(Recall { hits, abstained })
    }

    /// Queues a `Touch` for what `recall` returned, so reads stay reads. Best effort: it never
    /// waits for the write lock (a write, a backup, or a decider that calls `recall` while this
    /// thread holds it would otherwise block or deadlock) and swallows every error, so usage can
    /// be undercounted while the engine is busy. Read-only engines record nothing.
    fn record_use(&self, hits: &[RecallHit]) {
        if self.mode != OpenMode::ReadWrite || hits.is_empty() {
            return;
        }
        let Ok(_guard) = self.write.try_lock() else {
            return;
        };
        let now = self.clock.now();
        let ids: Vec<i64> = hits.iter().map(|h| h.memory.id.0).collect();
        let payload = serde_json::json!({ "ids": ids, "at": now }).to_string();
        let _ = self.store.enqueue(NewJob {
            kind: JobKind::Touch,
            payload,
            dedupe_key: None,
            run_after: now,
            at: now,
        });
    }

    /// Applies Touch jobs together: one transaction for all the usage, one to close the jobs
    /// (`uses + 1` per memory, stamped with the time of the query, not of the run). The caller
    /// holds the write lock. A malformed payload fails that job for good; a store error puts the
    /// rest back in the queue for a retry and is returned. Returns `(done, failed)`.
    fn apply_touch_batch(&self, jobs: Vec<JobRecord>) -> Result<(usize, usize)> {
        let now = self.clock.now();
        let (mut uses, mut good, mut bad) = (Vec::new(), Vec::new(), 0);
        for job in &jobs {
            match parse_touch(&job.payload) {
                Some((ids, at)) => {
                    uses.extend(ids.into_iter().map(|id| (id, at)));
                    good.push(job.id);
                }
                None => {
                    self.store
                        .fail_job(job.id, now, &Error::BadJobPayload.to_string(), None)?;
                    bad += 1;
                }
            }
        }
        let stored = self
            .store
            .record_usages(&uses)
            .and_then(|()| self.store.finish_jobs(&good, now));
        if let Err(e) = stored {
            for id in &good {
                let _ = self
                    .store
                    .fail_job(*id, now, &e.to_string(), Some(now + 60));
            }
            return Err(e);
        }
        Ok((good.len(), bad))
    }

    /// Keeps hits in rank order while their formatted lines fit the budget, stopping at the first
    /// that does not (skipping ahead to smaller ones would break the relevance order).
    fn within_budget(&self, hits: Vec<RecallHit>, opts: &RecallOpts) -> Vec<RecallHit> {
        let Some(budget) = opts.token_budget else {
            return hits;
        };
        let historical = opts.search.known_at.is_some();
        let mut used = 0;
        hits.into_iter()
            .take_while(|h| {
                used += self.tokens.count(&compact_line(&h.memory, historical));
                used <= budget
            })
            .collect()
    }

    /// Follows `replaced_by` from a block entry to the memory that is active now. A block is
    /// curated by a person and bypasses relevance filters, so an entry only follows an update
    /// whose current version a person vouched for (`Origin::User`); otherwise it is left out
    /// until a person edits the block.
    fn current(&self, mut id: MemoryId) -> Result<Option<MemoryRow>> {
        let mut via_update = false;
        for _ in 0..32 {
            let row = match self.store.get(id) {
                Ok(row) => row,
                Err(Error::NotFound) => return Ok(None),
                Err(e) => return Err(e),
            };
            match (row.state, row.replaced_by) {
                (State::Active, _) => {
                    return Ok((!via_update || row.origin == Origin::User).then_some(row));
                }
                (State::Replaced, Some(next)) => {
                    id = next;
                    via_update = true;
                }
                _ => return Ok(None),
            }
        }
        Ok(None)
    }

    /// Keeps `rows` in order while header plus lines fit `budget`, stopping at the first that
    /// does not fit (a section whose header or first line does not fit is dropped whole).
    fn fit(
        &self,
        header: &str,
        rows: Vec<MemoryRow>,
        budget: usize,
        cost: &dyn Fn(&MemoryRow) -> String,
    ) -> Vec<MemoryRow> {
        let mut used = self.tokens.count(header);
        if used > budget {
            return Vec::new();
        }
        rows.into_iter()
            .take_while(|r| {
                used += self.tokens.count(&cost(r));
                used <= budget
            })
            .collect()
    }

    /// What to put in the prompt: the fixed blocks (each up to its own budget), then, if there is
    /// a query, what `recall` finds (up to `recall_budget`), skipping what a block already shows.
    /// Block entries follow updates; archived or quarantined ones are left out.
    pub fn context(&self, scope: ScopeId, opts: &ContextOpts) -> Result<String> {
        let span = span!(
            "context",
            scope = scope.0,
            with_query = opts.query.is_some(),
            outcome = ::tracing::field::Empty,
        );
        outcome!(span, self.context_inner(scope, opts))
    }

    fn context_inner(&self, scope: ScopeId, opts: &ContextOpts) -> Result<String> {
        let historical = opts.recall.search.known_at.is_some();
        let json = opts.format == Format::Json;
        let line = |m: &MemoryRow| {
            if json {
                json_item(m)
            } else {
                compact_line(m, historical)
            }
        };
        let mut shown: HashSet<MemoryId> = HashSet::new();
        let mut sections: Vec<(String, Vec<MemoryRow>)> = Vec::new();

        for block in self.store.blocks(scope)? {
            let mut rows: Vec<MemoryRow> = Vec::new();
            for entry in self.store.block_memories(block.id)? {
                if let Some(row) = self.current(entry)?
                    && !shown.contains(&row.id)
                    && !rows.iter().any(|r| r.id == row.id)
                {
                    rows.push(row);
                }
            }
            let header = format!("## {}", block.name);
            let rows = self.fit(&header, rows, block.budget_tokens, &line);
            if !rows.is_empty() {
                shown.extend(rows.iter().map(|r| r.id));
                sections.push((block.name, rows));
            }
        }

        let mut relevant: Vec<MemoryRow> = Vec::new();
        if let Some(q) = opts.query.as_deref() {
            let recall_opts = RecallOpts {
                token_budget: None,
                ..opts.recall
            };
            let found = self.recall(scope, q, &recall_opts)?;
            let rows: Vec<MemoryRow> = found
                .hits
                .into_iter()
                .map(|h| h.memory)
                .filter(|m| !shown.contains(&m.id))
                .collect();
            relevant = self.fit("## relevante", rows, opts.recall_budget, &line);
        }

        if json {
            let items = |rows: &[MemoryRow]| -> Vec<serde_json::Value> {
                rows.iter()
                    .filter_map(|r| serde_json::to_value(r).ok())
                    .collect()
            };
            let blocks: Vec<serde_json::Value> = sections
                .iter()
                .map(|(name, rows)| serde_json::json!({ "name": name, "memories": items(rows) }))
                .collect();
            return Ok(
                serde_json::json!({ "blocks": blocks, "recall": items(&relevant) }).to_string(),
            );
        }
        let mut parts: Vec<String> = Vec::new();
        let mut push = |header: String, rows: &[MemoryRow]| {
            let body: Vec<String> = rows.iter().map(&line).collect();
            parts.push(format!("{header}\n{}", body.join("\n")));
        };
        for (name, rows) in &sections {
            push(format!("## {name}"), rows);
        }
        if !relevant.is_empty() {
            push("## relevante".to_string(), &relevant);
        }
        Ok(parts.join("\n\n"))
    }

    pub fn rebuild_projection(&self) -> Result<()> {
        let _g = self.write_guard();
        self.store.rebuild_projection()
    }
}
