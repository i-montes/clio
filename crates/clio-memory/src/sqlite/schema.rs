use rusqlite::Connection;

use crate::error::{Error, Result};

pub const VERSION: i64 = 1;

const DDL: &str = "
CREATE TABLE scopes(
    id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, language TEXT NOT NULL,
    retention_days INTEGER);
CREATE TABLE actors(
    id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, kind TEXT NOT NULL,
    quota_max INTEGER NOT NULL, quota_window INTEGER NOT NULL);
CREATE TABLE quota_counter(
    actor INTEGER NOT NULL REFERENCES actors(id), window_start INTEGER NOT NULL,
    count INTEGER NOT NULL, PRIMARY KEY(actor, window_start));
CREATE TABLE operations(
    id INTEGER PRIMARY KEY AUTOINCREMENT, scope INTEGER NOT NULL REFERENCES scopes(id),
    kind TEXT NOT NULL, memory INTEGER, changes TEXT NOT NULL,
    actor INTEGER NOT NULL REFERENCES actors(id), reason TEXT NOT NULL,
    at INTEGER NOT NULL, reverts INTEGER,
    -- The trace that caused the operation; null unless the telemetry feature is on.
    trace_id TEXT);
CREATE INDEX operations_actor ON operations(actor, id);
-- Which operations touched which memory (derived from the log, rebuilt with the projection).
CREATE TABLE operation_memories(
    memory INTEGER NOT NULL, op INTEGER NOT NULL REFERENCES operations(id),
    PRIMARY KEY(memory, op)) WITHOUT ROWID;
CREATE TABLE memories(
    id INTEGER PRIMARY KEY, scope INTEGER NOT NULL REFERENCES scopes(id), kind TEXT NOT NULL,
    content TEXT NOT NULL, content_hash TEXT NOT NULL, topic_key TEXT, origin TEXT NOT NULL,
    actor INTEGER NOT NULL REFERENCES actors(id), session TEXT, episode TEXT, source TEXT,
    state TEXT NOT NULL, valid_from INTEGER NOT NULL, valid_to INTEGER,
    known_since INTEGER NOT NULL, replaced_by INTEGER, supersedes INTEGER);
-- Derived data (embeddings), not part of the log: rebuilt by re-embedding, untouched by rebuild_projection.
CREATE TABLE vectors(
    memory INTEGER NOT NULL, model TEXT NOT NULL, dim INTEGER NOT NULL, vec BLOB NOT NULL,
    PRIMARY KEY(memory, model)) WITHOUT ROWID;
-- Fixed blocks are configuration, like scopes and actors: they are not in the log.
CREATE TABLE blocks(
    id INTEGER PRIMARY KEY, scope INTEGER NOT NULL REFERENCES scopes(id), name TEXT NOT NULL,
    budget_tokens INTEGER NOT NULL, UNIQUE(scope, name));
CREATE TABLE block_memories(
    block INTEGER NOT NULL REFERENCES blocks(id), position INTEGER NOT NULL, memory INTEGER NOT NULL,
    PRIMARY KEY(block, position));
-- Audit log of what the registered deciders said; written with the operation, untouched by rebuilds.
CREATE TABLE decisions(
    id INTEGER PRIMARY KEY AUTOINCREMENT, scope INTEGER NOT NULL REFERENCES scopes(id),
    op INTEGER REFERENCES operations(id), provider TEXT NOT NULL, mode TEXT NOT NULL,
    status TEXT NOT NULL, choice TEXT, confidence REAL, reason TEXT,
    acted INTEGER NOT NULL, at INTEGER NOT NULL);
CREATE INDEX decisions_scope_at ON decisions(scope, at);
-- Background work. Written with the operation that causes it; untouched by rebuilds.
CREATE TABLE jobs(
    id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, payload TEXT NOT NULL,
    priority INTEGER NOT NULL, state TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 5, run_after INTEGER NOT NULL, lease_until INTEGER,
    dedupe_key TEXT, error TEXT, created_at INTEGER NOT NULL, finished_at INTEGER);
-- How often and when each memory came out of a recall; filled in by Touch jobs.
CREATE TABLE usage(memory INTEGER PRIMARY KEY, uses INTEGER NOT NULL, last_used INTEGER NOT NULL);
-- What actors said about memories (useful / wrong); spent marks wrong signals already
-- consumed by a flag or a promote. Not part of the log or the projection.
CREATE TABLE feedback(
    id INTEGER PRIMARY KEY AUTOINCREMENT, memory INTEGER NOT NULL,
    actor INTEGER NOT NULL REFERENCES actors(id), signal TEXT NOT NULL, at INTEGER NOT NULL,
    spent INTEGER NOT NULL DEFAULT 0);
CREATE INDEX feedback_memory ON feedback(memory);
CREATE UNIQUE INDEX jobs_dedupe ON jobs(dedupe_key) WHERE state = 'Pending' AND dedupe_key IS NOT NULL;
CREATE INDEX jobs_ready ON jobs(state, run_after, priority);
CREATE INDEX memories_hash ON memories(scope, content_hash);
CREATE INDEX memories_topic ON memories(scope, kind, topic_key);
CREATE VIRTUAL TABLE memories_fts USING fts5(text, tokenize='unicode61 remove_diacritics 2');
";

/// Creates the schema on a fresh file; refuses files written by a newer version.
pub fn init(conn: &mut Connection) -> Result<()> {
    let found: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if found > VERSION {
        return Err(Error::SchemaTooNew(found));
    }
    if found == 0 {
        let tx = conn.transaction()?;
        tx.execute_batch(DDL)?;
        tx.pragma_update(None, "user_version", VERSION)?;
        tx.commit()?;
    }
    Ok(())
}
