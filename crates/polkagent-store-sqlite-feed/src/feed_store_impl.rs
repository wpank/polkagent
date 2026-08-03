//! [`FeedStore`] implementation backed by [`SqlitePool`].
//!
//! [`SqliteFeedStore`] is a newtype wrapper around [`SqlitePool`] that lets us
//! implement the foreign [`FeedStore`] trait without violating the orphan rule.
//!
//! All 14 trait methods are implemented by wrapping synchronous `rusqlite`
//! calls inside [`tokio::task::spawn_blocking`].  The writer connection is
//! acquired briefly via `SqlitePool::writer()` (a `parking_lot::Mutex` guard).
//!
//! ## Serialisation conventions
//!
//! - Enum/struct fields that are not directly representable as a TEXT/INTEGER
//!   column are stored as `_json TEXT` columns (e.g. `source_json`,
//!   `condition_json`).
//! - Timestamps are ISO-8601 / RFC 3339 strings.
//! - Booleans are stored as `INTEGER` (0/1).
//! - [`FeedStatus`] is stored as a plain string: `"active"`, `"paused"`, or
//!   `"error:<message>"`.

use async_trait::async_trait;
use uuid::Uuid;

use polkagent_feed::store::FeedStore;
use polkagent_feed::types::{Cursor, Feed, FeedId, FeedItem, FeedSource, FeedStatus};
use polkagent_feed::trigger::{Trigger, TriggerAction, TriggerCondition, TriggerId};
use polkagent_feed::recipe::{Recipe, RecipeId, RecipeParameter};
use polkagent_feed::{FeedError, Result};
use polkagent_store_sqlite::SqlitePool;

use crate::error::{map_json, map_sqlite, map_uuid, parse_ts};

// ---------------------------------------------------------------------------
// FeedStatus <-> TEXT encoding
// ---------------------------------------------------------------------------

fn encode_status(status: &FeedStatus) -> String {
    match status {
        FeedStatus::Active => "active".to_string(),
        FeedStatus::Paused => "paused".to_string(),
        FeedStatus::Error(msg) => format!("error:{msg}"),
    }
}

