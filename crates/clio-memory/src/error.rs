use std::path::PathBuf;

use crate::model::OpId;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("content is empty")]
    EmptyContent,
    #[error("text is longer than the configured limit")]
    TooLong,
    #[error("actor quota exceeded")]
    QuotaExceeded,
    #[error(
        "content rejected, nothing saved: it matches a known key format or has a 32+ character high-entropy run (a long id or URL path counts); shorten or remove that part"
    )]
    SecretDetected,
    #[error("invalid validity interval")]
    InvalidValidity,
    #[error("unknown scope")]
    UnknownScope,
    #[error("unknown actor")]
    UnknownActor,
    #[error("not found")]
    NotFound,
    #[error("memory is not in a state that allows this operation")]
    InvalidState,
    #[error("actor is not allowed to do this")]
    NotAllowed,
    #[error("operation {0:?} cannot be reverted: the memory changed since")]
    NotRevertible(OpId),
    #[error("another process owns the write lock on this file")]
    WriterLocked,
    #[error("this engine was opened read-only")]
    ReadOnly,
    #[error("{0} must not be accessible by group or others (expected mode 0600)")]
    InsecurePermissions(PathBuf),
    #[error(
        "opening a WAL database read-only needs a writable directory and writable -wal/-shm files next to it: {0}"
    )]
    UnwritableDirectory(PathBuf),
    #[error("database schema version {0} is newer than this build supports")]
    SchemaTooNew(i64),
    #[error("vector has the wrong size, a non-finite value or no direction")]
    BadVector,
    #[error("malformed job payload")]
    BadJobPayload,
    #[error("no embedder is configured")]
    NoEmbedder,
    #[error("embedder failed: {0}")]
    Embedding(String),
    #[error("import rejected: {0}")]
    BadImport(String),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
