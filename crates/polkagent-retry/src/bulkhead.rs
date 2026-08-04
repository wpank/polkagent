use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tracing::{debug, warn};

use crate::error::BulkheadFull;

/// A concurrency limiter (bulkhead pattern) that restricts how many operations
/// can execute simultaneously.
///
/// Uses a Tokio semaphore under the hood. If no permit is available within
/// `max_wait_time`, the call is rejected with [`BulkheadFull`].
#[derive(Debug, Clone)]
pub struct Bulkhead {
    semaphore: Arc<Semaphore>,
    max_concurrent: usize,
    max_wait_time: Duration,
}

impl Bulkhead {
    /// Create a new bulkhead.
    ///
    /// - `max_concurrent`: maximum number of concurrent operations
    /// - `max_wait_time`: how long to wait for a permit before rejecting
    pub fn new(max_concurrent: usize, max_wait_time: Duration) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            max_concurrent,
            max_wait_time,
        }
    }

    /// Execute an async operation within the bulkhead.
    ///
    /// Waits up to `max_wait_time` for a permit. If a permit is acquired,
    /// executes `f` and returns its result. Otherwise returns `Err(BulkheadFull)`.
    pub async fn execute<F, Fut, T>(&self, f: F) -> Result<T, BulkheadFull>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        debug!(
            available = self.semaphore.available_permits(),
            max = self.max_concurrent,
            "bulkhead: attempting to acquire permit"
        );

        let permit = match tokio::time::timeout(self.max_wait_time, self.semaphore.acquire()).await
        {
            Ok(Ok(permit)) => permit,
            Ok(Err(_closed)) => {
                // Semaphore was closed, treat as full.
                warn!("bulkhead semaphore closed");
                return Err(BulkheadFull {
                    waited: self.max_wait_time,
                });
            }
            Err(_elapsed) => {
                warn!(
                    max_wait = ?self.max_wait_time,
                    "bulkhead: timed out waiting for permit"
                );
                return Err(BulkheadFull {
                    waited: self.max_wait_time,
                });
            }
        };

        debug!("bulkhead: permit acquired, executing operation");
        let result = f().await;
        drop(permit);
        debug!("bulkhead: permit released");
        Ok(result)
    }

    /// Return the number of currently available permits.
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }

    /// Return the maximum concurrent operations allowed.
    pub fn max_concurrent(&self) -> usize {
        self.max_concurrent
    }

    /// Return the maximum wait time.
    pub fn max_wait_time(&self) -> Duration {
        self.max_wait_time
    }
}