fn decode_status(s: &str) -> FeedStatus {
    match s {
        "active" => FeedStatus::Active,
        "paused" => FeedStatus::Paused,
        other => {
            if let Some(msg) = other.strip_prefix("error:") {
                FeedStatus::Error(msg.to_string())
            } else {
                // Fallback: treat unknown strings as an error status.
                FeedStatus::Error(format!("unknown status: {other}"))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Newtype wrapper (orphan-rule workaround)
// ---------------------------------------------------------------------------

/// A thin wrapper around [`SqlitePool`] that implements [`FeedStore`].
///
/// Because both `FeedStore` and `SqlitePool` are defined in external crates,
/// Rust's orphan rule forbids implementing the trait directly on the pool.
/// `SqliteFeedStore` lives in *this* crate, so the impl is allowed.
///
/// Use [`SqliteFeedStore::new`] or the `From` / `Into` conversions to wrap an
/// existing pool.
#[derive(Debug, Clone)]
pub struct SqliteFeedStore(pub SqlitePool);

impl SqliteFeedStore {
    /// Wrap an existing [`SqlitePool`] as a [`FeedStore`] implementor.
    pub fn new(pool: SqlitePool) -> Self {
        Self(pool)
    }

    /// Return a reference to the underlying [`SqlitePool`].
    pub fn inner(&self) -> &SqlitePool {
        &self.0
    }

    /// Consume the wrapper and return the underlying [`SqlitePool`].
    pub fn into_inner(self) -> SqlitePool {
        self.0
    }
}

impl From<SqlitePool> for SqliteFeedStore {
    fn from(pool: SqlitePool) -> Self {
        Self(pool)
    }
}

// ---------------------------------------------------------------------------
// FeedStore implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl FeedStore for SqliteFeedStore {
    // ------------------------------------------------------------------
    // Feed CRUD
    // ------------------------------------------------------------------

    async fn create_feed(&self, feed: Feed) -> Result<Feed> {
        let pool = self.0.clone();
        let id_str = feed.id.to_string();
        let source_json = serde_json::to_string(&feed.source).map_err(map_json)?;
        let status_str = encode_status(&feed.status);
        let agent_id_str = feed.agent_id.to_string();
        let cursor_position = feed.cursor.position.clone();
        let cursor_last_processed_at = feed.cursor.last_processed_at.to_rfc3339();
        let cursor_items_processed = feed.cursor.items_processed as i64;
        let created_at = feed.created_at.to_rfc3339();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO feeds
                         (id, name, source_json, cursor_position, cursor_last_processed_at,
                          cursor_items_processed, status, agent_id, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        id_str,
                        feed.name,
                        source_json,
                        cursor_position,
                        cursor_last_processed_at,
                        cursor_items_processed,
                        status_str,
                        agent_id_str,
                        created_at,
                    ],
                )
                .map_err(map_sqlite)?;
            Ok(feed)
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    async fn get_feed(&self, id: &FeedId) -> Result<Feed> {
        let pool = self.0.clone();
        let id_str = id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let row = writer
                .query_row(
                    "SELECT id, name, source_json, cursor_position,
                            cursor_last_processed_at, cursor_items_processed,
                            status, agent_id, created_at
                     FROM feeds WHERE id = ?1",
                    [&id_str],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, i64>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, String>(7)?,
                            row.get::<_, String>(8)?,
                        ))
                    },
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        FeedError::NotFound(format!("feed {id_str}"))
                    }
                    other => map_sqlite(other),
                })?;

            decode_feed_row(row)
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    async fn list_feeds(&self) -> Result<Vec<Feed>> {
        let pool = self.0.clone();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let mut stmt = writer
                .prepare(
                    "SELECT id, name, source_json, cursor_position,
                            cursor_last_processed_at, cursor_items_processed,
                            status, agent_id, created_at
                     FROM feeds
                     ORDER BY created_at ASC",
                )
                .map_err(map_sqlite)?;

            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                })
                .map_err(map_sqlite)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;

            rows.into_iter().map(decode_feed_row).collect()
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    async fn update_cursor(&self, feed_id: &FeedId, cursor: Cursor) -> Result<()> {
        let pool = self.0.clone();
        let id_str = feed_id.to_string();
        let last_processed_at = cursor.last_processed_at.to_rfc3339();
        let items_processed = cursor.items_processed as i64;

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let n = writer
                .execute(
                    "UPDATE feeds
                     SET cursor_position = ?1,
                         cursor_last_processed_at = ?2,
                         cursor_items_processed = ?3
                     WHERE id = ?4",
                    rusqlite::params![
                        cursor.position,
                        last_processed_at,
                        items_processed,
                        id_str,
                    ],
                )
                .map_err(map_sqlite)?;
            if n == 0 {
                return Err(FeedError::NotFound(format!("feed {id_str}")));
            }
            Ok(())
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    // ------------------------------------------------------------------
    // Trigger CRUD
    // ------------------------------------------------------------------

    async fn create_trigger(&self, trigger: Trigger) -> Result<Trigger> {
        let pool = self.0.clone();
        let id_str = trigger.id.to_string();
        let feed_id_str = trigger.feed_id.to_string();
        let condition_json = serde_json::to_string(&trigger.condition).map_err(map_json)?;
        let action_json = serde_json::to_string(&trigger.action).map_err(map_json)?;
        let cooldown_secs = trigger.cooldown_secs.map(|s| s as i64);
        let last_fired_at = trigger.last_fired_at.map(|dt| dt.to_rfc3339());
        let enabled = i32::from(trigger.enabled);

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO feed_triggers
                         (id, name, feed_id, condition_json, action_json,
                          cooldown_secs, last_fired_at, enabled)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        id_str,
                        trigger.name,
                        feed_id_str,
                        condition_json,
                        action_json,
                        cooldown_secs,
                        last_fired_at,
                        enabled,
                    ],
                )
                .map_err(map_sqlite)?;
            Ok(trigger)
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    async fn get_trigger(&self, id: &TriggerId) -> Result<Trigger> {
        let pool = self.0.clone();
        let id_str = id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let row = writer
                .query_row(
                    "SELECT id, name, feed_id, condition_json, action_json,
                            cooldown_secs, last_fired_at, enabled
                     FROM feed_triggers WHERE id = ?1",
                    [&id_str],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, Option<i64>>(5)?,
                            row.get::<_, Option<String>>(6)?,
                            row.get::<_, i32>(7)?,
                        ))
                    },
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        FeedError::NotFound(format!("trigger {id_str}"))
                    }
                    other => map_sqlite(other),
                })?;

            decode_trigger_row(row)
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    async fn list_triggers(&self, feed_id: &FeedId) -> Result<Vec<Trigger>> {
        let pool = self.0.clone();
        let feed_id_str = feed_id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let mut stmt = writer
                .prepare(
                    "SELECT id, name, feed_id, condition_json, action_json,
                            cooldown_secs, last_fired_at, enabled
                     FROM feed_triggers
                     WHERE feed_id = ?1
                     ORDER BY id ASC",
                )
                .map_err(map_sqlite)?;

            let rows = stmt
                .query_map([&feed_id_str], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, i32>(7)?,
                    ))
                })
                .map_err(map_sqlite)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;

            rows.into_iter().map(decode_trigger_row).collect()
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    async fn update_trigger(&self, trigger: Trigger) -> Result<Trigger> {
        let pool = self.0.clone();
        let id_str = trigger.id.to_string();
        let feed_id_str = trigger.feed_id.to_string();
        let condition_json = serde_json::to_string(&trigger.condition).map_err(map_json)?;
        let action_json = serde_json::to_string(&trigger.action).map_err(map_json)?;
        let cooldown_secs = trigger.cooldown_secs.map(|s| s as i64);
        let last_fired_at = trigger.last_fired_at.map(|dt| dt.to_rfc3339());
        let enabled = i32::from(trigger.enabled);

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let n = writer
                .execute(
                    "UPDATE feed_triggers
                     SET name = ?1, feed_id = ?2, condition_json = ?3,
                         action_json = ?4, cooldown_secs = ?5,
                         last_fired_at = ?6, enabled = ?7
                     WHERE id = ?8",
                    rusqlite::params![
                        trigger.name,
                        feed_id_str,
                        condition_json,
                        action_json,
                        cooldown_secs,
                        last_fired_at,
                        enabled,
                        id_str,
                    ],
                )
                .map_err(map_sqlite)?;
            if n == 0 {
                return Err(FeedError::NotFound(format!("trigger {id_str}")));
            }
            Ok(trigger)
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    // ------------------------------------------------------------------
    // Recipe CRUD
    // ------------------------------------------------------------------

    async fn create_recipe(&self, recipe: Recipe) -> Result<Recipe> {
        let pool = self.0.clone();
        let id_str = recipe.id.to_string();
        let source_json = serde_json::to_string(&recipe.source).map_err(map_json)?;
        let trigger_json = serde_json::to_string(&recipe.trigger).map_err(map_json)?;
        let action_json = serde_json::to_string(&recipe.action).map_err(map_json)?;
        let parameters_json = serde_json::to_string(&recipe.parameters).map_err(map_json)?;

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO feed_recipes
                         (id, name, description, version, source_json,
                          trigger_json, action_json, parameters_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        id_str,
                        recipe.name,
                        recipe.description,
                        recipe.version,
                        source_json,
                        trigger_json,
                        action_json,
                        parameters_json,
                    ],
                )
                .map_err(map_sqlite)?;
            Ok(recipe)
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    async fn get_recipe(&self, id: &RecipeId) -> Result<Recipe> {
        let pool = self.0.clone();
        let id_str = id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let row = writer
                .query_row(
                    "SELECT id, name, description, version, source_json,
                            trigger_json, action_json, parameters_json
                     FROM feed_recipes WHERE id = ?1",
                    [&id_str],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, String>(7)?,
                        ))
                    },
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        FeedError::NotFound(format!("recipe {id_str}"))
                    }
                    other => map_sqlite(other),
                })?;

            decode_recipe_row(row)
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    async fn list_recipes(&self) -> Result<Vec<Recipe>> {
        let pool = self.0.clone();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let mut stmt = writer
                .prepare(
                    "SELECT id, name, description, version, source_json,
                            trigger_json, action_json, parameters_json
                     FROM feed_recipes
                     ORDER BY name ASC",
                )
                .map_err(map_sqlite)?;

            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                })
                .map_err(map_sqlite)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;

            rows.into_iter().map(decode_recipe_row).collect()
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    // ------------------------------------------------------------------
    // Feed item queue
    // ------------------------------------------------------------------

    async fn enqueue_item(&self, item: FeedItem) -> Result<FeedItem> {
        let pool = self.0.clone();
        let id_str = item.id.to_string();
        let feed_id_str = item.feed_id.to_string();
        let payload_json = serde_json::to_string(&item.payload).map_err(map_json)?;
        let received_at = item.received_at.to_rfc3339();
        let processed = i32::from(item.processed);

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO feed_items
                         (id, feed_id, payload_json, received_at, processed)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        id_str,
                        feed_id_str,
                        payload_json,
                        received_at,
                        processed,
                    ],
                )
                .map_err(map_sqlite)?;
            Ok(item)
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    async fn dequeue_items(&self, feed_id: &FeedId, limit: usize) -> Result<Vec<FeedItem>> {
        let pool = self.0.clone();
        let feed_id_str = feed_id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let mut stmt = writer
                .prepare(
                    "SELECT id, feed_id, payload_json, received_at, processed
                     FROM feed_items
                     WHERE feed_id = ?1 AND processed = 0
                     ORDER BY received_at ASC
                     LIMIT ?2",
                )
                .map_err(map_sqlite)?;

            let rows = stmt
                .query_map(
                    rusqlite::params![feed_id_str, limit as i64],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i32>(4)?,
                        ))
                    },
                )
                .map_err(map_sqlite)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;

            rows.into_iter().map(decode_item_row).collect()
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    async fn mark_processed(&self, item_id: Uuid) -> Result<()> {
        let pool = self.0.clone();
        let id_str = item_id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            // Idempotent: we do not error if the item does not exist or is
            // already processed, matching the MemoryStore behaviour.
            writer
                .execute(
                    "UPDATE feed_items SET processed = 1 WHERE id = ?1",
                    [&id_str],
                )
                .map_err(map_sqlite)?;
            Ok(())
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }
}

