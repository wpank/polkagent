//! Batch store trait — persistence boundary for batches.
//!
//! [`BatchStore`] defines the minimal storage contract for persisting batch
//! metadata and results. Implementations may use any backend (in-memory,
//! `SQLite`, Postgres, etc.).

use async_trait::async_trait;
use serde_json::Value;

use crate::batch::BatchId;
use crate::error::BatchError;
use crate::result::BatchResult;

/// Serialisable snapshot of a batch for persistence.
///
/// This avoids requiring the store to know about the generic payload type `T`.
/// The payload is stored as an opaque JSON value.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredBatch {
    /// Batch identifier.
    pub id: BatchId,
    /// Overall status tag (e.g. "collecting", "processing", "completed").
    pub status: String,
    /// Number of items in the batch.
    pub item_count: usize,
    /// Full batch data as an opaque JSON value.
    pub payload: Value,
    /// When the batch was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Persistence boundary for batch metadata and results.
///
/// Implementations must be `Send + Sync` so that the store can be shared
/// across async tasks.
#[async_trait]
pub trait BatchStore: Send + Sync {
    /// Persist a batch snapshot.
    ///
    /// Returns `BatchError::Store` on conflict or backend failure.
    async fn save_batch(&self, batch: StoredBatch) -> Result<(), BatchError>;

    /// Retrieve a batch snapshot by ID.
    ///
    /// Returns `BatchError::NotFound` if no batch exists with the given ID.
    async fn get_batch(&self, id: BatchId) -> Result<StoredBatch, BatchError>;

    /// Update the status of an existing batch.
    async fn update_status(&self, id: BatchId, status: &str) -> Result<(), BatchError>;

    /// Save a batch result after processing.
    async fn save_result(&self, result: BatchResult) -> Result<(), BatchError>;

    /// Retrieve the result for a given batch.
    ///
    /// Returns `BatchError::NotFound` if no result has been recorded.
    async fn get_result(&self, batch_id: BatchId) -> Result<BatchResult, BatchError>;

    /// List batch IDs by status, ordered by creation time descending.
    async fn list_by_status(
        &self,
        status: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<StoredBatch>, BatchError>;

    /// Delete a batch and its result.
    async fn delete(&self, id: BatchId) -> Result<(), BatchError>;
}
