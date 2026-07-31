//! Feed processing engine.
//!
//! [`FeedProcessor`] drives the core processing loop: it fetches items from the
//! store, evaluates every registered trigger, and returns the set of
//! [`TriggerResult`]s.  Cursor advancement is separated from item processing so
//! that a crash between the two produces at-least-once (not at-most-once)
//! redelivery semantics.

use std::sync::Arc;

use crate::error::Result;
use crate::store::FeedStore;
use crate::trigger::{evaluate_trigger, TriggerResult};
use crate::types::{Cursor, Feed, FeedId, FeedItem};

/// The feed processing engine.
///
/// `FeedProcessor` holds a reference to a [`FeedStore`] backend and provides
/// methods for processing individual items and advancing the durable cursor.
///
/// # At-least-once delivery guarantee
///
/// ```text
/// ┌──────────────────────────────────────────────────────────────────┐
/// │  1. dequeue_items()  →  items land in memory                     │
/// │  2. process_item()   →  triggers evaluated; side-effects queued  │
/// │  3. mark_processed() →  item flagged in store                    │
/// │  4. advance_cursor() →  cursor moves forward                     │
/// └──────────────────────────────────────────────────────────────────┘
/// ```
///
/// If the process crashes after step 2 but before step 4, the item will be
/// re-fetched on the next poll (cursor still points before it).  The trigger
/// results are therefore delivered **at least once**.  Callers that need
/// exactly-once semantics must apply their own deduplication on the action side.
pub struct FeedProcessor {
    store: Arc<dyn FeedStore>,
}

impl FeedProcessor {
    /// Create a new processor backed by the given store.
    #[must_use]
    pub fn new(store: Arc<dyn FeedStore>) -> Self {
        Self { store }
    }

    /// Evaluate all triggers registered for `feed` against a single `item`.
    ///
    /// Returns the list of [`TriggerResult`]s — one per registered trigger.
    /// The caller is responsible for acting on [`TriggerResult::Fire`] entries.
    ///
    /// # Idempotency
    ///
    /// Calling this method twice with the same `(feed, item)` pair produces the
    /// same results both times (assuming trigger state — specifically
    /// `last_fired_at` — has not changed between calls).
    pub async fn process_item(&self, feed: &Feed, item: &FeedItem) -> Result<Vec<TriggerResult>> {
        let triggers = self.store.list_triggers(&feed.id).await?;
        let results: Vec<TriggerResult> = triggers
            .iter()
            .map(|t| evaluate_trigger(t, item))
            .collect();
        Ok(results)
    }

    /// Advance the durable cursor for `feed_id` to `new_position`.
    ///
    /// This should be called **after** all trigger actions for an item have been
    /// dispatched (or at least queued durably).  Advancing the cursor before
    /// dispatch would risk losing the trigger fire if the process crashes.
    pub async fn advance_cursor(&self, feed_id: &FeedId, new_position: &str) -> Result<()> {
        // Read the current cursor so we can increment items_processed correctly.
        let feed = self.store.get_feed(feed_id).await?;
        let new_cursor = feed.cursor.advance(new_position);
        self.store.update_cursor(feed_id, new_cursor).await
    }

    /// Convenience helper: dequeue up to `batch_size` items, process each one
    /// against all triggers, mark each as processed, and return the combined
    /// results.
    ///
    /// The cursor is **not** advanced here — the caller must call
    /// [`Self::advance_cursor`] once it has durably committed the trigger
    /// outputs.
    pub async fn process_batch(
        &self,
        feed: &Feed,
        batch_size: usize,
    ) -> Result<Vec<(FeedItem, Vec<TriggerResult>)>> {
        let items = self.store.dequeue_items(&feed.id, batch_size).await?;
        let mut output = Vec::with_capacity(items.len());

        for item in items {
            let results = self.process_item(feed, &item).await?;
            self.store.mark_processed(item.id).await?;
            output.push((item, results));
        }

        Ok(output)
    }

    /// Return the underlying store handle (useful for tests that need to
    /// inspect stored state after processing).
    pub fn store(&self) -> &Arc<dyn FeedStore> {
        &self.store
    }
}

// ---------------------------------------------------------------------------
// Cursor helpers (re-exported for convenience)
// ---------------------------------------------------------------------------

/// Build a new cursor at the given position with zero items processed.
#[must_use]
pub fn initial_cursor(position: impl Into<String>) -> Cursor {
    Cursor::new(position)
}