// ---------------------------------------------------------------------------
// Row decoders
// ---------------------------------------------------------------------------

type FeedRow = (String, String, String, String, String, i64, String, String, String);

fn decode_feed_row(row: FeedRow) -> Result<Feed> {
    let (id_s, name, source_json_s, cursor_pos, cursor_ts_s, cursor_items, status_s, agent_id_s, created_at_s) = row;

    let id = id_s.parse::<Uuid>().map_err(map_uuid)?;
    let source: FeedSource = serde_json::from_str(&source_json_s).map_err(map_json)?;
    let cursor_last_processed_at = parse_ts(&cursor_ts_s)?;
    let agent_id = agent_id_s.parse::<Uuid>().map_err(map_uuid)?;
    let created_at = parse_ts(&created_at_s)?;

    Ok(Feed {
        id: FeedId::from_uuid(id),
        name,
        source,
        cursor: Cursor {
            position: cursor_pos,
            last_processed_at: cursor_last_processed_at,
            items_processed: cursor_items as u64,
        },
        status: decode_status(&status_s),
        agent_id: polkagent_core::AgentId::from(agent_id),
        created_at,
    })
}

type TriggerRow = (String, String, String, String, String, Option<i64>, Option<String>, i32);

fn decode_trigger_row(row: TriggerRow) -> Result<Trigger> {
    let (id_s, name, feed_id_s, cond_json_s, action_json_s, cooldown, last_fired_s, enabled_i) = row;

    let id = id_s.parse::<Uuid>().map_err(map_uuid)?;
    let feed_id = feed_id_s.parse::<Uuid>().map_err(map_uuid)?;
    let condition: TriggerCondition = serde_json::from_str(&cond_json_s).map_err(map_json)?;
    let action: TriggerAction = serde_json::from_str(&action_json_s).map_err(map_json)?;
    let last_fired_at = last_fired_s
        .as_deref()
        .map(parse_ts)
        .transpose()?;

    Ok(Trigger {
        id: TriggerId::from_uuid(id),
        name,
        feed_id: FeedId::from_uuid(feed_id),
        condition,
        action,
        cooldown_secs: cooldown.map(|s| s as u64),
        last_fired_at,
        enabled: enabled_i != 0,
    })
}

