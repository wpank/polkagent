use std::fmt;
use std::future::Future;
use std::time::Duration;

use tracing::{debug, warn};

use crate::error::RetryError;

/// A wrapper that adds a timeout to any async operation.
///
/// If the inner operation does not complete within the configured duration,
/// a `RetryError::Timeout` is returned.
#[derive(Debug, Clone)]
pub struct TimeoutWrapper {
    /// The maximum duration to wait for the operation.
    pub timeout: Duration,
}

impl TimeoutWrapper {
    /// Create a new timeout wrapper with the given duration.
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }

    /// Execute the given async function with the configured timeout.
    ///
    /// Returns the function's result if it completes in time, or
    /// `Err(RetryError::Timeout)` if the timeout elapses.
    pub async fn execute<F, Fut, T, E>(&self, f: F) -> Result<T, RetryError<E>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: fmt::Debug + fmt::Display,
    {
        debug!(timeout = ?self.timeout, "executing with timeout");

        match tokio::time::timeout(self.timeout, f()).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(e)) => {
                debug!(error = %e, "operation failed within timeout");
                Err(RetryError::Exhausted(crate::error::RetryExhausted {
                    last_error: e,
                    attempts: 1,
                    total_elapsed: self.timeout, // approximate
                }))
            }
            Err(_elapsed) => {
                warn!(timeout = ?self.timeout, "operation timed out");
                Err(RetryError::Timeout(self.timeout))
            }
        }
    }

    /// Execute a fallible async function with timeout, returning the
    /// inner `Result<T, E>` directly on success or `None` on timeout.
    pub async fn run<F, Fut, T, E>(&self, f: F) -> Result<T, TimeoutError<E>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        match tokio::time::timeout(self.timeout, f()).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(e)) => Err(TimeoutError::Inner(e)),
            Err(_elapsed) => Err(TimeoutError::Elapsed(self.timeout)),
        }
    }
}

/// Error type for timeout-wrapped operations.
#[derive(Debug)]
pub enum TimeoutError<E> {
    /// The inner operation returned an error.
    Inner(E),
    /// The operation exceeded the allowed duration.
    Elapsed(Duration),
}

impl<E: fmt::Display> fmt::Display for TimeoutError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inner(e) => write!(f, "{e}"),
            Self::Elapsed(d) => write!(f, "operation timed out after {d:?}"),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for TimeoutError<E> {}

impl<E> TimeoutError<E> {
    /// Returns `true` if this error is a timeout.
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Elapsed(_))
    }

    /// Returns the inner error if this is not a timeout.
    pub fn into_inner(self) -> Option<E> {
        match self {
            Self::Inner(e) => Some(e),
            Self::Elapsed(_) => None,
        }
    }
}
