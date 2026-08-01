//! Batch configuration types.
//!
//! [`BatchConfig`] controls how a batch is processed: concurrency level,
//! maximum batch size, timer-based flushing, and error handling policy.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Policy that governs how errors in individual items are handled during
/// batch processing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorPolicy {
    /// Stop processing immediately when the first item fails. Items that have
    /// not yet started are left in `Pending` state.
    FailFast,
    /// Continue processing remaining items even when some fail. Failed items
    /// are recorded but do not block the batch.
    #[default]
    SkipFailed,
    /// Retry failed items up to [`BatchConfig::max_retries`] times before
    /// recording them as permanently failed.
    RetryFailed,
}

/// Configuration for batch processing behaviour.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchConfig {
    /// Maximum number of items a batch can hold. Once reached, the collector
    /// will flush the batch. A value of `0` means unlimited.
    pub max_size: usize,

    /// Maximum time to wait before flushing an incomplete batch. The collector
    /// will flush even if the batch has not reached `max_size`.
    #[serde(with = "duration_millis")]
    pub max_wait: Duration,

    /// Number of items to process concurrently. A value of `1` means
    /// sequential processing; values greater than `1` enable parallel
    /// processing with a semaphore limiting concurrent handlers.
    pub concurrency: usize,

    /// Error handling policy for the batch.
    pub error_policy: ErrorPolicy,

    /// Maximum number of retries per item when using [`ErrorPolicy::RetryFailed`].
    /// Ignored for other policies.
    pub max_retries: u32,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_size: 100,
            max_wait: Duration::from_secs(5),
            concurrency: 1,
            error_policy: ErrorPolicy::default(),
            max_retries: 3,
        }
    }
}

impl BatchConfig {
    /// Create a config for sequential processing.
    #[must_use]
    pub fn sequential() -> Self {
        Self {
            concurrency: 1,
            ..Self::default()
        }
    }

    /// Create a config for parallel processing with the given concurrency.
    #[must_use]
    pub fn parallel(concurrency: usize) -> Self {
        Self {
            concurrency: concurrency.max(1),
            ..Self::default()
        }
    }

    /// Set the maximum batch size.
    #[must_use]
    pub fn with_max_size(mut self, max_size: usize) -> Self {
        self.max_size = max_size;
        self
    }

    /// Set the maximum wait duration.
    #[must_use]
    pub fn with_max_wait(mut self, max_wait: Duration) -> Self {
        self.max_wait = max_wait;
        self
    }

    /// Set the error policy.
    #[must_use]
    pub fn with_error_policy(mut self, policy: ErrorPolicy) -> Self {
        self.error_policy = policy;
        self
    }

    /// Set the maximum retries (relevant for `RetryFailed` policy).
    #[must_use]
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }
}

/// Serde helper to serialize/deserialize `Duration` as milliseconds.
mod duration_millis {
    use serde::{self, Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let millis = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        serializer.serialize_u64(millis)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let millis = u64::deserialize(deserializer)?;
        Ok(Duration::from_millis(millis))
    }
}
