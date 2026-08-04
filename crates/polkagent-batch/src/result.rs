//! Batch result types.
//!
//! [`BatchResult`] summarises the outcome of processing a complete batch,
//! providing aggregate counts and per-item results for downstream consumers.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::batch::BatchId;
use crate::item::{ItemId, ItemStatus};

/// Result of processing a single item within a batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemResult {
    /// Identifier of the item.
    pub item_id: ItemId,
    /// Final status of the item.
    pub status: ItemStatus,
    /// The result value, if the item succeeded.
    pub result: Option<serde_json::Value>,
    /// Error message, if the item failed.
    pub error: Option<String>,
}

/// Aggregate result of processing a batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResult {
    /// Identifier of the processed batch.
    pub batch_id: BatchId,
    /// Total number of items in the batch.
    pub total: usize,
    /// Number of items that succeeded.
    pub succeeded: usize,
    /// Number of items that failed.
    pub failed: usize,
    /// Number of items that were skipped.
    pub skipped: usize,
    /// Per-item results in the order they were added to the batch.
    pub results: Vec<ItemResult>,
    /// When processing started.
    pub started_at: DateTime<Utc>,
    /// When processing completed.
    pub completed_at: DateTime<Utc>,
}

impl BatchResult {
    /// Build a `BatchResult` from a processed batch.
    #[must_use]
    pub fn from_items(
        batch_id: BatchId,
        items: &[(
            ItemId,
            ItemStatus,
            Option<serde_json::Value>,
            Option<String>,
        )],
        started_at: DateTime<Utc>,
    ) -> Self {
        let total = items.len();
        let mut succeeded = 0;
        let mut failed = 0;
        let mut skipped = 0;
        let mut results = Vec::with_capacity(total);

        for (item_id, status, result, error) in items {
            match status {
                ItemStatus::Completed => succeeded += 1,
                ItemStatus::Failed => failed += 1,
                ItemStatus::Skipped => skipped += 1,
                _ => {}
            }
            results.push(ItemResult {
                item_id: *item_id,
                status: status.clone(),
                result: result.clone(),
                error: error.clone(),
            });
        }

        Self {
            batch_id,
            total,
            succeeded,
            failed,
            skipped,
            results,
            started_at,
            completed_at: Utc::now(),
        }
    }

    /// Returns `true` if all items succeeded.
    #[must_use]
    pub fn all_succeeded(&self) -> bool {
        self.succeeded == self.total
    }

    /// Returns `true` if any item failed.
    #[must_use]
    pub fn has_failures(&self) -> bool {
        self.failed > 0
    }

    /// Returns the number of items still pending (not processed).
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.total
            .saturating_sub(self.succeeded + self.failed + self.skipped)
    }
}
