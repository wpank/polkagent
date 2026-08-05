//! Error types for the migration management tool.

use std::path::PathBuf;

use thiserror::Error;

/// All errors that can be produced by the migration manager.
#[derive(Debug, Error)]
pub enum MigrationError {
    /// A rusqlite database error.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// JSON serialisation or deserialisation failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// A migration SQL could not be executed.
    #[error("migration v{version} ({name}) failed: {reason}")]
    Apply {
        version: u32,
        name: String,
        reason: String,
    },

    /// A checksum mismatch was detected: an applied migration's SQL has changed
    /// since it was originally run.
    #[error(
        "checksum mismatch for migration v{version} ({name}): expected {expected}, got {actual}"
    )]
    ChecksumMismatch {
        version: u32,
        name: String,
        expected: String,
        actual: String,
    },

    /// Rollback was requested but no down SQL is available.
    #[error("rollback not available for migration v{version} ({name}): no down.sql provided")]
    RollbackUnavailable { version: u32, name: String },

    /// A migration file could not be read or written.
    #[error("I/O error at {}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    /// A lock file already exists, indicating another migration process is
    /// running (or was interrupted).
    #[error("migration lock file exists at {}: another process may be running", path.display())]
    Locked { path: PathBuf },

    /// No migrations have been applied, so rollback is a no-op.
    #[error("no migrations applied; nothing to roll back")]
    NothingToRollback,

    /// Version ordering violation: migrations are not monotonically increasing.
    #[error("version ordering violation: version {version} appears after {previous}")]
    VersionOrdering { version: u32, previous: u32 },

    /// A migration definition has an invalid version number (e.g. zero).
    #[error("invalid migration version: {0}")]
    InvalidVersion(u32),

    /// The `schema_migrations` table is missing or corrupt.
    #[error("schema_migrations table error: {0}")]
    SchemaTable(String),
}

/// Convenience alias.
pub type MigrationResult<T> = Result<T, MigrationError>;
