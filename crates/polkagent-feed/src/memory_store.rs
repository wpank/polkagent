//! In-memory [`FeedStore`] implementation for use in tests and development.
//!
//! All data is held in `Arc<Mutex<…>>` structures so the store can be shared
//! across async tasks.  There is no persistence: all state is lost when the
//! store is dropped.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::error::{FeedError, Result};
use crate::recipe::{Recipe, RecipeId};
use crate::store::FeedStore;
use crate::trigger::{Trigger, TriggerId};
use crate::types::{Cursor, Feed, FeedId, FeedItem};

// ---------------------------------------------------------------------------
// MemoryStore
// ---------------------------------------------------------------------------

/// Shared inner state, protected by a single async mutex.
#[derive(Debug, Default)]
struct Inner {
    feeds: HashMap<FeedId, Feed>,
    triggers: HashMap<TriggerId, Trigger>,
    recipes: HashMap<RecipeId, Recipe>,
    /// Items ordered by insertion time (FIFO).
    items: Vec<FeedItem>,
}

/// An in-memory [`FeedStore`] suitable for unit tests.
#[derive(Debug, Clone, Default)]
pub struct MemoryStore {
    inner: Arc<Mutex<Inner>>,
}

impl MemoryStore {
    /// Create a new, empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl FeedStore for MemoryStore {
    // ------------------------------------------------------------------
    // Feed CRUD
    // ------------------------------------------------------------------

    async fn create_feed(&self, feed: Feed) -> Result<Feed> {
        let mut guard = self.inner.lock().await;
        guard.feeds.insert(feed.id, feed.clone());
        Ok(feed)
    }

    async fn get_feed(&self, id: &FeedId) -> Result<Feed> {
        let guard = self.inner.lock().await;
        guard
            .feeds
            .get(id)
            .cloned()
            .ok_or_else(|| FeedError::NotFound(format!("feed {id}")))
    }

    async fn list_feeds(&self) -> Result<Vec<Feed>> {
        let guard = self.inner.lock().await;
        Ok(guard.feeds.values().cloned().collect())
    }

    async fn update_cursor(&self, feed_id: &FeedId, cursor: Cursor) -> Result<()> {
        let mut guard = self.inner.lock().await;
        let feed = guard
            .feeds
            .get_mut(feed_id)
            .ok_or_else(|| FeedError::NotFound(format!("feed {feed_id}")))?;
        feed.cursor = cursor;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Trigger CRUD
    // ------------------------------------------------------------------

    async fn create_trigger(&self, trigger: Trigger) -> Result<Trigger> {
        let mut guard = self.inner.lock().await;
        guard.triggers.insert(trigger.id, trigger.clone());
        Ok(trigger)
    }

    async fn get_trigger(&self, id: &TriggerId) -> Result<Trigger> {
        let guard = self.inner.lock().await;
        guard
            .triggers
            .get(id)
            .cloned()
            .ok_or_else(|| FeedError::NotFound(format!("trigger {id}")))
    }

    async fn list_triggers(&self, feed_id: &FeedId) -> Result<Vec<Trigger>> {
        let guard = self.inner.lock().await;
        Ok(guard
            .triggers
            .values()
            .filter(|t| &t.feed_id == feed_id)
            .cloned()
            .collect())
    }

    async fn update_trigger(&self, trigger: Trigger) -> Result<Trigger> {
        let mut guard = self.inner.lock().await;
        if !guard.triggers.contains_key(&trigger.id) {
            return Err(FeedError::NotFound(format!("trigger {}", trigger.id)));
        }
        guard.triggers.insert(trigger.id, trigger.clone());
        Ok(trigger)
    }

    // ------------------------------------------------------------------
    // Recipe CRUD
    // ------------------------------------------------------------------

    async fn create_recipe(&self, recipe: Recipe) -> Result<Recipe> {
        let mut guard = self.inner.lock().await;
        guard.recipes.insert(recipe.id, recipe.clone());
        Ok(recipe)
    }

    async fn get_recipe(&self, id: &RecipeId) -> Result<Recipe> {
        let guard = self.inner.lock().await;
        guard
            .recipes
            .get(id)
            .cloned()
            .ok_or_else(|| FeedError::NotFound(format!("recipe {id}")))
    }

    async fn list_recipes(&self) -> Result<Vec<Recipe>> {
        let guard = self.inner.lock().await;
        Ok(guard.recipes.values().cloned().collect())
    }

    // ------------------------------------------------------------------
    // Feed item queue
    // ------------------------------------------------------------------

    async fn enqueue_item(&self, item: FeedItem) -> Result<FeedItem> {
        let mut guard = self.inner.lock().await;
        guard.items.push(item.clone());
        Ok(item)
    }

    async fn dequeue_items(&self, feed_id: &FeedId, limit: usize) -> Result<Vec<FeedItem>> {
        let guard = self.inner.lock().await;
        let items: Vec<FeedItem> = guard
            .items
            .iter()
            .filter(|i| &i.feed_id == feed_id && !i.processed)
            .take(limit)
            .cloned()
            .collect();
        Ok(items)
    }

    async fn mark_processed(&self, item_id: uuid::Uuid) -> Result<()> {
        let mut guard = self.inner.lock().await;
        for item in guard.items.iter_mut() {
            if item.id == item_id {
                item.processed = true;
                return Ok(());
            }
        }
        // Idempotent: if the item is already gone or never existed, succeed.
        Ok(())
    }
}
