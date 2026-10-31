use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use rusqlite::types::Type;
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Row, TransactionBehavior, named_params, params,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Error, Result};
use crate::idle::{DEFAULT_IDLE, IdleCache};
use crate::model::{
    ActorId, ActorInfo, ActorKind, Block, BlockId, Change, Check, DecisionRecord, ExportSummary,
    Hit, JobKind, JobRecord, JobState, MemoryId, MemoryRow, MemoryType, NewOperation, OpId,
    OpenMode, Operation, QueueStats, Quota, ScopeId, ScopeInfo, ScopeStats, SearchOpts, Signal,
    State, Stats,
};
use crate::platform::{self, WriterLock};
use crate::secrets::find_secret;
use crate::stem::fts_query;
use crate::store::{NewDecision, NewJob, Store};
use crate::telemetry::acceptable;
use crate::transfer::{EXPORT_FORMAT, EXPORT_FORMAT_VERSION, ExportRecord, check_header};
use crate::vector::{VectorIndex, dot, dot_f32, to_blob};

mod fold;
mod schema;
#[cfg(test)]
mod tests;

const MEM_COLS: &str = "id, scope, kind, content, content_hash, topic_key, origin, actor, session, \
    episode, source, state, valid_from, valid_to, known_since, replaced_by, supersedes";
const OP_COLS: &str = "id, scope, kind, memory, changes, actor, reason, at, reverts, trace_id";

/// Enums are stored as their bare variant name.
fn enc<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v)
        .map(|s| s.trim_matches('"').to_owned())
        .unwrap_or_default()
}

fn dec<T: DeserializeOwned>(s: &str) -> std::result::Result<T, serde_json::Error> {
    serde_json::from_str(&format!("\"{s}\""))
}

fn conv_err(i: usize, e: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(i, Type::Text, Box::new(e))
}

fn dec_at<T: DeserializeOwned>(r: &Row<'_>, i: usize) -> rusqlite::Result<T> {
    let s: String = r.get(i)?;
    dec(&s).map_err(|e| conv_err(i, e))
}

/// `cols` with every column name prefixed by `alias.`, for queries that join tables.
fn prefixed(cols: &str, alias: &str) -> String {
    cols.split(", ")
        .map(|c| format!("{alias}.{c}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// SQL conditions (over alias `m`, named params `:as_of`, `:known_at`, `:kind`) deciding which
/// memories a read may see. Text and vector search share it so they can never disagree.
/// Without `as_of`/`known_at` only active memories match; with either, replaced ones count too
/// (they were true once). Quarantine only with `include_quarantine`.
fn visibility(opts: &SearchOpts) -> String {
    let mut states = vec!["'Active'"];
    if opts.as_of.is_some() || opts.known_at.is_some() {
        states.push("'Replaced'");
    }
    if opts.include_quarantine {
        states.push("'Quarantine'");
    }
    format!(
        "m.state IN ({})
         AND (:as_of IS NULL OR (m.valid_from <= :as_of AND (m.valid_to IS NULL OR m.valid_to > :as_of
              OR (:known_at IS NOT NULL AND m.replaced_by IS NOT NULL
                  AND (SELECT r.known_since FROM memories r WHERE r.id = m.replaced_by) > :known_at))))
         AND (:known_at IS NULL OR m.known_since <= :known_at)
         AND (:kind IS NULL OR m.kind = :kind)",
        states.join(", ")
    )
}

/// The vector scan. It must be driven from `vectors` (CROSS JOIN fixes the order): walking
/// `memories` by its scope index and jumping to `vectors` row by row is ~8x slower at 50 000
/// memories (578 ms vs 71 ms for a whole `recall`), because every row becomes a random read.
fn nearest_sql(opts: &SearchOpts) -> String {
    format!(
        "SELECT v.memory, v.vec FROM vectors v
         CROSS JOIN memories m ON m.id = v.memory
         WHERE v.model = :model AND v.dim = :dim AND m.scope = :scope AND {}",
        visibility(opts)
    )
}

fn history_sql() -> String {
    format!(
        "SELECT {} FROM operation_memories m JOIN operations o ON o.id = m.op
         WHERE m.memory = ?1 ORDER BY o.id",
        prefixed(OP_COLS, "o")
    )
}

fn by_actor_sql() -> String {
    format!("SELECT {OP_COLS} FROM operations WHERE actor = ?1 ORDER BY id")
}

const JOB_COLS: &str = "id, kind, payload, priority, state, attempts, max_attempts, run_after, \
    lease_until, dedupe_key, error, created_at, finished_at";

fn job_from(r: &Row<'_>) -> rusqlite::Result<JobRecord> {
    Ok(JobRecord {
        id: r.get(0)?,
        kind: dec_at(r, 1)?,
        payload: r.get(2)?,
        priority: r.get(3)?,
        state: dec_at(r, 4)?,
        attempts: r.get(5)?,
        max_attempts: r.get(6)?,
        run_after: r.get(7)?,
        lease_until: r.get(8)?,
        dedupe_key: r.get(9)?,
        error: r.get(10)?,
        created_at: r.get(11)?,
        finished_at: r.get(12)?,
    })
}

fn insert_job(conn: &Connection, job: &NewJob) -> Result<bool> {
    let changed = conn.execute(
        "INSERT OR IGNORE INTO jobs(kind, payload, priority, state, run_after, dedupe_key, created_at)
         VALUES (?1, ?2, ?3, 'Pending', ?4, ?5, ?6)",
        params![
            enc(&job.kind),
            job.payload,
            job.kind.priority(),
            job.run_after,
            job.dedupe_key,
            job.at
        ],
    )?;
    Ok(changed > 0)
}

/// True if another `Pending` job holds the same dedupe key as job `id`.
fn has_pending_twin(conn: &Connection, id: i64) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM jobs t JOIN jobs j ON j.id = ?1
                        WHERE t.dedupe_key = j.dedupe_key AND t.state = 'Pending' AND t.id != j.id)",
        [id],
        |r| r.get(0),
    )?)
}

/// Returns a job to `Pending` at `at`, or closes it as `Done` when a twin already waits.
fn requeue(conn: &Connection, id: i64, at: i64, now: i64, error: &str) -> Result<()> {
    if has_pending_twin(conn, id)? {
        conn.execute(
            "UPDATE jobs SET state = 'Done', error = ?2, finished_at = ?3, lease_until = NULL
             WHERE id = ?1 AND state IN ('Running', 'Failed')",
            params![id, format!("{error} (superseded by a pending job)"), now],
        )?;
    } else {
        conn.execute(
            "UPDATE jobs SET state = 'Pending', run_after = ?2, error = ?3, lease_until = NULL
             WHERE id = ?1 AND state = 'Running'",
            params![id, at, error],
        )?;
    }
    Ok(())
}

/// The internal actor that signs background operations; the name prefix is reserved.
const SYSTEM_ACTOR: &str = "clio-memory:system";

const DECISION_COLS: &str =
    "id, scope, op, provider, mode, status, choice, confidence, reason, acted, at";

fn decision_from(r: &Row<'_>) -> rusqlite::Result<DecisionRecord> {
    let choice: Option<String> = r.get(6)?;
    Ok(DecisionRecord {
        id: r.get(0)?,
        scope: ScopeId(r.get(1)?),
        op: r.get::<_, Option<i64>>(2)?.map(OpId),
        provider: r.get(3)?,
        mode: dec_at(r, 4)?,
        status: dec_at(r, 5)?,
        choice: choice
            .map(|c| dec(&c))
            .transpose()
            .map_err(|e| conv_err(6, e))?,
        confidence: r.get::<_, Option<f64>>(7)?.map(|c| c as f32),
        reason: r.get(8)?,
        acted: r.get(9)?,
        at: r.get(10)?,
    })
}

/// Runs `sql` and hands every row, mapped by `map`, to `f` without collecting them.
fn stream<T>(
    conn: &Connection,
    sql: &str,
    map: impl Fn(&Row<'_>) -> rusqlite::Result<T>,
    f: &mut dyn FnMut(T) -> Result<()>,
) -> Result<()> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        f(map(row)?)?;
    }
    Ok(())
}

