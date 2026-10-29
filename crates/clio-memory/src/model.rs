use serde::{Deserialize, Serialize};

macro_rules! id {
    ($($name:ident),*) => {$(
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name(pub i64);
    )*};
}
id!(ScopeId, MemoryId, OpId, ActorId, BlockId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryType {
    Working,
    Episodic,
    Semantic,
    Procedural,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    User,
    Agent,
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Active,
    Quarantine,
    Archived,
    Replaced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActorKind {
    Agent,
    Session,
    Person,
    Importer,
    /// Internal: signs the operations of background jobs (forgetting). It cannot be registered
    /// through `Engine::register_actor` and has no review powers.
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpKind {
    Add,
    Update,
    Noop,
    Promote,
    Archive,
    Revert,
    /// A memory sent to quarantine for review because of repeated "wrong" feedback.
    Flag,
}

/// A row of the projection; also the snapshot stored in the operation log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryRow {
    pub id: MemoryId,
    pub scope: ScopeId,
    pub kind: MemoryType,
    pub content: String,
    pub content_hash: String,
    pub topic_key: Option<String>,
    pub origin: Origin,
    pub actor: ActorId,
    pub session: Option<String>,
    pub episode: Option<String>,
    pub source: Option<String>,
    pub state: State,
    pub valid_from: i64,
    pub valid_to: Option<i64>,
    pub known_since: i64,
    pub replaced_by: Option<MemoryId>,
    /// Set on a quarantined candidate that would replace this memory once promoted.
    pub supersedes: Option<MemoryId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub memory: MemoryId,
    pub before: Option<MemoryRow>,
    pub after: MemoryRow,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Operation {
    pub id: OpId,
    pub scope: ScopeId,
    pub kind: OpKind,
    pub memory: Option<MemoryId>,
    pub changes: Vec<Change>,
    pub actor: ActorId,
    pub reason: String,
    pub at: i64,
    pub reverts: Option<OpId>,
    /// The trace that caused it (telemetry feature); `None` without the feature, and in exports
    /// that predate the field.
    #[serde(default)]
    pub trace_id: Option<String>,
}

/// An operation not yet appended to the log (the store assigns the id).
#[derive(Debug, Clone, PartialEq)]
pub struct NewOperation {
    pub scope: ScopeId,
    pub kind: OpKind,
    pub memory: Option<MemoryId>,
    pub changes: Vec<Change>,
    pub actor: ActorId,
    pub reason: String,
    pub at: i64,
    pub reverts: Option<OpId>,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    pub scope: ScopeId,
    pub actor: ActorId,
    pub kind: MemoryType,
    pub content: String,
    pub topic_key: Option<String>,
    pub origin: Origin,
    pub session: Option<String>,
    pub episode: Option<String>,
    pub source: Option<String>,
    pub valid_from: Option<i64>,
    pub valid_to: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    pub op: OpId,
    pub kind: OpKind,
    pub memory: MemoryId,
    pub state: State,
}

/// Proposals an actor may make per window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quota {
    pub max: u32,
    pub window_secs: i64,
}

impl Default for Quota {
    fn default() -> Self {
        Self {
            max: 100,
            window_secs: 3600,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOpts {
    pub include_quarantine: bool,
    /// What was true at this time (validity axis).
    pub as_of: Option<i64>,
    /// What the system knew at this time (knowledge axis).
    pub known_at: Option<i64>,
    pub kind: Option<MemoryType>,
    pub limit: usize,
}

impl Default for SearchOpts {
    fn default() -> Self {
        Self {
            include_quarantine: false,
            as_of: None,
            known_at: None,
            kind: None,
            limit: 10,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub memory: MemoryRow,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RevertReport {
    pub reverted: Vec<OpId>,
    pub skipped: Vec<OpId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    ReadWrite,
    ReadOnly,
}

/// Options for `recall`. `search.limit` is the number of results returned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecallOpts {
    pub search: SearchOpts,
    /// Minimum cosine for a vector match to count as relevant.
    pub min_vector_score: f32,
    /// Fraction of the query's words (after stemming) a text match must contain to count as
    /// relevant. BM25 scores are not used for this: they depend on corpus size (all near zero
    /// with one or two memories) and rate common words like "de" as meaningful.
    pub min_text_coverage: f64,
    pub token_budget: Option<usize>,
}

impl Default for RecallOpts {
    // Checked once against potion-multilingual-128M on a 12-memory Spanish bench
    // (`real_model_abstention_bench`): questions with an answer scored 0.44-0.77 cosine and
    // questions without one 0.11-0.20, so 0.30 sits between them. A small sample: still to be
    // tuned by the evaluation harness (v2). The thresholds lean towards "no results".
    fn default() -> Self {
        Self {
            search: SearchOpts::default(),
            min_vector_score: 0.30,
            min_text_coverage: 0.5,
            token_budget: None,
        }
    }
}

/// One recalled memory with how it scored, so a caller can see why it came out.
#[derive(Debug, Clone, PartialEq)]
pub struct RecallHit {
    pub memory: MemoryRow,
    pub score: f64,
    pub text: Option<f64>,
    pub vector: Option<f32>,
    pub recency: f64,
    /// What feedback says about it, 0.5 when nobody has said anything.
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recall {
    pub hits: Vec<RecallHit>,
    /// True when nothing was relevant (not when a budget cut everything).
    pub abstained: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    /// One short line per fact; what models should read.
    #[default]
    Compact,
    /// Full rows as JSON; for programs.
    Json,
}

/// A fixed block: memories that always go into the prompt (identity, rules, preferences),
/// in priority order, up to a token budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub id: BlockId,
    pub scope: ScopeId,
    pub name: String,
    pub budget_tokens: usize,
}

/// What goes into the prompt at the start of a session.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextOpts {
    /// What the agent is about to work on; without it only the fixed blocks are returned.
    pub query: Option<String>,
    /// Its `token_budget` is ignored here: `recall_budget` applies instead.
    pub recall: RecallOpts,
    /// Tokens for the recalled section. Each block has its own budget.
    pub recall_budget: usize,
    pub format: Format,
}

impl Default for ContextOpts {
    fn default() -> Self {
        Self {
            query: None,
            recall: RecallOpts::default(),
            recall_budget: 400,
            format: Format::Compact,
        }
    }
}

/// Size limits in bytes (not characters) for what `propose` and the name-taking methods accept.
/// A memory is an atomic fact, so the defaults are small; raise `max_content_bytes` for long
/// procedures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_content_bytes: usize,
    /// For `topic_key`, `session`, `episode`, `source` and scope, actor and block names.
    pub max_field_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_content_bytes: 16 * 1024,
            max_field_bytes: 256,
        }
    }
}

/// What a decider is asked about: the proposal and the memories most like it.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub scope: ScopeId,
    pub kind: MemoryType,
    /// Normalized content, already past the secret detector.
    pub content: String,
    pub topic_key: Option<String>,
    /// The origin after the rules' trust check (an agent cannot claim `User`).
    pub origin: Origin,
    pub actor_kind: ActorKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Neighbor {
    pub memory: MemoryRow,
    /// Cosine similarity to the candidate.
    pub similarity: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    pub candidate: Candidate,
    /// Active memories of the same scope, most similar first (empty without an embedder).
    pub neighbors: Vec<Neighbor>,
}

/// A decider can only keep the proposal as a new memory or ask a person to look at it.
/// It can never drop the text, replace, archive or delete: whatever it says, the proposal is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Choice {
    Add,
    /// The candidate goes to quarantine; a person resolves it with `promote` or `archive`.
    Review,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub choice: Choice,
    /// In `[0, 1]`; anything else (including NaN) makes the decision invalid.
    pub confidence: f32,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeciderMode {
    Active,
    /// Consulted and logged, never acted on: to compare a new decider before trusting it.
    Shadow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecisionStatus {
    Decided,
    /// The decider answered "I don't know".
    None,
    BelowThreshold,
    Invalid,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub id: i64,
    pub scope: ScopeId,
    /// The operation written in the same transaction.
    pub op: Option<OpId>,
    pub provider: String,
    pub mode: DeciderMode,
    pub status: DecisionStatus,
    pub choice: Option<Choice>,
    /// `None` when the decider gave none, and also when it gave NaN (SQLite stores NaN as NULL);
    /// the status then says `Invalid`.
    pub confidence: Option<f32>,
    pub reason: Option<String>,
    pub acted: bool,
    pub at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobKind {
    /// Computes the vectors that are missing.
    Embed,
    /// Applies recorded uses (a recall touched these memories).
    Touch,
    /// Archives memories that were not used for the scope's retention period.
    Decay,
    Backup,
    Vacuum,
}

impl JobKind {
    pub(crate) fn priority(self) -> i32 {
        match self {
            JobKind::Embed => 10,
            JobKind::Touch => 5,
            JobKind::Decay => 3,
            JobKind::Backup => 2,
            JobKind::Vacuum => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    Pending,
    Running,
    Done,
    /// In the error queue: out of attempts, or failed for a reason retrying cannot fix.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobRecord {
    pub id: i64,
    pub kind: JobKind,
    /// Ids, a scope, a time or a path; never memory text.
    pub payload: String,
    pub priority: i32,
    pub state: JobState,
    pub attempts: u32,
    pub max_attempts: u32,
    pub run_after: i64,
    pub lease_until: Option<i64>,
    pub dedupe_key: Option<String>,
    pub error: Option<String>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct QueueStats {
    pub pending: usize,
    pub running: usize,
    pub failed: usize,
    pub oldest_pending_age_secs: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScopeInfo {
    pub id: ScopeId,
    pub name: String,
    pub language: String,
    /// `None` means nothing is ever forgotten for lack of use.
    pub retention_days: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActorInfo {
    pub id: ActorId,
    pub name: String,
    pub kind: ActorKind,
}

/// Memories of one scope by state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScopeStats {
    pub scope: ScopeId,
    pub name: String,
    pub active: u64,
    pub quarantine: u64,
    pub archived: u64,
    pub replaced: u64,
}

/// A cheap picture of the file: sizes, row counts and the queue. Reads only.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stats {
    pub file_bytes: u64,
    /// Size of the `-wal` file next to the database, 0 when there is none.
    pub wal_bytes: u64,
    pub scopes: Vec<ScopeStats>,
    pub operations: u64,
    pub decisions: u64,
    pub vectors: u64,
    /// Memories still waiting for a vector of the current model (0 without an embedder).
    pub pending_embeddings: usize,
    pub queue: QueueStats,
}

/// One health check of `Engine::doctor`; `name` is stable so scripts can match on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorReport {
    pub checks: Vec<Check>,
}

impl DoctorReport {
    pub fn all_ok(&self) -> bool {
        self.checks.iter().all(|c| c.ok)
    }

    pub fn get(&self, name: &str) -> Option<&Check> {
        self.checks.iter().find(|c| c.name == name)
    }
}

/// How `Engine::import_memories` labels what it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportOpts {
    /// Where the memories claim to come from. `External` (the default) sends them to
    /// quarantine for a person to review; `User` is for a person or importer who vouches for them.
    pub origin: Origin,
}

impl Default for ImportOpts {
    fn default() -> Self {
        Self {
            origin: Origin::External,
        }
    }
}

/// What `Engine::import_memories` did. `read` counts non-blank lines; every line ends up in
/// exactly one of `added` (active, including updates by topic), `noop` (already known),
/// `quarantined` or `rejected_total`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportSummary {
    pub read: usize,
    pub added: usize,
    pub noop: usize,
    pub quarantined: usize,
    /// `(1-based line number, reason)` of the first 100 rejected lines.
    pub rejected: Vec<(usize, String)>,
    pub rejected_total: usize,
    /// Set when the run stopped early because storage or the input failed (not because a line
    /// was bad): the line it was on and why. Everything before it was committed and is counted.
    pub aborted: Option<(usize, String)>,
}

/// How many lines of each kind an export wrote (or an import restored).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExportSummary {
    pub scopes: usize,
    pub actors: usize,
    pub operations: usize,
    pub decisions: usize,
    pub blocks: usize,
    pub usage: usize,
    pub feedback: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOpts {
    pub max_jobs: usize,
    /// Allow `Backup` and `Vacuum`. The crate does not detect an idle machine: the caller decides.
    pub allow_heavy: bool,
    /// Run only jobs of this kind and leave the rest queued.
    pub only: Option<JobKind>,
}

impl Default for RunOpts {
    fn default() -> Self {
        Self {
            max_jobs: 100,
            allow_heavy: false,
            only: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunReport {
    pub done: usize,
    pub retried: usize,
    pub failed: usize,
    /// Heavy jobs that were ready but not allowed to run.
    pub skipped_heavy: usize,
    /// Jobs a dead worker left `Running` whose lease expired.
    pub recovered: usize,
}

/// What an actor says about a memory it saw: useful, or wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Signal {
    Useful,
    Wrong,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FeedbackSummary {
    pub useful: u32,
    /// "Wrong" signals not yet spent by a flag or a promote.
    pub wrong: u32,
    /// `clamp(0.5 + 0.1 * useful - 0.2 * wrong, 0, 1)`.
    pub confidence: f32,
}
