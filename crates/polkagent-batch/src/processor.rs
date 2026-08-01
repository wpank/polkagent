//! Batch processor — executes a handler function over every item in a batch.
//!
//! The [`BatchProcessor`] supports both sequential and parallel execution,
//! controlled by [`BatchConfig::concurrency`](crate::config::BatchConfig::concurrency).
//! Error handling respects the configured [`ErrorPolicy`](crate::config::ErrorPolicy).

use std::future::Future;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use crate::batch::{Batch, BatchStatus};
use crate::config::ErrorPolicy;
use crate::error::BatchError;
use crate::item::ItemStatus;
use crate::result::BatchResult;

/// Processes batches of items using a caller-supplied async handler.
#[derive(Debug)]
pub struct BatchProcessor;

impl BatchProcessor {
    /// Process all items in `batch` by applying `handler` to each payload.
    ///
    /// The handler is called with each item's payload and must return a
    /// `Result<serde_json::Value, String>`. The concurrency level and error
    /// policy are taken from the batch's configuration.
    ///
    /// # Concurrency
    ///
    /// - `concurrency == 1` executes items sequentially.
    /// - `concurrency > 1` executes items in parallel, limited by a semaphore.
    ///
    /// # Error policies
    ///
    /// - **`FailFast`**: the first item failure aborts remaining items.
    /// - **`SkipFailed`**: failures are recorded; processing continues.
    /// - **`RetryFailed`**: failed items are retried up to `max_retries` times.
    pub async fn process<T, F, Fut>(
        batch: &mut Batch<T>,
        handler: F,
    ) -> Result<BatchResult, BatchError>
    where
        T: Send + Sync + Clone + 'static,
        F: Fn(T) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<serde_json::Value, String>> + Send,
    {
        let started_at = Utc::now();
        batch.status = BatchStatus::Processing;
        let concurrency = batch.config.concurrency.max(1);
        let error_policy = batch.config.error_policy;
        let max_retries = batch.config.max_retries;

        info!(
            batch_id = %batch.id,
            item_count = batch.items.len(),
            concurrency,
            error_policy = ?error_policy,
            "starting batch processing"
        );

        if concurrency == 1 {
            Self::process_sequential(batch, &handler, error_policy, max_retries).await;
        } else {
            Self::process_parallel(batch, handler, concurrency, error_policy, max_retries).await;
        }

        // Determine final batch status.
        let has_failures = batch.items.iter().any(|i| i.status == ItemStatus::Failed);
        let was_aborted = error_policy == ErrorPolicy::FailFast && has_failures;

        batch.status = if was_aborted {
            BatchStatus::Aborted
        } else {
            BatchStatus::Completed
        };

        let items: Vec<_> = batch
            .items
            .iter()
            .map(|i| (i.id, i.status.clone(), i.result.clone(), i.error.clone()))
            .collect();

        let result = BatchResult::from_items(batch.id, &items, started_at);

        info!(
            batch_id = %batch.id,
            total = result.total,
            succeeded = result.succeeded,
            failed = result.failed,
            skipped = result.skipped,
            "batch processing complete"
        );

        Ok(result)
    }

