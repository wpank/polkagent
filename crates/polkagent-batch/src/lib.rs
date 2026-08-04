//! Batch processing for the Polkagent platform.
//!
//! This crate provides infrastructure for grouping multiple operations into
//! batches and processing them efficiently, with configurable concurrency
//! and error handling policies.
//!
//! # Architecture
//!
//! ```text
//!  Producer                 BatchCollector             BatchProcessor
//!    │                           │                          │
//!    │── submit(payload) ───────►│                          │
//!    │── submit(payload) ───────►│                          │
//!    │   ...                     │                          │
//!    │                           │── flush (size/timer) ──► │
//!    │                           │   Batch<T>               │
//!    │                           │                          │── handler(item) ──►
//!    │                           │                          │── handler(item) ──►
//!    │                           │                          │   (parallel)
//!    │                           │                          │
//!    │◄── BatchResult ──────────────────────────────────────│
//! ```
//!
//! # Key types
//!
//! - [`Batch`] — a collection of [`BatchItem`]s with shared configuration.
//! - [`BatchCollector`] — accumulates items and flushes them as batches
//!   (by size threshold or timer).
//! - [`BatchProcessor`] — executes a handler over each item in a batch,
//!   with configurable concurrency and error handling.
//! - [`BatchConfig`] — controls max size, max wait, concurrency, and
//!   error policy.
//! - [`BatchResult`] — aggregate outcome with per-item details.
//! - [`BatchStore`] / [`InMemoryBatchStore`] — persistence boundary for
//!   batch metadata.
//!
//! # Concurrency
//!
//! Processing concurrency is controlled by [`BatchConfig::concurrency`]:
//! - `1` = sequential processing (items are processed one at a time).
//! - `N > 1` = parallel processing with a semaphore limiting to `N`
//!   concurrent handlers.
//!
//! # Error policies
//!
//! - **`FailFast`** — stop on the first item failure; skip remaining items.
//! - **`SkipFailed`** — record failures; continue processing all items.
//! - **`RetryFailed`** — retry failed items up to `max_retries` times.

