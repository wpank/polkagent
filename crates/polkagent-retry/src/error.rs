use std::fmt;
use std::time::Duration;

/// Error returned when all retry attempts have been exhausted.
#[derive(Debug)]
pub struct RetryExhausted<E> {
    /// The last error encountered before giving up.
    pub last_error: E,
    /// Total number of attempts made (initial + retries).
    pub attempts: u32,
    /// Total elapsed time across all attempts.
    pub total_elapsed: Duration,
}

impl<E: fmt::Display> fmt::Display for RetryExhausted<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "retry exhausted after {} attempts ({:?}): {}",
            self.attempts, self.total_elapsed, self.last_error
        )
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for RetryExhausted<E> {}

/// Error returned when a circuit breaker is open and rejecting requests.
#[derive(Debug, Clone, thiserror::Error)]
#[error("circuit breaker is open, remaining duration: {remaining:?}")]
pub struct CircuitOpen {
    /// How long until the circuit breaker transitions to half-open.
    pub remaining: Duration,
}

/// Error returned when a bulkhead has no available permits.
#[derive(Debug, Clone, thiserror::Error)]
#[error("bulkhead full: max concurrency reached, waited {waited:?}")]
pub struct BulkheadFull {
    /// How long we waited trying to acquire a permit.
    pub waited: Duration,
}

/// Unified error type for the retry crate.
#[derive(Debug, thiserror::Error)]
pub enum RetryError<E: fmt::Debug + fmt::Display> {
    /// All retries were exhausted.
    #[error(transparent)]
    Exhausted(RetryExhausted<E>),

    /// The circuit breaker rejected the call.
    #[error(transparent)]
    CircuitOpen(CircuitOpen),

    /// The bulkhead rejected the call due to concurrency limits.
    #[error(transparent)]
    BulkheadFull(BulkheadFull),

    /// The operation timed out.
    #[error("operation timed out after {0:?}")]
    Timeout(Duration),
}