fn export_all(
    conn: &Connection,
    now: i64,
    f: &mut dyn FnMut(ExportRecord) -> Result<()>,
) -> Result<()> {
    let schema_version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    f(ExportRecord::Header {
        format: EXPORT_FORMAT.into(),
        format_version: EXPORT_FORMAT_VERSION.into(),
        schema_version,
        exported_at: now,
    })?;
    stream(
        conn,
        "SELECT id, name, language, retention_days FROM scopes ORDER BY id",
        |r| {
            Ok(ExportRecord::Scope {
                id: ScopeId(r.get(0)?),
                name: r.get(1)?,
                language: r.get(2)?,
                retention_days: r.get(3)?,
            })
        },
        f,
    )?;
    stream(
        conn,
        "SELECT id, name, kind, quota_max, quota_window FROM actors ORDER BY id",
        |r| {
            Ok(ExportRecord::Actor {
                id: ActorId(r.get(0)?),
                name: r.get(1)?,
                kind: dec_at(r, 2)?,
                quota_max: r.get(3)?,
                quota_window: r.get(4)?,
            })
        },
        f,
    )?;
    stream(
        conn,
        &format!("SELECT {OP_COLS} FROM operations ORDER BY id"),
        |r| op_from(r).map(ExportRecord::Operation),
        f,
    )?;
    stream(
        conn,
        &format!("SELECT {DECISION_COLS} FROM decisions ORDER BY id"),
        |r| decision_from(r).map(ExportRecord::Decision),
        f,
    )?;
    stream(
        conn,
        "SELECT id, scope, name, budget_tokens FROM blocks ORDER BY id",
        |r| block_from(r).map(ExportRecord::Block),
        f,
    )?;
    stream(
        conn,
        "SELECT block, position, memory FROM block_memories ORDER BY block, position",
        |r| {
            Ok(ExportRecord::BlockMemory {
                block: BlockId(r.get(0)?),
                position: r.get(1)?,
                memory: MemoryId(r.get(2)?),
            })
        },
        f,
    )?;
    stream(
        conn,
        "SELECT memory, uses, last_used FROM usage ORDER BY memory",
        |r| {
            Ok(ExportRecord::Usage {
                memory: MemoryId(r.get(0)?),
                uses: r.get(1)?,
                last_used: r.get(2)?,
            })
        },
        f,
    )?;
    stream(
        conn,
        "SELECT id, memory, actor, signal, at, spent FROM feedback ORDER BY id",
        |r| {
            Ok(ExportRecord::Feedback {
                id: r.get(0)?,
                memory: MemoryId(r.get(1)?),
                actor: ActorId(r.get(2)?),
                signal: dec_at(r, 3)?,
                at: r.get(4)?,
                spent: r.get(5)?,
            })
        },
        f,
    )
}

/// Writes one record of an import with its original ids. Everything is checked here, inside the
/// restore transaction, so a bad record aborts the whole thing.
/// Every text a memory snapshot carries (the same set `propose` checks).
fn row_has_secret(m: &MemoryRow) -> bool {
    [
        Some(&m.content),
        m.topic_key.as_ref(),
        m.session.as_ref(),
        m.episode.as_ref(),
        m.source.as_ref(),
    ]
    .into_iter()
    .flatten()
    .any(|t| find_secret(t))
}

fn restore_record(tx: &Connection, record: &ExportRecord) -> Result<()> {
    let bad = |m: &str| Err(Error::BadImport(m.into()));
    match record {
        ExportRecord::Header { .. } => {}
        ExportRecord::Scope {
            id,
            name,
            language,
            retention_days,
        } => {
            if find_secret(name) || find_secret(language) {
                return bad("a scope name looks like a secret");
            }
            tx.execute(
                "INSERT INTO scopes(id, name, language, retention_days) VALUES (?1, ?2, ?3, ?4)",
                params![id.0, name, language, retention_days],
            )?;
        }
        ExportRecord::Actor {
            id,
            name,
            kind,
            quota_max,
            quota_window,
        } => {
            // the internal actor and its reserved name go together, in both directions
            if (*kind == ActorKind::System) != (name == SYSTEM_ACTOR)
                || (*kind != ActorKind::System && name.starts_with("clio-memory:"))
            {
                return bad("an actor uses the reserved clio-memory: name or kind wrongly");
            }
            if find_secret(name) {
                return bad("an actor name looks like a secret");
            }
            tx.execute(
                "INSERT INTO actors(id, name, kind, quota_max, quota_window) VALUES (?1,?2,?3,?4,?5)",
                params![id.0, name, enc(kind), quota_max, quota_window],
            )?;
        }
        ExportRecord::Operation(op) => {
            // S-18: nothing that looks like a secret comes back through a restore
            let tainted = find_secret(&op.reason)
                || op.changes.iter().any(|c| {
                    row_has_secret(&c.after) || c.before.as_ref().is_some_and(row_has_secret)
                });
            if tainted {
                return bad("an operation contains something that looks like a secret");
            }
            if op.trace_id.as_deref().is_some_and(|t| !acceptable(t)) {
                return bad("an operation has a trace id that is not a plain identifier");
            }
            tx.execute(
                "INSERT INTO operations(id, scope, kind, memory, changes, actor, reason, at, reverts, trace_id)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![
                    op.id.0,
                    op.scope.0,
                    enc(&op.kind),
                    op.memory.map(|m| m.0),
                    serde_json::to_string(&op.changes)?,
                    op.actor.0,
                    op.reason,
                    op.at,
                    op.reverts.map(|o| o.0),
                    op.trace_id,
                ],
            )?;
        }
        ExportRecord::Decision(d) => {
            if d.reason.as_deref().is_some_and(find_secret) || find_secret(&d.provider) {
                return bad("a decision contains something that looks like a secret");
            }
            tx.execute(
                &format!("INSERT INTO decisions({DECISION_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)"),
                params![
                    d.id,
                    d.scope.0,
                    d.op.map(|o| o.0),
                    d.provider,
                    enc(&d.mode),
                    enc(&d.status),
                    d.choice.map(|c| enc(&c)),
                    d.confidence,
                    d.reason,
                    d.acted,
                    d.at,
                ],
            )?;
        }
        ExportRecord::Block(b) => {
            if find_secret(&b.name) {
                return bad("a block name looks like a secret");
            }
            tx.execute(
                "INSERT INTO blocks(id, scope, name, budget_tokens) VALUES (?1, ?2, ?3, ?4)",
                params![
                    b.id.0,
                    b.scope.0,
                    b.name,
                    i64::try_from(b.budget_tokens).unwrap_or(i64::MAX)
                ],
            )?;
        }
        ExportRecord::BlockMemory {
            block,
            position,
            memory,
        } => {
            tx.execute(
                "INSERT INTO block_memories(block, position, memory) VALUES (?1, ?2, ?3)",
                params![block.0, position, memory.0],
            )?;
        }
        ExportRecord::Usage {
            memory,
            uses,
            last_used,
        } => {
            tx.execute(
                "INSERT INTO usage(memory, uses, last_used) VALUES (?1, ?2, ?3)",
                params![memory.0, uses, last_used],
            )?;
        }
        ExportRecord::Feedback {
            id,
            memory,
            actor,
            signal,
            at,
            spent,
        } => {
            tx.execute(
                "INSERT INTO feedback(id, memory, actor, signal, at, spent) VALUES (?1,?2,?3,?4,?5,?6)",
                params![id, memory.0, actor.0, enc(signal), at, spent],
            )?;
        }
    }
    Ok(())
}

fn block_from(r: &Row<'_>) -> rusqlite::Result<Block> {
    Ok(Block {
        id: BlockId(r.get(0)?),
        scope: ScopeId(r.get(1)?),
        name: r.get(2)?,
        budget_tokens: usize::try_from(r.get::<_, i64>(3)?).unwrap_or(usize::MAX),
    })
}