type RecipeRow = (String, String, String, String, String, String, String, String);

fn decode_recipe_row(row: RecipeRow) -> Result<Recipe> {
    let (id_s, name, description, version, source_json_s, trigger_json_s, action_json_s, params_json_s) = row;

    let id = id_s.parse::<Uuid>().map_err(map_uuid)?;
    let source: FeedSource = serde_json::from_str(&source_json_s).map_err(map_json)?;
    let trigger: TriggerCondition = serde_json::from_str(&trigger_json_s).map_err(map_json)?;
    let action: TriggerAction = serde_json::from_str(&action_json_s).map_err(map_json)?;
    let parameters: Vec<RecipeParameter> = serde_json::from_str(&params_json_s).map_err(map_json)?;

    Ok(Recipe {
        id: RecipeId::from_uuid(id),
        name,
        description,
        version,
        source,
        trigger,
        action,
        parameters,
    })
}

type ItemRow = (String, String, String, String, i32);

fn decode_item_row(row: ItemRow) -> Result<FeedItem> {
    let (id_s, feed_id_s, payload_json_s, received_at_s, processed_i) = row;

    let id = id_s.parse::<Uuid>().map_err(map_uuid)?;
    let feed_id = feed_id_s.parse::<Uuid>().map_err(map_uuid)?;
    let payload: serde_json::Value = serde_json::from_str(&payload_json_s).map_err(map_json)?;
    let received_at = parse_ts(&received_at_s)?;

    Ok(FeedItem {
        id,
        feed_id: FeedId::from_uuid(feed_id),
        payload,
        received_at,
        processed: processed_i != 0,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations;
    use chrono::{Duration, Utc};
    use polkagent_core::AgentId;
    use polkagent_feed::store::FeedStore;
    use polkagent_feed::trigger::{CompOp, TriggerCondition};

    /// Create an in-memory pool with the feed schema applied, wrapped in
    /// [`SqliteFeedStore`] so the [`FeedStore`] trait is available.
    fn test_pool() -> SqliteFeedStore {
        let pool = SqlitePool::open_in_memory().expect("open in-memory pool");
        {
            let writer = pool.writer();
            migrations::migrate(&writer).expect("migrate");
        }
        SqliteFeedStore::new(pool)
    }

    /// Helper to build a minimal test feed.
    fn make_feed(name: &str) -> Feed {
        Feed::new(
            name.to_string(),
            FeedSource::Schedule {
                cron: "0 * * * *".to_string(),
            },
            AgentId::new(),
        )
    }

    /// Helper to build a minimal trigger.
    fn make_trigger(feed_id: FeedId) -> Trigger {
        Trigger::new(
            "test-trigger",
            feed_id,
            TriggerCondition::Always,
            TriggerAction::Notify {
                channel: "slack:#alerts".to_string(),
                message: "triggered!".to_string(),
            },
        )
    }

    /// Helper to build a minimal recipe.
    fn make_recipe(name: &str) -> Recipe {
        Recipe {
            id: RecipeId::new(),
            name: name.to_string(),
            description: "A test recipe".to_string(),
            version: "1.0.0".to_string(),
            source: FeedSource::Schedule {
                cron: "*/5 * * * *".to_string(),
            },
            trigger: TriggerCondition::Always,
            action: TriggerAction::StartRun {
                prompt_template: "Run {{task}}".to_string(),
                agent_id: "agent-1".to_string(),
            },
            parameters: vec![],
        }
    }

    // =====================================================================
    // Feed CRUD
    // =====================================================================

    #[tokio::test]
    async fn test_create_and_get_feed() {
        let pool = test_pool();
        let feed = make_feed("my-feed");
        let feed_id = feed.id;

        let created = FeedStore::create_feed(&pool, feed).await.expect("create");
        assert_eq!(created.id, feed_id);
        assert_eq!(created.name, "my-feed");

        let fetched = FeedStore::get_feed(&pool, &feed_id).await.expect("get");
        assert_eq!(fetched.id, feed_id);
        assert_eq!(fetched.name, "my-feed");
        assert!(matches!(fetched.status, FeedStatus::Active));
    }

    #[tokio::test]
    async fn test_get_feed_not_found() {
        let pool = test_pool();
        let err = FeedStore::get_feed(&pool, &FeedId::new())
            .await
            .expect_err("should fail");
        assert!(matches!(err, FeedError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_list_feeds_empty() {
        let pool = test_pool();
        let feeds = FeedStore::list_feeds(&pool).await.expect("list");
        assert!(feeds.is_empty());
    }

    #[tokio::test]
    async fn test_list_feeds_multiple() {
        let pool = test_pool();
        FeedStore::create_feed(&pool, make_feed("feed-a"))
            .await
            .expect("create a");
        FeedStore::create_feed(&pool, make_feed("feed-b"))
            .await
            .expect("create b");
        FeedStore::create_feed(&pool, make_feed("feed-c"))
            .await
            .expect("create c");

        let feeds = FeedStore::list_feeds(&pool).await.expect("list");
        assert_eq!(feeds.len(), 3);
    }

    #[tokio::test]
    async fn test_update_cursor() {
        let pool = test_pool();
        let feed = make_feed("cursor-feed");
        let feed_id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create");

        let new_cursor = Cursor {
            position: "block-42".to_string(),
            last_processed_at: Utc::now(),
            items_processed: 10,
        };
        FeedStore::update_cursor(&pool, &feed_id, new_cursor.clone())
            .await
            .expect("update cursor");

        let fetched = FeedStore::get_feed(&pool, &feed_id).await.expect("get");
        assert_eq!(fetched.cursor.position, "block-42");
        assert_eq!(fetched.cursor.items_processed, 10);
    }

    #[tokio::test]
    async fn test_update_cursor_not_found() {
        let pool = test_pool();
        let cursor = Cursor::new("pos");
        let err = FeedStore::update_cursor(&pool, &FeedId::new(), cursor)
            .await
            .expect_err("should fail");
        assert!(matches!(err, FeedError::NotFound(_)));
    }

    // =====================================================================
    // Trigger CRUD
    // =====================================================================

    #[tokio::test]
    async fn test_create_and_get_trigger() {
        let pool = test_pool();
        let feed = make_feed("trigger-feed");
        let feed_id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create feed");

        let trigger = make_trigger(feed_id);
        let trigger_id = trigger.id;
        let created = FeedStore::create_trigger(&pool, trigger)
            .await
            .expect("create trigger");
        assert_eq!(created.id, trigger_id);

        let fetched = FeedStore::get_trigger(&pool, &trigger_id)
            .await
            .expect("get trigger");
        assert_eq!(fetched.id, trigger_id);
        assert_eq!(fetched.name, "test-trigger");
        assert!(fetched.enabled);
        assert!(matches!(fetched.condition, TriggerCondition::Always));
    }

    #[tokio::test]
    async fn test_get_trigger_not_found() {
        let pool = test_pool();
        let err = FeedStore::get_trigger(&pool, &TriggerId::new())
            .await
            .expect_err("should fail");
        assert!(matches!(err, FeedError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_list_triggers_empty() {
        let pool = test_pool();
        let feed = make_feed("empty-triggers-feed");
        let feed_id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create feed");

        let triggers = FeedStore::list_triggers(&pool, &feed_id)
            .await
            .expect("list triggers");
        assert!(triggers.is_empty());
    }

    #[tokio::test]
    async fn test_list_triggers_filters_by_feed() {
        let pool = test_pool();
        let feed_a = make_feed("feed-a");
        let feed_b = make_feed("feed-b");
        let id_a = feed_a.id;
        let id_b = feed_b.id;
        FeedStore::create_feed(&pool, feed_a).await.expect("a");
        FeedStore::create_feed(&pool, feed_b).await.expect("b");

        FeedStore::create_trigger(&pool, make_trigger(id_a))
            .await
            .expect("trigger a1");
        FeedStore::create_trigger(&pool, make_trigger(id_a))
            .await
            .expect("trigger a2");
        FeedStore::create_trigger(&pool, make_trigger(id_b))
            .await
            .expect("trigger b1");

        let triggers_a = FeedStore::list_triggers(&pool, &id_a)
            .await
            .expect("list a");
        assert_eq!(triggers_a.len(), 2);

        let triggers_b = FeedStore::list_triggers(&pool, &id_b)
            .await
            .expect("list b");
        assert_eq!(triggers_b.len(), 1);
    }

    #[tokio::test]
    async fn test_update_trigger() {
        let pool = test_pool();
        let feed = make_feed("update-trigger-feed");
        let feed_id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create feed");

        let mut trigger = make_trigger(feed_id);
        let trigger_id = trigger.id;
        FeedStore::create_trigger(&pool, trigger.clone())
            .await
            .expect("create trigger");

        // Update: disable and set last_fired_at.
        trigger.enabled = false;
        trigger.last_fired_at = Some(Utc::now());
        trigger.name = "updated-trigger".to_string();

        let updated = FeedStore::update_trigger(&pool, trigger)
            .await
            .expect("update trigger");
        assert_eq!(updated.name, "updated-trigger");
        assert!(!updated.enabled);
        assert!(updated.last_fired_at.is_some());

        // Verify via get.
        let fetched = FeedStore::get_trigger(&pool, &trigger_id)
            .await
            .expect("get");
        assert_eq!(fetched.name, "updated-trigger");
        assert!(!fetched.enabled);
    }

    #[tokio::test]
    async fn test_update_trigger_not_found() {
        let pool = test_pool();
        let feed = make_feed("dummy-feed");
        let feed_id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create feed");

        let trigger = make_trigger(feed_id);
        let err = FeedStore::update_trigger(&pool, trigger)
            .await
            .expect_err("should fail");
        assert!(matches!(err, FeedError::NotFound(_)));
    }

    // =====================================================================
    // Recipe CRUD
    // =====================================================================

    #[tokio::test]
    async fn test_create_and_get_recipe() {
        let pool = test_pool();
        let recipe = make_recipe("my-recipe");
        let recipe_id = recipe.id;

        let created = FeedStore::create_recipe(&pool, recipe)
            .await
            .expect("create recipe");
        assert_eq!(created.id, recipe_id);
        assert_eq!(created.name, "my-recipe");

        let fetched = FeedStore::get_recipe(&pool, &recipe_id)
            .await
            .expect("get recipe");
        assert_eq!(fetched.id, recipe_id);
        assert_eq!(fetched.name, "my-recipe");
        assert_eq!(fetched.description, "A test recipe");
        assert_eq!(fetched.version, "1.0.0");
    }

    #[tokio::test]
    async fn test_get_recipe_not_found() {
        let pool = test_pool();
        let err = FeedStore::get_recipe(&pool, &RecipeId::new())
            .await
            .expect_err("should fail");
        assert!(matches!(err, FeedError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_list_recipes_empty() {
        let pool = test_pool();
        let recipes = FeedStore::list_recipes(&pool).await.expect("list");
        assert!(recipes.is_empty());
    }

    #[tokio::test]
    async fn test_list_recipes_multiple() {
        let pool = test_pool();
        FeedStore::create_recipe(&pool, make_recipe("alpha"))
            .await
            .expect("a");
        FeedStore::create_recipe(&pool, make_recipe("beta"))
            .await
            .expect("b");

        let recipes = FeedStore::list_recipes(&pool).await.expect("list");
        assert_eq!(recipes.len(), 2);
        // Ordered by name ASC.
        assert_eq!(recipes[0].name, "alpha");
        assert_eq!(recipes[1].name, "beta");
    }

    // =====================================================================
    // Feed item queue
    // =====================================================================

    #[tokio::test]
    async fn test_enqueue_item() {
        let pool = test_pool();
        let feed = make_feed("queue-feed");
        let feed_id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create feed");

        let item = FeedItem::new(feed_id, serde_json::json!({"key": "value"}));
        let item_id = item.id;
        let enqueued = FeedStore::enqueue_item(&pool, item)
            .await
            .expect("enqueue");
        assert_eq!(enqueued.id, item_id);
        assert!(!enqueued.processed);
    }

    #[tokio::test]
    async fn test_dequeue_items_fifo() {
        let pool = test_pool();
        let feed = make_feed("fifo-feed");
        let feed_id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create feed");

        // Enqueue items with spaced timestamps.
        let base = Utc::now();
        let mut ids = Vec::new();
        for i in 0..5 {
            let mut item = FeedItem::new(feed_id, serde_json::json!({"index": i}));
            item.received_at = base + Duration::seconds(i);
            ids.push(item.id);
            FeedStore::enqueue_item(&pool, item).await.expect("enqueue");
        }

        // Dequeue 3 items -- should get the first 3 in FIFO order.
        let dequeued = FeedStore::dequeue_items(&pool, &feed_id, 3)
            .await
            .expect("dequeue");
        assert_eq!(dequeued.len(), 3);
        assert_eq!(dequeued[0].id, ids[0]);
        assert_eq!(dequeued[1].id, ids[1]);
        assert_eq!(dequeued[2].id, ids[2]);
    }

    #[tokio::test]
    async fn test_dequeue_items_skips_processed() {
        let pool = test_pool();
        let feed = make_feed("skip-processed");
        let feed_id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create feed");

        let base = Utc::now();
        let mut item1 = FeedItem::new(feed_id, serde_json::json!(1));
        item1.received_at = base;
        let item1_id = item1.id;
        FeedStore::enqueue_item(&pool, item1).await.expect("enqueue 1");

        let mut item2 = FeedItem::new(feed_id, serde_json::json!(2));
        item2.received_at = base + Duration::seconds(1);
        let item2_id = item2.id;
        FeedStore::enqueue_item(&pool, item2).await.expect("enqueue 2");

        // Mark item1 as processed.
        FeedStore::mark_processed(&pool, item1_id)
            .await
            .expect("mark processed");

        // Dequeue should only return item2.
        let dequeued = FeedStore::dequeue_items(&pool, &feed_id, 10)
            .await
            .expect("dequeue");
        assert_eq!(dequeued.len(), 1);
        assert_eq!(dequeued[0].id, item2_id);
    }

    #[tokio::test]
    async fn test_dequeue_items_empty() {
        let pool = test_pool();
        let feed = make_feed("empty-queue");
        let feed_id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create feed");

        let dequeued = FeedStore::dequeue_items(&pool, &feed_id, 10)
            .await
            .expect("dequeue");
        assert!(dequeued.is_empty());
    }

    #[tokio::test]
    async fn test_mark_processed() {
        let pool = test_pool();
        let feed = make_feed("mark-feed");
        let feed_id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create feed");

        let item = FeedItem::new(feed_id, serde_json::json!("data"));
        let item_id = item.id;
        FeedStore::enqueue_item(&pool, item).await.expect("enqueue");

        FeedStore::mark_processed(&pool, item_id)
            .await
            .expect("mark processed");

        // Dequeue should return nothing.
        let dequeued = FeedStore::dequeue_items(&pool, &feed_id, 10)
            .await
            .expect("dequeue");
        assert!(dequeued.is_empty());
    }

    #[tokio::test]
    async fn test_mark_processed_idempotent() {
        let pool = test_pool();
        let feed = make_feed("idempotent-feed");
        let feed_id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create feed");

        let item = FeedItem::new(feed_id, serde_json::json!("data"));
        let item_id = item.id;
        FeedStore::enqueue_item(&pool, item).await.expect("enqueue");

        // Mark twice -- should not error.
        FeedStore::mark_processed(&pool, item_id)
            .await
            .expect("first mark");
        FeedStore::mark_processed(&pool, item_id)
            .await
            .expect("second mark (idempotent)");
    }

    #[tokio::test]
    async fn test_mark_processed_nonexistent_item() {
        let pool = test_pool();
        // Mark a random UUID -- should succeed (idempotent).
        FeedStore::mark_processed(&pool, Uuid::now_v7())
            .await
            .expect("mark nonexistent item");
    }

    // =====================================================================
    // Feed source round-trips
    // =====================================================================

    #[tokio::test]
    async fn test_webhook_source_round_trip() {
        let pool = test_pool();
        let mut feed = make_feed("webhook-feed");
        feed.source = FeedSource::Webhook {
            path: "/hooks/github".to_string(),
            secret_hash: Some("abc123".to_string()),
        };
        let feed_id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create");

        let fetched = FeedStore::get_feed(&pool, &feed_id).await.expect("get");
        match &fetched.source {
            FeedSource::Webhook { path, secret_hash } => {
                assert_eq!(path, "/hooks/github");
                assert_eq!(secret_hash.as_deref(), Some("abc123"));
            }
            other => panic!("unexpected source: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_event_bus_source_round_trip() {
        let pool = test_pool();
        let mut feed = make_feed("event-bus-feed");
        feed.source = FeedSource::EventBus {
            filter: polkagent_feed::EventFilter {
                event_kinds: vec!["run.completed".to_string()],
                agent_ids: vec![],
            },
        };
        let feed_id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create");

        let fetched = FeedStore::get_feed(&pool, &feed_id).await.expect("get");
        match &fetched.source {
            FeedSource::EventBus { filter } => {
                assert_eq!(filter.event_kinds, vec!["run.completed".to_string()]);
                assert!(filter.agent_ids.is_empty());
            }
            other => panic!("unexpected source: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_chain_state_source_round_trip() {
        let pool = test_pool();
        let mut feed = make_feed("chain-state-feed");
        feed.source = FeedSource::ChainState {
            query: "storage.balances".to_string(),
            interval_secs: 60,
        };
        let feed_id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create");

        let fetched = FeedStore::get_feed(&pool, &feed_id).await.expect("get");
        match &fetched.source {
            FeedSource::ChainState { query, interval_secs } => {
                assert_eq!(query, "storage.balances");
                assert_eq!(*interval_secs, 60);
            }
            other => panic!("unexpected source: {other:?}"),
        }
    }

    // =====================================================================
    // FeedStatus round-trips
    // =====================================================================

    #[tokio::test]
    async fn test_paused_status_round_trip() {
        let pool = test_pool();
        let mut feed = make_feed("paused-feed");
        feed.status = FeedStatus::Paused;
        let feed_id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create");

        let fetched = FeedStore::get_feed(&pool, &feed_id).await.expect("get");
        assert!(matches!(fetched.status, FeedStatus::Paused));
    }

    #[tokio::test]
    async fn test_error_status_round_trip() {
        let pool = test_pool();
        let mut feed = make_feed("error-feed");
        feed.status = FeedStatus::Error("something broke".to_string());
        let feed_id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create");

        let fetched = FeedStore::get_feed(&pool, &feed_id).await.expect("get");
        match &fetched.status {
            FeedStatus::Error(msg) => assert_eq!(msg, "something broke"),
            other => panic!("expected Error status, got {other:?}"),
        }
    }

    // =====================================================================
    // Trigger condition / action variants
    // =====================================================================

    #[tokio::test]
    async fn test_trigger_with_threshold_condition() {
        let pool = test_pool();
        let feed = make_feed("threshold-feed");
        let feed_id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create feed");

        let mut trigger = make_trigger(feed_id);
        trigger.condition = TriggerCondition::Threshold {
            field: "/balance".to_string(),
            op: CompOp::Gt,
            value: 100.0,
        };
        trigger.cooldown_secs = Some(300);
        let trigger_id = trigger.id;
        FeedStore::create_trigger(&pool, trigger)
            .await
            .expect("create trigger");

        let fetched = FeedStore::get_trigger(&pool, &trigger_id)
            .await
            .expect("get trigger");
        match &fetched.condition {
            TriggerCondition::Threshold { field, op, value } => {
                assert_eq!(field, "/balance");
                assert!(matches!(op, CompOp::Gt));
                assert!((value - 100.0).abs() < f64::EPSILON);
            }
            other => panic!("expected Threshold, got {other:?}"),
        }
        assert_eq!(fetched.cooldown_secs, Some(300));
    }

    #[tokio::test]
    async fn test_trigger_with_composite_condition() {
        let pool = test_pool();
        let feed = make_feed("composite-feed");
        let feed_id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create feed");

        let mut trigger = make_trigger(feed_id);
        trigger.condition = TriggerCondition::And(vec![
            TriggerCondition::Always,
            TriggerCondition::Not(Box::new(TriggerCondition::JsonPath {
                path: "/status".to_string(),
                expected: serde_json::json!("disabled"),
            })),
        ]);
        let trigger_id = trigger.id;
        FeedStore::create_trigger(&pool, trigger)
            .await
            .expect("create");

        let fetched = FeedStore::get_trigger(&pool, &trigger_id)
            .await
            .expect("get");
        assert!(matches!(fetched.condition, TriggerCondition::And(_)));
    }

    // =====================================================================
    // Recipe with parameters
    // =====================================================================

    #[tokio::test]
    async fn test_recipe_with_parameters() {
        use polkagent_feed::recipe::{ParamType, RecipeParameter};

        let pool = test_pool();
        let mut recipe = make_recipe("parameterized");
        recipe.parameters = vec![
            RecipeParameter {
                name: "threshold".to_string(),
                description: "The threshold value".to_string(),
                param_type: ParamType::Number,
                default: Some("100".to_string()),
                required: false,
            },
            RecipeParameter {
                name: "channel".to_string(),
                description: "Notification channel".to_string(),
                param_type: ParamType::String,
                default: None,
                required: true,
            },
        ];
        let recipe_id = recipe.id;
        FeedStore::create_recipe(&pool, recipe).await.expect("create");

        let fetched = FeedStore::get_recipe(&pool, &recipe_id)
            .await
            .expect("get");
        assert_eq!(fetched.parameters.len(), 2);
        assert_eq!(fetched.parameters[0].name, "threshold");
        assert_eq!(fetched.parameters[0].param_type, ParamType::Number);
        assert!(!fetched.parameters[0].required);
        assert_eq!(fetched.parameters[1].name, "channel");
        assert!(fetched.parameters[1].required);
    }

    // =====================================================================
    // Dequeue items respects feed isolation
    // =====================================================================

    #[tokio::test]
    async fn test_dequeue_items_isolates_feeds() {
        let pool = test_pool();
        let feed_a = make_feed("iso-a");
        let feed_b = make_feed("iso-b");
        let id_a = feed_a.id;
        let id_b = feed_b.id;
        FeedStore::create_feed(&pool, feed_a).await.expect("a");
        FeedStore::create_feed(&pool, feed_b).await.expect("b");

        // Enqueue items for both feeds.
        FeedStore::enqueue_item(&pool, FeedItem::new(id_a, serde_json::json!("a1")))
            .await
            .expect("enqueue a1");
        FeedStore::enqueue_item(&pool, FeedItem::new(id_b, serde_json::json!("b1")))
            .await
            .expect("enqueue b1");
        FeedStore::enqueue_item(&pool, FeedItem::new(id_a, serde_json::json!("a2")))
            .await
            .expect("enqueue a2");

        let dequeued_a = FeedStore::dequeue_items(&pool, &id_a, 10)
            .await
            .expect("dequeue a");
        assert_eq!(dequeued_a.len(), 2);

        let dequeued_b = FeedStore::dequeue_items(&pool, &id_b, 10)
            .await
            .expect("dequeue b");
        assert_eq!(dequeued_b.len(), 1);
    }

    // =====================================================================
    // Dequeue respects limit when fewer items exist
    // =====================================================================

    #[tokio::test]
    async fn test_dequeue_items_limit_exceeds_count() {
        let pool = test_pool();
        let feed = make_feed("limit-feed");
        let feed_id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create feed");

        FeedStore::enqueue_item(&pool, FeedItem::new(feed_id, serde_json::json!(1)))
            .await
            .expect("enqueue");

        let dequeued = FeedStore::dequeue_items(&pool, &feed_id, 100)
            .await
            .expect("dequeue");
        assert_eq!(dequeued.len(), 1);
    }
}
