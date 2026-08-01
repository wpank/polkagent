//! Error types for the audit subsystem.

use thiserror::Error;

/// Errors that can occur during audit operations.
#[derive(Debug, Error)]
pub enum AuditError {
    /// Serialization or deserialization failed.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// An integrity check failed — the hash chain has been tampered with.
    #[error("integrity violation: {message}")]
    IntegrityViolation {
        /// Human-readable description of the violation.
        message: String,
    },

    /// The requested audit entry was not found.
    #[error("audit entry not found: {id}")]
    NotFound {
        /// The ID that was looked up.
        id: String,
    },

    /// A query was malformed or contained invalid parameters.
    #[error("invalid query: {reason}")]
    InvalidQuery {
        /// Description of what was wrong.
        reason: String,
    },

    /// An I/O or storage-level error occurred.
    #[error("storage error: {0}")]
    Storage(String),

    /// A lock could not be acquired (should not happen with `parking_lot`
    /// unless there is a logic error).
    #[error("lock poisoned: {0}")]
    LockPoisoned(String),
}

/// Convenience alias for audit results.
pub type AuditResult<T> = Result<T, AuditError>;