/// A WAL database needs `-shm`/`-wal` files even for readers, so SQLite refuses to read it from a
/// directory the process cannot write to. The failure shows up on the first read, not on `open`
/// (an unreadable database file fails earlier, with its own error, and must not be blamed on the
/// directory). Say that instead of passing the raw error through.
/// (No `immutable` fallback: next to a live writer it would read stale data.)
fn read_only_failure(path: &Path, e: rusqlite::Error) -> Error {
    use rusqlite::ErrorCode::{CannotOpen, ReadOnly};
    match &e {
        rusqlite::Error::SqliteFailure(f, _) if matches!(f.code, ReadOnly | CannotOpen) => {
            let dir = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            Error::UnwritableDirectory(dir.to_path_buf())
        }
        _ => e.into(),
    }
}

fn memory_from(r: &Row<'_>) -> rusqlite::Result<MemoryRow> {
    Ok(MemoryRow {
        id: MemoryId(r.get(0)?),
        scope: ScopeId(r.get(1)?),
        kind: dec_at(r, 2)?,
        content: r.get(3)?,
        content_hash: r.get(4)?,
        topic_key: r.get(5)?,
        origin: dec_at(r, 6)?,
        actor: ActorId(r.get(7)?),
        session: r.get(8)?,
        episode: r.get(9)?,
        source: r.get(10)?,
        state: dec_at(r, 11)?,
        valid_from: r.get(12)?,
        valid_to: r.get(13)?,
        known_since: r.get(14)?,
        replaced_by: r.get::<_, Option<i64>>(15)?.map(MemoryId),
        supersedes: r.get::<_, Option<i64>>(16)?.map(MemoryId),
    })
}

fn op_from(r: &Row<'_>) -> rusqlite::Result<Operation> {
    let changes: Vec<Change> =
        serde_json::from_str(&r.get::<_, String>(4)?).map_err(|e| conv_err(4, e))?;
    Ok(Operation {
        id: OpId(r.get(0)?),
        scope: ScopeId(r.get(1)?),
        kind: dec_at(r, 2)?,
        memory: r.get::<_, Option<i64>>(3)?.map(MemoryId),
        changes,
        actor: ActorId(r.get(5)?),
        reason: r.get(6)?,
        at: r.get(7)?,
        reverts: r.get::<_, Option<i64>>(8)?.map(OpId),
        trace_id: r.get(9)?,
    })
}

/// A scan result ordered by quality: higher score is greater, and between equal scores the
/// lower id is greater (so ties come out in id order).
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Candidate {
    key: u32,
    tie: Reverse<i64>,
    id: MemoryId,
    score_bits: u32,
}

impl Candidate {
    fn new(id: MemoryId, score: f32) -> Self {
        let bits = score.to_bits();
        // maps f32 to an unsigned integer with the same order as the float
        let key = if bits & 0x8000_0000 != 0 {
            !bits
        } else {
            bits | 0x8000_0000
        };
        Self {
            key,
            tie: Reverse(id.0),
            id,
            score_bits: bits,
        }
    }
    fn score(&self) -> f32 {
        f32::from_bits(self.score_bits)
    }
}

/// Above this many bytes of vectors a search scans the file instead of keeping a copy in memory
/// (about 250 000 memories of 256 dimensions).
const MAX_VECTOR_CACHE_BYTES: usize = 256 << 20;

/// All the vectors of one model, decoded and contiguous: the position of a memory in `pos` is
/// where its vector starts in `data`. Visibility is never cached; it comes from the file on
/// every search.
struct VecCache {
    dim: usize,
    pos: HashMap<i64, usize>,
    data: Vec<f32>,
}

impl VecCache {
    fn load(conn: &Connection, model: &str, dim: usize) -> Result<Self> {
        let mut stmt =
            conn.prepare("SELECT memory, vec FROM vectors WHERE model = ?1 AND dim = ?2")?;
        let mut rows = stmt.query(params![model, i64::try_from(dim).unwrap_or(i64::MAX)])?;
        let mut cache = Self {
            dim,
            pos: HashMap::new(),
            data: Vec::new(),
        };
        while let Some(r) = rows.next()? {
            let blob = r.get_ref(1)?.as_blob().map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(1, Type::Blob, Box::new(e))
            })?;
            if blob.len() != dim * 4 {
                continue;
            }
            cache.pos.insert(r.get(0)?, cache.pos.len());
            cache.data.extend(
                blob.chunks_exact(4)
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])),
            );
        }
        Ok(cache)
    }

    fn vector(&self, id: i64) -> Option<&[f32]> {
        let at = *self.pos.get(&id)? * self.dim;
        self.data.get(at..at + self.dim)
    }
}

/// One long-lived read-only connection and the vector copies that go with it. A copy is valid
/// while its stamp does not change. A read-write store is the only writer of `vectors` (it holds
/// the writer lock), so its stamp is its own count of vector commits: a recall writes usage on
/// every call, and that must not throw the copy away. A read-only store has no such count and
/// uses `PRAGMA data_version`, which changes whenever another connection commits anything.
struct Reader {
    conn: Connection,
    version: i64,
    caches: HashMap<(String, usize), Arc<VecCache>>,
    max_bytes: usize,
}

impl Reader {
    fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        mmap(&conn)?;
        Ok(Self {
            conn,
            version: -1,
            caches: HashMap::new(),
            max_bytes: MAX_VECTOR_CACHE_BYTES,
        })
    }

    /// The copy of the vectors for `key` (fresh) and the ids visible under `opts`, or `None` when
    /// the vectors are too many to keep in memory.
    fn prepare(
        &mut self,
        key: &(String, usize),
        scope: ScopeId,
        opts: &SearchOpts,
        epoch: Option<u64>,
    ) -> Result<Option<(Arc<VecCache>, Vec<i64>)>> {
        let version: i64 = match epoch {
            Some(e) => i64::try_from(e).unwrap_or(i64::MAX),
            None => self
                .conn
                .pragma_query_value(None, "data_version", |r| r.get(0))?,
        };
        if version != self.version {
            self.caches.clear();
            self.version = version;
        }
        if !self.caches.contains_key(key) {
            let dim = i64::try_from(key.1).unwrap_or(i64::MAX);
            let count: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM vectors WHERE model = ?1 AND dim = ?2",
                params![key.0, dim],
                |r| r.get(0),
            )?;
            if usize::try_from(count)
                .unwrap_or(usize::MAX)
                .saturating_mul(key.1 * 4)
                > self.max_bytes
            {
                return Ok(None);
            }
            let loaded = VecCache::load(&self.conn, &key.0, key.1)?;
            self.caches.insert(key.clone(), Arc::new(loaded));
        }
        let Some(cache) = self.caches.get(key).cloned() else {
            return Ok(None);
        };
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT m.id FROM memories m WHERE m.scope = :scope AND {}",
            visibility(opts)
        ))?;
        let ids = stmt
            .query_map(
                named_params! {
                    ":scope": scope.0,
                    ":as_of": opts.as_of,
                    ":known_at": opts.known_at,
                    ":kind": opts.kind.map(|k| enc(&k)),
                },
                |r| r.get::<_, i64>(0),
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Some((cache, ids)))
    }
}

/// The reader leaves memory after five idle minutes, copies of the vectors included.
fn vector_reader(path: &Path) -> Result<IdleCache<Reader>> {
    let owned = path.to_path_buf();
    Ok(IdleCache::new(
        Reader::open(path)?,
        Some(DEFAULT_IDLE),
        move || Reader::open(&owned),
    ))
}

/// Lets SQLite read the file through the OS page cache instead of copying every page.
fn mmap(conn: &Connection) -> Result<()> {
    let _: i64 = conn.query_row("PRAGMA mmap_size = 268435456", [], |r| r.get(0))?;
    Ok(())
}

pub struct SqliteStore {
    path: PathBuf,
    write: Option<Mutex<Connection>>,
    vectors: IdleCache<Reader>,
    /// Commits that changed `vectors`; see `Reader`.
    vector_epoch: AtomicU64,
    _lock: Option<WriterLock>,
}

