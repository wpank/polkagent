//! Durable feed store trait with cursor persistence, atomic transactions,
//! deduplication, and gap detection.
//!
//! This module implements the requirements from PRD-09 §7:
//!
//! - [`DurableFeedStore`] — extends [`FeedStore`] with cursor persistence,
//!   atomic advance + action creation, and deduplication.
//! - [`FeedCursor`] — a named cursor bookmark that can be saved and loaded
//!   independently of the [`Feed`] record.
//! - [`TriggerDedup`] — a deduplication record for trigger fires.
//! - [`Gap`] — a detected gap in a sequential chain feed.
//! - [`InMemoryDurableFeedStore`] — an in-memory implementation for testing.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::error::{FeedError, Result};
use crate::memory_store::MemoryStore;
use crate::recipe::{Recipe, RecipeId};
use crate::store::FeedStore;
use crate::trigger::{Trigger, TriggerAction, TriggerId};
use crate::types::{Cursor, Feed, FeedId, FeedItem};

// ---------------------------------------------------------------------------
// FeedCursor
// ---------------------------------------------------------------------------

/// A named, durable cursor bookmark for a specific feed.
///
/// Unlike the cursor embedded in a [`Feed`] record, a `FeedCursor` can be
/// saved and loaded independently, allowing external processes (e.g. a chain
/// indexer) to checkpoint their own position without touching feed metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedCursor {
    /// The feed this cursor belongs to.
    pub feed_id: FeedId,

    /// An opaque, source-specific string encoding the current read position.
    pub position: String,

    /// When this cursor was last saved.
    pub saved_at: DateTime<Utc>,
}

