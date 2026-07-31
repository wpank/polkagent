//! Error types for the conversation subsystem.

use thiserror::Error;

/// All errors that can be produced by the conversation subsystem.
#[derive(Debug, Error)]
pub enum ConversationError {
    /// A conversation with the given identifier was not found.
    #[error("conversation not found: {0}")]
    NotFound(String),

    /// A message with the given identifier was not found.
    #[error("message not found: {0}")]
    MessageNotFound(String),

    /// JSON serialisation or deserialisation failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// A UUID could not be parsed.
    #[error("invalid id: {0}")]
    InvalidId(#[from] uuid::Error),

    /// An operation was logically invalid.
    #[error("invalid operation: {0}")]
    InvalidOperation(String),

    /// The context window token budget has been exceeded.
    #[error("context window exceeded: {current} tokens > {max} allowed")]
    ContextWindowExceeded {
        /// Current token count.
        current: u32,
        /// Maximum allowed token count.
        max: u32,
    },

    /// A lock could not be acquired (poisoned or contended).
    #[error("lock error: {0}")]
    Lock(String),

    /// An unexpected internal error.
    #[error("internal error: {0}")]
    Internal(String),
}

/// Convenience alias.
pub type ConversationResult<T> = Result<T, ConversationError>;
