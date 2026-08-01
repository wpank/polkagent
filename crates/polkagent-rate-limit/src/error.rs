//! Error types for the rate-limiting crate.

use std::fmt;

use thiserror::Error;

/// Errors produced by rate-limiting operations.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RateLimitError {
    /// The caller has exceeded its rate limit.
    #[error("rate limit exceeded: retry after {retry_after_ms}ms")]
    Exceeded {
        /// How many milliseconds the caller should wait before retrying.
        retry_after_ms: u64,
    },

    /// The caller has exhausted its quota for the current period.
    #[error("quota exhausted: resets at {reset_at}")]
    QuotaExhausted {
        /// ISO-8601 timestamp when the quota resets.
        reset_at: String,
    },

    /// An invalid configuration was provided.
    #[error("invalid rate limit configuration: {reason}")]
    InvalidConfig {
        /// Description of the configuration error.
        reason: String,
    },
}

/// A convenient `Result` alias for rate-limiting operations.
pub type Result<T> = std::result::Result<T, RateLimitError>;

/// Information about a rate-limit rejection, suitable for HTTP headers.
#[derive(Debug, Clone)]
pub struct RejectionInfo {
    /// The rate-limit key that was checked.
    pub key: String,
    /// Milliseconds until the caller should retry.
    pub retry_after_ms: u64,
    /// Remaining quota (always 0 on rejection).
    pub remaining: u32,
    /// ISO-8601 reset timestamp, if known.
    pub reset_at: Option<String>,
}

impl fmt::Display for RejectionInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "rate limited: key={}, retry_after={}ms",
            self.key, self.retry_after_ms
        )
    }
}
