use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::backoff::BackoffStrategy;
use crate::classifier::{AlwaysRetry, ErrorClassifier};

/// Configuration governing how retries are performed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts (does not count the initial attempt).
    pub max_retries: u32,
    /// Backoff strategy for computing inter-retry delays.
    pub backoff: BackoffStrategy,
    /// Optional per-attempt timeout. If `None`, individual attempts are not timed out.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_duration_millis"
    )]
    pub per_attempt_timeout: Option<Duration>,
}

impl RetryPolicy {
    /// Create a policy with fixed backoff.
    pub fn fixed(max_retries: u32, delay: Duration) -> Self {
        Self {
            max_retries,
            backoff: BackoffStrategy::Fixed(delay),
            per_attempt_timeout: None,
        }
    }

    /// Create a policy with exponential backoff (no jitter by default).
    pub fn exponential(max_retries: u32, base: Duration) -> Self {
        Self {
            max_retries,
            backoff: BackoffStrategy::Exponential {
                base,
                max: base.saturating_mul(64),
                jitter: false,
            },
            per_attempt_timeout: None,
        }
    }

    /// Create a policy with exponential backoff and jitter.
    pub fn exponential_with_jitter(max_retries: u32, base: Duration, max: Duration) -> Self {
        Self {
            max_retries,
            backoff: BackoffStrategy::Exponential {
                base,
                max,
                jitter: true,
            },
            per_attempt_timeout: None,
        }
    }

    /// Create a policy with linear backoff.
    pub fn linear(max_retries: u32, step: Duration, max: Duration) -> Self {
        Self {
            max_retries,
            backoff: BackoffStrategy::Linear { step, max },
            per_attempt_timeout: None,
        }
    }

    /// Set an optional per-attempt timeout.
    #[must_use]
    pub fn with_per_attempt_timeout(mut self, timeout: Duration) -> Self {
        self.per_attempt_timeout = Some(timeout);
        self
    }

    /// Create the default error classifier (retries all errors).
    pub fn default_classifier<E>() -> AlwaysRetry {
        AlwaysRetry
    }

    /// Check whether a given error should be retried using the provided classifier.
    pub fn should_retry<E>(
        &self,
        error: &E,
        attempt: u32,
        classifier: &dyn ErrorClassifier<E>,
    ) -> bool {
        if attempt >= self.max_retries {
            return false;
        }
        matches!(
            classifier.classify(error),
            crate::classifier::ErrorClass::Retryable
        )
    }
}

/// Serde helper for `Option<Duration>` encoded as milliseconds.
mod optional_duration_millis {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    // Serde's `with` callback ABI passes the field as `&Option<T>`.
    #[allow(clippy::ref_option)]
    pub fn serialize<S: Serializer>(
        value: &Option<Duration>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(d) => u64::try_from(d.as_millis())
                .map_err(serde::ser::Error::custom)?
                .serialize(serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Duration>, D::Error> {
        let opt: Option<u64> = Option::deserialize(deserializer)?;
        Ok(opt.map(Duration::from_millis))
    }
}
