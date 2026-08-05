//! Typed errors shared by interaction services and surface adapters.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Stable machine-readable interaction error categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionErrorCode {
    /// Input or command arguments are invalid.
    InvalidRequest,
    /// A target, interaction, turn, run, or approval does not exist.
    NotFound,
    /// Current durable state conflicts with the requested operation.
    Conflict,
    /// The interaction already has incompatible active work.
    Busy,
    /// Policy or caller authority denied the operation.
    PermissionDenied,
    /// Interaction or runtime configuration is invalid.
    InvalidConfig,
    /// The configured runtime cannot provide the requested capability.
    Unsupported,
    /// A transient provider, executor, store, or transport failure occurred.
    Unavailable,
    /// An internal invariant or backend operation failed.
    Internal,
}

impl std::fmt::Display for InteractionErrorCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::InvalidRequest => "invalid_request",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Busy => "busy",
            Self::PermissionDenied => "permission_denied",
            Self::InvalidConfig => "invalid_config",
            Self::Unsupported => "unsupported",
            Self::Unavailable => "unavailable",
            Self::Internal => "internal",
        };
        formatter.write_str(value)
    }
}

/// A surface-neutral interaction failure.
///
/// `details` must contain only redacted, presentation-safe values. Raw
/// prompts, credentials, and provider payloads do not belong in this type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Error)]
#[error("{code}: {message}")]
pub struct InteractionError {
    /// Stable error category.
    pub code: InteractionErrorCode,
    /// Safe human-readable explanation.
    pub message: String,
    /// Whether retrying without changing the request may succeed.
    pub retryable: bool,
    /// Optional structured, redacted context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl InteractionError {
    /// Construct a non-retryable error without structured details.
    pub fn new(code: InteractionErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: false,
            details: None,
        }
    }

    /// Mark this error as safe to retry.
    #[must_use]
    pub const fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }

    /// Attach redacted structured context.
    #[must_use]
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    /// Construct an invalid-request error.
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(InteractionErrorCode::InvalidRequest, message)
    }

    /// Construct an invalid-configuration error.
    pub fn invalid_config(message: impl Into<String>) -> Self {
        Self::new(InteractionErrorCode::InvalidConfig, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_stable_and_safe() {
        let error = InteractionError::invalid_request("prompt is empty");
        assert_eq!(error.to_string(), "invalid_request: prompt is empty");
        assert!(!error.retryable);
    }

    #[test]
    fn error_round_trip_preserves_details() {
        let error = InteractionError::new(InteractionErrorCode::Unavailable, "provider down")
            .retryable()
            .with_details(serde_json::json!({"provider": "local"}));
        let value = serde_json::to_value(&error).expect("serialize error");
        let decoded: InteractionError = serde_json::from_value(value).expect("deserialize error");
        assert_eq!(decoded, error);
    }
}
