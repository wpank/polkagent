//! In-memory batch store implementation.
//!
//! [`InMemoryBatchStore`] is a simple hash-map-backed store intended for
//! testing and development. It is **not** durable — all data is lost when
//! the process exits.

use std::collections::HashMap;

use async_trait::async_trait;
use parking_lot::RwLock;

use crate::batch::BatchId;
use crate::error::BatchError;
use crate::result::BatchResult;
use crate::store::{BatchStore, StoredBatch};

/// An in-memory implementation of [`BatchStore`].
///
/// Thread-safe via `parking_lot::RwLock`. Suitable for unit tests and
/// ephemeral workloads.
#[derive(Debug, Default)]
pub struct InMemoryBatchStore {
    batches: RwLock<HashMap<BatchId, StoredBatch>>,
    results: RwLock<HashMap<BatchId, BatchResult>>,
}

impl InMemoryBatchStore {
    /// Create a new empty in-memory store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the number of stored batches.
    #[must_use]
    pub fn batch_count(&self) -> usize {
        self.batches.read().len()
    }

    /// Returns the number of stored results.
    #[must_use]
    pub fn result_count(&self) -> usize {
        self.results.read().len()
    }
}

#[async_trait]
impl BatchStore for InMemoryBatchStore {
    async fn save_batch(&self, batch: StoredBatch) -> Result<(), BatchError> {
        let mut batches = self.batches.write();
        if batches.contains_key(&batch.id) {
            return Err(BatchError::Store {
                message: format!("batch {} already exists", batch.id),
            });
        }
        batches.insert(batch.id, batch);
        Ok(())
    }

    async fn get_batch(&self, id: BatchId) -> Result<StoredBatch, BatchError> {
        self.batches
            .read()
            .get(&id)
            .cloned()
            .ok_or_else(|| BatchError::NotFound {
                resource_type: "Batch",
                id: id.to_string(),
            })
    }

    async fn update_status(&self, id: BatchId, status: &str) -> Result<(), BatchError> {
        let mut batches = self.batches.write();
        let batch = batches.get_mut(&id).ok_or_else(|| BatchError::NotFound {
            resource_type: "Batch",
            id: id.to_string(),
        })?;
        batch.status = status.to_string();
        Ok(())
    }

    async fn save_result(&self, result: BatchResult) -> Result<(), BatchError> {
        self.results.write().insert(result.batch_id, result);
        Ok(())
    }

    async fn get_result(&self, batch_id: BatchId) -> Result<BatchResult, BatchError> {
        self.results
            .read()
            .get(&batch_id)
            .cloned()
            .ok_or_else(|| BatchError::NotFound {
                resource_type: "BatchResult",
                id: batch_id.to_string(),
            })
    }

    async fn list_by_status(
        &self,
        status: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<StoredBatch>, BatchError> {
        let batches = self.batches.read();
        let mut matching: Vec<&StoredBatch> = batches
            .values()
            .filter(|b| b.status == status)
            .collect();

        // Sort by created_at descending.
        matching.sort_by_key(|b| std::cmp::Reverse(b.created_at));

        let result = matching
            .into_iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect();

        Ok(result)
    }

    async fn delete(&self, id: BatchId) -> Result<(), BatchError> {
        let removed = self.batches.write().remove(&id);
        self.results.write().remove(&id);

        if removed.is_none() {
            return Err(BatchError::NotFound {
                resource_type: "Batch",
                id: id.to_string(),
            });
        }
        Ok(())
    }
}
