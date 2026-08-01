//! Error types for the `polkagent-surface-webhook` crate.

use thiserror::Error;
use uuid::Uuid;

/// All errors that can originate from webhook operations.
#[derive(Debug, Error)]
pub enum WebhookError {
    /// The webhook URL is malformed or unsupported.
    #[error("invalid webhook URL: {0}")]
    InvalidUrl(String),

    /// Payload serialization failed.
    #[error("payload serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// HTTP request failed.
    #[error("HTTP delivery error: {0}")]
    Http(String),

    /// The remote server returned a non-2xx status code.
    #[error("remote returned status {status}: {body}")]
    RemoteError {
        /// HTTP status code.
        status: u16,
        /// Response body (truncated).
        body: String,
    },

    /// The delivery timed out.
    #[error("delivery timed out after {0:?}")]
    Timeout(std::time::Duration),

    /// The webhook subscription was not found.
    #[error("webhook subscription not found: {0}")]
    NotFound(Uuid),

    /// A duplicate webhook subscription already exists.
    #[error("duplicate webhook subscription: {0}")]
    Duplicate(Uuid),

    /// The delivery record was not found.
    #[error("delivery not found: {0}")]
    DeliveryNotFound(Uuid),

    /// The maximum number of retry attempts has been exhausted.
    #[error("max retries exhausted after {attempts} attempts for delivery {delivery_id}")]
    RetriesExhausted {
        /// The delivery that failed.
        delivery_id: Uuid,
        /// Number of attempts made.
        attempts: u32,
    },

    /// HMAC signature verification failed.
    #[error("signature verification failed")]
    SignatureInvalid,
}

/// Convenience type alias.
pub type Result<T> = std::result::Result<T, WebhookError>;
