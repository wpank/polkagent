//! [`FeedStore`] trait implementation for [`SqlitePool`].
//!
//! Wraps synchronous `rusqlite` calls in [`tokio::task::spawn_blocking`] to
//! satisfy the async trait interface. The writer connection is protected by a
//! `parking_lot::Mutex` inside `SqlitePool`, so each method acquires it briefly
//! within the blocking closure.

use async_trait::async_trait;
use chrono::DateTime;
use uuid::Uuid;

use polkagent_feed::{
    Cursor, Feed, FeedId, FeedItem, FeedSource, FeedStatus, FeedStore,
    Recipe, RecipeId, RecipeParameter, Trigger, TriggerAction, TriggerCondition, TriggerId,
    error::{FeedError, Result as FeedResult},
};

use crate::pool::SqlitePool;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Map a `rusqlite::Error` to `FeedError`.
fn map_err(e: rusqlite::Error) -> FeedError {
    FeedError::ProcessingError(format!("sqlite error: {e}"))
}

/// Map a `serde_json::Error` to `FeedError`.
fn map_json_err(e: serde_json::Error) -> FeedError {
    FeedError::ProcessingError(format!("json error: {e}"))
}

/// Parse an ISO-8601 timestamp string into `DateTime<Utc>`.
fn parse_ts(s: &str) -> FeedResult<chrono::DateTime<chrono::Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|e| FeedError::ProcessingError(format!("invalid timestamp '{s}': {e}")))
}

/// Encode a `FeedStatus` as a plain TEXT value stored in the `status` column.
///
/// `Active`  → `"active"`
/// `Paused`  → `"paused"`
/// `Error(msg)` → `"error:<msg>"`
fn encode_status(status: &FeedStatus) -> String {
    match status {
        FeedStatus::Active => "active".to_string(),
        FeedStatus::Paused => "paused".to_string(),
        FeedStatus::Error(msg) => format!("error:{msg}"),
    }
}

/// Decode a `FeedStatus` from its stored TEXT form.
fn decode_status(s: &str) -> FeedStatus {
    match s {
        "active" => FeedStatus::Active,
        "paused" => FeedStatus::Paused,
        other if other.starts_with("error:") => FeedStatus::Error(other[6..].to_string()),
        other => FeedStatus::Error(format!("unknown status: {other}")),
    }
}

// ---------------------------------------------------------------------------
// Row → domain type converters
// ---------------------------------------------------------------------------

/// Raw data read from the `feeds` table before converting to [`Feed`].
struct RawFeed {
    id: String,
    name: String,
    source_json: String,
    cursor_position: String,
    cursor_last_processed_at: String,
    cursor_items_processed: i64,
    status: String,
    agent_id: String,
    created_at: String,
}

impl RawFeed {
    fn into_feed(self) -> FeedResult<Feed> {
        let id: FeedId = self
            .id
            .parse()
            .map_err(|e| FeedError::ProcessingError(format!("invalid feed id '{}': {e}", self.id)))?;

        let source: FeedSource =
            serde_json::from_str(&self.source_json).map_err(map_json_err)?;

        let last_processed_at = parse_ts(&self.cursor_last_processed_at)?;
        let cursor = Cursor {
            position: self.cursor_position,
            last_processed_at,
            items_processed: self.cursor_items_processed as u64,
        };

        let status = decode_status(&self.status);

        let agent_id: polkagent_core::AgentId = self
            .agent_id
            .parse()
            .map_err(|e| FeedError::ProcessingError(format!("invalid agent id '{}': {e}", self.agent_id)))?;

        let created_at = parse_ts(&self.created_at)?;

        Ok(Feed {
            id,
            name: self.name,
            source,
            cursor,
            status,
            agent_id,
            created_at,
        })
    }
}

