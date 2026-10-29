//! The JSON Lines export format (`clio-memory-export`). One object per line, each with a `type`.
//! The same records are read back by `Engine::import`.

use std::io::BufRead;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

use crate::model::{
    ActorId, ActorKind, Block, BlockId, DecisionRecord, ExportSummary, MemoryId, MemoryType,
    Operation, ScopeId, Signal,
};

/// Semver of the file format. A reader accepts the same major version and not a newer minor.
pub const EXPORT_FORMAT_VERSION: &str = "1.0.0";
pub(crate) const EXPORT_FORMAT: &str = "clio-memory-export";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ExportRecord {
    Header {
        format: String,
        format_version: String,
        schema_version: i64,
        exported_at: i64,
    },
    Scope {
        id: ScopeId,
        name: String,
        language: String,
        retention_days: Option<u32>,
    },
    /// Quota limits are configuration and travel with the actor; the counters do not.
    Actor {
        id: ActorId,
        name: String,
        kind: ActorKind,
        quota_max: u32,
        quota_window: i64,
    },
    Operation(Operation),
    Decision(DecisionRecord),
    Block(Block),
    BlockMemory {
        block: BlockId,
        position: i64,
        memory: MemoryId,
    },
    Usage {
        memory: MemoryId,
        uses: u32,
        last_used: i64,
    },
    Feedback {
        id: i64,
        memory: MemoryId,
        actor: ActorId,
        signal: Signal,
        at: i64,
        spent: bool,
    },
}

impl ExportRecord {
    pub(crate) fn tally(&self, summary: &mut ExportSummary) {
        match self {
            Self::Header { .. } | Self::BlockMemory { .. } => {}
            Self::Scope { .. } => summary.scopes += 1,
            Self::Actor { .. } => summary.actors += 1,
            Self::Operation(_) => summary.operations += 1,
            Self::Decision(_) => summary.decisions += 1,
            Self::Block(_) => summary.blocks += 1,
            Self::Usage { .. } => summary.usage += 1,
            Self::Feedback { .. } => summary.feedback += 1,
        }
    }
}

/// The longest line `import` reads; a line is one operation snapshot at most.
pub(crate) const MAX_LINE_BYTES: usize = 8 << 20;

pub(crate) enum Line {
    Eof,
    Read,
    /// Longer than `MAX_LINE_BYTES`; the rest of the line has been skipped and `buf` is empty.
    TooLong,
}

/// Reads one line into `buf` (cleared first, newline kept) without ever holding more than
/// `MAX_LINE_BYTES`, so a hostile file cannot make the reader allocate without bound. An
/// oversized line is skipped to its end and reported, so the caller can go on with the next.
pub(crate) fn read_line_bounded(input: &mut dyn BufRead, buf: &mut Vec<u8>) -> Result<Line> {
    buf.clear();
    loop {
        let chunk = input.fill_buf()?;
        if chunk.is_empty() {
            return Ok(if buf.is_empty() {
                Line::Eof
            } else {
                Line::Read
            });
        }
        let (take, done) = match chunk.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (chunk.len(), false),
        };
        if buf.len() + take > MAX_LINE_BYTES {
            buf.clear();
            input.consume(take);
            return skip_rest_of_line(input, done).map(|()| Line::TooLong);
        }
        buf.extend_from_slice(&chunk[..take]);
        input.consume(take);
        if done {
            return Ok(Line::Read);
        }
    }
}

fn skip_rest_of_line(input: &mut dyn BufRead, mut done: bool) -> Result<()> {
    while !done {
        let chunk = input.fill_buf()?;
        if chunk.is_empty() {
            break;
        }
        let (take, found) = match chunk.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (chunk.len(), false),
        };
        input.consume(take);
        done = found;
    }
    Ok(())
}

/// The kind names `import_memories` accepts, in any letter case.
pub(crate) fn parse_kind(name: &str) -> Option<MemoryType> {
    Some(match name.to_ascii_lowercase().as_str() {
        "working" => MemoryType::Working,
        "episodic" => MemoryType::Episodic,
        "semantic" => MemoryType::Semantic,
        "procedural" => MemoryType::Procedural,
        _ => return None,
    })
}

/// One line of a loose-memories file. Unknown fields are ignored: the files come from elsewhere.
#[derive(Debug, Deserialize)]
pub(crate) struct LooseMemory {
    pub content: String,
    pub kind: Option<String>,
    pub topic_key: Option<String>,
    pub valid_from: Option<i64>,
    pub valid_to: Option<i64>,
    pub source: Option<String>,
}

/// Accepts the same major version and not a newer minor (the patch never matters), and only the
/// schema this build writes: restoring across schemas would need a migration that does not exist.
pub(crate) fn check_header(
    format: &str,
    format_version: &str,
    schema_version: i64,
    expected_schema: i64,
) -> Result<()> {
    let bad = |m: String| Err(Error::BadImport(m));
    if format != EXPORT_FORMAT {
        return bad(format!("not a {EXPORT_FORMAT} file"));
    }
    let parse = |v: &str| -> Option<(u32, u32)> {
        let mut it = v.split('.');
        let (major, minor) = (it.next()?.parse().ok()?, it.next()?.parse().ok()?);
        it.next()?.parse::<u32>().ok()?;
        it.next().is_none().then_some((major, minor))
    };
    let (Some((major, minor)), Some((my_major, my_minor))) =
        (parse(format_version), parse(EXPORT_FORMAT_VERSION))
    else {
        return bad(format!("unreadable format version {format_version:?}"));
    };
    if major != my_major {
        return bad(format!(
            "format version {format_version} is not readable by this build ({EXPORT_FORMAT_VERSION})"
        ));
    }
    if minor > my_minor {
        return bad(format!(
            "format version {format_version} is newer than this build ({EXPORT_FORMAT_VERSION})"
        ));
    }
    if schema_version != expected_schema {
        return bad(format!(
            "schema version {schema_version} differs from this build's {expected_schema}"
        ));
    }
    Ok(())
}