impl SqliteStore {
    /// `ReadWrite` takes the exclusive writer lock (`WriterLocked` if another process has it);
    /// `ReadOnly` never writes.
    pub fn open(path: &Path, mode: OpenMode) -> Result<Self> {
        if mode == OpenMode::ReadOnly {
            platform::check_db_file(path)?;
            let probe = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let version: i64 = probe
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .map_err(|e| read_only_failure(path, e))?;
            if version > schema::VERSION {
                return Err(Error::SchemaTooNew(version));
            }
            if version == 0 {
                return Err(Error::NotFound); // empty or never initialised
            }
            return Ok(Self {
                path: path.to_path_buf(),
                write: None,
                vectors: vector_reader(path)?,
                vector_epoch: AtomicU64::new(0),
                _lock: None,
            });
        }
        platform::prepare_db_file(path)?;
        let lock = platform::try_lock_writer(path)?;
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        schema::init(&mut conn)?;
        Ok(Self {
            path: path.to_path_buf(),
            write: Some(Mutex::new(conn)),
            vectors: vector_reader(path)?,
            vector_epoch: AtomicU64::new(0),
            _lock: Some(lock),
        })
    }

    fn writer(&self) -> Result<MutexGuard<'_, Connection>> {
        let m = self.write.as_ref().ok_or(Error::ReadOnly)?;
        Ok(m.lock().unwrap_or_else(PoisonError::into_inner))
    }

    // shortcut: one short-lived read-only connection per read; pool them if recall latency needs it.
    fn reader(&self) -> Result<Connection> {
        let conn = Connection::open_with_flags(&self.path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        mmap(&conn)?;
        Ok(conn)
    }

    fn ops(&self, sql: &str, arg: i64) -> Result<Vec<Operation>> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([arg], op_from)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn find_one(&self, sql: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Option<MemoryRow>> {
        Ok(self
            .reader()?
            .query_row(sql, args, memory_from)
            .optional()?)
    }
}

impl VectorIndex for SqliteStore {
    fn nearest(
        &self,
        scope: ScopeId,
        model: &str,
        query: &[f32],
        opts: &SearchOpts,
        k: usize,
    ) -> Result<Vec<(MemoryId, f32)>> {
        if k == 0 {
            return Ok(Vec::new());
        }
        let key = (model.to_string(), query.len());
        let epoch = self
            .write
            .is_some()
            .then(|| self.vector_epoch.load(Ordering::SeqCst));
        let Some((cache, ids)) = self
            .vectors
            .with(|r| r.prepare(&key, scope, opts, epoch))??
        else {
            return self.nearest_scan(scope, model, query, opts, k);
        };
        // Bounded min-heap, as in the scan: the same candidates rank the same way.
        let mut best: BinaryHeap<Reverse<Candidate>> = BinaryHeap::new();
        for id in ids {
            let Some(v) = cache.vector(id) else { continue };
            let candidate = Candidate::new(MemoryId(id), dot_f32(v, query));
            if best.len() < k {
                best.push(Reverse(candidate));
            } else if best.peek().is_some_and(|worst| candidate > worst.0) {
                best.pop();
                best.push(Reverse(candidate));
            }
        }
        Ok(best
            .into_sorted_vec()
            .into_iter()
            .map(|Reverse(c)| (c.id, c.score()))
            .collect())
    }
}

impl SqliteStore {
    /// The search without a copy in memory: reads every vector from the file. It is what a search
    /// does when there are too many vectors to keep, and what the tests compare the copy against.
    fn nearest_scan(
        &self,
        scope: ScopeId,
        model: &str,
        query: &[f32],
        opts: &SearchOpts,
        k: usize,
    ) -> Result<Vec<(MemoryId, f32)>> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(&nearest_sql(opts))?;
        let dim = i64::try_from(query.len()).unwrap_or(i64::MAX);
        let mut rows = stmt.query(named_params! {
            ":model": model,
            ":dim": dim,
            ":scope": scope.0,
            ":as_of": opts.as_of,
            ":known_at": opts.known_at,
            ":kind": opts.kind.map(|k| enc(&k)),
        })?;
        // Bounded min-heap: the worst kept candidate sits on top, so each row costs O(log k).
        let mut best: BinaryHeap<Reverse<Candidate>> = BinaryHeap::new();
        while let Some(r) = rows.next()? {
            let blob = r.get_ref(1)?.as_blob().map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(1, Type::Blob, Box::new(e))
            })?;
            if blob.len() != query.len() * 4 {
                continue;
            }
            let candidate = Candidate::new(MemoryId(r.get(0)?), dot(blob, query));
            if best.len() < k {
                best.push(Reverse(candidate));
            } else if best.peek().is_some_and(|worst| candidate > worst.0) {
                best.pop();
                best.push(Reverse(candidate));
            }
        }
        let best = best
            .into_sorted_vec()
            .into_iter()
            .map(|Reverse(c)| (c.id, c.score()))
            .collect();
        Ok(best)
    }
}

/// Writes one operation, its decisions and jobs inside a transaction the caller owns.
fn append_in(
    tx: &Connection,
    op: NewOperation,
    decisions: &[NewDecision],
    jobs: &[NewJob],
) -> Result<OpId> {
    tx.execute(
        "INSERT INTO operations(scope, kind, memory, changes, actor, reason, at, reverts, trace_id)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            op.scope.0,
            enc(&op.kind),
            op.memory.map(|m| m.0),
            serde_json::to_string(&op.changes)?,
            op.actor.0,
            op.reason,
            op.at,
            op.reverts.map(|o| o.0),
            op.trace_id,
        ],
    )?;
    let id = OpId(tx.last_insert_rowid());
    fold::link(tx, id, op.memory, &op.changes)?;
    for change in &op.changes {
        fold::apply_change(tx, &change.after)?;
    }
    for d in decisions {
        tx.execute(
                "INSERT INTO decisions(scope, op, provider, mode, status, choice, confidence, reason, acted, at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![
                    d.scope.0,
                    id.0,
                    d.provider,
                    enc(&d.mode),
                    enc(&d.status),
                    d.choice.map(|c| enc(&c)),
                    d.confidence,
                    d.reason,
                    d.acted,
                    d.at,
                ],
            )?;
    }
    for job in jobs {
        insert_job(tx, job)?;
    }
    Ok(id)
}

fn spend_wrong_in(conn: &Connection, memory: MemoryId) -> Result<()> {
    conn.execute(
        "UPDATE feedback SET spent = 1 WHERE memory = ?1 AND signal = 'Wrong' AND spent = 0",
        [memory.0],
    )?;
    Ok(())
}

impl Store for SqliteStore {
    fn create_scope(&self, name: &str, language: &str) -> Result<ScopeId> {
        let conn = self.writer()?;
        conn.execute(
            "INSERT INTO scopes(name, language) VALUES (?1, ?2)",
            [name, language],
        )?;
        Ok(ScopeId(conn.last_insert_rowid()))
    }

    fn scope_language(&self, scope: ScopeId) -> Result<String> {
        self.reader()?
            .query_row(
                "SELECT language FROM scopes WHERE id = ?1",
                [scope.0],
                |r| r.get(0),
            )
            .optional()?
            .ok_or(Error::UnknownScope)
    }

    fn register_actor(&self, name: &str, kind: ActorKind, quota: Quota) -> Result<ActorId> {
        let conn = self.writer()?;
        conn.execute(
            "INSERT INTO actors(name, kind, quota_max, quota_window) VALUES (?1, ?2, ?3, ?4)",
            params![name, enc(&kind), quota.max, quota.window_secs],
        )?;
        Ok(ActorId(conn.last_insert_rowid()))
    }

    fn actor_kind(&self, actor: ActorId) -> Result<ActorKind> {
        let kind: Option<String> = self
            .reader()?
            .query_row("SELECT kind FROM actors WHERE id = ?1", [actor.0], |r| {
                r.get(0)
            })
            .optional()?;
        let kind = kind.ok_or(Error::UnknownActor)?;
        Ok(dec(&kind)?)
    }

