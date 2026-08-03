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
    /// Returns `true` if this error represents a UNIQUE or PRIMARY KEY
    /// constraint violation from SQLite.
    ///
    /// This checks the `extended_code` to distinguish uniqueness violations
    /// from other constraint errors such as FOREIGN KEY violations
    /// (extended code 787 = SQLITE_CONSTRAINT_FOREIGNKEY).
    ///
    /// Matched extended codes:
    /// - 2067 = `SQLITE_CONSTRAINT_UNIQUE`
    /// - 1555 = `SQLITE_CONSTRAINT_PRIMARYKEY`
    #[must_use]
    pub fn is_unique_violation(err: &rusqlite::Error) -> bool {
        matches!(
            err,
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: rusqlite::ffi::ErrorCode::ConstraintViolation,
                    extended_code: 2067 | 1555,
                },
                _,
            )
        )
    }

    /// Returns `true` if this error represents a FOREIGN KEY constraint
    /// violation from SQLite (extended code 787 = SQLITE_CONSTRAINT_FOREIGNKEY).
    #[must_use]
    pub fn is_fk_violation(err: &rusqlite::Error) -> bool {
        matches!(
            err,
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: rusqlite::ffi::ErrorCode::ConstraintViolation,
                    extended_code: 787, // SQLITE_CONSTRAINT_FOREIGNKEY
                },
                _,
            )
        )
    }
}
