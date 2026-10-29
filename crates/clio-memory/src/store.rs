use crate::error::Result;
use crate::model::{
    ActorId, ActorInfo, ActorKind, Block, BlockId, Check, Choice, DeciderMode, DecisionRecord,
    DecisionStatus, ExportSummary, Hit, JobKind, JobRecord, JobState, MemoryId, MemoryRow,
    MemoryType, NewOperation, OpId, Operation, QueueStats, Quota, ScopeId, ScopeInfo, SearchOpts,
    Signal, State, Stats,
};
use crate::transfer::ExportRecord;
use crate::vector::VectorIndex;

/// A background job to enqueue. Priority and heaviness come from the kind.
pub struct NewJob {
    pub kind: JobKind,
    pub payload: String,
    /// While a job with this key is `Pending`, another with the same key is ignored.
    pub dedupe_key: Option<String>,
    pub run_after: i64,
    pub at: i64,
}

/// A decision to be recorded together with the operation it led to.
pub struct NewDecision {
    pub scope: ScopeId,
    pub provider: String,
    pub mode: DeciderMode,
    pub status: DecisionStatus,
    pub choice: Option<Choice>,
    pub confidence: Option<f32>,
    pub reason: Option<String>,
    pub acted: bool,
    pub at: i64,
}

/// Persistence boundary: the operation log, the projection and the text index.
/// SQLite is the only implementation for now; the trait leaves room for another (e.g. Postgres).
pub trait Store: VectorIndex + Send + Sync {
    fn create_scope(&self, name: &str, language: &str) -> Result<ScopeId>;
    fn scope_language(&self, scope: ScopeId) -> Result<String>;
    fn register_actor(&self, name: &str, kind: ActorKind, quota: Quota) -> Result<ActorId>;
    fn actor_kind(&self, actor: ActorId) -> Result<ActorKind>;
    /// Counts one proposal in the actor's current window; `QuotaExceeded` when it is over the max.
    /// Commits on its own so a later rejected proposal still counts.
    fn bump_quota(&self, actor: ActorId, now: i64) -> Result<()>;

    fn find_active_by_hash(&self, scope: ScopeId, hash: &str) -> Result<Option<MemoryRow>>;
    fn find_active_by_topic(
        &self,
        scope: ScopeId,
        kind: MemoryType,
        topic: &str,
    ) -> Result<Option<MemoryRow>>;
    /// Safe to call only while the caller serializes writes.
    fn next_memory_id(&self) -> Result<MemoryId>;

    /// One transaction: insert the operation, then fold its changes into the projection and index.
    #[cfg(test)]
    fn append(&self, op: NewOperation) -> Result<OpId> {
        self.append_all(op, Vec::new(), Vec::new())
    }
    /// Like `append`, and in the same transaction records the decisions that led to the
    /// operation, each pointing at it. A failed append leaves no decision behind.
    #[cfg(test)]
    fn append_with_decisions(&self, op: NewOperation, decisions: Vec<NewDecision>) -> Result<OpId> {
        self.append_all(op, decisions, Vec::new())
    }
    /// The operation, its decisions and the background jobs it causes, in one transaction: a
    /// failed append leaves neither decisions nor jobs, and a successful one never lacks them.
    fn append_all(
        &self,
        op: NewOperation,
        decisions: Vec<NewDecision>,
        jobs: Vec<NewJob>,
    ) -> Result<OpId>;

    /// `append_all`, and in the same transaction marks the memory's "wrong" signals as spent
    /// (a promote consumes them: either both happen or neither does).
    fn append_spending(
        &self,
        op: NewOperation,
        decisions: Vec<NewDecision>,
        jobs: Vec<NewJob>,
        spend: MemoryId,
    ) -> Result<OpId>;

    /// Records a feedback signal and, if it is a "wrong" that brings the memory's unspent wrong
    /// signals to `threshold`, also appends the operation `flag` builds and spends them, all in
    /// one transaction: a failure anywhere keeps neither the signal nor the flag.
    fn record_feedback_flagging(
        &self,
        memory: MemoryId,
        actor: ActorId,
        signal: Signal,
        at: i64,
        threshold: u32,
        flag: &dyn Fn() -> (NewOperation, Vec<NewJob>),
    ) -> Result<Option<OpId>>;