    fn bump_quota(&self, actor: ActorId, now: i64) -> Result<()> {
        let mut conn = self.writer()?;
        let tx = conn.transaction()?;
        let (max, window): (i64, i64) = tx
            .query_row(
                "SELECT quota_max, quota_window FROM actors WHERE id = ?1",
                [actor.0],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or(Error::UnknownActor)?;
        let start = now - now.rem_euclid(window.max(1));
        let count: i64 = tx
            .query_row(
                "SELECT count FROM quota_counter WHERE actor = ?1 AND window_start = ?2",
                [actor.0, start],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if count >= max {
            return Err(Error::QuotaExceeded);
        }
        tx.execute(
            "INSERT OR REPLACE INTO quota_counter(actor, window_start, count) VALUES (?1, ?2, ?3)",
            [actor.0, start, count + 1],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn find_active_by_hash(&self, scope: ScopeId, hash: &str) -> Result<Option<MemoryRow>> {
        self.find_one(
            &format!("SELECT {MEM_COLS} FROM memories WHERE scope = ?1 AND content_hash = ?2 AND state = 'Active'"),
            &[&scope.0, &hash],
        )
    }

    fn find_active_by_topic(
        &self,
        scope: ScopeId,
        kind: MemoryType,
        topic: &str,
    ) -> Result<Option<MemoryRow>> {
        self.find_one(
            &format!("SELECT {MEM_COLS} FROM memories WHERE scope = ?1 AND kind = ?2 AND topic_key = ?3 AND state = 'Active'"),
            &[&scope.0, &enc(&kind), &topic],
        )
    }

    fn next_memory_id(&self) -> Result<MemoryId> {
        let conn = self.writer()?;
        let max: i64 = conn.query_row("SELECT COALESCE(MAX(id), 0) FROM memories", [], |r| {
            r.get(0)
        })?;
        Ok(MemoryId(max + 1))
    }

    fn append_all(
        &self,
        op: NewOperation,
        decisions: Vec<NewDecision>,
        jobs: Vec<NewJob>,
    ) -> Result<OpId> {
        let mut conn = self.writer()?;
        let tx = conn.transaction()?;
        let id = append_in(&tx, op, &decisions, &jobs)?;
        tx.commit()?;
        Ok(id)
    }

    fn append_spending(
        &self,
        op: NewOperation,
        decisions: Vec<NewDecision>,
        jobs: Vec<NewJob>,
        spend: MemoryId,
    ) -> Result<OpId> {
        let mut conn = self.writer()?;
        let tx = conn.transaction()?;
        let id = append_in(&tx, op, &decisions, &jobs)?;
        spend_wrong_in(&tx, spend)?;
        tx.commit()?;
        Ok(id)
    }

    fn record_feedback_flagging(
        &self,
        memory: MemoryId,
        actor: ActorId,
        signal: Signal,
        at: i64,
        threshold: u32,
        flag: &dyn Fn() -> (NewOperation, Vec<NewJob>),
    ) -> Result<Option<OpId>> {
        let mut conn = self.writer()?;
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO feedback(memory, actor, signal, at) VALUES (?1, ?2, ?3, ?4)",
            params![memory.0, actor.0, enc(&signal), at],
        )?;
        let mut flagged = None;
        if signal == Signal::Wrong {
            let unspent: u32 = tx.query_row(
                "SELECT COUNT(*) FROM feedback WHERE memory = ?1 AND signal = 'Wrong' AND spent = 0",
                [memory.0],
                |r| r.get(0),
            )?;
            if unspent >= threshold {
                let (op, jobs) = flag();
                flagged = Some(append_in(&tx, op, &[], &jobs)?);
                spend_wrong_in(&tx, memory)?;
            }
        }
        tx.commit()?;
        Ok(flagged)
    }

    fn enqueue(&self, job: NewJob) -> Result<bool> {
        let conn = self.writer()?;
        insert_job(&conn, &job)
    }

    fn claim_job(
        &self,
        now: i64,
        only: Option<JobKind>,
        allow_heavy: bool,
        lease_secs: i64,
    ) -> Result<Option<JobRecord>> {
        let mut conn = self.writer()?;
        let tx = conn.transaction()?;
        let found = tx
            .query_row(
                &format!(
                    "SELECT {JOB_COLS} FROM jobs WHERE state = 'Pending' AND run_after <= ?1
                       AND (?2 = 1 OR kind NOT IN ('Backup', 'Vacuum'))
                       AND (?3 IS NULL OR kind = ?3)
                     ORDER BY priority DESC, id LIMIT 1"
                ),
                params![now, allow_heavy, only.map(|k| enc(&k))],
                job_from,
            )
            .optional()?;
        let Some(mut job) = found else {
            return Ok(None);
        };
        tx.execute(
            "UPDATE jobs SET state = 'Running', attempts = attempts + 1, lease_until = ?2 WHERE id = ?1",
            params![job.id, now + lease_secs],
        )?;
        tx.commit()?;
        job.state = JobState::Running;
        job.attempts += 1;
        job.lease_until = Some(now + lease_secs);
        Ok(Some(job))
    }

    fn finish_job(&self, id: i64, now: i64) -> Result<()> {
        self.writer()?.execute(
            "UPDATE jobs SET state = 'Done', finished_at = ?2, lease_until = NULL, error = NULL
             WHERE id = ?1 AND state = 'Running'",
            params![id, now],
        )?;
        Ok(())
    }

    fn finish_jobs(&self, ids: &[i64], now: i64) -> Result<()> {
        let mut conn = self.writer()?;
        let tx = conn.transaction()?;
        for id in ids {
            tx.execute(
                "UPDATE jobs SET state = 'Done', finished_at = ?2, lease_until = NULL, error = NULL
                 WHERE id = ?1 AND state = 'Running'",
                params![id, now],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn claim_jobs(
        &self,
        now: i64,
        kind: JobKind,
        lease_secs: i64,
        limit: usize,
    ) -> Result<Vec<JobRecord>> {
        let mut conn = self.writer()?;
        let tx = conn.transaction()?;
        let mut claimed: Vec<JobRecord> = {
            let mut stmt = tx.prepare(&format!(
                "SELECT {JOB_COLS} FROM jobs WHERE state = 'Pending' AND run_after <= ?1 AND kind = ?2
                 ORDER BY id LIMIT ?3"
            ))?;
            let limit = i64::try_from(limit).unwrap_or(i64::MAX);
            stmt.query_map(params![now, enc(&kind), limit], job_from)?
                .collect::<std::result::Result<_, _>>()?
        };
        for job in &mut claimed {
            tx.execute(
                "UPDATE jobs SET state = 'Running', attempts = attempts + 1, lease_until = ?2 WHERE id = ?1",
                params![job.id, now + lease_secs],
            )?;
            job.state = JobState::Running;
            job.attempts += 1;
            job.lease_until = Some(now + lease_secs);
        }
        tx.commit()?;
        Ok(claimed)
    }

    fn fail_job(&self, id: i64, now: i64, error: &str, retry_at: Option<i64>) -> Result<()> {
        let mut conn = self.writer()?;
        let tx = conn.transaction()?;
        match retry_at {
            Some(at) => requeue(&tx, id, at, now, error)?,
            None => {
                tx.execute(
                    "UPDATE jobs SET state = 'Failed', error = ?2, finished_at = ?3, lease_until = NULL
                     WHERE id = ?1 AND state = 'Running'",
                    params![id, error, now],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn recover_leases(&self, now: i64) -> Result<usize> {
        let mut conn = self.writer()?;
        let tx = conn.transaction()?;
        let expired: Vec<(i64, u32, u32)> = {
            let mut stmt = tx.prepare(
                "SELECT id, attempts, max_attempts FROM jobs WHERE state = 'Running' AND lease_until < ?1",
            )?;
            stmt.query_map([now], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        for (id, attempts, max) in &expired {
            if attempts >= max {
                tx.execute(
                    "UPDATE jobs SET state = 'Failed', error = 'lease expired', finished_at = ?2, lease_until = NULL WHERE id = ?1",
                    params![id, now],
                )?;
            } else {
                requeue(&tx, *id, now, now, "lease expired")?;
            }
        }
        tx.commit()?;
        Ok(expired.len())
    }

    fn jobs(&self, state: Option<JobState>, limit: usize) -> Result<Vec<JobRecord>> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {JOB_COLS} FROM jobs WHERE (?1 IS NULL OR state = ?1) ORDER BY id DESC LIMIT ?2"
        ))?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = stmt.query_map(params![state.map(|s| enc(&s)), limit], job_from)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn queue_stats(&self, now: i64) -> Result<QueueStats> {
        let conn = self.reader()?;
        let count = |state: &str| -> Result<usize> {
            let n: i64 =
                conn.query_row("SELECT COUNT(*) FROM jobs WHERE state = ?1", [state], |r| {
                    r.get(0)
                })?;
            Ok(usize::try_from(n).unwrap_or(0))
        };
        let oldest: Option<i64> = conn.query_row(
            "SELECT MIN(created_at) FROM jobs WHERE state = 'Pending'",
            [],
            |r| r.get(0),
        )?;
        Ok(QueueStats {
            pending: count("Pending")?,
            running: count("Running")?,
            failed: count("Failed")?,
            oldest_pending_age_secs: oldest.map(|t| now - t),
        })
    }

    fn retry_job(&self, id: i64, now: i64) -> Result<()> {
        let mut conn = self.writer()?;
        let tx = conn.transaction()?;
        let state: Option<String> = tx
            .query_row("SELECT state FROM jobs WHERE id = ?1", [id], |r| r.get(0))
            .optional()?;
        match state.as_deref() {
            None => return Err(Error::NotFound),
            Some("Failed") => {}
            Some(_) => return Err(Error::InvalidState),
        }
        if has_pending_twin(&tx, id)? {
            // an identical job is already waiting: retrying this one would duplicate it
            requeue(&tx, id, now, now, "retried")?;
        } else {
            tx.execute(
                "UPDATE jobs SET state = 'Pending', attempts = 0, run_after = ?2, lease_until = NULL,
                        error = NULL, finished_at = NULL WHERE id = ?1",
                params![id, now],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn prune_jobs(&self, done_before: i64, failed_before: i64) -> Result<usize> {
        Ok(self.writer()?.execute(
            "DELETE FROM jobs WHERE (state = 'Done' AND finished_at < ?1)
                                 OR (state = 'Failed' AND finished_at < ?2)",
            params![done_before, failed_before],
        )?)
    }

    fn set_retention(&self, scope: ScopeId, days: Option<u32>) -> Result<()> {
        let changed = self.writer()?.execute(
            "UPDATE scopes SET retention_days = ?2 WHERE id = ?1",
            params![scope.0, days],
        )?;
        if changed == 0 {
            return Err(Error::UnknownScope);
        }
        Ok(())
    }

    fn retention(&self, scope: ScopeId) -> Result<Option<u32>> {
        self.reader()?
            .query_row(
                "SELECT retention_days FROM scopes WHERE id = ?1",
                [scope.0],
                |r| r.get(0),
            )
            .optional()?
            .ok_or(Error::UnknownScope)
    }

    fn decay_candidates(&self, scope: ScopeId, cutoff: i64) -> Result<Vec<MemoryRow>> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM memories m LEFT JOIN usage u ON u.memory = m.id
             WHERE m.scope = ?1 AND m.state = 'Active' AND m.kind IN ('Working', 'Episodic')
               AND MAX(m.known_since, COALESCE(u.last_used, 0),
                       COALESCE((SELECT MAX(f.at) FROM feedback f
                                 WHERE f.memory = m.id AND f.signal = 'Useful'), 0),
                       COALESCE((SELECT MAX(o.at) FROM operation_memories om
                                 JOIN operations o ON o.id = om.op
                                 WHERE om.memory = m.id AND o.kind IN ('Revert', 'Promote')), 0)) < ?2
             ORDER BY m.id",
            prefixed(MEM_COLS, "m")
        ))?;
        let rows = stmt.query_map(params![scope.0, cutoff], memory_from)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn system_actor(&self) -> Result<ActorId> {
        let conn = self.writer()?;
        if let Some(id) = conn
            .query_row(
                "SELECT id FROM actors WHERE name = ?1 AND kind = 'System'",
                [SYSTEM_ACTOR],
                |r| r.get(0),
            )
            .optional()?
        {
            return Ok(ActorId(id));
        }
        conn.execute(
            "INSERT INTO actors(name, kind, quota_max, quota_window) VALUES (?1, ?2, ?3, 3600)",
            params![SYSTEM_ACTOR, enc(&ActorKind::System), i64::from(u32::MAX)],
        )?;
        Ok(ActorId(conn.last_insert_rowid()))
    }

    fn backup_to(&self, path: &Path) -> Result<()> {
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        match std::fs::remove_file(&tmp) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        platform::create_private_file(&tmp)?;
        let copied = (|| -> Result<()> {
            let source = self.writer()?;
            let mut dest = Connection::open(&tmp)?;
            let backup = rusqlite::backup::Backup::new(&source, &mut dest)?;
            backup.run_to_completion(256, Duration::from_millis(10), None)?;
            Ok(())
        })();
        let published = copied.and_then(|()| Ok(std::fs::hard_link(&tmp, path)?));
        let _ = std::fs::remove_file(&tmp);
        published
    }

    fn vacuum(&self) -> Result<()> {
        self.writer()?.execute_batch("VACUUM")?;
        Ok(())
    }

    fn record_usages(&self, uses: &[(MemoryId, i64)]) -> Result<()> {
        let mut conn = self.writer()?;
        let tx = conn.transaction()?;
        for (id, at) in uses {
            tx.execute(
                "INSERT INTO usage(memory, uses, last_used) VALUES (?1, 1, ?2)
                 ON CONFLICT(memory) DO UPDATE SET uses = uses + 1, last_used = MAX(last_used, ?2)",
                params![id.0, at],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn feedback_counts(&self, memory: MemoryId) -> Result<(u32, u32)> {
        let conn = self.reader()?;
        let count =
            |sql: &str| -> Result<u32> { Ok(conn.query_row(sql, [memory.0], |r| r.get(0))?) };
        Ok((
            count("SELECT COUNT(*) FROM feedback WHERE memory = ?1 AND signal = 'Useful'")?,
            count(
                "SELECT COUNT(*) FROM feedback WHERE memory = ?1 AND signal = 'Wrong' AND spent = 0",
            )?,
        ))
    }

    fn feedback_counts_many(
        &self,
        ids: &[MemoryId],
    ) -> Result<std::collections::HashMap<MemoryId, (u32, u32)>> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT COALESCE(SUM(signal = 'Useful'), 0), COALESCE(SUM(signal = 'Wrong' AND spent = 0), 0)
             FROM feedback WHERE memory = ?1",
        )?;
        let mut out = std::collections::HashMap::new();
        for id in ids {
            let (useful, wrong) =
                stmt.query_row([id.0], |r| Ok((r.get::<_, u32>(0)?, r.get::<_, u32>(1)?)))?;
            if useful + wrong > 0 {
                out.insert(*id, (useful, wrong));
            }
        }
        Ok(out)
    }

    fn doctor(&self, model: Option<&str>) -> Result<Vec<Check>> {
        let conn = self.reader()?;
        let mut checks = Vec::new();
        let mut check = |name: &str, ok: bool, detail: String| {
            checks.push(Check {
                name: name.to_owned(),
                ok,
                detail,
            });
        };

        match platform::check_db_file(&self.path) {
            Ok(()) => check("permissions", true, "mode 0600".into()),
            Err(Error::InsecurePermissions(_)) => check(
                "permissions",
                false,
                "the file is accessible by group or others; run chmod 600".into(),
            ),
            Err(e) => check("permissions", false, e.to_string()),
        }

        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        check(
            "schema_version",
            version == schema::VERSION,
            format!("file {version}, this build {}", schema::VERSION),
        );

        let expected = fold::preview(&conn)?;
        let mut stmt = conn.prepare(&format!("SELECT {MEM_COLS} FROM memories"))?;
        let mut stored = std::collections::HashMap::new();
        for row in stmt.query_map([], memory_from)? {
            let row = row?;
            stored.insert(row.id, row);
        }
        let mut bad: Vec<i64> = expected
            .iter()
            .filter(|(id, row)| stored.get(id) != Some(row))
            .map(|(id, _)| id.0)
            .chain(
                stored
                    .keys()
                    .filter(|id| !expected.contains_key(id))
                    .map(|id| id.0),
            )
            .collect();
        bad.sort_unstable();
        let shown: Vec<String> = bad.iter().take(10).map(i64::to_string).collect();
        check(
            "projection_matches_log",
            bad.is_empty(),
            if bad.is_empty() {
                format!("{} memories match the log", expected.len())
            } else {
                format!(
                    "{} memories differ from the log (ids: {}{}); run rebuild_projection",
                    bad.len(),
                    shown.join(", "),
                    if bad.len() > 10 { ", …" } else { "" }
                )
            },
        );

        let count = |sql: &str| -> Result<i64> { Ok(conn.query_row(sql, [], |r| r.get(0))?) };
        let orphan_vectors =
            count("SELECT COUNT(*) FROM vectors WHERE memory NOT IN (SELECT id FROM memories)")?;
        check(
            "no_orphan_vectors",
            orphan_vectors == 0,
            format!("{orphan_vectors} vectors without a memory"),
        );
        let orphans = count(
            "SELECT (SELECT COUNT(*) FROM usage WHERE memory NOT IN (SELECT id FROM memories))
                  + (SELECT COUNT(*) FROM feedback WHERE memory NOT IN (SELECT id FROM memories))",
        )?;
        check(
            "no_orphan_usage_or_feedback",
            orphans == 0,
            format!("{orphans} usage or feedback rows without a memory"),
        );
        let dangling = count(
            "SELECT COUNT(*) FROM block_memories WHERE memory NOT IN (SELECT id FROM memories)",
        )?;
        check(
            "block_entries_exist",
            dangling == 0,
            format!("{dangling} block entries point to a missing memory"),
        );
        let failed = count("SELECT COUNT(*) FROM jobs WHERE state = 'Failed'")?;
        check(
            "failed_jobs",
            failed == 0,
            format!("{failed} jobs in the error queue"),
        );
        let pending = match model {
            Some(m) => conn.query_row(
                "SELECT COUNT(*) FROM memories m WHERE m.state != 'Archived'
                 AND NOT EXISTS (SELECT 1 FROM vectors v WHERE v.memory = m.id AND v.model = ?1)",
                [m],
                |r| r.get::<_, i64>(0),
            )?,
            None => 0,
        };
        check(
            "pending_embeddings",
            true,
            format!("{pending} memories waiting for a vector"),
        );
        Ok(checks)
    }

    fn export_snapshot(
        &self,
        now: i64,
        f: &mut dyn FnMut(ExportRecord) -> Result<()>,
    ) -> Result<()> {
        // One read transaction: every table is read from the same WAL snapshot, so a writer
        // proposing meanwhile cannot leave a line that points at an operation not exported.
        let conn = self.reader()?;
        conn.execute_batch("BEGIN")?;
        let result = export_all(&conn, now, f);
        let _ = conn.execute_batch("COMMIT"); // nothing to commit; just ends the snapshot
        result
    }

    fn restore(
        &self,
        next: &mut dyn FnMut() -> Result<Option<(u64, ExportRecord)>>,
        embed_job: Option<NewJob>,
    ) -> Result<ExportSummary> {
        let mut conn = self.writer()?;
        // dropping `tx` on any early return rolls everything back
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let count = |sql: &str| -> Result<i64> { Ok(tx.query_row(sql, [], |r| r.get(0))?) };
        let occupied = count(
            "SELECT (SELECT COUNT(*) FROM scopes) + (SELECT COUNT(*) FROM operations)",
        )? + count(&format!(
            "SELECT COUNT(*) FROM actors WHERE NOT (kind = 'System' AND name = '{SYSTEM_ACTOR}')"
        ))?;
        if occupied > 0 {
            return Err(Error::InvalidState);
        }
        // the internal actor is replaced by the exported one (it may not even be in the file)
        tx.execute_batch(&format!(
            "DELETE FROM quota_counter WHERE actor IN (SELECT id FROM actors WHERE name = '{SYSTEM_ACTOR}');
             DELETE FROM actors WHERE name = '{SYSTEM_ACTOR}';"
        ))?;

        let mut summary = ExportSummary::default();
        let mut seen_header = false;
        while let Some((line, record)) = next()? {
            let at_line = |e: Error| match e {
                Error::BadImport(m) => Error::BadImport(format!("line {line}: {m}")),
                Error::Sqlite(e) => Error::BadImport(format!("line {line}: {e}")),
                other => other,
            };
            match &record {
                ExportRecord::Header {
                    format,
                    format_version,
                    schema_version,
                    ..
                } => {
                    if seen_header {
                        return Err(at_line(Error::BadImport("a second header".into())));
                    }
                    check_header(format, format_version, *schema_version, schema::VERSION)
                        .map_err(at_line)?;
                    seen_header = true;
                }
                _ if !seen_header => {
                    return Err(at_line(Error::BadImport(
                        "the first line must be the header".into(),
                    )));
                }
                _ => restore_record(&tx, &record).map_err(at_line)?,
            }
            record.tally(&mut summary);
        }
        if !seen_header {
            return Err(Error::BadImport("the file is empty".into()));
        }
        fold::rebuild_in(&tx).map_err(|e| match e {
            // the snapshots name a scope or actor the file does not have, or similar
            Error::Sqlite(e) => Error::BadImport(format!("the file is inconsistent: {e}")),
            other => other,
        })?;
        if let Some(job) = &embed_job {
            insert_job(&tx, job)?;
        }
        tx.commit()?;
        Ok(summary)
    }

    fn scopes(&self) -> Result<Vec<ScopeInfo>> {
        let conn = self.reader()?;
        let mut stmt =
            conn.prepare("SELECT id, name, language, retention_days FROM scopes ORDER BY id")?;
        let rows = stmt.query_map([], |r| {
            Ok(ScopeInfo {
                id: ScopeId(r.get(0)?),
                name: r.get(1)?,
                language: r.get(2)?,
                retention_days: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn actors(&self) -> Result<Vec<ActorInfo>> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare("SELECT id, name, kind FROM actors ORDER BY id")?;
        let rows = stmt.query_map([], |r| {
            Ok(ActorInfo {
                id: ActorId(r.get(0)?),
                name: r.get(1)?,
                kind: dec_at(r, 2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn stats(&self, model: Option<&str>, now: i64) -> Result<Stats> {
        let file_bytes = std::fs::metadata(&self.path)?.len();
        let mut wal = self.path.as_os_str().to_owned();
        wal.push("-wal");
        let wal_bytes = std::fs::metadata(&wal).map_or(0, |m| m.len());
        let conn = self.reader()?;
        let count = |sql: &str| -> Result<u64> {
            let n: i64 = conn.query_row(sql, [], |r| r.get(0))?;
            Ok(u64::try_from(n).unwrap_or(0))
        };
        let mut stmt = conn.prepare(
            "SELECT s.id, s.name,
                    COALESCE(SUM(m.state = 'Active'), 0), COALESCE(SUM(m.state = 'Quarantine'), 0),
                    COALESCE(SUM(m.state = 'Archived'), 0), COALESCE(SUM(m.state = 'Replaced'), 0)
             FROM scopes s LEFT JOIN memories m ON m.scope = s.id GROUP BY s.id ORDER BY s.id",
        )?;
        let scopes = stmt
            .query_map([], |r| {
                let n = |i| r.get::<_, i64>(i).map(|v| u64::try_from(v).unwrap_or(0));
                Ok(ScopeStats {
                    scope: ScopeId(r.get(0)?),
                    name: r.get(1)?,
                    active: n(2)?,
                    quarantine: n(3)?,
                    archived: n(4)?,
                    replaced: n(5)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        let pending_embeddings = match model {
            Some(model) => {
                let n: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM memories m WHERE m.state != 'Archived'
                     AND NOT EXISTS (SELECT 1 FROM vectors v WHERE v.memory = m.id AND v.model = ?1)",
                    [model],
                    |r| r.get(0),
                )?;
                usize::try_from(n).unwrap_or(0)
            }
            None => 0,
        };
        Ok(Stats {
            file_bytes,
            wal_bytes,
            scopes,
            operations: count("SELECT COUNT(*) FROM operations")?,
            decisions: count("SELECT COUNT(*) FROM decisions")?,
            vectors: count("SELECT COUNT(*) FROM vectors")?,
            pending_embeddings,
            queue: self.queue_stats(now)?,
        })
    }

    fn usage_counts(&self, ids: &[MemoryId]) -> Result<std::collections::HashMap<MemoryId, u32>> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare("SELECT uses FROM usage WHERE memory = ?1")?;
        let mut out = std::collections::HashMap::new();
        for id in ids {
            if let Some(uses) = stmt.query_row([id.0], |r| r.get::<_, u32>(0)).optional()? {
                out.insert(*id, uses);
            }
        }
        Ok(out)
    }

    fn count_ready(&self, kind: JobKind, now: i64) -> Result<usize> {
        let n: i64 = self.reader()?.query_row(
            "SELECT COUNT(*) FROM jobs WHERE state = 'Pending' AND run_after <= ?1 AND kind = ?2",
            params![now, enc(&kind)],
            |r| r.get(0),
        )?;
        Ok(usize::try_from(n).unwrap_or(0))
    }

    fn count_ready_heavy(&self, now: i64) -> Result<usize> {
        let n: i64 = self.reader()?.query_row(
            "SELECT COUNT(*) FROM jobs WHERE state = 'Pending' AND run_after <= ?1 AND kind IN ('Backup', 'Vacuum')",
            [now],
            |r| r.get(0),
        )?;
        Ok(usize::try_from(n).unwrap_or(0))
    }

    fn decisions(&self, scope: ScopeId, from: i64, to: i64) -> Result<Vec<DecisionRecord>> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            &format!("SELECT {DECISION_COLS} FROM decisions WHERE scope = ?1 AND at >= ?2 AND at < ?3 ORDER BY id"),
        )?;
        let rows = stmt.query_map(params![scope.0, from, to], decision_from)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn get(&self, id: MemoryId) -> Result<MemoryRow> {
        self.find_one(
            &format!("SELECT {MEM_COLS} FROM memories WHERE id = ?1"),
            &[&id.0],
        )?
        .ok_or(Error::NotFound)
    }

    fn list(
        &self,
        scope: ScopeId,
        state: Option<State>,
        kind: Option<MemoryType>,
    ) -> Result<Vec<MemoryRow>> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {MEM_COLS} FROM memories WHERE scope = ?1
             AND (?2 IS NULL OR state = ?2) AND (?3 IS NULL OR kind = ?3) ORDER BY id"
        ))?;
        let rows = stmt.query_map(
            params![scope.0, state.map(|s| enc(&s)), kind.map(|k| enc(&k))],
            memory_from,
        )?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn search(&self, scope: ScopeId, query: &str, opts: &SearchOpts) -> Result<Vec<Hit>> {
        let Some(q) = fts_query(&self.scope_language(scope)?, query) else {
            return Ok(Vec::new());
        };
        let cols = prefixed(MEM_COLS, "m");
        let conn = self.reader()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {cols}, -bm25(memories_fts) AS score
             FROM memories_fts JOIN memories m ON m.id = memories_fts.rowid
             WHERE memories_fts MATCH :q AND m.scope = :scope AND {}
             ORDER BY score DESC, m.id LIMIT :limit",
            visibility(opts)
        ))?;
        let limit = i64::try_from(opts.limit).unwrap_or(i64::MAX);
        let rows = stmt.query_map(
            named_params! {
                ":q": q,
                ":scope": scope.0,
                ":as_of": opts.as_of,
                ":known_at": opts.known_at,
                ":kind": opts.kind.map(|k| enc(&k)),
                ":limit": limit,
            },
            |r| {
                Ok(Hit {
                    memory: memory_from(r)?,
                    score: r.get(17)?,
                })
            },
        )?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn history(&self, id: MemoryId) -> Result<Vec<Operation>> {
        self.ops(&history_sql(), id.0)
    }

    fn operation(&self, id: OpId) -> Result<Operation> {
        self.ops(
            &format!("SELECT {OP_COLS} FROM operations WHERE id = ?1"),
            id.0,
        )?
        .pop()
        .ok_or(Error::NotFound)
    }

    fn operations_from(&self, from: OpId) -> Result<Vec<Operation>> {
        self.ops(
            &format!("SELECT {OP_COLS} FROM operations WHERE id >= ?1 ORDER BY id"),
            from.0,
        )
    }

    fn operations_by_actor(&self, actor: ActorId) -> Result<Vec<Operation>> {
        self.ops(&by_actor_sql(), actor.0)
    }

    fn pending_embeddings(&self, model: &str, limit: usize) -> Result<Vec<(MemoryId, String)>> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT m.id, m.content FROM memories m
             WHERE m.state != 'Archived'
               AND NOT EXISTS (SELECT 1 FROM vectors v WHERE v.memory = m.id AND v.model = ?1)
             ORDER BY m.id LIMIT ?2",
        )?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = stmt.query_map(params![model, limit], |r| {
            Ok((MemoryId(r.get(0)?), r.get(1)?))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn put_vectors(&self, model: &str, dim: usize, vectors: &[(MemoryId, Vec<f32>)]) -> Result<()> {
        let mut conn = self.writer()?;
        let tx = conn.transaction()?;
        let dim = i64::try_from(dim).unwrap_or(i64::MAX);
        for (id, v) in vectors {
            let changed = tx.execute(
                "INSERT OR REPLACE INTO vectors(memory, model, dim, vec)
                 SELECT ?1, ?2, ?3, ?4 WHERE EXISTS (SELECT 1 FROM memories WHERE id = ?1)",
                params![id.0, model, dim, to_blob(v)],
            )?;
            if changed == 0 {
                return Err(Error::NotFound);
            }
        }
        tx.commit()?;
        // After the commit: a search that sees the new count also sees the new vectors.
        self.vector_epoch.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn mark_unembeddable(&self, model: &str, memories: &[MemoryId]) -> Result<()> {
        let mut conn = self.writer()?;
        let tx = conn.transaction()?;
        for id in memories {
            tx.execute(
                "INSERT OR REPLACE INTO vectors(memory, model, dim, vec)
                 SELECT ?1, ?2, 0, x'' WHERE EXISTS (SELECT 1 FROM memories WHERE id = ?1)",
                params![id.0, model],
            )?;
        }
        tx.commit()?;
        self.vector_epoch.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn create_block(&self, scope: ScopeId, name: &str, budget_tokens: usize) -> Result<BlockId> {
        let conn = self.writer()?;
        let budget = i64::try_from(budget_tokens).unwrap_or(i64::MAX);
        conn.execute(
            "INSERT INTO blocks(scope, name, budget_tokens) VALUES (?1, ?2, ?3)",
            params![scope.0, name, budget],
        )?;
        Ok(BlockId(conn.last_insert_rowid()))
    }

    fn block(&self, id: BlockId) -> Result<Block> {
        self.reader()?
            .query_row(
                "SELECT id, scope, name, budget_tokens FROM blocks WHERE id = ?1",
                [id.0],
                block_from,
            )
            .optional()?
            .ok_or(Error::NotFound)
    }

    fn blocks(&self, scope: ScopeId) -> Result<Vec<Block>> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT id, scope, name, budget_tokens FROM blocks WHERE scope = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([scope.0], block_from)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn set_block_memories(&self, block: BlockId, memories: &[MemoryId]) -> Result<()> {
        let mut conn = self.writer()?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM block_memories WHERE block = ?1", [block.0])?;
        for (position, m) in memories.iter().enumerate() {
            let position = i64::try_from(position).unwrap_or(i64::MAX);
            tx.execute(
                "INSERT INTO block_memories(block, position, memory) VALUES (?1, ?2, ?3)",
                params![block.0, position, m.0],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn block_memories(&self, block: BlockId) -> Result<Vec<MemoryId>> {
        let conn = self.reader()?;
        let mut stmt =
            conn.prepare("SELECT memory FROM block_memories WHERE block = ?1 ORDER BY position")?;
        let rows = stmt.query_map([block.0], |r| Ok(MemoryId(r.get(0)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn rebuild_projection(&self) -> Result<()> {
        let mut conn = self.writer()?;
        fold::rebuild(&mut conn)
    }
}
