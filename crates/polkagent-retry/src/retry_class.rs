//! Retry classification for provider errors.
//!
//! [`RetryClass`] maps a [`ProviderError`] into one of a small set of
//! actionable categories that the retry/fallback machinery can act on.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use polkagent_executor_trait::ProviderError;

/// How the retry/fallback layer should handle a given error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetryClass {
    /// The error is transient and the operation can be retried immediately.
    RetryImmediate,
    /// The error is transient but the caller should wait before retrying
    /// (e.g. `Retry-After` header from a 429).
    RetryAfter(#[serde(with = "duration_millis")] Duration),
    /// The error is transient but retrying the same provider is unlikely
    /// to help; the caller should fall back to an alternate provider.
    Fallback,
    /// The error is permanent for this request; neither retry nor fallback
    /// will help.
    NoRetry,
    /// The error cannot be classified. Callers should treat this the same
    /// as `NoRetry` unless they have additional context.
    Unknown,
}

impl RetryClass {
    /// Returns `true` if the class indicates the operation can be retried
    /// (either immediately or after a delay).
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::RetryImmediate | Self::RetryAfter(_))
    }

    /// Returns `true` if the class indicates a fallback provider should be tried.
    #[must_use]
    pub fn is_fallback(&self) -> bool {
        matches!(self, Self::Fallback)
    }

    /// Returns the delay before retrying, if any.
    #[must_use]
    pub fn retry_delay(&self) -> Option<Duration> {
        match self {
            Self::RetryAfter(d) => Some(*d),
            _ => None,
        }
    }
}

/// Classify a [`ProviderError`] into a [`RetryClass`].
///
/// This is the single source of truth for mapping provider errors to retry
/// decisions. The logic mirrors `ProviderError::is_retryable` but adds the
/// richer `RetryAfter` and `Fallback` distinctions.
pub fn classify_provider_error(error: &ProviderError) -> RetryClass {
    match error {
        ProviderError::RateLimit {
            retry_after_secs: Some(secs),
            ..
        } => RetryClass::RetryAfter(Duration::from_secs(*secs)),

        ProviderError::RateLimit {
            retry_after_secs: None,
            ..
        } => RetryClass::RetryImmediate,

        ProviderError::Timeout { .. } => RetryClass::RetryImmediate,

        ProviderError::ServerError { status, .. } => match status {
            // 503 often indicates the provider is down for a while;
            // fallback is more appropriate than retry.
            503 => RetryClass::Fallback,
            // Other server errors (500, 502, 504) are usually transient.
            _ => RetryClass::RetryImmediate,
        },

        ProviderError::AuthFailure { .. } => RetryClass::NoRetry,
        ProviderError::ContentPolicy { .. } => RetryClass::NoRetry,
        ProviderError::ContextOverflow { .. } => RetryClass::Fallback,
        ProviderError::ModelNotFound { .. } => RetryClass::Fallback,
    }
}