    /// Returns false when the pending-only dedupe index ignored the job.
    fn enqueue(&self, job: NewJob) -> Result<bool>;
    /// Best priority first, then lowest id, among `Pending` jobs with `run_after <= now`; marks it
    /// `Running` with `attempts + 1` and a lease. `only` restricts the kind; heavy kinds need
    /// `allow_heavy`.
    fn claim_job(
        &self,
        now: i64,
        only: Option<JobKind>,
        allow_heavy: bool,
        lease_secs: i64,
    ) -> Result<Option<JobRecord>>;
    /// Like `claim_job` for many jobs of one kind at once (by id, up to `limit`).
    fn claim_jobs(
        &self,
        now: i64,
        kind: JobKind,
        lease_secs: i64,
        limit: usize,
    ) -> Result<Vec<JobRecord>>;
    /// Only a `Running` job can be finished, failed or requeued; anything else is left alone (a
    /// worker that lost its lease must not revive or overwrite a job someone else finished).
    fn finish_job(&self, id: i64, now: i64) -> Result<()>;
    fn finish_jobs(&self, ids: &[i64], now: i64) -> Result<()>;
    /// `Some(t)` returns the job to `Pending` at `t` (if another `Pending` job holds its dedupe
    /// key it is closed as `Done` with a note instead); `None` sends it to `Failed`.
    fn fail_job(&self, id: i64, now: i64, error: &str, retry_at: Option<i64>) -> Result<()>;
    /// `Running` jobs whose lease expired go back to `Pending`, or to `Failed` ("lease expired")
    /// when they already used all their attempts. Returns how many.
    fn recover_leases(&self, now: i64) -> Result<usize>;
    /// Newest first.
    fn jobs(&self, state: Option<JobState>, limit: usize) -> Result<Vec<JobRecord>>;
    fn queue_stats(&self, now: i64) -> Result<QueueStats>;
    /// `Failed` -> `Pending` with attempts reset (or closed as `Done` when an identical job is
    /// already pending); `InvalidState` for any other state.
    fn retry_job(&self, id: i64, now: i64) -> Result<()>;
    /// Deletes `Done` jobs finished before `done_before` and `Failed` ones (the error queue,
    /// kept longer) finished before `failed_before`.
    fn prune_jobs(&self, done_before: i64, failed_before: i64) -> Result<usize>;
    fn count_ready_heavy(&self, now: i64) -> Result<usize>;
    /// Pending jobs of `kind` that are due.
    fn count_ready(&self, kind: JobKind, now: i64) -> Result<usize>;

    /// Days after which unused working and episodic memories are archived; `None` = never.
    fn set_retention(&self, scope: ScopeId, days: Option<u32>) -> Result<()>;
    fn retention(&self, scope: ScopeId) -> Result<Option<u32>>;
    /// Active working and episodic memories of the scope whose last activity (last use, learned
    /// at, last revert or promote of it) is before `cutoff`, by id.
    fn decay_candidates(&self, scope: ScopeId, cutoff: i64) -> Result<Vec<MemoryRow>>;
    /// The internal actor that signs background operations; created on first use.
    fn system_actor(&self) -> Result<ActorId>;

    /// Hot backup with SQLite's online backup API into a new file `path` (mode 0600). Written to
    /// `<path>.tmp` first and published with a hard link, so an existing `path` is never
    /// overwritten (`AlreadyExists`) and a stale `.tmp` from a crash is replaced.
    fn backup_to(&self, path: &std::path::Path) -> Result<()>;
    fn vacuum(&self) -> Result<()>;

    /// For each `(memory, at)`: `uses + 1` and `last_used = max(old, at)`, in one transaction.
    fn record_usages(&self, uses: &[(MemoryId, i64)]) -> Result<()>;
    /// `(useful, unspent wrong)` signals of a memory.
    fn feedback_counts(&self, memory: MemoryId) -> Result<(u32, u32)>;

