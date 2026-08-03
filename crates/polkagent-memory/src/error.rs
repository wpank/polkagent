//! Error types for the memory subsystem.

use thiserror::Error;

/// All errors that can be produced by the memory subsystem.
#[derive(Debug, Error)]
pub enum MemoryError {
    /// A rusqlite database error.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// JSON serialisation or deserialisation failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// A record with the given identifier was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// A UUID could not be parsed from its stored text form.
    #[error("invalid id: {0}")]
    InvalidId(#[from] uuid::Error),

    /// A timestamp stored in the database is not valid RFC 3339.
    #[error("invalid timestamp: {0}")]
    InvalidTimestamp(String),

    /// Schema migration failed.
    #[error("migration error: {0}")]
    Migration(String),

    /// An operation was logically invalid (e.g. ending an already-ended episode).
    #[error("invalid operation: {0}")]
    InvalidOperation(String),

    /// An I/O operation failed (e.g. writing or reading episode log files).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Convenience alias.
pub type MemoryResult<T> = Result<T, MemoryError>;