pub mod batch;
pub mod collector;
pub mod config;
pub mod error;
pub mod item;
pub mod memory_store;
pub mod processor;
pub mod result;
pub mod store;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use batch::{Batch, BatchId, BatchStatus};
pub use collector::BatchCollector;
pub use config::{BatchConfig, ErrorPolicy};
pub use error::BatchError;
pub use item::{BatchItem, ItemId, ItemStatus};
pub use memory_store::InMemoryBatchStore;
pub use processor::BatchProcessor;
pub use result::{BatchResult, ItemResult};
pub use store::{BatchStore, StoredBatch};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::json;
    use tokio::time;

    use super::*;

    // ======================================================================
    // Batch creation tests
    // ======================================================================

    #[test]
    fn batch_new_is_empty() {
        let batch: Batch<String> = Batch::new(BatchConfig::default());
        assert!(batch.is_empty());
        assert_eq!(batch.len(), 0);
        assert_eq!(batch.status, BatchStatus::Collecting);
    }

    #[test]
    fn batch_add_item() {
        let mut batch: Batch<i32> = Batch::new(BatchConfig::default());
        assert!(batch.push(42));
        assert_eq!(batch.len(), 1);
        assert!(!batch.is_empty());
    }

    #[test]
    fn batch_respects_max_size() {
        let config = BatchConfig::default().with_max_size(2);
        let mut batch: Batch<i32> = Batch::new(config);
        assert!(batch.push(1));
        assert!(batch.push(2));
        assert!(!batch.push(3)); // Rejected
        assert_eq!(batch.len(), 2);
        assert!(batch.is_full());
    }

    #[test]
    fn batch_unlimited_size() {
        let config = BatchConfig::default().with_max_size(0);
        let mut batch: Batch<i32> = Batch::new(config);
        for i in 0..1000 {
            assert!(batch.push(i));
        }
        assert_eq!(batch.len(), 1000);
        assert!(!batch.is_full());
    }

    #[test]
    fn batch_with_id() {
        let id = BatchId::new();
        let batch: Batch<String> = Batch::with_id(id, BatchConfig::default());
        assert_eq!(batch.id, id);
    }

    #[test]
    fn batch_count_by_status() {
        let mut batch: Batch<i32> = Batch::new(BatchConfig::default());
        batch.push(1);
        batch.push(2);
        batch.push(3);
        assert_eq!(batch.count_by_status(&ItemStatus::Pending), 3);
        batch.items[0].status = ItemStatus::Completed;
        batch.items[1].status = ItemStatus::Failed;
        assert_eq!(batch.completed_count(), 1);
        assert_eq!(batch.failed_count(), 1);
        assert_eq!(batch.count_by_status(&ItemStatus::Pending), 1);
    }

    // ======================================================================
    // Item tests
    // ======================================================================

    #[test]
    fn item_new_is_pending() {
        let item = BatchItem::new("hello");
        assert_eq!(item.status, ItemStatus::Pending);
        assert!(!item.is_done());
        assert!(!item.is_success());
        assert!(item.result.is_none());
        assert!(item.error.is_none());
    }

    #[test]
    fn item_with_explicit_id() {
        let id = ItemId::new();
        let item = BatchItem::with_id(id, 42);
        assert_eq!(item.id, id);
    }

    #[test]
    fn item_id_display_and_parse() {
        let id = ItemId::new();
        let s = id.to_string();
        let parsed: ItemId = s.parse().expect("valid UUID string");
        assert_eq!(id, parsed);
    }

    #[test]
    fn item_id_serde_round_trip() {
        let id = ItemId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        let back: ItemId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }

    #[test]
    fn item_status_display() {
        assert_eq!(ItemStatus::Pending.to_string(), "pending");
        assert_eq!(ItemStatus::Processing.to_string(), "processing");
        assert_eq!(ItemStatus::Completed.to_string(), "completed");
        assert_eq!(ItemStatus::Failed.to_string(), "failed");
        assert_eq!(ItemStatus::Skipped.to_string(), "skipped");
    }

    // ======================================================================
    // Config tests
    // ======================================================================

    #[test]
    fn config_defaults() {
        let config = BatchConfig::default();
        assert_eq!(config.max_size, 100);
        assert_eq!(config.concurrency, 1);
        assert_eq!(config.error_policy, ErrorPolicy::SkipFailed);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn config_sequential() {
        let config = BatchConfig::sequential();
        assert_eq!(config.concurrency, 1);
    }

    #[test]
    fn config_parallel() {
        let config = BatchConfig::parallel(8);
        assert_eq!(config.concurrency, 8);
    }

    #[test]
    fn config_parallel_clamps_to_one() {
        let config = BatchConfig::parallel(0);
        assert_eq!(config.concurrency, 1);
    }

    #[test]
    fn config_builder_chain() {
        let config = BatchConfig::parallel(4)
            .with_max_size(50)
            .with_max_wait(Duration::from_secs(10))
            .with_error_policy(ErrorPolicy::FailFast)
            .with_max_retries(5);
        assert_eq!(config.concurrency, 4);
        assert_eq!(config.max_size, 50);
        assert_eq!(config.max_wait, Duration::from_secs(10));
        assert_eq!(config.error_policy, ErrorPolicy::FailFast);
        assert_eq!(config.max_retries, 5);
    }

    #[test]
    fn config_serde_round_trip() {
        let config = BatchConfig::parallel(4).with_max_size(50);
        let json = serde_json::to_string(&config).expect("serialize");
        let back: BatchConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.concurrency, 4);
        assert_eq!(back.max_size, 50);
    }

    // ======================================================================
    // Sequential processing tests
    // ======================================================================

    #[tokio::test]
    async fn process_sequential_all_succeed() {
        let config = BatchConfig::sequential().with_max_size(0);
        let mut batch: Batch<i32> = Batch::new(config);
        for i in 1..=5 {
            batch.push(i);
        }

        let result = BatchProcessor::process(&mut batch, |n| async move { Ok(json!(n * 2)) })
            .await
            .expect("processing should succeed");

        assert_eq!(result.total, 5);
        assert_eq!(result.succeeded, 5);
        assert_eq!(result.failed, 0);
        assert!(result.all_succeeded());
        assert!(!result.has_failures());
        assert_eq!(batch.status, BatchStatus::Completed);
    }

    #[tokio::test]
    async fn process_sequential_skip_failed() {
        let config = BatchConfig::sequential()
            .with_max_size(0)
            .with_error_policy(ErrorPolicy::SkipFailed);
        let mut batch: Batch<i32> = Batch::new(config);
        for i in 1..=5 {
            batch.push(i);
        }

        let result = BatchProcessor::process(&mut batch, |n| async move {
            if n == 3 {
                Err("item 3 failed".to_string())
            } else {
                Ok(json!(n))
            }
        })
        .await
        .expect("processing should succeed");

        assert_eq!(result.total, 5);
        assert_eq!(result.succeeded, 4);
        assert_eq!(result.failed, 1);
        assert_eq!(result.skipped, 0);
        assert!(!result.all_succeeded());
        assert!(result.has_failures());
        assert_eq!(batch.status, BatchStatus::Completed);
    }

    #[tokio::test]
    async fn process_sequential_fail_fast() {
        let config = BatchConfig::sequential()
            .with_max_size(0)
            .with_error_policy(ErrorPolicy::FailFast);
        let mut batch: Batch<i32> = Batch::new(config);
        for i in 1..=5 {
            batch.push(i);
        }

        let result = BatchProcessor::process(&mut batch, |n| async move {
            if n == 2 {
                Err("item 2 failed".to_string())
            } else {
                Ok(json!(n))
            }
        })
        .await
        .expect("processing should succeed");

        // Item 1 succeeded, item 2 failed, items 3-5 skipped.
        assert_eq!(result.succeeded, 1);
        assert_eq!(result.failed, 1);
        assert_eq!(result.skipped, 3);
        assert_eq!(batch.status, BatchStatus::Aborted);
    }

    #[tokio::test]
    async fn process_sequential_retry_failed() {
        let call_count = Arc::new(AtomicU32::new(0));
        let config = BatchConfig::sequential()
            .with_max_size(0)
            .with_error_policy(ErrorPolicy::RetryFailed)
            .with_max_retries(2);
        let mut batch: Batch<i32> = Batch::new(config);
        batch.push(1);

        let cc = Arc::clone(&call_count);
        let result = BatchProcessor::process(&mut batch, move |_n| {
            let cc = Arc::clone(&cc);
            async move {
                let count = cc.fetch_add(1, Ordering::Relaxed);
                if count < 2 {
                    Err("transient".to_string())
                } else {
                    Ok(json!("ok"))
                }
            }
        })
        .await
        .expect("processing should succeed");

        // Should succeed after 2 retries (3 total calls).
        assert_eq!(result.succeeded, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(call_count.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn process_sequential_retry_exhausted() {
        let config = BatchConfig::sequential()
            .with_max_size(0)
            .with_error_policy(ErrorPolicy::RetryFailed)
            .with_max_retries(1);
        let mut batch: Batch<i32> = Batch::new(config);
        batch.push(1);

        let result = BatchProcessor::process(&mut batch, |_n| async move {
            Err::<serde_json::Value, _>("always fails".to_string())
        })
        .await
        .expect("processing should succeed");

        // max_retries=1 means 1 initial attempt + 1 retry = 2 total, then fail.
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 1);
    }

    #[tokio::test]
    async fn process_empty_batch() {
        let config = BatchConfig::sequential();
        let mut batch: Batch<i32> = Batch::new(config);

        let result = BatchProcessor::process(&mut batch, |n| async move { Ok(json!(n)) })
            .await
            .expect("processing should succeed");

        assert_eq!(result.total, 0);
        assert_eq!(result.succeeded, 0);
        assert!(result.all_succeeded()); // Vacuously true.
    }

    // ======================================================================
    // Parallel processing tests
    // ======================================================================

    #[tokio::test]
    async fn process_parallel_all_succeed() {
        let config = BatchConfig::parallel(4).with_max_size(0);
        let mut batch: Batch<i32> = Batch::new(config);
        for i in 1..=10 {
            batch.push(i);
        }

        let result = BatchProcessor::process(&mut batch, |n| async move { Ok(json!(n * 2)) })
            .await
            .expect("processing should succeed");

        assert_eq!(result.total, 10);
        assert_eq!(result.succeeded, 10);
        assert!(result.all_succeeded());
    }

    #[tokio::test]
    async fn process_parallel_skip_failed() {
        let config = BatchConfig::parallel(4)
            .with_max_size(0)
            .with_error_policy(ErrorPolicy::SkipFailed);
        let mut batch: Batch<i32> = Batch::new(config);
        for i in 1..=6 {
            batch.push(i);
        }

        let result = BatchProcessor::process(&mut batch, |n| async move {
            if n % 3 == 0 {
                Err(format!("item {n} failed"))
            } else {
                Ok(json!(n))
            }
        })
        .await
        .expect("processing should succeed");

        assert_eq!(result.succeeded, 4); // 1, 2, 4, 5
        assert_eq!(result.failed, 2); // 3, 6
        assert_eq!(batch.status, BatchStatus::Completed);
    }

    #[tokio::test]
    async fn process_parallel_respects_concurrency() {
        let active = Arc::new(AtomicU32::new(0));
        let max_active = Arc::new(AtomicU32::new(0));

        let config = BatchConfig::parallel(2).with_max_size(0);
        let mut batch: Batch<i32> = Batch::new(config);
        for i in 0..8 {
            batch.push(i);
        }

        let a = Arc::clone(&active);
        let m = Arc::clone(&max_active);
        let _result = BatchProcessor::process(&mut batch, move |n| {
            let a = Arc::clone(&a);
            let m = Arc::clone(&m);
            async move {
                let current = a.fetch_add(1, Ordering::Relaxed) + 1;
                // Track maximum concurrent executions.
                m.fetch_max(current, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(10)).await;
                a.fetch_sub(1, Ordering::Relaxed);
                Ok(json!(n))
            }
        })
        .await
        .expect("processing should succeed");

        // Max active should not exceed the semaphore limit of 2.
        assert!(max_active.load(Ordering::Relaxed) <= 2);
    }

    // ======================================================================
    // Collector tests
    // ======================================================================

    #[tokio::test]
    async fn collector_flushes_on_max_size() {
        let config = BatchConfig::default().with_max_size(3);
        let (collector, mut rx) = BatchCollector::<i32>::new(config);

        collector.submit(1).await.expect("submit");
        collector.submit(2).await.expect("submit");
        assert_eq!(collector.buffered_count(), 2);

        collector.submit(3).await.expect("submit");

        // Should have flushed.
        let batch = rx.try_recv().expect("should receive batch");
        assert_eq!(batch.items.len(), 3);
        assert_eq!(collector.buffered_count(), 0);
    }

    #[tokio::test]
    async fn collector_manual_flush() {
        let config = BatchConfig::default().with_max_size(100);
        let (collector, mut rx) = BatchCollector::<i32>::new(config);

        collector.submit(1).await.expect("submit");
        collector.submit(2).await.expect("submit");
        collector.flush().await.expect("flush");

        let batch = rx.try_recv().expect("should receive batch");
        assert_eq!(batch.items.len(), 2);
    }

    #[tokio::test]
    async fn collector_flush_empty_is_noop() {
        let config = BatchConfig::default();
        let (collector, mut rx) = BatchCollector::<i32>::new(config);

        collector.flush().await.expect("flush empty");

        // Nothing should be in the channel.
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn collector_timer_flush() {
        let config = BatchConfig::default()
            .with_max_size(100)
            .with_max_wait(Duration::from_millis(50));
        let (collector, mut rx) = BatchCollector::<i32>::new(config);

        let _timer = collector.start_timer();

        collector.submit(1).await.expect("submit");
        collector.submit(2).await.expect("submit");

        // Wait for the timer to flush.
        let batch = time::timeout(Duration::from_millis(200), rx.recv())
            .await
            .expect("should receive within timeout")
            .expect("should have batch");

        assert_eq!(batch.items.len(), 2);
    }

    #[tokio::test]
    async fn collector_shutdown_rejects_items() {
        let config = BatchConfig::default();
        let (collector, _rx) = BatchCollector::<i32>::new(config);

        collector.shutdown();
        assert!(collector.is_shut_down());

        let err = collector.submit(1).await.unwrap_err();
        assert!(matches!(err, BatchError::CollectorShutDown));
    }

    #[tokio::test]
    async fn collector_multiple_flushes() {
        let config = BatchConfig::default().with_max_size(2);
        let (collector, mut rx) = BatchCollector::<i32>::new(config);

        collector.submit(1).await.expect("submit");
        collector.submit(2).await.expect("submit");
        // First flush (size-triggered).

        collector.submit(3).await.expect("submit");
        collector.submit(4).await.expect("submit");
        // Second flush (size-triggered).

        let batch1 = rx.try_recv().expect("first batch");
        let batch2 = rx.try_recv().expect("second batch");

        assert_eq!(batch1.items.len(), 2);
        assert_eq!(batch2.items.len(), 2);
        assert_ne!(batch1.id, batch2.id);
    }

    // ======================================================================
    // BatchResult tests
    // ======================================================================

    #[test]
    fn batch_result_from_items() {
        let batch_id = BatchId::new();
        let items = vec![
            (ItemId::new(), ItemStatus::Completed, Some(json!(1)), None),
            (
                ItemId::new(),
                ItemStatus::Failed,
                None,
                Some("err".to_string()),
            ),
            (ItemId::new(), ItemStatus::Skipped, None, None),
            (ItemId::new(), ItemStatus::Completed, Some(json!(4)), None),
        ];
        let started = chrono::Utc::now();
        let result = BatchResult::from_items(batch_id, &items, started);

        assert_eq!(result.batch_id, batch_id);
        assert_eq!(result.total, 4);
        assert_eq!(result.succeeded, 2);
        assert_eq!(result.failed, 1);
        assert_eq!(result.skipped, 1);
        assert!(!result.all_succeeded());
        assert!(result.has_failures());
        assert_eq!(result.pending_count(), 0);
    }

    #[test]
    fn batch_result_all_succeeded() {
        let batch_id = BatchId::new();
        let items = vec![
            (ItemId::new(), ItemStatus::Completed, Some(json!(1)), None),
            (ItemId::new(), ItemStatus::Completed, Some(json!(2)), None),
        ];
        let result = BatchResult::from_items(batch_id, &items, chrono::Utc::now());

        assert!(result.all_succeeded());
        assert!(!result.has_failures());
    }

    #[test]
    fn batch_result_serde_round_trip() {
        let batch_id = BatchId::new();
        let items = vec![(
            ItemId::new(),
            ItemStatus::Completed,
            Some(json!("ok")),
            None,
        )];
        let result = BatchResult::from_items(batch_id, &items, chrono::Utc::now());
        let json = serde_json::to_string(&result).expect("serialize");
        let back: BatchResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.batch_id, batch_id);
        assert_eq!(back.total, 1);
    }

    // ======================================================================
    // BatchId / ItemId tests
    // ======================================================================

    #[test]
    fn batch_id_uniqueness() {
        let a = BatchId::new();
        let b = BatchId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn batch_id_serde_round_trip() {
        let id = BatchId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        let back: BatchId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }

    #[test]
    fn batch_id_from_uuid() {
        let uuid = uuid::Uuid::now_v7();
        let id = BatchId::from_uuid(uuid);
        assert_eq!(id.as_uuid(), uuid);
    }

    // ======================================================================
    // Error tests
    // ======================================================================

    #[test]
    fn batch_error_retryable() {
        let e = BatchError::Concurrency {
            message: "sem".into(),
        };
        assert!(e.is_retryable());

        let e = BatchError::Store {
            message: "db".into(),
        };
        assert!(e.is_retryable());

        let e = BatchError::CollectorShutDown;
        assert!(!e.is_retryable());
    }

    #[test]
    fn batch_error_display() {
        let id = BatchId::new();
        let e = BatchError::BatchFull {
            batch_id: id,
            max_size: 10,
        };
        let msg = format!("{e}");
        assert!(msg.contains("full"));
        assert!(msg.contains("10"));
    }

    // ======================================================================
    // InMemoryBatchStore tests
    // ======================================================================

    #[tokio::test]
    async fn memory_store_save_and_get() {
        let store = InMemoryBatchStore::new();
        let id = BatchId::new();
        let batch = StoredBatch {
            id,
            status: "collecting".into(),
            item_count: 5,
            payload: json!({"items": [1, 2, 3, 4, 5]}),
            created_at: chrono::Utc::now(),
        };

        store.save_batch(batch.clone()).await.expect("save");
        assert_eq!(store.batch_count(), 1);

        let got = store.get_batch(id).await.expect("get");
        assert_eq!(got.id, id);
        assert_eq!(got.status, "collecting");
        assert_eq!(got.item_count, 5);
    }

    #[tokio::test]
    async fn memory_store_duplicate_fails() {
        let store = InMemoryBatchStore::new();
        let id = BatchId::new();
        let batch = StoredBatch {
            id,
            status: "collecting".into(),
            item_count: 0,
            payload: json!(null),
            created_at: chrono::Utc::now(),
        };

        store.save_batch(batch.clone()).await.expect("save");
        let err = store.save_batch(batch).await.unwrap_err();
        assert!(matches!(err, BatchError::Store { .. }));
    }

    #[tokio::test]
    async fn memory_store_update_status() {
        let store = InMemoryBatchStore::new();
        let id = BatchId::new();
        let batch = StoredBatch {
            id,
            status: "collecting".into(),
            item_count: 0,
            payload: json!(null),
            created_at: chrono::Utc::now(),
        };

        store.save_batch(batch).await.expect("save");
        store.update_status(id, "completed").await.expect("update");

        let got = store.get_batch(id).await.expect("get");
        assert_eq!(got.status, "completed");
    }

    #[tokio::test]
    async fn memory_store_save_and_get_result() {
        let store = InMemoryBatchStore::new();
        let batch_id = BatchId::new();
        let result = BatchResult::from_items(batch_id, &[], chrono::Utc::now());

        store.save_result(result).await.expect("save result");
        assert_eq!(store.result_count(), 1);

        let got = store.get_result(batch_id).await.expect("get result");
        assert_eq!(got.batch_id, batch_id);
    }

    #[tokio::test]
    async fn memory_store_not_found() {
        let store = InMemoryBatchStore::new();
        let err = store.get_batch(BatchId::new()).await.unwrap_err();
        assert!(matches!(err, BatchError::NotFound { .. }));
    }

    #[tokio::test]
    async fn memory_store_list_by_status() {
        let store = InMemoryBatchStore::new();

        for i in 0..5 {
            let status = if i % 2 == 0 {
                "completed"
            } else {
                "collecting"
            };
            let batch = StoredBatch {
                id: BatchId::new(),
                status: status.into(),
                item_count: i,
                payload: json!(null),
                created_at: chrono::Utc::now(),
            };
            store.save_batch(batch).await.expect("save");
        }

        let completed = store
            .list_by_status("completed", 10, 0)
            .await
            .expect("list");
        assert_eq!(completed.len(), 3);

        let collecting = store
            .list_by_status("collecting", 10, 0)
            .await
            .expect("list");
        assert_eq!(collecting.len(), 2);
    }

    #[tokio::test]
    async fn memory_store_list_with_pagination() {
        let store = InMemoryBatchStore::new();

        for _ in 0..5 {
            let batch = StoredBatch {
                id: BatchId::new(),
                status: "completed".into(),
                item_count: 0,
                payload: json!(null),
                created_at: chrono::Utc::now(),
            };
            store.save_batch(batch).await.expect("save");
        }

        let page1 = store.list_by_status("completed", 2, 0).await.expect("list");
        assert_eq!(page1.len(), 2);

        let page2 = store.list_by_status("completed", 2, 2).await.expect("list");
        assert_eq!(page2.len(), 2);

        let page3 = store.list_by_status("completed", 2, 4).await.expect("list");
        assert_eq!(page3.len(), 1);
    }

    #[tokio::test]
    async fn memory_store_delete() {
        let store = InMemoryBatchStore::new();
        let id = BatchId::new();
        let batch = StoredBatch {
            id,
            status: "completed".into(),
            item_count: 0,
            payload: json!(null),
            created_at: chrono::Utc::now(),
        };

        store.save_batch(batch).await.expect("save");
        store.delete(id).await.expect("delete");
        assert_eq!(store.batch_count(), 0);

        let err = store.get_batch(id).await.unwrap_err();
        assert!(matches!(err, BatchError::NotFound { .. }));
    }

    #[tokio::test]
    async fn memory_store_delete_not_found() {
        let store = InMemoryBatchStore::new();
        let err = store.delete(BatchId::new()).await.unwrap_err();
        assert!(matches!(err, BatchError::NotFound { .. }));
    }

    // ======================================================================
    // Concurrent batch tests
    // ======================================================================

    #[tokio::test]
    async fn concurrent_batches_independent() {
        let config = BatchConfig::parallel(2).with_max_size(0);

        let mut batch_a: Batch<i32> = Batch::new(config.clone());
        let mut batch_b: Batch<i32> = Batch::new(config);

        for i in 1..=3 {
            batch_a.push(i);
        }
        for i in 10..=12 {
            batch_b.push(i);
        }

        let (result_a, result_b) = tokio::join!(
            BatchProcessor::process(&mut batch_a, |n| async move { Ok(json!(n * 10)) }),
            BatchProcessor::process(&mut batch_b, |n| async move { Ok(json!(n + 100)) }),
        );

        let ra = result_a.expect("batch A");
        let rb = result_b.expect("batch B");

        assert_eq!(ra.total, 3);
        assert_eq!(ra.succeeded, 3);
        assert_eq!(rb.total, 3);
        assert_eq!(rb.succeeded, 3);
    }

    // ======================================================================
    // Progress tracking tests
    // ======================================================================

    #[test]
    fn batch_result_pending_count() {
        let batch_id = BatchId::new();
        let items = vec![
            (ItemId::new(), ItemStatus::Completed, Some(json!(1)), None),
            (ItemId::new(), ItemStatus::Pending, None, None),
            (ItemId::new(), ItemStatus::Pending, None, None),
        ];
        let result = BatchResult::from_items(batch_id, &items, chrono::Utc::now());

        assert_eq!(result.pending_count(), 2);
    }

    #[test]
    fn error_policy_serde() {
        let policy = ErrorPolicy::RetryFailed;
        let json = serde_json::to_string(&policy).expect("serialize");
        assert_eq!(json, "\"retry_failed\"");
        let back: ErrorPolicy = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, ErrorPolicy::RetryFailed);
    }

    #[test]
    fn batch_status_display() {
        assert_eq!(BatchStatus::Collecting.to_string(), "collecting");
        assert_eq!(BatchStatus::Processing.to_string(), "processing");
        assert_eq!(BatchStatus::Completed.to_string(), "completed");
        assert_eq!(BatchStatus::Aborted.to_string(), "aborted");
    }
}
