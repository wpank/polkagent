//! Forward-only schema migrations for the Polkagent SQLite store.
//!
//! Each migration is a numbered SQL batch.  The `schema_migrations` table
//! tracks which migrations have been applied; the version is the migration
//! number (1-based, monotonic).
//!
//! # Adding a new migration
//!
//! 1. Add a new entry to [`MIGRATIONS`] with the next version number and a
//!    description string.  The SQL is the full DDL/DML for that migration.
//! 2. Do **not** modify existing entries — migrations are forward-only.
//! 3. Run the test suite to verify the migration applies cleanly.

use chrono::Utc;
use rusqlite::Connection;
use tracing::{debug, info};

use crate::error::{StoreError, StoreResult};

// ---------------------------------------------------------------------------
// Migration definitions
// ---------------------------------------------------------------------------

/// The full text of the initial schema.  Embedded as a compile-time constant.
const SCHEMA_V1: &str = include_str!("schema.sql");

/// V2: Event store tables (durable_events, diagnostic_events, global counter).
const SCHEMA_V2: &str = include_str!("v2_event_store.sql");

/// V3: Columns required by the `EffectStore` trait (state, retry_class,
/// worker_id, payload_json, attempt_id, run_id, consumed).
const SCHEMA_V3: &str = include_str!("v3_effect_store.sql");

/// V4: Payment store tables (payment_intents, payment_receipts, cost_records).
const SCHEMA_V4: &str = include_str!("v4_payment_store.sql");

/// V5: Conversation store tables (conversations, conversation_messages).
const SCHEMA_V5: &str = include_str!("v5_conversation_store.sql");

/// V6: Group store tables (groups, group_members).
const SCHEMA_V6: &str = include_str!("v6_group_store.sql");

/// V7: Feed store tables (feeds, feed_triggers, feed_recipes, feed_items).
const SCHEMA_V7: &str = include_str!("v7_feed_store.sql");

/// V8: Skill registry table (skills).
const SCHEMA_V8: &str = include_str!("v8_skill_store.sql");

/// V9: Add priority column to effect_intents for claim ordering.
const SCHEMA_V9: &str = include_str!("v9_intent_priority.sql");

/// V10: Add deadline_at column to runs table for persistent timeout deadlines.
const SCHEMA_V10: &str = include_str!("v10_run_deadline.sql");

/// V11: Add started_at column to runs table (missing from early databases).
const SCHEMA_V11: &str = include_str!("v11_run_started_at.sql");

/// V12: Enforce unique agent names with a UNIQUE index on agents(name).
const SCHEMA_V12: &str = include_str!("v12_agents_unique_name.sql");

/// Each entry is `(version, description, sql)`.
const MIGRATIONS: &[(u32, &str, &str)] = &[
    (1, "initial schema", SCHEMA_V1),
    (2, "event store tables", SCHEMA_V2),
    (3, "effect store trait columns", SCHEMA_V3),
    (4, "payment store tables", SCHEMA_V4),
    (5, "conversation store tables", SCHEMA_V5),
    (6, "group store tables", SCHEMA_V6),
    (7, "feed store tables", SCHEMA_V7),
    (8, "skill registry table", SCHEMA_V8),
    (9, "intent priority column", SCHEMA_V9),
    (10, "run deadline_at column", SCHEMA_V10),
    (11, "run started_at column", SCHEMA_V11),
    (12, "unique agent names index", SCHEMA_V12),
];

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Apply any pending migrations to `conn`.
///
/// This function is idempotent: already-applied migrations are skipped.
/// It must be called with the **writer** connection before any reads or writes.
pub fn migrate(conn: &Connection) -> StoreResult<()> {
    // Bootstrap: create the schema_migrations table if it does not yet exist.
    // We cannot rely on the schema already existing at this point.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version     INTEGER PRIMARY KEY,
            description TEXT    NOT NULL,
            applied_at  TEXT    NOT NULL,
            checksum    TEXT    NOT NULL
        );",
    )
    .map_err(|e| StoreError::Migration(format!("bootstrap schema_migrations: {e}")))?;

    let applied_version: u32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(|e| StoreError::Migration(format!("query max version: {e}")))?;

    debug!(applied_version, "current schema version");

    for &(version, description, sql) in MIGRATIONS {
        if version <= applied_version {
            debug!(version, "migration already applied, skipping");
            continue;
        }

        info!(version, description, "applying migration");

        // Execute the migration SQL.
        conn.execute_batch(sql)
            .map_err(|e| StoreError::Migration(format!("v{version} ({description}): {e}")))?;

        // Record the migration.
        let checksum = compute_checksum(sql);
        let applied_at = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO schema_migrations (version, description, applied_at, checksum)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![version, description, applied_at, checksum],
        )
        .map_err(|e| StoreError::Migration(format!("record v{version}: {e}")))?;

        info!(version, "migration applied successfully");
    }

    Ok(())
}

/// Return the current schema version recorded in the database, or `0` if the
/// `schema_migrations` table does not yet exist.
pub fn current_version(conn: &Connection) -> StoreResult<u32> {
    // The table might not exist if the database is brand new and `migrate` has
    // not been called yet.
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='schema_migrations'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false);

    if !exists {
        return Ok(0);
    }

    let version: u32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(|e| StoreError::Migration(format!("query current version: {e}")))?;

    Ok(version)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// A simple, stable checksum for migration SQL: the hex-encoded SHA-256 of
/// the UTF-8 bytes of the SQL string.
///
/// We use a hand-rolled hex encoding of a rudimentary checksum so that this
/// module has no dependency on external crypto crates.  The checksum is stored
/// for human-readable audit purposes; it is not verified on startup (which
/// would require re-hashing and could break if the SQL text is reformatted).
fn compute_checksum(sql: &str) -> String {
    // FNV-1a 64-bit hash, encoded as hex — lightweight and stable.
    let mut hash: u64 = 14_695_981_039_346_656_037;
    for byte in sql.bytes() {
        hash ^= u64(byte);
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    format!("{hash:016x}")
}

#[allow(clippy::cast_lossless)]
fn u64(b: u8) -> u64 {
    b as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn open_mem() -> Connection {
        Connection::open_in_memory().expect("in-memory db")
    }

    #[test]
    fn migrate_applies_all_cleanly() {
        let conn = open_mem();
        migrate(&conn).expect("migrate");
        let version = current_version(&conn).expect("version");
        assert_eq!(version, 12);
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = open_mem();
        migrate(&conn).expect("first migrate");
        migrate(&conn).expect("second migrate (idempotent)");
        let version = current_version(&conn).expect("version");
        assert_eq!(version, 12);
    }

    #[test]
    fn current_version_zero_before_migration() {
        let conn = open_mem();
        let version = current_version(&conn).expect("version");
        assert_eq!(version, 0);
    }

    #[test]
    fn schema_migrations_table_populated() {
        let conn = open_mem();
        migrate(&conn).expect("migrate");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, MIGRATIONS.len() as i64);
    }

    #[test]
    fn all_expected_tables_exist_after_migration() {
        let conn = open_mem();
        migrate(&conn).expect("migrate");

        let tables = [
            "agents",
            "runs",
            "turns",
            "steps",
            "effect_intents",
            "effect_attempts",
            "effect_outcomes",
            "artifacts",
            "artifact_bodies",
            "artifact_lineage",
            "run_events",
            "payment_intents",
            "payment_receipts",
            "cost_records",
            "conversations",
            "conversation_messages",
            "groups",
            "group_members",
            "feeds",
            "feed_triggers",
            "feed_recipes",
            "feed_items",
            "skills",
        ];

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
}
