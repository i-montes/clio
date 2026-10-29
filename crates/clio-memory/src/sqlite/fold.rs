use std::collections::HashMap;

use rusqlite::{Connection, params};

use super::{OP_COLS, enc, op_from};
use crate::error::Result;
use crate::model::{Change, MemoryId, MemoryRow, OpId, ScopeId};
use crate::stem::stem_text;

fn language(conn: &Connection, scope: ScopeId) -> Result<String> {
    Ok(conn.query_row(
        "SELECT language FROM scopes WHERE id = ?1",
        [scope.0],
        |r| r.get(0),
    )?)
}

/// The projection is a fold: every change upserts its `after` row and refreshes its index entry.
pub fn apply_change(conn: &Connection, after: &MemoryRow) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO memories(id, scope, kind, content, content_hash, topic_key, origin,
            actor, session, episode, source, state, valid_from, valid_to, known_since, replaced_by, supersedes)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
        params![
            after.id.0,
            after.scope.0,
            enc(&after.kind),
            after.content,
            after.content_hash,
            after.topic_key,
            enc(&after.origin),
            after.actor.0,
            after.session,
            after.episode,
            after.source,
            enc(&after.state),
            after.valid_from,
            after.valid_to,
            after.known_since,
            after.replaced_by.map(|m| m.0),
            after.supersedes.map(|m| m.0),
        ],
    )?;
    let text = stem_text(&language(conn, after.scope)?, &after.content);
    conn.execute("DELETE FROM memories_fts WHERE rowid = ?1", [after.id.0])?;
    conn.execute(
        "INSERT INTO memories_fts(rowid, text) VALUES (?1, ?2)",
        params![after.id.0, text],
    )?;
    Ok(())
}

/// Records which memories an operation touched, so `history` is an index lookup.
pub fn link(
    conn: &Connection,
    op: OpId,
    memory: Option<MemoryId>,
    changes: &[Change],
) -> Result<()> {
    for m in memory.into_iter().chain(changes.iter().map(|c| c.memory)) {
        conn.execute(
            "INSERT OR IGNORE INTO operation_memories(memory, op) VALUES (?1, ?2)",
            [m.0, op.0],
        )?;
    }
    Ok(())
}

/// What `rebuild` would produce for `memories`, computed in memory (the last change to each
/// memory wins). Writes nothing; holds one row per memory, not the log.
pub fn preview(conn: &Connection) -> Result<HashMap<MemoryId, MemoryRow>> {
    let mut out = HashMap::new();
    let mut stmt = conn.prepare(&format!("SELECT {OP_COLS} FROM operations ORDER BY id"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        for change in op_from(row)?.changes {
            out.insert(change.memory, change.after);
        }
    }
    Ok(out)
}

/// Refolds the whole log, one operation at a time (the log is never held in memory).
pub fn rebuild(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    rebuild_in(&tx)?;
    tx.commit()?;
    Ok(())
}

/// The refold itself, inside a transaction the caller owns (restoring an export uses it too).
pub fn rebuild_in(tx: &Connection) -> Result<()> {
    tx.execute_batch(
        "DELETE FROM memories; DELETE FROM memories_fts; DELETE FROM operation_memories;",
    )?;
    {
        let mut stmt = tx.prepare(&format!("SELECT {OP_COLS} FROM operations ORDER BY id"))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let op = op_from(row)?;
            link(tx, op.id, op.memory, &op.changes)?;
            for change in &op.changes {
                apply_change(tx, &change.after)?;
            }
        }
    }
    Ok(())
}
