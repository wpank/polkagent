//! Error types for the SQLite storage adapter.

use thiserror::Error;

/// All errors that can be produced by the SQLite store.
#[derive(Debug, Error)]
pub enum StoreError {
    /// A rusqlite database error.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// JSON serialisation or deserialisation failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// A record with the given identifier was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// An insert was rejected because a record with the same unique key already
    /// exists (e.g. duplicate event sequence, duplicate idempotency key).
    #[error("duplicate: {0}")]
    Duplicate(String),

    /// A migration could not be applied.
    #[error("migration failed: {0}")]
    Migration(String),

    /// An immutability constraint was violated (e.g. attempt to UPDATE an
    /// artifact or effect outcome).
    #[error("immutability violation: {0}")]
    Immutable(String),

    /// A UUID could not be parsed from its stored text form.
    #[error("invalid id: {0}")]
    InvalidId(#[from] uuid::Error),

    /// A timestamp stored in the database is not valid RFC 3339.
    #[error("invalid timestamp: {0}")]
    InvalidTimestamp(String),

    /// The pool writer lock was poisoned (should never happen with parking_lot).
    #[error("pool error: {0}")]
    Pool(String),
}

/// Convenience alias.
pub type StoreResult<T> = Result<T, StoreError>;

impl StoreError {
    /// Returns `true` if this error represents a UNIQUE constraint violation
    /// from SQLite (error code 2067 or 19 with extended code 2067).
    #[must_use]
    pub fn is_unique_violation(err: &rusqlite::Error) -> bool {
        matches!(
            err,
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: rusqlite::ffi::ErrorCode::ConstraintViolation,
                    ..
                },
                _,
            )
        )
    }
}