    /// How many times each of the memories has been used (absent = never).
    /// `(useful, unspent wrong)` for each id that has any signal; the rest are absent.
    fn feedback_counts_many(
        &self,
        ids: &[MemoryId],
    ) -> Result<std::collections::HashMap<MemoryId, (u32, u32)>>;
    /// Health checks over the file; reads only. `model` is the embedder's, if there is one.
    fn doctor(&self, model: Option<&str>) -> Result<Vec<Check>>;
    /// Streams the whole file as export records from one consistent snapshot, header first.
    fn export_snapshot(
        &self,
        now: i64,
        f: &mut dyn FnMut(ExportRecord) -> Result<()>,
    ) -> Result<()>;
    /// Restores an export into an empty file in one transaction: `next` yields the records with
    /// their line numbers. Nothing is kept unless every record is valid.
    fn restore(
        &self,
        next: &mut dyn FnMut() -> Result<Option<(u64, ExportRecord)>>,
        embed_job: Option<NewJob>,
    ) -> Result<ExportSummary>;
    fn scopes(&self) -> Result<Vec<ScopeInfo>>;
    fn actors(&self) -> Result<Vec<ActorInfo>>;
    /// Counts and sizes for `Engine::stats`; `model` is the embedder's, if there is one.
    fn stats(&self, model: Option<&str>, now: i64) -> Result<Stats>;
    fn usage_counts(&self, ids: &[MemoryId]) -> Result<std::collections::HashMap<MemoryId, u32>>;
    /// Decisions with `at` in `[from, to)`, oldest first. They are audit data: not part of the
    /// projection and untouched by `rebuild_projection`.
    fn decisions(&self, scope: ScopeId, from: i64, to: i64) -> Result<Vec<DecisionRecord>>;

    fn get(&self, id: MemoryId) -> Result<MemoryRow>;
    fn list(
        &self,
        scope: ScopeId,
        state: Option<State>,
        kind: Option<MemoryType>,
    ) -> Result<Vec<MemoryRow>>;
    /// Stemmed full-text search in one scope, best match first. The query is never read as FTS
    /// syntax. Without `as_of`/`known_at` only active memories match; with either, replaced
    /// ones count too (they were true once). Quarantine only with `include_quarantine`.
    fn search(&self, scope: ScopeId, query: &str, opts: &SearchOpts) -> Result<Vec<Hit>>;
    /// Operations touching the memory, oldest first.
    fn history(&self, id: MemoryId) -> Result<Vec<Operation>>;
    fn operation(&self, id: OpId) -> Result<Operation>;
    /// Operations with id >= `from`, oldest first.
    fn operations_from(&self, from: OpId) -> Result<Vec<Operation>>;
    fn operations_by_actor(&self, actor: ActorId) -> Result<Vec<Operation>>;

    /// Memories that are not archived and have no vector for `model`, oldest first, with their text.
    fn pending_embeddings(&self, model: &str, limit: usize) -> Result<Vec<(MemoryId, String)>>;
    /// Stores already-validated, normalized vectors in one transaction; replaces existing ones.
    /// A memory that does not exist fails the whole batch.
    fn put_vectors(&self, model: &str, dim: usize, vectors: &[(MemoryId, Vec<f32>)]) -> Result<()>;

    /// Records that these memories cannot be represented by `model` (e.g. an emoji-only text),
    /// so they stop showing up as pending. Stored as a zero-dimension vector, which `nearest`
    /// never matches.
    fn mark_unembeddable(&self, model: &str, memories: &[MemoryId]) -> Result<()>;
    fn create_block(&self, scope: ScopeId, name: &str, budget_tokens: usize) -> Result<BlockId>;
    fn block(&self, id: BlockId) -> Result<Block>;
    fn blocks(&self, scope: ScopeId) -> Result<Vec<Block>>;
    /// Replaces the block's memory list; position is the order given.
    fn set_block_memories(&self, block: BlockId, memories: &[MemoryId]) -> Result<()>;
    fn block_memories(&self, block: BlockId) -> Result<Vec<MemoryId>>;

    /// Drops the projection and index and refolds the whole log in one transaction.
    fn rebuild_projection(&self) -> Result<()>;
}
