//! Feed storage trait.
//!
//! [`FeedStore`] is the persistence abstraction used by [`crate::processor`].
//! All methods are async to support both in-memory and durable backends without
//! changing the calling code.

use async_trait::async_trait;

use crate::error::Result;
use crate::recipe::{Recipe, RecipeId};
use crate::trigger::{Trigger, TriggerId};
use crate::types::{Cursor, Feed, FeedId, FeedItem};

/// Persistence abstraction for feeds, triggers, recipes, and feed items.
///
/// Implementations must be `Send + Sync` so they can be shared across async
/// tasks.
#[async_trait]
pub trait FeedStore: Send + Sync {
    // ------------------------------------------------------------------
    // Feed CRUD
    // ------------------------------------------------------------------

    /// Persist a new feed and return it.
    async fn create_feed(&self, feed: Feed) -> Result<Feed>;

    /// Retrieve a feed by its id.
    async fn get_feed(&self, id: &FeedId) -> Result<Feed>;

    /// List all feeds, optionally filtered to a specific agent.
    async fn list_feeds(&self) -> Result<Vec<Feed>>;

    /// Atomically advance the cursor for a feed.
    ///
    /// Implementations must update only the cursor, leaving all other fields
    /// unchanged.
    async fn update_cursor(&self, feed_id: &FeedId, cursor: Cursor) -> Result<()>;

    // ------------------------------------------------------------------
    // Trigger CRUD
    // ------------------------------------------------------------------

    /// Persist a new trigger.
    async fn create_trigger(&self, trigger: Trigger) -> Result<Trigger>;

    /// Retrieve a trigger by its id.
    async fn get_trigger(&self, id: &TriggerId) -> Result<Trigger>;

    /// List all triggers registered for the given feed.
    async fn list_triggers(&self, feed_id: &FeedId) -> Result<Vec<Trigger>>;

    /// Replace a trigger's stored state (used to update `last_fired_at`).
    async fn update_trigger(&self, trigger: Trigger) -> Result<Trigger>;

    // ------------------------------------------------------------------
    // Recipe CRUD
    // ------------------------------------------------------------------

    /// Persist a new recipe.
    async fn create_recipe(&self, recipe: Recipe) -> Result<Recipe>;

    /// Retrieve a recipe by its id.
    async fn get_recipe(&self, id: &RecipeId) -> Result<Recipe>;

    /// List all available recipes.
    async fn list_recipes(&self) -> Result<Vec<Recipe>>;

    // ------------------------------------------------------------------
    // Feed item queue
    // ------------------------------------------------------------------

    /// Add an item to the feed's processing queue.
    async fn enqueue_item(&self, item: FeedItem) -> Result<FeedItem>;

    /// Dequeue up to `limit` unprocessed items for the given feed, in FIFO
    /// order.
    async fn dequeue_items(&self, feed_id: &FeedId, limit: usize) -> Result<Vec<FeedItem>>;

    /// Mark an item as processed (idempotent).
    async fn mark_processed(&self, item_id: uuid::Uuid) -> Result<()>;
}
