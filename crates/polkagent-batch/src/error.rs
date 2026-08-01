//! Error types for batch processing.

use thiserror::Error;

use crate::batch::BatchId;
use crate::item::ItemId;

/// Unified error type for batch operations.
#[derive(Debug, Error)]
pub enum BatchError {
    /// The batch has reached its maximum configured size.
    #[error("batch {batch_id} is full (max_size={max_size})")]
    BatchFull {
        /// The batch that rejected the item.
        batch_id: BatchId,
        /// The configured maximum size.
        max_size: usize,
    },

    /// A batch or item with the given ID was not found.
    #[error("not found: {resource_type} id={id}")]
    NotFound {
        /// Short name of the resource type (e.g. "Batch", "Item").
        resource_type: &'static str,
        /// String representation of the requested ID.
        id: String,
    },

    /// Processing of an individual item failed.
    #[error("item {item_id} failed: {message}")]
    ItemFailed {
        /// The item that failed.
        item_id: ItemId,
        /// A human-readable error message.
        message: String,
    },

    /// Processing was aborted due to a `FailFast` error policy.
    #[error("batch {batch_id} aborted: fail-fast triggered by item {item_id}")]
    Aborted {
        /// The batch that was aborted.
        batch_id: BatchId,
        /// The item whose failure triggered the abort.
        item_id: ItemId,
    },

    /// The batch is in an invalid state for the attempted operation.
    #[error("invalid state: batch {batch_id} is {state}, expected {expected}")]
    InvalidState {
        /// The batch in question.
        batch_id: BatchId,
        /// Current state of the batch.
        state: String,
        /// The state that was expected.
        expected: String,
    },

    /// A concurrency limit or semaphore acquisition failed.
    #[error("concurrency error: {message}")]
    Concurrency {
        /// A human-readable description of the concurrency problem.
        message: String,
    },

    /// An error from the underlying store.
    #[error("store error: {message}")]
    Store {
        /// A human-readable description of the store error.
        message: String,
    },

    /// Serialisation or deserialisation failed.
    #[error("serialisation error: {message}")]
    Serialisation {
        /// A human-readable description of the serialisation error.
        message: String,
    },

    /// The collector has been shut down and no longer accepts items.
    #[error("collector shut down")]
    CollectorShutDown,
}

impl BatchError {
    /// Returns `true` if the error is retryable.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Concurrency { .. } | Self::Store { .. })
    }
}
