//! Schema migration for the group store tables.
//!
//! This module provides an idempotent migration that creates the `groups` and
//! `group_members` tables.  It is designed to be called against a database that
//! may already have these tables (e.g. from the main `polkagent-store-sqlite`
//! migration chain) — `CREATE TABLE IF NOT EXISTS` ensures no conflict.
//!
//! # Usage
//!
//! ```no_run
//! use polkagent_store_sqlite::SqlitePool;
//! use polkagent_store_sqlite_group::migrations;
//!
//! let pool = SqlitePool::open("/tmp/test.db").expect("open");
//! let writer = pool.writer();
//! migrations::migrate_groups(&writer).expect("migrate");
//! ```

use rusqlite::Connection;
use tracing::debug;

/// Error type for migration failures.
#[derive(Debug, thiserror::Error)]
#[error("group migration failed: {0}")]
pub struct MigrationError(String);

/// Apply the group store migration to `conn`.
///
/// This function is idempotent — it uses `CREATE TABLE IF NOT EXISTS` and
/// `CREATE INDEX IF NOT EXISTS`, so re-running it is safe and free of side
/// effects.
pub fn migrate_groups(conn: &Connection) -> Result<(), MigrationError> {
    debug!("applying group store migration");

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS groups (
            id               TEXT    PRIMARY KEY,
            name             TEXT    NOT NULL,
            description      TEXT    NOT NULL DEFAULT '',
            owner_agent_id   TEXT    NOT NULL,
            quorum_policy    TEXT    NOT NULL DEFAULT 'majority',
            budget_json      TEXT    NOT NULL DEFAULT '{}',
            created_at       TEXT    NOT NULL,
            updated_at       TEXT    NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_groups_owner
            ON groups(owner_agent_id);

        CREATE TABLE IF NOT EXISTS group_members (
            group_id           TEXT    NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
            agent_id           TEXT    NOT NULL,
            role               TEXT    NOT NULL DEFAULT 'worker',
            grant_override_json TEXT,
            joined_at          TEXT    NOT NULL,
            PRIMARY KEY (group_id, agent_id)
        );

        CREATE INDEX IF NOT EXISTS idx_group_members_agent
            ON group_members(agent_id);
        ",
    )
    .map_err(|e| MigrationError(format!("{e}")))?;

    debug!("group store migration applied successfully");
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
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("enable FKs");
        conn
    }

    #[test]
    fn migrate_creates_tables() {
        let conn = open_mem();
        migrate_groups(&conn).expect("migrate");

        for table in &["groups", "group_members"] {
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
        migrate_groups(&conn).expect("first");
        migrate_groups(&conn).expect("second (idempotent)");
    }

    #[test]
    fn foreign_key_enforced() {
        let conn = open_mem();
        migrate_groups(&conn).expect("migrate");

        let result = conn.execute(
            "INSERT INTO group_members (group_id, agent_id, role, joined_at)
             VALUES ('nonexistent', 'agent-1', 'worker', '2025-01-01T00:00:00Z')",
            [],
        );
        assert!(result.is_err(), "FK should reject dangling group_id");
    }

    #[test]
    fn cascade_delete_removes_members() {
        let conn = open_mem();
        migrate_groups(&conn).expect("migrate");

        conn.execute(
            "INSERT INTO groups (id, name, owner_agent_id, quorum_policy, budget_json, created_at, updated_at)
             VALUES ('g1', 'test', 'owner', 'majority', '{}', '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')",
            [],
        )
        .expect("insert group");

        conn.execute(
            "INSERT INTO group_members (group_id, agent_id, role, joined_at)
             VALUES ('g1', 'a1', 'worker', '2025-01-01T00:00:00Z')",
            [],
        )
        .expect("insert member");

        conn.execute("DELETE FROM groups WHERE id = 'g1'", [])
            .expect("delete group");

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM group_members WHERE group_id = 'g1'",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(count, 0, "cascade should remove members");
    }
}
