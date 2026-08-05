//! Batch collector — accumulates items and flushes them as complete batches.
//!
//! The [`BatchCollector`] buffers incoming items and produces a [`Batch`] when
//! either `max_size` items have been accumulated **or** `max_wait` has elapsed
//! since the first item was added.

use std::sync::Arc;

use chrono::Utc;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio::time::{self, Instant};
use tracing::{debug, info};

use crate::batch::{Batch, BatchStatus};
use crate::config::BatchConfig;
use crate::error::BatchError;
use crate::item::BatchItem;

/// A collector that accumulates items and flushes them as batches.
///
/// The collector uses an internal buffer protected by a mutex. Callers add
/// items via [`submit`](Self::submit). Batches are produced when:
///
/// 1. The buffer reaches `max_size`, **or**
/// 2. `max_wait` elapses since the first item was added.
///
/// Use [`start_timer`](Self::start_timer) to spawn the background timer task, and
/// consume flushed batches from the returned receiver.
pub struct BatchCollector<T> {
    config: BatchConfig,
    buffer: Arc<Mutex<CollectorBuffer<T>>>,
    tx: mpsc::Sender<Batch<T>>,
    shut_down: Arc<std::sync::atomic::AtomicBool>,
}

struct CollectorBuffer<T> {
    items: Vec<BatchItem<T>>,
    first_item_at: Option<Instant>,
}

impl<T> CollectorBuffer<T> {
    fn new() -> Self {
        Self {
            items: Vec::new(),
            first_item_at: None,
        }
    }

    fn drain(&mut self) -> Vec<BatchItem<T>> {
        self.first_item_at = None;
        std::mem::take(&mut self.items)
    }

    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl<T: Send + 'static> BatchCollector<T> {
    /// Create a new collector with the given config.
    ///
    /// Returns the collector and a receiver for flushed batches.
    #[must_use]
    pub fn new(config: BatchConfig) -> (Self, mpsc::Receiver<Batch<T>>) {
        let (tx, rx) = mpsc::channel(32);
        let collector = Self {
            config,
            buffer: Arc::new(Mutex::new(CollectorBuffer::new())),
            tx,
            shut_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        (collector, rx)
    }

    /// Submit a single payload into the collector.
    ///
    /// If this payload causes the buffer to reach `max_size`, the buffer is
    /// flushed immediately.
    pub async fn submit(&self, payload: T) -> Result<(), BatchError> {
        if self.shut_down.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(BatchError::CollectorShutDown);
        }

        let should_flush = {
            let mut buf = self.buffer.lock();

            if buf.first_item_at.is_none() {
                buf.first_item_at = Some(Instant::now());
            }

            buf.items.push(BatchItem::new(payload));

            self.config.max_size > 0 && buf.items.len() >= self.config.max_size
        };

        if should_flush {
            self.flush().await?;
        }

        Ok(())
    }

    /// Manually flush the current buffer contents as a batch.
    pub async fn flush(&self) -> Result<(), BatchError> {
        let items = {
            let mut buf = self.buffer.lock();
            if buf.is_empty() {
                return Ok(());
            }
            buf.drain()
        };

        let item_count = items.len();
        let mut batch = Batch {
            id: crate::batch::BatchId::new(),
            items,
            status: BatchStatus::Collecting,
            created_at: Utc::now(),
            config: self.config.clone(),
        };
        batch.status = BatchStatus::Collecting;

        debug!(batch_id = %batch.id, item_count, "flushing batch");

        self.tx
            .send(batch)
            .await
            .map_err(|_| BatchError::CollectorShutDown)?;

        Ok(())
    }

    /// Start the background timer task that flushes on `max_wait`.
    ///
    /// The timer checks periodically whether `max_wait` has elapsed since the
    /// first item was added to the buffer. If so, it flushes.
    ///
    /// Returns a `JoinHandle` for the timer task.
    pub fn start_timer(&self) -> tokio::task::JoinHandle<()>
    where
        T: Send + 'static,
    {
        let buffer = Arc::clone(&self.buffer);
        let tx = self.tx.clone();
        let max_wait = self.config.max_wait;
        let config = self.config.clone();
        let shut_down = Arc::clone(&self.shut_down);

        tokio::spawn(async move {
            let check_interval = max_wait / 4;
            let check_interval = if check_interval.is_zero() {
                std::time::Duration::from_millis(10)
            } else {
                check_interval
            };

            loop {
                time::sleep(check_interval).await;

                if shut_down.load(std::sync::atomic::Ordering::Relaxed) {
                    // Final flush on shutdown.
                    let items = {
                        let mut buf = buffer.lock();
                        if buf.is_empty() {
                            break;
                        }
                        buf.drain()
                    };

                    let batch = Batch {
                        id: crate::batch::BatchId::new(),
                        items,
                        status: BatchStatus::Collecting,
                        created_at: Utc::now(),
                        config: config.clone(),
                    };

                    let _ = tx.send(batch).await;
                    break;
                }

                let should_flush = {
                    let buf = buffer.lock();
                    if buf.is_empty() {
                        false
                    } else if let Some(first) = buf.first_item_at {
                        first.elapsed() >= max_wait
                    } else {
                        false
                    }
                };

                if should_flush {
                    let items = {
                        let mut buf = buffer.lock();
                        if buf.is_empty() {
                            continue;
                        }
                        buf.drain()
                    };

                    let item_count = items.len();
                    let batch = Batch {
                        id: crate::batch::BatchId::new(),
                        items,
                        status: BatchStatus::Collecting,
                        created_at: Utc::now(),
                        config: config.clone(),
                    };

                    info!(batch_id = %batch.id, item_count, "timer-triggered flush");

                    if tx.send(batch).await.is_err() {
                        break;
                    }
                }
            }
        })
    }

    /// Shut down the collector. No further items will be accepted.
    pub fn shutdown(&self) {
        self.shut_down
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Returns `true` if the collector has been shut down.
    #[must_use]
    pub fn is_shut_down(&self) -> bool {
        self.shut_down.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Returns the current number of buffered items.
    #[must_use]
    pub fn buffered_count(&self) -> usize {
        self.buffer.lock().items.len()
    }
}