    /// Process items one at a time.
    async fn process_sequential<T, F, Fut>(
        batch: &mut Batch<T>,
        handler: &F,
        error_policy: ErrorPolicy,
        max_retries: u32,
    ) where
        T: Clone,
        F: Fn(T) -> Fut,
        Fut: Future<Output = Result<serde_json::Value, String>>,
    {
        for item in &mut batch.items {
            if item.status != ItemStatus::Pending {
                continue;
            }

            item.status = ItemStatus::Processing;
            let payload = item.payload.clone();

            match error_policy {
                ErrorPolicy::RetryFailed => {
                    let mut attempts = 0u32;
                    loop {
                        match handler(payload.clone()).await {
                            Ok(value) => {
                                item.status = ItemStatus::Completed;
                                item.result = Some(value);
                                break;
                            }
                            Err(e) => {
                                attempts += 1;
                                if attempts > max_retries {
                                    warn!(item_id = %item.id, attempts, "item exhausted retries");
                                    item.status = ItemStatus::Failed;
                                    item.error = Some(e);
                                    break;
                                }
                                debug!(item_id = %item.id, attempts, "retrying item");
                            }
                        }
                    }
                }
                _ => match handler(payload).await {
                    Ok(value) => {
                        item.status = ItemStatus::Completed;
                        item.result = Some(value);
                    }
                    Err(e) => {
                        item.status = ItemStatus::Failed;
                        item.error = Some(e);

                        if error_policy == ErrorPolicy::FailFast {
                            // Mark remaining pending items as skipped.
                            break;
                        }
                    }
                },
            }
        }

        // If FailFast, mark all remaining pending items as skipped.
        if error_policy == ErrorPolicy::FailFast {
            let had_failure = batch.items.iter().any(|i| i.status == ItemStatus::Failed);
            if had_failure {
                for item in &mut batch.items {
                    if item.status == ItemStatus::Pending || item.status == ItemStatus::Processing {
                        item.status = ItemStatus::Skipped;
                    }
                }
            }
        }
    }

    /// Process items in parallel with a semaphore.
    async fn process_parallel<T, F, Fut>(
        batch: &mut Batch<T>,
        handler: F,
        concurrency: usize,
        error_policy: ErrorPolicy,
        max_retries: u32,
    ) where
        T: Send + Sync + Clone + 'static,
        F: Fn(T) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<serde_json::Value, String>> + Send,
    {
        let semaphore = Arc::new(Semaphore::new(concurrency));
        let handler = Arc::new(handler);
        let abort_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Collect results: (index, status, result, error)
        let mut join_handles = Vec::with_capacity(batch.items.len());

        for (idx, item) in batch.items.iter_mut().enumerate() {
            if item.status != ItemStatus::Pending {
                continue;
            }
            item.status = ItemStatus::Processing;

            let payload = item.payload.clone();
            let sem = Arc::clone(&semaphore);
            let h = Arc::clone(&handler);
            let abort = Arc::clone(&abort_flag);

            let handle = tokio::spawn(async move {
                let _permit = sem.acquire().await.map_err(|e| e.to_string());

                // Check if we should abort before starting.
                if error_policy == ErrorPolicy::FailFast
                    && abort.load(std::sync::atomic::Ordering::Relaxed)
                {
                    return (idx, ItemStatus::Skipped, None, None);
                }

                match error_policy {
                    ErrorPolicy::RetryFailed => {
                        let mut attempts = 0u32;
                        loop {
                            match h(payload.clone()).await {
                                Ok(value) => {
                                    return (idx, ItemStatus::Completed, Some(value), None);
                                }
                                Err(e) => {
                                    attempts += 1;
                                    if attempts > max_retries {
                                        return (
                                            idx,
                                            ItemStatus::Failed,
                                            None,
                                            Some(e),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    _ => match h(payload).await {
                        Ok(value) => (idx, ItemStatus::Completed, Some(value), None),
                        Err(e) => {
                            if error_policy == ErrorPolicy::FailFast {
                                abort.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            (idx, ItemStatus::Failed, None, Some(e))
                        }
                    },
                }
            });
            join_handles.push(handle);
        }

        // Await all spawned tasks and apply results.
        for handle in join_handles {
            if let Ok((idx, status, result, error)) = handle.await {
                if let Some(item) = batch.items.get_mut(idx) {
                    item.status = status;
                    item.result = result;
                    item.error = error;
                }
            }
        }

        // If FailFast was triggered, mark remaining processing items as skipped.
        if error_policy == ErrorPolicy::FailFast
            && abort_flag.load(std::sync::atomic::Ordering::Relaxed)
        {
            for item in &mut batch.items {
                if item.status == ItemStatus::Processing {
                    item.status = ItemStatus::Skipped;
                }
            }
        }
    }
}