/// Serde helper for Duration as milliseconds.
mod duration_millis {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        value.as_millis().serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        let ms = u64::deserialize(deserializer)?;
        Ok(Duration::from_millis(ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_with_retry_after_maps_to_retry_after() {
        let err = ProviderError::RateLimit {
            retry_after_secs: Some(30),
            message: "slow down".into(),
        };
        let class = classify_provider_error(&err);
        assert_eq!(class, RetryClass::RetryAfter(Duration::from_secs(30)));
        assert!(class.is_retryable());
        assert!(!class.is_fallback());
        assert_eq!(class.retry_delay(), Some(Duration::from_secs(30)));
    }

    #[test]
    fn rate_limit_without_retry_after_maps_to_retry_immediate() {
        let err = ProviderError::RateLimit {
            retry_after_secs: None,
            message: "slow down".into(),
        };
        let class = classify_provider_error(&err);
        assert_eq!(class, RetryClass::RetryImmediate);
        assert!(class.is_retryable());
        assert_eq!(class.retry_delay(), None);
    }

    #[test]
    fn timeout_maps_to_retry_immediate() {
        let err = ProviderError::Timeout {
            message: "timed out".into(),
        };
        let class = classify_provider_error(&err);
        assert_eq!(class, RetryClass::RetryImmediate);
        assert!(class.is_retryable());
    }

    #[test]
    fn server_error_500_maps_to_retry_immediate() {
        let err = ProviderError::ServerError {
            status: 500,
            message: "internal".into(),
        };
        assert_eq!(
            classify_provider_error(&err),
            RetryClass::RetryImmediate
        );
    }

    #[test]
    fn server_error_502_maps_to_retry_immediate() {
        let err = ProviderError::ServerError {
            status: 502,
            message: "bad gateway".into(),
        };
        assert_eq!(
            classify_provider_error(&err),
            RetryClass::RetryImmediate
        );
    }

    #[test]
    fn server_error_503_maps_to_fallback() {
        let err = ProviderError::ServerError {
            status: 503,
            message: "unavailable".into(),
        };
        let class = classify_provider_error(&err);
        assert_eq!(class, RetryClass::Fallback);
        assert!(!class.is_retryable());
        assert!(class.is_fallback());
    }

    #[test]
    fn server_error_504_maps_to_retry_immediate() {
        let err = ProviderError::ServerError {
            status: 504,
            message: "gateway timeout".into(),
        };
        assert_eq!(
            classify_provider_error(&err),
            RetryClass::RetryImmediate
        );
    }

    #[test]
    fn auth_failure_maps_to_no_retry() {
        let err = ProviderError::AuthFailure {
            status: 401,
            message: "bad key".into(),
        };
        let class = classify_provider_error(&err);
        assert_eq!(class, RetryClass::NoRetry);
        assert!(!class.is_retryable());
        assert!(!class.is_fallback());
    }

    #[test]
    fn content_policy_maps_to_no_retry() {
        let err = ProviderError::ContentPolicy {
            message: "blocked".into(),
        };
        assert_eq!(classify_provider_error(&err), RetryClass::NoRetry);
    }

    #[test]
    fn context_overflow_maps_to_fallback() {
        let err = ProviderError::ContextOverflow {
            message: "too many tokens".into(),
        };
        let class = classify_provider_error(&err);
        assert_eq!(class, RetryClass::Fallback);
        assert!(class.is_fallback());
    }

    #[test]
    fn model_not_found_maps_to_fallback() {
        let err = ProviderError::ModelNotFound {
            model: "gpt-99".into(),
            message: "not found".into(),
        };
        let class = classify_provider_error(&err);
        assert_eq!(class, RetryClass::Fallback);
        assert!(class.is_fallback());
    }

    #[test]
    fn retry_class_unknown_is_not_retryable() {
        let class = RetryClass::Unknown;
        assert!(!class.is_retryable());
        assert!(!class.is_fallback());
        assert_eq!(class.retry_delay(), None);
    }

    #[test]
    fn retry_class_serde_roundtrip_immediate() {
        let class = RetryClass::RetryImmediate;
        let json = serde_json::to_string(&class).expect("serialize");
        let back: RetryClass = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, class);
    }

    #[test]
    fn retry_class_serde_roundtrip_retry_after() {
        let class = RetryClass::RetryAfter(Duration::from_secs(42));
        let json = serde_json::to_string(&class).expect("serialize");
        let back: RetryClass = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, class);
    }

    #[test]
    fn retry_class_serde_roundtrip_all_variants() {
        let variants = vec![
            RetryClass::RetryImmediate,
            RetryClass::RetryAfter(Duration::from_millis(500)),
            RetryClass::Fallback,
            RetryClass::NoRetry,
            RetryClass::Unknown,
        ];
        for v in variants {
            let json = serde_json::to_string(&v).expect("serialize");
            let back: RetryClass = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, v, "roundtrip failed for {v:?}");
        }
    }
}
