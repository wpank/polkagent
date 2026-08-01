//! DDL for the feed store tables.
//!
//! This module provides a single [`migrate`] function that creates the four
//! feed-related tables (`feeds`, `feed_triggers`, `feed_recipes`, `feed_items`)
//! and their indexes if they do not already exist.
//!
//! Because this crate is compiled separately from `polkagent-store-sqlite` (to
//! isolate the serde trait-solver recursion), the migration here uses
//! `CREATE TABLE IF NOT EXISTS` rather than the version-tracked migration
//! system in the main store crate.  When the main store's `migrate()` has
//! already been run (it includes a V7 migration with the same DDL), this
//! function is a no-op.

use rusqlite::Connection;
use tracing::debug;

use polkagent_feed::FeedError;

/// The full DDL for the feed store tables.
const FEED_SCHEMA: &str = r#"
-- ---------------------------------------------------------------------------
-- Feeds
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS feeds (
    id                        TEXT    PRIMARY KEY,
    name                      TEXT    NOT NULL,
    source_json               TEXT    NOT NULL,
    cursor_position           TEXT    NOT NULL DEFAULT '',
    cursor_last_processed_at  TEXT    NOT NULL,
    cursor_items_processed    INTEGER NOT NULL DEFAULT 0,
    status                    TEXT    NOT NULL DEFAULT 'active',
    agent_id                  TEXT    NOT NULL,
    created_at                TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_feeds_agent_id
    ON feeds(agent_id);

CREATE INDEX IF NOT EXISTS idx_feeds_status
    ON feeds(status);

-- ---------------------------------------------------------------------------
-- Feed Triggers
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS feed_triggers (
    id              TEXT    PRIMARY KEY,
    name            TEXT    NOT NULL,
    feed_id         TEXT    NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
    condition_json  TEXT    NOT NULL,
    action_json     TEXT    NOT NULL,
    cooldown_secs   INTEGER,
    last_fired_at   TEXT,
    enabled         INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX IF NOT EXISTS idx_feed_triggers_feed_id
    ON feed_triggers(feed_id);

-- ---------------------------------------------------------------------------
-- Feed Recipes
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS feed_recipes (
    id               TEXT    PRIMARY KEY,
    name             TEXT    NOT NULL,
    description      TEXT    NOT NULL DEFAULT '',
    version          TEXT    NOT NULL DEFAULT '1.0.0',
    source_json      TEXT    NOT NULL,
    trigger_json     TEXT    NOT NULL,
    action_json      TEXT    NOT NULL,
    parameters_json  TEXT    NOT NULL DEFAULT '[]'
);

-- ---------------------------------------------------------------------------
-- Feed Items (queue)
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS feed_items (
    id           TEXT    PRIMARY KEY,
    feed_id      TEXT    NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
    payload_json TEXT    NOT NULL,
    received_at  TEXT    NOT NULL,
    processed    INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_feed_items_feed_id
    ON feed_items(feed_id);

CREATE INDEX IF NOT EXISTS idx_feed_items_processed
    ON feed_items(processed);

CREATE INDEX IF NOT EXISTS idx_feed_items_feed_processed
    ON feed_items(feed_id, processed, received_at);
"#;

/// Create the feed store tables if they do not already exist.
///
/// This is idempotent: calling it multiple times (or after the main store's
/// V7 migration has already run) is safe.
///
/// # Errors
///
/// Returns [`FeedError::ProcessingError`] if the DDL fails.
pub fn migrate(conn: &Connection) -> polkagent_feed::Result<()> {
    debug!("applying feed store migration (IF NOT EXISTS)");
    conn.execute_batch(FEED_SCHEMA)
        .map_err(|e| FeedError::ProcessingError(format!("feed migration failed: {e}")))?;
    debug!("feed store migration complete");
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn open_mem() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch("PRAGMA foreign_keys = ON;").expect("pragma");
        conn
    }

    #[test]
    fn migrate_creates_all_tables() {
        let conn = open_mem();
        migrate(&conn).expect("migrate");

        let tables = ["feeds", "feed_triggers", "feed_recipes", "feed_items"];
        for table in &tables {
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get::<_, i64>(0),
                )
                .map(|n| n > 0)
                .expect("query");
            assert!(exists, "table '{table}' should exist after migration");
        }
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = open_mem();
        migrate(&conn).expect("first migrate");
        migrate(&conn).expect("second migrate");
    }
}