impl FeedCursor {
    /// Create a new cursor at the given position.
    #[must_use]
    pub fn new(feed_id: FeedId, position: impl Into<String>) -> Self {
        Self {
            feed_id,
            position: position.into(),
            saved_at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// TriggerDedup
// ---------------------------------------------------------------------------

/// A deduplication record for a trigger fire, preventing the same trigger
/// event from being processed more than once.
///
/// `dedup_key` is a caller-supplied opaque string (e.g. a hash of the item
/// id + trigger id) that uniquely identifies a potential trigger fire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerDedup {
    /// The unique key identifying this trigger/item combination.
    pub dedup_key: String,

    /// The feed that produced the triggering item.
    pub feed_id: FeedId,

    /// The trigger that fired.
    pub trigger_id: TriggerId,

    /// When this dedup record was created.
    pub created_at: DateTime<Utc>,
}

impl TriggerDedup {
    /// Create a new dedup record.
    #[must_use]
    pub fn new(dedup_key: impl Into<String>, feed_id: FeedId, trigger_id: TriggerId) -> Self {
        Self {
            dedup_key: dedup_key.into(),
            feed_id,
            trigger_id,
            created_at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// Gap
// ---------------------------------------------------------------------------

/// A detected gap in a sequential chain feed's item stream.
///
/// When a chain feed is expected to deliver items at contiguous sequence
/// numbers (e.g. block heights), a `Gap` records a range of missing items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gap {
    /// The feed where the gap was detected.
    pub feed_id: FeedId,

    /// The first sequence number that is missing.
    pub from_sequence: u64,

    /// The last sequence number in the missing range (inclusive).
    pub to_sequence: u64,

    /// When the gap was detected.
    pub detected_at: DateTime<Utc>,
}

impl Gap {
    /// Create a new gap record.
    #[must_use]
    pub fn new(feed_id: FeedId, from_sequence: u64, to_sequence: u64) -> Self {
        Self {
            feed_id,
            from_sequence,
            to_sequence,
            detected_at: Utc::now(),
        }
    }

    /// How many items are missing in this gap.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.to_sequence.saturating_sub(self.from_sequence) + 1
    }
}

// ---------------------------------------------------------------------------
// PendingAction
// ---------------------------------------------------------------------------

/// A trigger action that has been created atomically alongside a cursor
/// advance, but not yet dispatched.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingAction {
    /// Unique identifier for this pending action.
    pub id: Uuid,

    /// The feed that generated this action.
    pub feed_id: FeedId,

    /// The trigger that fired.
    pub trigger_id: TriggerId,

    /// The action to be dispatched.
    pub action: TriggerAction,

    /// The new cursor position that was atomically set when this action was
    /// created.
    pub cursor_position: String,

    /// When this action was created.
    pub created_at: DateTime<Utc>,
}

impl PendingAction {
    /// Create a new pending action.
    #[must_use]
    pub fn new(
        feed_id: FeedId,
        trigger_id: TriggerId,
        action: TriggerAction,
        cursor_position: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            feed_id,
            trigger_id,
            action,
            cursor_position: cursor_position.into(),
            created_at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// DurableFeedStore
// ---------------------------------------------------------------------------

/// Extension of [`FeedStore`] with cursor persistence, atomic transactions,
/// deduplication, and gap detection.
#[async_trait]
pub trait DurableFeedStore: FeedStore {
    // ------------------------------------------------------------------
    // Cursor persistence
    // ------------------------------------------------------------------

    /// Persist the cursor for the given feed, independently of the feed
    /// record itself.
    ///
    /// This allows external processes (e.g. a chain indexer) to checkpoint
    /// their position without touching feed metadata.
    async fn save_cursor(&self, feed_id: &FeedId, cursor: FeedCursor) -> Result<()>;

    /// Load the most recently saved cursor for the given feed.
    ///
    /// Returns `None` if no cursor has been saved yet.
    async fn load_cursor(&self, feed_id: &FeedId) -> Result<Option<FeedCursor>>;

    // ------------------------------------------------------------------
    // Atomic advance
    // ------------------------------------------------------------------

    /// Atomically advance the feed cursor to `new_cursor` and create a
    /// [`PendingAction`] for `action`.
    ///
    /// Both operations succeed or fail together: either the cursor is
    /// advanced and the action is persisted, or neither is.  This prevents
    /// the loss of trigger fires if the process crashes between advancing
    /// the cursor and persisting the action.
    ///
    /// Returns the created [`PendingAction`].
    async fn atomic_advance(
        &self,
        feed_id: &FeedId,
        new_cursor: FeedCursor,
        trigger_id: TriggerId,
        action: TriggerAction,
    ) -> Result<PendingAction>;

    /// List all pending actions for the given feed that have not yet been
    /// dispatched.
    async fn list_pending_actions(&self, feed_id: &FeedId) -> Result<Vec<PendingAction>>;

    /// Mark a pending action as dispatched (idempotent).
    async fn mark_action_dispatched(&self, action_id: Uuid) -> Result<()>;

    // ------------------------------------------------------------------
    // Deduplication
    // ------------------------------------------------------------------

    /// Record a deduplication entry for a trigger fire.
    ///
    /// If the `dedup_key` already exists, this is a no-op (idempotent).
    async fn record_dedup(&self, dedup: TriggerDedup) -> Result<()>;

    /// Check whether a trigger fire with the given `dedup_key` has already
    /// been recorded.
    ///
    /// Returns `true` if the key exists (duplicate), `false` otherwise.
    async fn check_dedup(&self, dedup_key: &str) -> Result<bool>;

    // ------------------------------------------------------------------
    // Gap detection
    // ------------------------------------------------------------------

    /// Detect gaps in a sequential chain feed's item stream.
    ///
    /// `expected_sequence` is the sequence number that the caller expects
    /// to see next.  The implementation compares the observed sequence
    /// numbers in the store against the expected sequence to identify any
    /// missing ranges.
    ///
    /// Returns a list of [`Gap`]s describing the missing ranges.
    async fn detect_gaps(&self, feed_id: &FeedId, expected_sequence: u64) -> Result<Vec<Gap>>;

    /// Record a sequence number as observed for the given feed.
    ///
    /// This is used by chain feed processors to register each block/event
    /// they process so that gap detection can compare against the observed set.
    async fn record_sequence(&self, feed_id: &FeedId, sequence: u64) -> Result<()>;
}

// ---------------------------------------------------------------------------
// InMemoryDurableFeedStore
// ---------------------------------------------------------------------------

/// Shared inner state for the in-memory durable store.
#[derive(Debug, Default)]
struct DurableInner {
    /// Named cursors keyed by feed id.
    cursors: HashMap<FeedId, FeedCursor>,

    /// Dedup records keyed by `dedup_key`.
    dedup_records: HashMap<String, TriggerDedup>,

    /// Observed sequence numbers per feed.
    sequences: HashMap<FeedId, Vec<u64>>,

    /// Pending actions that have not yet been dispatched.
    pending_actions: Vec<PendingAction>,

    /// IDs of actions that have been marked dispatched.
    dispatched_action_ids: std::collections::HashSet<Uuid>,
}

/// An in-memory [`DurableFeedStore`] suitable for unit tests.
///
/// Wraps a [`MemoryStore`] for the base [`FeedStore`] operations and adds
/// durable cursor persistence, atomic advance, deduplication, and gap
/// detection — all in memory.
#[derive(Debug, Clone, Default)]
pub struct InMemoryDurableFeedStore {
    /// Underlying base store for feeds, triggers, recipes, and items.
    base: MemoryStore,

    /// Additional durable state.
    durable: Arc<Mutex<DurableInner>>,
}

impl InMemoryDurableFeedStore {
    /// Create a new empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

// Delegate all FeedStore methods to the inner MemoryStore.
#[async_trait]
impl FeedStore for InMemoryDurableFeedStore {
    async fn create_feed(&self, feed: Feed) -> Result<Feed> {
        self.base.create_feed(feed).await
    }

    async fn get_feed(&self, id: &FeedId) -> Result<Feed> {
        self.base.get_feed(id).await
    }

    async fn list_feeds(&self) -> Result<Vec<Feed>> {
        self.base.list_feeds().await
    }

    async fn update_cursor(&self, feed_id: &FeedId, cursor: Cursor) -> Result<()> {
        self.base.update_cursor(feed_id, cursor).await
    }

    async fn create_trigger(&self, trigger: Trigger) -> Result<Trigger> {
        self.base.create_trigger(trigger).await
    }

    async fn get_trigger(&self, id: &TriggerId) -> Result<Trigger> {
        self.base.get_trigger(id).await
    }

    async fn list_triggers(&self, feed_id: &FeedId) -> Result<Vec<Trigger>> {
        self.base.list_triggers(feed_id).await
    }

    async fn update_trigger(&self, trigger: Trigger) -> Result<Trigger> {
        self.base.update_trigger(trigger).await
    }

    async fn create_recipe(&self, recipe: Recipe) -> Result<Recipe> {
        self.base.create_recipe(recipe).await
    }

    async fn get_recipe(&self, id: &RecipeId) -> Result<Recipe> {
        self.base.get_recipe(id).await
    }

    async fn list_recipes(&self) -> Result<Vec<Recipe>> {
        self.base.list_recipes().await
    }

    async fn enqueue_item(&self, item: FeedItem) -> Result<FeedItem> {
        self.base.enqueue_item(item).await
    }

    async fn dequeue_items(&self, feed_id: &FeedId, limit: usize) -> Result<Vec<FeedItem>> {
        self.base.dequeue_items(feed_id, limit).await
    }

    async fn mark_processed(&self, item_id: uuid::Uuid) -> Result<()> {
        self.base.mark_processed(item_id).await
    }
}

#[async_trait]
impl DurableFeedStore for InMemoryDurableFeedStore {
    // ------------------------------------------------------------------
    // Cursor persistence
    // ------------------------------------------------------------------

    async fn save_cursor(&self, feed_id: &FeedId, cursor: FeedCursor) -> Result<()> {
        // Validate the cursor belongs to the given feed.
        if &cursor.feed_id != feed_id {
            return Err(FeedError::ProcessingError(format!(
                "cursor feed_id {} does not match requested feed_id {}",
                cursor.feed_id, feed_id
            )));
        }
        let mut guard = self.durable.lock().await;
        guard.cursors.insert(*feed_id, cursor);
        Ok(())
    }

    async fn load_cursor(&self, feed_id: &FeedId) -> Result<Option<FeedCursor>> {
        let guard = self.durable.lock().await;
        Ok(guard.cursors.get(feed_id).cloned())
    }

    // ------------------------------------------------------------------
    // Atomic advance
    // ------------------------------------------------------------------

    async fn atomic_advance(
        &self,
        feed_id: &FeedId,
        new_cursor: FeedCursor,
        trigger_id: TriggerId,
        action: TriggerAction,
    ) -> Result<PendingAction> {
        // Both operations must succeed together. We hold the durable lock
        // for the entire operation so nothing can observe a partial state.
        let mut guard = self.durable.lock().await;

        // Validate cursor ownership.
        if &new_cursor.feed_id != feed_id {
            return Err(FeedError::ProcessingError(format!(
                "cursor feed_id {} does not match requested feed_id {}",
                new_cursor.feed_id, feed_id
            )));
        }

        let cursor_position = new_cursor.position.clone();

        // Save cursor.
        guard.cursors.insert(*feed_id, new_cursor);

        // Create the pending action.
        let pending = PendingAction::new(*feed_id, trigger_id, action, cursor_position);
        guard.pending_actions.push(pending.clone());

        Ok(pending)
    }

    async fn list_pending_actions(&self, feed_id: &FeedId) -> Result<Vec<PendingAction>> {
        let guard = self.durable.lock().await;
        let actions: Vec<PendingAction> = guard
            .pending_actions
            .iter()
            .filter(|a| &a.feed_id == feed_id && !guard.dispatched_action_ids.contains(&a.id))
            .cloned()
            .collect();
        Ok(actions)
    }

    async fn mark_action_dispatched(&self, action_id: Uuid) -> Result<()> {
        let mut guard = self.durable.lock().await;
        guard.dispatched_action_ids.insert(action_id);
        Ok(())
    }

    // ------------------------------------------------------------------
    // Deduplication
    // ------------------------------------------------------------------

    async fn record_dedup(&self, dedup: TriggerDedup) -> Result<()> {
        let mut guard = self.durable.lock().await;
        // Idempotent: if key already exists, do nothing.
        guard
            .dedup_records
            .entry(dedup.dedup_key.clone())
            .or_insert(dedup);
        Ok(())
    }

    async fn check_dedup(&self, dedup_key: &str) -> Result<bool> {
        let guard = self.durable.lock().await;
        Ok(guard.dedup_records.contains_key(dedup_key))
    }

    // ------------------------------------------------------------------
    // Gap detection
    // ------------------------------------------------------------------

    async fn detect_gaps(&self, feed_id: &FeedId, expected_sequence: u64) -> Result<Vec<Gap>> {
        let guard = self.durable.lock().await;
        let sequences = match guard.sequences.get(feed_id) {
            Some(s) => s.clone(),
            None => return Ok(Vec::new()),
        };
        drop(guard);

        if sequences.is_empty() {
            return Ok(Vec::new());
        }

        let mut sorted = sequences;
        sorted.sort_unstable();
        sorted.dedup();

        let Some(max_observed) = sorted.last().copied() else {
            return Ok(Vec::new());
        };
        if max_observed < expected_sequence {
            // No observed sequences reach the expected position yet.
            return Ok(Vec::new());
        }

        let mut gaps = Vec::new();
        let mut current = expected_sequence;

        for &seq in &sorted {
            if seq < expected_sequence {
                // Already before our window — skip.
                continue;
            }
            if seq > current {
                // Gap from `current` to `seq - 1`.
                gaps.push(Gap::new(*feed_id, current, seq - 1));
            }
            current = seq.saturating_add(1);
        }

        Ok(gaps)
    }

    async fn record_sequence(&self, feed_id: &FeedId, sequence: u64) -> Result<()> {
        let mut guard = self.durable.lock().await;
        guard.sequences.entry(*feed_id).or_default().push(sequence);
        Ok(())
    }
}