fn row_to_raw_feed(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawFeed> {
    Ok(RawFeed {
        id: row.get(0)?,
        name: row.get(1)?,
        source_json: row.get(2)?,
        cursor_position: row.get(3)?,
        cursor_last_processed_at: row.get(4)?,
        cursor_items_processed: row.get(5)?,
        status: row.get(6)?,
        agent_id: row.get(7)?,
        created_at: row.get(8)?,
    })
}

/// Raw data read from the `feed_triggers` table.
struct RawTrigger {
    id: String,
    name: String,
    feed_id: String,
    condition_json: String,
    action_json: String,
    cooldown_secs: Option<i64>,
    last_fired_at: Option<String>,
    enabled: i64,
}

impl RawTrigger {
    fn into_trigger(self) -> FeedResult<Trigger> {
        let id: TriggerId = self
            .id
            .parse()
            .map_err(|e| FeedError::ProcessingError(format!("invalid trigger id '{}': {e}", self.id)))?;

        let feed_id: FeedId = self
            .feed_id
            .parse()
            .map_err(|e| FeedError::ProcessingError(format!("invalid feed id '{}': {e}", self.feed_id)))?;

        let condition: TriggerCondition =
            serde_json::from_str(&self.condition_json).map_err(map_json_err)?;

        let action: TriggerAction =
            serde_json::from_str(&self.action_json).map_err(map_json_err)?;

        let last_fired_at = self
            .last_fired_at
            .as_deref()
            .map(parse_ts)
            .transpose()?;

        Ok(Trigger {
            id,
            name: self.name,
            feed_id,
            condition,
            action,
            cooldown_secs: self.cooldown_secs.map(|s| s as u64),
            last_fired_at,
            enabled: self.enabled != 0,
        })
    }
}

fn row_to_raw_trigger(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawTrigger> {
    Ok(RawTrigger {
        id: row.get(0)?,
        name: row.get(1)?,
        feed_id: row.get(2)?,
        condition_json: row.get(3)?,
        action_json: row.get(4)?,
        cooldown_secs: row.get(5)?,
        last_fired_at: row.get(6)?,
        enabled: row.get(7)?,
    })
}

/// Raw data read from the `feed_recipes` table.
struct RawRecipe {
    id: String,
    name: String,
    description: String,
    version: String,
    source_json: String,
    trigger_json: String,
    action_json: String,
    parameters_json: String,
}

impl RawRecipe {
    fn into_recipe(self) -> FeedResult<Recipe> {
        let id: RecipeId = self
            .id
            .parse()
            .map_err(|e| FeedError::ProcessingError(format!("invalid recipe id '{}': {e}", self.id)))?;

        let source: FeedSource =
            serde_json::from_str(&self.source_json).map_err(map_json_err)?;

        let trigger: TriggerCondition =
            serde_json::from_str(&self.trigger_json).map_err(map_json_err)?;

        let action: TriggerAction =
            serde_json::from_str(&self.action_json).map_err(map_json_err)?;

        let parameters: Vec<RecipeParameter> =
            serde_json::from_str(&self.parameters_json).map_err(map_json_err)?;

        Ok(Recipe {
            id,
            name: self.name,
            description: self.description,
            version: self.version,
            source,
            trigger,
            action,
            parameters,
        })
    }
}

fn row_to_raw_recipe(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRecipe> {
    Ok(RawRecipe {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        version: row.get(3)?,
        source_json: row.get(4)?,
        trigger_json: row.get(5)?,
        action_json: row.get(6)?,
        parameters_json: row.get(7)?,
    })
}

/// Raw data read from the `feed_items` table.
struct RawFeedItem {
    id: String,
    feed_id: String,
    payload_json: String,
    received_at: String,
    processed: i64,
}

impl RawFeedItem {
    fn into_feed_item(self) -> FeedResult<FeedItem> {
        let id: Uuid = self
            .id
            .parse()
            .map_err(|e| FeedError::ProcessingError(format!("invalid item id '{}': {e}", self.id)))?;

        let feed_id: FeedId = self
            .feed_id
            .parse()
            .map_err(|e| FeedError::ProcessingError(format!("invalid feed id '{}': {e}", self.feed_id)))?;

        let payload: serde_json::Value =
            serde_json::from_str(&self.payload_json).map_err(map_json_err)?;

        let received_at = parse_ts(&self.received_at)?;

        Ok(FeedItem {
            id,
            feed_id,
            payload,
            received_at,
            processed: self.processed != 0,
        })
    }
}

fn row_to_raw_feed_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawFeedItem> {
    Ok(RawFeedItem {
        id: row.get(0)?,
        feed_id: row.get(1)?,
        payload_json: row.get(2)?,
        received_at: row.get(3)?,
        processed: row.get(4)?,
    })
}

// ---------------------------------------------------------------------------
// FeedStore implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl FeedStore for SqlitePool {
    // ------------------------------------------------------------------
    // Feed CRUD
    // ------------------------------------------------------------------

    async fn create_feed(&self, feed: Feed) -> FeedResult<Feed> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let source_json = serde_json::to_string(&feed.source).map_err(map_json_err)?;
            let cursor_last_processed_at = feed.cursor.last_processed_at.to_rfc3339();
            let status_str = encode_status(&feed.status);
            let id_str = feed.id.to_string();
            let agent_id_str = feed.agent_id.to_string();
            let created_at_str = feed.created_at.to_rfc3339();

            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO feeds (
                        id, name, source_json,
                        cursor_position, cursor_last_processed_at, cursor_items_processed,
                        status, agent_id, created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        id_str,
                        feed.name,
                        source_json,
                        feed.cursor.position,
                        cursor_last_processed_at,
                        feed.cursor.items_processed as i64,
                        status_str,
                        agent_id_str,
                        created_at_str,
                    ],
                )
                .map_err(|e| {
                    if crate::error::StoreError::is_unique_violation(&e) {
                        FeedError::ProcessingError(format!("feed '{}' already exists", id_str))
                    } else {
                        map_err(e)
                    }
                })?;

            Ok(feed)
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    async fn get_feed(&self, id: &FeedId) -> FeedResult<Feed> {
        let pool = self.clone();
        let id_str = id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let raw = writer
                .query_row(
                    "SELECT id, name, source_json,
                            cursor_position, cursor_last_processed_at, cursor_items_processed,
                            status, agent_id, created_at
                     FROM feeds WHERE id = ?1",
                    [&id_str],
                    row_to_raw_feed,
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        FeedError::NotFound(format!("feed '{id_str}'"))
                    }
                    other => map_err(other),
                })?;
            raw.into_feed()
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    async fn list_feeds(&self) -> FeedResult<Vec<Feed>> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let mut stmt = writer
                .prepare(
                    "SELECT id, name, source_json,
                            cursor_position, cursor_last_processed_at, cursor_items_processed,
                            status, agent_id, created_at
                     FROM feeds
                     ORDER BY created_at ASC",
                )
                .map_err(map_err)?;

            let raws = stmt
                .query_map([], row_to_raw_feed)
                .map_err(map_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_err)?;

            raws.into_iter().map(RawFeed::into_feed).collect()
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    async fn update_cursor(&self, feed_id: &FeedId, cursor: Cursor) -> FeedResult<()> {
        let pool = self.clone();
        let id_str = feed_id.to_string();

        tokio::task::spawn_blocking(move || {
            let last_processed_at = cursor.last_processed_at.to_rfc3339();
            let writer = pool.writer();

            let n = writer
                .execute(
                    "UPDATE feeds
                     SET cursor_position          = ?1,
                         cursor_last_processed_at = ?2,
                         cursor_items_processed   = ?3
                     WHERE id = ?4",
                    rusqlite::params![
                        cursor.position,
                        last_processed_at,
                        cursor.items_processed as i64,
                        id_str,
                    ],
                )
                .map_err(map_err)?;

            if n == 0 {
                return Err(FeedError::NotFound(format!("feed '{id_str}'")));
            }
            Ok(())
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    // ------------------------------------------------------------------
    // Trigger CRUD
    // ------------------------------------------------------------------

    async fn create_trigger(&self, trigger: Trigger) -> FeedResult<Trigger> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let condition_json =
                serde_json::to_string(&trigger.condition).map_err(map_json_err)?;
            let action_json = serde_json::to_string(&trigger.action).map_err(map_json_err)?;
            let id_str = trigger.id.to_string();
            let feed_id_str = trigger.feed_id.to_string();
            let last_fired_at = trigger
                .last_fired_at
                .as_ref()
                .map(|dt| dt.to_rfc3339());
            let enabled: i64 = if trigger.enabled { 1 } else { 0 };

            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO feed_triggers (
                        id, name, feed_id, condition_json, action_json,
                        cooldown_secs, last_fired_at, enabled
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        id_str,
                        trigger.name,
                        feed_id_str,
                        condition_json,
                        action_json,
                        trigger.cooldown_secs.map(|s| s as i64),
                        last_fired_at,
                        enabled,
                    ],
                )
                .map_err(|e| {
                    if crate::error::StoreError::is_unique_violation(&e) {
                        FeedError::ProcessingError(format!(
                            "trigger '{}' already exists",
                            id_str
                        ))
                    } else {
                        map_err(e)
                    }
                })?;

            Ok(trigger)
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    async fn get_trigger(&self, id: &TriggerId) -> FeedResult<Trigger> {
        let pool = self.clone();
        let id_str = id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let raw = writer
                .query_row(
                    "SELECT id, name, feed_id, condition_json, action_json,
                            cooldown_secs, last_fired_at, enabled
                     FROM feed_triggers WHERE id = ?1",
                    [&id_str],
                    row_to_raw_trigger,
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        FeedError::NotFound(format!("trigger '{id_str}'"))
                    }
                    other => map_err(other),
                })?;
            raw.into_trigger()
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    async fn list_triggers(&self, feed_id: &FeedId) -> FeedResult<Vec<Trigger>> {
        let pool = self.clone();
        let feed_id_str = feed_id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let mut stmt = writer
                .prepare(
                    "SELECT id, name, feed_id, condition_json, action_json,
                            cooldown_secs, last_fired_at, enabled
                     FROM feed_triggers
                     WHERE feed_id = ?1
                     ORDER BY rowid ASC",
                )
                .map_err(map_err)?;

            let raws = stmt
                .query_map([&feed_id_str], row_to_raw_trigger)
                .map_err(map_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_err)?;

            raws.into_iter().map(RawTrigger::into_trigger).collect()
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    async fn update_trigger(&self, trigger: Trigger) -> FeedResult<Trigger> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let condition_json =
                serde_json::to_string(&trigger.condition).map_err(map_json_err)?;
            let action_json = serde_json::to_string(&trigger.action).map_err(map_json_err)?;
            let id_str = trigger.id.to_string();
            let feed_id_str = trigger.feed_id.to_string();
            let last_fired_at = trigger
                .last_fired_at
                .as_ref()
                .map(|dt| dt.to_rfc3339());
            let enabled: i64 = if trigger.enabled { 1 } else { 0 };

            let writer = pool.writer();
            let n = writer
                .execute(
                    "UPDATE feed_triggers
                     SET name           = ?1,
                         feed_id        = ?2,
                         condition_json = ?3,
                         action_json    = ?4,
                         cooldown_secs  = ?5,
                         last_fired_at  = ?6,
                         enabled        = ?7
                     WHERE id = ?8",
                    rusqlite::params![
                        trigger.name,
                        feed_id_str,
                        condition_json,
                        action_json,
                        trigger.cooldown_secs.map(|s| s as i64),
                        last_fired_at,
                        enabled,
                        id_str,
                    ],
                )
                .map_err(map_err)?;

            if n == 0 {
                return Err(FeedError::NotFound(format!("trigger '{id_str}'")));
            }
            Ok(trigger)
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    // ------------------------------------------------------------------
    // Recipe CRUD
    // ------------------------------------------------------------------

    async fn create_recipe(&self, recipe: Recipe) -> FeedResult<Recipe> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let source_json = serde_json::to_string(&recipe.source).map_err(map_json_err)?;
            let trigger_json = serde_json::to_string(&recipe.trigger).map_err(map_json_err)?;
            let action_json = serde_json::to_string(&recipe.action).map_err(map_json_err)?;
            let parameters_json =
                serde_json::to_string(&recipe.parameters).map_err(map_json_err)?;
            let id_str = recipe.id.to_string();

            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO feed_recipes (
                        id, name, description, version,
                        source_json, trigger_json, action_json, parameters_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
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
                .map_err(|e| {
                    if crate::error::StoreError::is_unique_violation(&e) {
                        FeedError::ProcessingError(format!(
                            "recipe '{}' already exists",
                            id_str
                        ))
                    } else {
                        map_err(e)
                    }
                })?;

            Ok(recipe)
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    async fn get_recipe(&self, id: &RecipeId) -> FeedResult<Recipe> {
        let pool = self.clone();
        let id_str = id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let raw = writer
                .query_row(
                    "SELECT id, name, description, version,
                            source_json, trigger_json, action_json, parameters_json
                     FROM feed_recipes WHERE id = ?1",
                    [&id_str],
                    row_to_raw_recipe,
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        FeedError::NotFound(format!("recipe '{id_str}'"))
                    }
                    other => map_err(other),
                })?;
            raw.into_recipe()
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    async fn list_recipes(&self) -> FeedResult<Vec<Recipe>> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let mut stmt = writer
                .prepare(
                    "SELECT id, name, description, version,
                            source_json, trigger_json, action_json, parameters_json
                     FROM feed_recipes
                     ORDER BY rowid ASC",
                )
                .map_err(map_err)?;

            let raws = stmt
                .query_map([], row_to_raw_recipe)
                .map_err(map_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_err)?;

            raws.into_iter().map(RawRecipe::into_recipe).collect()
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    // ------------------------------------------------------------------
    // Feed item queue
    // ------------------------------------------------------------------

    async fn enqueue_item(&self, item: FeedItem) -> FeedResult<FeedItem> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let payload_json = serde_json::to_string(&item.payload).map_err(map_json_err)?;
            let id_str = item.id.to_string();
            let feed_id_str = item.feed_id.to_string();
            let received_at_str = item.received_at.to_rfc3339();
            let processed: i64 = if item.processed { 1 } else { 0 };

            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO feed_items (id, feed_id, payload_json, received_at, processed)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        id_str,
                        feed_id_str,
                        payload_json,
                        received_at_str,
                        processed,
                    ],
                )
                .map_err(|e| {
                    if crate::error::StoreError::is_unique_violation(&e) {
                        FeedError::ProcessingError(format!(
                            "feed item '{}' already exists",
                            id_str
                        ))
                    } else {
                        map_err(e)
                    }
                })?;

            Ok(item)
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    async fn dequeue_items(&self, feed_id: &FeedId, limit: usize) -> FeedResult<Vec<FeedItem>> {
        let pool = self.clone();
        let feed_id_str = feed_id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let mut stmt = writer
                .prepare(
                    "SELECT id, feed_id, payload_json, received_at, processed
                     FROM feed_items
                     WHERE feed_id = ?1 AND processed = 0
                     ORDER BY received_at ASC, rowid ASC
                     LIMIT ?2",
                )
                .map_err(map_err)?;

            let raws = stmt
                .query_map(
                    rusqlite::params![feed_id_str, limit as i64],
                    row_to_raw_feed_item,
                )
                .map_err(map_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_err)?;

            raws.into_iter()
                .map(RawFeedItem::into_feed_item)
                .collect()
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }

    async fn mark_processed(&self, item_id: Uuid) -> FeedResult<()> {
        let pool = self.clone();
        let id_str = item_id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            // Idempotent: no error if already processed or not found.
            writer
                .execute(
                    "UPDATE feed_items SET processed = 1 WHERE id = ?1",
                    [&id_str],
                )
                .map_err(map_err)?;
            Ok(())
        })
        .await
        .map_err(|e| FeedError::ProcessingError(format!("blocking task panicked: {e}")))?
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// These contract tests use `expect` to pinpoint the exact database setup or
// feed-store operation that violated the fixture's asserted invariant.
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::migrations;

    use polkagent_core::AgentId;
    use polkagent_feed::{
        CompOp, FeedSource, FeedStatus, ParamType, RecipeParameter, TriggerAction,
        TriggerCondition,
    };

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn test_pool() -> SqlitePool {
        let pool = SqlitePool::open_in_memory().expect("open in-memory pool");
        {
            let writer = pool.writer();
            migrations::migrate(&writer).expect("migrate");
        }
        pool
    }

    fn make_feed(name: &str) -> Feed {
        Feed::new(
            name,
            FeedSource::Schedule {
                cron: "0 * * * *".to_string(),
            },
            AgentId::new(),
        )
    }

    fn make_webhook_feed(name: &str) -> Feed {
        Feed::new(
            name,
            FeedSource::Webhook {
                path: "/hooks/test".to_string(),
                secret_hash: None,
            },
            AgentId::new(),
        )
    }

    fn make_trigger(feed_id: FeedId) -> Trigger {
        Trigger::new(
            "always-trigger",
            feed_id,
            TriggerCondition::Always,
            TriggerAction::Notify {
                channel: "slack:#alerts".to_string(),
                message: "fired!".to_string(),
            },
        )
    }

    fn make_recipe() -> Recipe {
        Recipe {
            id: RecipeId::new(),
            name: "test-recipe".to_string(),
            description: "A test recipe".to_string(),
            version: "1.0.0".to_string(),
            source: FeedSource::Schedule {
                cron: "0 0 * * *".to_string(),
            },
            trigger: TriggerCondition::Always,
            action: TriggerAction::Notify {
                channel: "slack:#general".to_string(),
                message: "daily ping".to_string(),
            },
            parameters: vec![],
        }
    }

    fn make_item(feed_id: FeedId) -> FeedItem {
        FeedItem::new(feed_id, serde_json::json!({"key": "value"}))
    }

    // -----------------------------------------------------------------------
    // Feed CRUD tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn feed_create_and_get() {
        let pool = test_pool();
        let feed = make_feed("my-feed");
        let id = feed.id;

        FeedStore::create_feed(&pool, feed.clone())
            .await
            .expect("create feed");

        let got = FeedStore::get_feed(&pool, &id)
            .await
            .expect("get feed");

        assert_eq!(got.id, id);
        assert_eq!(got.name, "my-feed");
        assert!(matches!(got.source, FeedSource::Schedule { .. }));
        assert_eq!(got.agent_id, feed.agent_id);
    }

    #[tokio::test]
    async fn feed_get_not_found() {
        let pool = test_pool();
        let id = FeedId::new();
        let err = FeedStore::get_feed(&pool, &id)
            .await
            .expect_err("should not find non-existent feed");
        assert!(
            matches!(err, FeedError::NotFound(_)),
            "expected NotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn feed_list_empty() {
        let pool = test_pool();
        let feeds = FeedStore::list_feeds(&pool)
            .await
            .expect("list feeds");
        assert!(feeds.is_empty());
    }

    #[tokio::test]
    async fn feed_list_multiple() {
        let pool = test_pool();
        FeedStore::create_feed(&pool, make_feed("feed-a"))
            .await
            .expect("create a");
        FeedStore::create_feed(&pool, make_feed("feed-b"))
            .await
            .expect("create b");
        FeedStore::create_feed(&pool, make_webhook_feed("feed-c"))
            .await
            .expect("create c");

        let feeds = FeedStore::list_feeds(&pool)
            .await
            .expect("list feeds");
        assert_eq!(feeds.len(), 3);
    }

    #[tokio::test]
    async fn feed_source_variants_roundtrip() {
        let pool = test_pool();

        let schedule_feed = make_feed("schedule-feed");
        FeedStore::create_feed(&pool, schedule_feed.clone())
            .await
            .expect("create schedule feed");
        let got = FeedStore::get_feed(&pool, &schedule_feed.id)
            .await
            .expect("get schedule feed");
        assert!(matches!(got.source, FeedSource::Schedule { cron } if cron == "0 * * * *"));

        let webhook_feed = make_webhook_feed("webhook-feed");
        FeedStore::create_feed(&pool, webhook_feed.clone())
            .await
            .expect("create webhook feed");
        let got = FeedStore::get_feed(&pool, &webhook_feed.id)
            .await
            .expect("get webhook feed");
        assert!(matches!(got.source, FeedSource::Webhook { path, .. } if path == "/hooks/test"));

        let event_feed = Feed::new(
            "event-feed",
            FeedSource::EventBus {
                filter: polkagent_feed::EventFilter::allow_all(),
            },
            AgentId::new(),
        );
        FeedStore::create_feed(&pool, event_feed.clone())
            .await
            .expect("create event feed");
        let got = FeedStore::get_feed(&pool, &event_feed.id)
            .await
            .expect("get event feed");
        assert!(matches!(got.source, FeedSource::EventBus { .. }));

        let chain_feed = Feed::new(
            "chain-feed",
            FeedSource::ChainState {
                query: "block_height".to_string(),
                interval_secs: 60,
            },
            AgentId::new(),
        );
        FeedStore::create_feed(&pool, chain_feed.clone())
            .await
            .expect("create chain feed");
        let got = FeedStore::get_feed(&pool, &chain_feed.id)
            .await
            .expect("get chain feed");
        assert!(matches!(got.source, FeedSource::ChainState { interval_secs: 60, .. }));
    }

    #[tokio::test]
    async fn feed_status_active_roundtrip() {
        let pool = test_pool();
        let feed = make_feed("status-feed");
        let id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create");
        let got = FeedStore::get_feed(&pool, &id).await.expect("get");
        assert!(matches!(got.status, FeedStatus::Active));
    }

    #[tokio::test]
    async fn update_cursor_advances_position() {
        let pool = test_pool();
        let feed = make_feed("cursor-feed");
        let id = feed.id;
        FeedStore::create_feed(&pool, feed).await.expect("create");

        let new_cursor = Cursor {
            position: "block-42".to_string(),
            last_processed_at: chrono::Utc::now(),
            items_processed: 7,
        };
        FeedStore::update_cursor(&pool, &id, new_cursor.clone())
            .await
            .expect("update cursor");

        let got = FeedStore::get_feed(&pool, &id).await.expect("get");
        assert_eq!(got.cursor.position, "block-42");
        assert_eq!(got.cursor.items_processed, 7);
    }

    #[tokio::test]
    async fn update_cursor_not_found() {
        let pool = test_pool();
        let id = FeedId::new();
        let cursor = Cursor::new("pos");
        let err = FeedStore::update_cursor(&pool, &id, cursor)
            .await
            .expect_err("should fail for non-existent feed");
        assert!(
            matches!(err, FeedError::NotFound(_)),
            "expected NotFound, got: {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Trigger CRUD tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn trigger_create_and_get() {
        let pool = test_pool();
        let feed = FeedStore::create_feed(&pool, make_feed("feed"))
            .await
            .expect("create feed");

        let trigger = make_trigger(feed.id);
        let tid = trigger.id;
        FeedStore::create_trigger(&pool, trigger.clone())
            .await
            .expect("create trigger");

        let got = FeedStore::get_trigger(&pool, &tid)
            .await
            .expect("get trigger");
        assert_eq!(got.id, tid);
        assert_eq!(got.name, "always-trigger");
        assert_eq!(got.feed_id, feed.id);
        assert!(matches!(got.condition, TriggerCondition::Always));
        assert!(got.enabled);
        assert!(got.cooldown_secs.is_none());
        assert!(got.last_fired_at.is_none());
    }

    #[tokio::test]
    async fn trigger_get_not_found() {
        let pool = test_pool();
        let tid = TriggerId::new();
        let err = FeedStore::get_trigger(&pool, &tid)
            .await
            .expect_err("should not find non-existent trigger");
        assert!(
            matches!(err, FeedError::NotFound(_)),
            "expected NotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn trigger_list_for_feed() {
        let pool = test_pool();
        let feed = FeedStore::create_feed(&pool, make_feed("feed"))
            .await
            .expect("create feed");

        let t1 = make_trigger(feed.id);
        let t2 = Trigger::new(
            "threshold-trigger",
            feed.id,
            TriggerCondition::Threshold {
                field: "/value".to_string(),
                op: CompOp::Gt,
                value: 100.0,
            },
            TriggerAction::PublishEvent {
                kind: "threshold.breached".to_string(),
                payload: serde_json::json!({}),
            },
        );

        FeedStore::create_trigger(&pool, t1).await.expect("create t1");
        FeedStore::create_trigger(&pool, t2).await.expect("create t2");

        let triggers = FeedStore::list_triggers(&pool, &feed.id)
            .await
            .expect("list triggers");
        assert_eq!(triggers.len(), 2);
    }

    #[tokio::test]
    async fn trigger_list_empty_for_unknown_feed() {
        let pool = test_pool();
        let feed_id = FeedId::new();
        let triggers = FeedStore::list_triggers(&pool, &feed_id)
            .await
            .expect("list triggers");
        assert!(triggers.is_empty());
    }

    #[tokio::test]
    async fn trigger_update_last_fired_at() {
        let pool = test_pool();
        let feed = FeedStore::create_feed(&pool, make_feed("feed"))
            .await
            .expect("create feed");
        let mut trigger = make_trigger(feed.id);
        FeedStore::create_trigger(&pool, trigger.clone())
            .await
            .expect("create trigger");

        trigger.last_fired_at = Some(chrono::Utc::now());
        trigger.cooldown_secs = Some(30);
        let updated = FeedStore::update_trigger(&pool, trigger.clone())
            .await
            .expect("update trigger");

        assert!(updated.last_fired_at.is_some());
        assert_eq!(updated.cooldown_secs, Some(30));

        let got = FeedStore::get_trigger(&pool, &trigger.id)
            .await
            .expect("get trigger");
        assert!(got.last_fired_at.is_some());
        assert_eq!(got.cooldown_secs, Some(30));
    }

    #[tokio::test]
    async fn trigger_update_not_found() {
        let pool = test_pool();
        let feed = FeedStore::create_feed(&pool, make_feed("feed"))
            .await
            .expect("create feed");
        let trigger = make_trigger(feed.id);
        let err = FeedStore::update_trigger(&pool, trigger)
            .await
            .expect_err("should fail for non-existent trigger");
        assert!(
            matches!(err, FeedError::NotFound(_)),
            "expected NotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn trigger_enabled_disabled_roundtrip() {
        let pool = test_pool();
        let feed = FeedStore::create_feed(&pool, make_feed("feed"))
            .await
            .expect("create feed");
        let mut trigger = make_trigger(feed.id);
        trigger.enabled = false;
        FeedStore::create_trigger(&pool, trigger.clone())
            .await
            .expect("create trigger");
        let got = FeedStore::get_trigger(&pool, &trigger.id)
            .await
            .expect("get trigger");
        assert!(!got.enabled);
    }

    // -----------------------------------------------------------------------
    // Recipe CRUD tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn recipe_create_and_get() {
        let pool = test_pool();
        let recipe = make_recipe();
        let rid = recipe.id;

        FeedStore::create_recipe(&pool, recipe.clone())
            .await
            .expect("create recipe");

        let got = FeedStore::get_recipe(&pool, &rid)
            .await
            .expect("get recipe");
        assert_eq!(got.id, rid);
        assert_eq!(got.name, "test-recipe");
        assert_eq!(got.version, "1.0.0");
        assert!(matches!(got.trigger, TriggerCondition::Always));
    }

    #[tokio::test]
    async fn recipe_get_not_found() {
        let pool = test_pool();
        let rid = RecipeId::new();
        let err = FeedStore::get_recipe(&pool, &rid)
            .await
            .expect_err("should not find non-existent recipe");
        assert!(
            matches!(err, FeedError::NotFound(_)),
            "expected NotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn recipe_list_empty() {
        let pool = test_pool();
        let recipes = FeedStore::list_recipes(&pool).await.expect("list recipes");
        assert!(recipes.is_empty());
    }

    #[tokio::test]
    async fn recipe_list_multiple() {
        let pool = test_pool();
        FeedStore::create_recipe(&pool, make_recipe())
            .await
            .expect("create r1");

        let r2 = Recipe {
            id: RecipeId::new(),
            name: "recipe-2".to_string(),
            description: "Second recipe".to_string(),
            version: "2.0.0".to_string(),
            source: FeedSource::Webhook {
                path: "/hook".to_string(),
                secret_hash: Some("abc123".to_string()),
            },
            trigger: TriggerCondition::JsonPath {
                path: "/status".to_string(),
                expected: serde_json::json!("ok"),
            },
            action: TriggerAction::StartRun {
                prompt_template: "Run for {{payload}}".to_string(),
                agent_id: "agent-1".to_string(),
            },
            parameters: vec![RecipeParameter {
                name: "threshold".to_string(),
                description: "Alert threshold".to_string(),
                param_type: ParamType::Number,
                default: Some("100".to_string()),
                required: false,
            }],
        };
        FeedStore::create_recipe(&pool, r2).await.expect("create r2");

        let recipes = FeedStore::list_recipes(&pool).await.expect("list");
        assert_eq!(recipes.len(), 2);
    }

    #[tokio::test]
    async fn recipe_with_parameters_roundtrip() {
        let pool = test_pool();
        let recipe = Recipe {
            id: RecipeId::new(),
            name: "param-recipe".to_string(),
            description: "Recipe with params".to_string(),
            version: "1.0.0".to_string(),
            source: FeedSource::Schedule {
                cron: "{{cron}}".to_string(),
            },
            trigger: TriggerCondition::Always,
            action: TriggerAction::Notify {
                channel: "{{channel}}".to_string(),
                message: "alert".to_string(),
            },
            parameters: vec![
                RecipeParameter {
                    name: "cron".to_string(),
                    description: "Cron expression".to_string(),
                    param_type: ParamType::String,
                    default: Some("0 * * * *".to_string()),
                    required: false,
                },
                RecipeParameter {
                    name: "channel".to_string(),
                    description: "Notification channel".to_string(),
                    param_type: ParamType::String,
                    default: None,
                    required: true,
                },
            ],
        };
        let rid = recipe.id;
        FeedStore::create_recipe(&pool, recipe)
            .await
            .expect("create");
        let got = FeedStore::get_recipe(&pool, &rid).await.expect("get");
        assert_eq!(got.parameters.len(), 2);
        assert_eq!(got.parameters[0].name, "cron");
        assert_eq!(got.parameters[1].name, "channel");
        assert!(got.parameters[1].required);
    }

    // -----------------------------------------------------------------------
    // Feed item queue tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn item_enqueue_and_dequeue() {
        let pool = test_pool();
        let feed = FeedStore::create_feed(&pool, make_feed("feed"))
            .await
            .expect("create feed");

        let item = make_item(feed.id);
        let item_id = item.id;
        FeedStore::enqueue_item(&pool, item.clone())
            .await
            .expect("enqueue");

        let items = FeedStore::dequeue_items(&pool, &feed.id, 10)
            .await
            .expect("dequeue");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, item_id);
        assert!(!items[0].processed);
    }

    #[tokio::test]
    async fn item_dequeue_empty() {
        let pool = test_pool();
        let feed = FeedStore::create_feed(&pool, make_feed("feed"))
            .await
            .expect("create feed");

        let items = FeedStore::dequeue_items(&pool, &feed.id, 10)
            .await
            .expect("dequeue");
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn item_mark_processed() {
        let pool = test_pool();
        let feed = FeedStore::create_feed(&pool, make_feed("feed"))
            .await
            .expect("create feed");

        let item = make_item(feed.id);
        let item_id = item.id;
        FeedStore::enqueue_item(&pool, item).await.expect("enqueue");

        FeedStore::mark_processed(&pool, item_id)
            .await
            .expect("mark processed");

        // Dequeue should now return nothing.
        let items = FeedStore::dequeue_items(&pool, &feed.id, 10)
            .await
            .expect("dequeue");
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn item_mark_processed_idempotent() {
        let pool = test_pool();
        let feed = FeedStore::create_feed(&pool, make_feed("feed"))
            .await
            .expect("create feed");

        let item = make_item(feed.id);
        let item_id = item.id;
        FeedStore::enqueue_item(&pool, item).await.expect("enqueue");

        FeedStore::mark_processed(&pool, item_id)
            .await
            .expect("first mark");
        // Second call should not error.
        FeedStore::mark_processed(&pool, item_id)
            .await
            .expect("second mark (idempotent)");
    }

    #[tokio::test]
    async fn item_mark_processed_unknown_id_no_error() {
        // mark_processed is specified as idempotent; unknown IDs should not error.
        let pool = test_pool();
        let unknown = Uuid::now_v7();
        FeedStore::mark_processed(&pool, unknown)
            .await
            .expect("mark unknown item is no-op");
    }

    #[tokio::test]
    async fn item_dequeue_fifo_ordering() {
        let pool = test_pool();
        let feed = FeedStore::create_feed(&pool, make_feed("feed"))
            .await
            .expect("create feed");

        // Enqueue three items in order.
        let item1 = FeedItem::new(feed.id, serde_json::json!({"seq": 1}));
        let item2 = FeedItem::new(feed.id, serde_json::json!({"seq": 2}));
        let item3 = FeedItem::new(feed.id, serde_json::json!({"seq": 3}));
        let id1 = item1.id;
        let id2 = item2.id;
        let id3 = item3.id;

        FeedStore::enqueue_item(&pool, item1).await.expect("enqueue 1");
        FeedStore::enqueue_item(&pool, item2).await.expect("enqueue 2");
        FeedStore::enqueue_item(&pool, item3).await.expect("enqueue 3");

        let items = FeedStore::dequeue_items(&pool, &feed.id, 10)
            .await
            .expect("dequeue");

        assert_eq!(items.len(), 3);
        assert_eq!(items[0].id, id1, "first item should be id1");
        assert_eq!(items[1].id, id2, "second item should be id2");
        assert_eq!(items[2].id, id3, "third item should be id3");
    }

    #[tokio::test]
    async fn item_dequeue_respects_limit() {
        let pool = test_pool();
        let feed = FeedStore::create_feed(&pool, make_feed("feed"))
            .await
            .expect("create feed");

        for i in 0..5 {
            let item = FeedItem::new(feed.id, serde_json::json!({"i": i}));
            FeedStore::enqueue_item(&pool, item).await.expect("enqueue");
        }

        let items = FeedStore::dequeue_items(&pool, &feed.id, 3)
            .await
            .expect("dequeue with limit");
        assert_eq!(items.len(), 3);
    }

    #[tokio::test]
    async fn item_dequeue_skips_processed() {
        let pool = test_pool();
        let feed = FeedStore::create_feed(&pool, make_feed("feed"))
            .await
            .expect("create feed");

        let item1 = FeedItem::new(feed.id, serde_json::json!({"seq": 1}));
        let item2 = FeedItem::new(feed.id, serde_json::json!({"seq": 2}));
        let id1 = item1.id;
        let id2 = item2.id;

        FeedStore::enqueue_item(&pool, item1).await.expect("enqueue 1");
        FeedStore::enqueue_item(&pool, item2).await.expect("enqueue 2");

        FeedStore::mark_processed(&pool, id1)
            .await
            .expect("mark 1 processed");

        let items = FeedStore::dequeue_items(&pool, &feed.id, 10)
            .await
            .expect("dequeue");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, id2);
    }

    #[tokio::test]
    async fn item_dequeue_isolated_by_feed() {
        let pool = test_pool();
        let feed_a = FeedStore::create_feed(&pool, make_feed("feed-a"))
            .await
            .expect("create feed-a");
        let feed_b = FeedStore::create_feed(&pool, make_feed("feed-b"))
            .await
            .expect("create feed-b");

        FeedStore::enqueue_item(&pool, FeedItem::new(feed_a.id, serde_json::json!({"feed": "a"})))
            .await
            .expect("enqueue for a");
        FeedStore::enqueue_item(&pool, FeedItem::new(feed_b.id, serde_json::json!({"feed": "b"})))
            .await
            .expect("enqueue for b");

        let items_a = FeedStore::dequeue_items(&pool, &feed_a.id, 10)
            .await
            .expect("dequeue a");
        assert_eq!(items_a.len(), 1);
        assert_eq!(items_a[0].payload["feed"], "a");

        let items_b = FeedStore::dequeue_items(&pool, &feed_b.id, 10)
            .await
            .expect("dequeue b");
        assert_eq!(items_b.len(), 1);
        assert_eq!(items_b[0].payload["feed"], "b");
    }

    // -----------------------------------------------------------------------
    // Object safety / trait bound checks
    // -----------------------------------------------------------------------

    #[allow(dead_code)]
    fn _pool_is_feed_store_object_safe(_s: &dyn FeedStore) {}

    #[allow(dead_code)]
    fn _pool_implements_feed_store() {
        fn assert_feed_store<T: FeedStore>() {}
        assert_feed_store::<SqlitePool>();
    }
}
