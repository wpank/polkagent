//! Error types for the `polkagent-feed` crate.

use thiserror::Error;

/// All errors that can arise during feed, trigger, and recipe operations.
#[derive(Debug, Error)]
pub enum FeedError {
    /// The requested resource (feed, trigger, recipe, or item) was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// A trigger condition expression is malformed or references an invalid path.
    #[error("invalid condition: {0}")]
    InvalidCondition(String),

    /// The trigger is currently in its cooldown window and cannot be fired.
    #[error("trigger cooldown active")]
    CooldownActive,

    /// An error occurred while processing a feed item.
    #[error("processing error: {0}")]
    ProcessingError(String),

    /// One or more required recipe parameters are missing or have the wrong type.
    #[error("invalid recipe params: {0}")]
    InvalidRecipeParams(String),
}

/// Convenience `Result` alias for `FeedError`.
pub type Result<T> = std::result::Result<T, FeedError>;
