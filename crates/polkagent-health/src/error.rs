//! Error types for health checking.

use thiserror::Error;

/// Errors that can occur during health checking.
#[derive(Debug, Error)]
pub enum HealthError {
    /// A health check timed out.
    #[error("health check `{name}` timed out after {timeout_ms}ms")]
    Timeout {
        /// Name of the check that timed out.
        name: String,
        /// Timeout duration in milliseconds.
        timeout_ms: u64,
    },

    /// A health check encountered a connection error.
    #[error("connection failed for `{name}`: {reason}")]
    ConnectionFailed {
        /// Name of the check.
        name: String,
        /// Reason for the failure.
        reason: String,
    },

    /// An I/O error occurred during a health check.
    #[error("I/O error for `{name}`: {source}")]
    Io {
        /// Name of the check.
        name: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// A check was not found in the registry.
    #[error("health check `{name}` not found")]
    CheckNotFound {
        /// Name of the missing check.
        name: String,
    },

    /// The reporter is already running.
    #[error("health reporter is already running")]
    AlreadyRunning,

    /// The reporter is not running.
    #[error("health reporter is not running")]
    NotRunning,

    /// A generic internal error.
    #[error("health check error: {0}")]
    Internal(String),
}
