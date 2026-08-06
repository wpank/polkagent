//! Forward-only schema migrations for the Polkagent `SQLite` store.
//!
//! Each migration is a numbered SQL batch.  The `schema_migrations` table
//! tracks which migrations have been applied; the version is the migration
//! number (1-based, monotonic).
//!
//! # Adding a new migration
//!
//! 1. Add a new entry to `MIGRATIONS` with the next version number and a
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

/// V2: Event store tables (`durable_events`, `diagnostic_events`, global counter).
const SCHEMA_V2: &str = include_str!("v2_event_store.sql");

/// V3: Columns required by the `EffectStore` trait (state, `retry_class`,
/// `worker_id`, `payload_json`, `attempt_id`, `run_id`, consumed).
const SCHEMA_V3: &str = include_str!("v3_effect_store.sql");

/// V4: Payment store tables (`payment_intents`, `payment_receipts`, `cost_records`).
const SCHEMA_V4: &str = include_str!("v4_payment_store.sql");

/// V5: Conversation store tables (conversations, `conversation_messages`).
const SCHEMA_V5: &str = include_str!("v5_conversation_store.sql");

/// V6: Group store tables (groups, `group_members`).
const SCHEMA_V6: &str = include_str!("v6_group_store.sql");

/// V7: Feed store tables (feeds, `feed_triggers`, `feed_recipes`, `feed_items`).
const SCHEMA_V7: &str = include_str!("v7_feed_store.sql");

/// V8: Skill registry table (skills).
const SCHEMA_V8: &str = include_str!("v8_skill_store.sql");

/// V9: Add priority column to `effect_intents` for claim ordering.
const SCHEMA_V9: &str = include_str!("v9_intent_priority.sql");

/// V10: Add `deadline_at` column to runs table for persistent timeout deadlines.
const SCHEMA_V10: &str = include_str!("v10_run_deadline.sql");

/// V11: Add `started_at` column to runs table (missing from early databases).
const SCHEMA_V11: &str = include_str!("v11_run_started_at.sql");

/// V12: Enforce unique agent names with a UNIQUE index on agents(name).
const SCHEMA_V12: &str = include_str!("v12_agents_unique_name.sql");

/// V13: Durable interaction turns, run correlations, and replayable events.
const SCHEMA_V13: &str = include_str!("v13_interaction_store.sql");

/// V14: Safe durable interaction configuration and lifecycle.
const SCHEMA_V14: &str = include_str!("v14_interaction_sessions.sql");

/// V15: Preserve artifact digest algorithm and data classification.
const SCHEMA_V15: &str = include_str!("v15_artifact_projection.sql");

/// V16: Protect exact effect outcome attempt/run lineage from mutation.
const SCHEMA_V16: &str = include_str!("v16_effect_outcome_lineage.sql");

/// V17: Preserve immutable interaction-origin working-directory provenance.
const SCHEMA_V17: &str = include_str!("v17_interaction_origin.sql");

/// V18: Durable approval/checkpoint coordinator and authoritative effect state.
const SCHEMA_V18: &str = include_str!("v18_approval_foundation.sql");

/// V19: Preserve the complete durable run-event metadata envelope.
const SCHEMA_V19: &str = include_str!("v19_run_event_metadata.sql");

/// V20: Normalize legacy approval coordinator event IDs to stable UUIDs.
const SCHEMA_V20: &str = include_str!("v20_approval_event_ids.sql");

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
    (13, "durable interaction turns and events", SCHEMA_V13),
    (14, "durable interaction sessions", SCHEMA_V14),
    (15, "complete artifact projection", SCHEMA_V15),
    (16, "immutable effect outcome lineage", SCHEMA_V16),
    (17, "immutable interaction origin cwd", SCHEMA_V17),
    (18, "durable approval and checkpoint foundation", SCHEMA_V18),
    (19, "complete run event metadata", SCHEMA_V19),
    (20, "normalize approval event identifiers", SCHEMA_V20),
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
        .is_ok_and(|n| n > 0);

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
// These migration assertions use `expect` to identify the exact schema setup
// or query that broke the in-memory migration fixture.
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::pool::SqlitePool;
    use polkagent_store_trait::event::{EventFilter, EventStore};
    use rusqlite::Connection;

    fn open_mem() -> Connection {
        Connection::open_in_memory().expect("in-memory db")
    }

    fn migrate_through(conn: &Connection, final_version: u32) {
        for &(version, description, sql) in MIGRATIONS {
            if version > final_version {
                break;
            }
            conn.execute_batch(sql).expect("apply historical migration");
            conn.execute(
                "INSERT INTO schema_migrations (version, description, applied_at, checksum)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    version,
                    description,
                    Utc::now().to_rfc3339(),
                    compute_checksum(sql)
                ],
            )
            .expect("record historical migration");
        }
    }

    #[test]
    fn migrate_applies_all_cleanly() {
        let conn = open_mem();
        migrate(&conn).expect("migrate");
        let version = current_version(&conn).expect("version");
        assert_eq!(version, 20);
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = open_mem();
        migrate(&conn).expect("first migrate");
        migrate(&conn).expect("second migrate (idempotent)");
        let version = current_version(&conn).expect("version");
        assert_eq!(version, 20);
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
        assert_eq!(
            count,
            i64::try_from(MIGRATIONS.len()).expect("migration count fits in i64")
        );
    }

    #[tokio::test]
    async fn v19_preserves_legacy_rowids_and_backfills_diagnostic_durability() {
        let directory = tempfile::tempdir().expect("legacy database directory");
        let path = directory.path().join("events-v18.sqlite");
        let durable_id = uuid::Uuid::now_v7().to_string();
        let diagnostic_id = uuid::Uuid::now_v7().to_string();
        let run_id = uuid::Uuid::now_v7().to_string();
        let agent_id = uuid::Uuid::now_v7().to_string();
        let (durable_rowid, diagnostic_rowid) = {
            let conn = Connection::open(&path).expect("open V18 database");
            migrate_through(&conn, 18);
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO agents (id, name, created_at, updated_at)
                 VALUES (?1, 'legacy-event-agent', ?2, ?2)",
                rusqlite::params![agent_id, now],
            )
            .expect("seed legacy agent");
            conn.execute(
                "INSERT INTO runs (id, agent_id, state, created_at, updated_at)
                 VALUES (?1, ?2, 'created', ?3, ?3)",
                rusqlite::params![run_id, agent_id, now],
            )
            .expect("seed legacy run");
            conn.execute(
                "INSERT INTO run_events
                    (id, run_id, sequence, kind, data_json, timestamp,
                     correlation_id, schema_version)
                 VALUES (?1, ?2, 1, 'run_created', '\"run_created\"', ?3,
                         NULL, 1)",
                rusqlite::params![durable_id, run_id, now],
            )
            .expect("seed legacy durable event");
            let durable_rowid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO run_events
                    (id, run_id, sequence, kind, data_json, timestamp,
                     correlation_id, schema_version)
                 VALUES (?1, ?2, 2, 'diagnostic:diagnostic_log', '{}', ?3,
                         NULL, 1)",
                rusqlite::params![diagnostic_id, run_id, now],
            )
            .expect("seed legacy diagnostic event");
            (durable_rowid, conn.last_insert_rowid())
        };

        let pool = SqlitePool::open(&path).expect("open legacy database through pool");
        migrate(&pool.writer()).expect("migrate V18 database through V20");
        assert_eq!(current_version(&pool.writer()).expect("version"), 20);

        let rows: Vec<(String, i64, String, Option<String>, String)> = {
            let writer = pool.writer();
            let mut statement = writer
                .prepare(
                    "SELECT id, rowid, durability, causation_id, scope_id
                     FROM run_events ORDER BY rowid",
                )
                .expect("prepare migrated event query");
            statement
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })
                .expect("query migrated events")
                .collect::<Result<_, _>>()
                .expect("collect migrated events")
        };
        assert_eq!(
            rows[0],
            (
                durable_id.clone(),
                durable_rowid,
                "durable".to_owned(),
                None,
                String::new()
            )
        );
        assert_eq!(
            rows[1],
            (
                diagnostic_id.clone(),
                diagnostic_rowid,
                "diagnostic".to_owned(),
                None,
                String::new()
            )
        );

        let durable = pool
            .read_from_cursor(0, 16)
            .await
            .expect("read migrated durable rows");
        assert_eq!(durable.len(), 1);
        assert_eq!(durable[0].id, durable_id);
        assert_eq!(
            durable[0].global_sequence,
            u64::try_from(durable_rowid).expect("positive durable rowid")
        );

        let all = pool
            .query(EventFilter {
                include_diagnostic: true,
                ..Default::default()
            })
            .await
            .expect("read migrated diagnostic row");
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].id, diagnostic_id);
        assert_eq!(all[1].event_type, "diagnostic_log");
        assert_eq!(all[1].durability, "diagnostic");
        assert_eq!(
            all[1].global_sequence,
            u64::try_from(diagnostic_rowid).expect("positive diagnostic rowid")
        );
    }

    #[tokio::test]
    async fn v20_normalizes_legacy_approval_event_ids_without_moving_cursors() {
        let directory = tempfile::tempdir().expect("legacy approval database directory");
        let path = directory.path().join("approval-events-v19.sqlite");
        let approval_id = uuid::Uuid::now_v7().to_string();
        let run_id = uuid::Uuid::now_v7().to_string();
        let agent_id = uuid::Uuid::now_v7().to_string();
        let legacy_ids = [
            format!("approval:{approval_id}:requested"),
            format!("approval:{approval_id}:resolved"),
            format!("approval:{approval_id}:effects-resolved"),
        ];
        let expected_ids = [
            format!("a1{}", &approval_id[2..]),
            format!("a2{}", &approval_id[2..]),
            format!("a3{}", &approval_id[2..]),
        ];
        let kinds = [
            polkagent_core::EventKind::ApprovalRequested {
                request_id: approval_id.clone(),
            },
            polkagent_core::EventKind::ApprovalGranted {
                approval_id: approval_id.clone(),
            },
            polkagent_core::EventKind::EffectsResolved,
        ];
        let event_types = ["approval_requested", "approval_granted", "effects_resolved"];
        let rowids = {
            let conn = Connection::open(&path).expect("open V19 database");
            migrate_through(&conn, 19);
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO agents (id, name, created_at, updated_at)
                 VALUES (?1, 'legacy-approval-event-agent', ?2, ?2)",
                rusqlite::params![agent_id, now],
            )
            .expect("seed legacy approval agent");
            conn.execute(
                "INSERT INTO runs (id, agent_id, state, created_at, updated_at)
                 VALUES (?1, ?2, 'running', ?3, ?3)",
                rusqlite::params![run_id, agent_id, now],
            )
            .expect("seed legacy approval run");
            let mut rowids = Vec::new();
            for (index, ((legacy_id, kind), event_type)) in
                legacy_ids.iter().zip(&kinds).zip(event_types).enumerate()
            {
                conn.execute(
                    "INSERT INTO run_events
                        (id, run_id, sequence, kind, data_json, timestamp,
                         correlation_id, schema_version, durability, scope_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, 'durable', '')",
                    rusqlite::params![
                        legacy_id,
                        run_id,
                        u64::try_from(index + 1).expect("small sequence"),
                        event_type,
                        serde_json::to_string(kind).expect("serialize approval event"),
                        now,
                        approval_id,
                    ],
                )
                .expect("seed legacy approval event");
                rowids.push(conn.last_insert_rowid());
            }
            rowids
        };

        let pool = SqlitePool::open(&path).expect("open V19 approval database through pool");
        migrate(&pool.writer()).expect("migrate V19 database to V20");
        assert_eq!(current_version(&pool.writer()).expect("version"), 20);

        let events = pool
            .read_from_cursor(0, 16)
            .await
            .expect("read normalized approval events");
        assert_eq!(events.len(), 3);
        for (index, event) in events.iter().enumerate() {
            assert_eq!(event.id, expected_ids[index]);
            assert!(uuid::Uuid::parse_str(&event.id).is_ok());
            assert_eq!(
                event.global_sequence,
                u64::try_from(rowids[index]).expect("positive approval event rowid")
            );
            polkagent_event::decode_run_event(event.clone())
                .expect("decode normalized approval event");
        }
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
            "interaction_turns",
            "interaction_turn_runs",
            "interaction_events",
            "interaction_sessions",
            "approval_requests",
            "execution_checkpoints",
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

    #[test]
    fn artifact_projection_columns_exist_with_legacy_defaults() {
        let conn = open_mem();
        migrate(&conn).expect("migrate");
        let mut statement = conn
            .prepare("PRAGMA table_info(artifacts)")
            .expect("prepare artifact column query");
        let columns = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, Option<String>>(4)?))
            })
            .expect("query artifact columns")
            .collect::<Result<std::collections::HashMap<_, _>, _>>()
            .expect("collect artifact columns");

        assert_eq!(
            columns.get("algorithm").and_then(Option::as_deref),
            Some("'blake3'")
        );
        assert_eq!(
            columns.get("classification").and_then(Option::as_deref),
            Some("'public'")
        );
    }

    #[test]
    fn artifact_projection_migration_backfills_legacy_rows_truthfully() {
        let conn = open_mem();
        conn.execute_batch(SCHEMA_V1).expect("apply legacy schema");
        let artifact_id = uuid::Uuid::now_v7().to_string();
        conn.execute(
            "INSERT INTO artifacts
                (id, kind, digest_hex, size_bytes, metadata_json, created_at)
             VALUES (?1, 'legacy', ?2, 0, '{}', '2024-01-01T00:00:00Z')",
            rusqlite::params![artifact_id, blake3::hash(b"").to_hex().to_string()],
        )
        .expect("insert legacy artifact");

        conn.execute_batch(SCHEMA_V15)
            .expect("apply artifact projection migration");
        let (algorithm, classification): (String, String) = conn
            .query_row(
                "SELECT algorithm, classification FROM artifacts WHERE id = ?1",
                [&artifact_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read migrated artifact");
        assert_eq!(algorithm, "blake3");
        assert_eq!(classification, "public");
    }

    #[test]
    fn effect_outcome_lineage_is_immutable_but_consumption_can_advance() {
        let conn = open_mem();
        migrate(&conn).expect("migrate");
        conn.execute_batch("PRAGMA foreign_keys = OFF;")
            .expect("isolate trigger from foreign-key fixtures");
        let outcome_id = uuid::Uuid::now_v7().to_string();
        conn.execute(
            "INSERT INTO effect_outcomes
                (id, intent_id, status, result_json, created_at,
                 attempt_id, run_id, consumed)
             VALUES (?1, ?2, 'success', '{\"variant\":\"success\"}',
                     '2024-01-01T00:00:00Z', ?3, ?4, 0)",
            rusqlite::params![
                outcome_id,
                uuid::Uuid::now_v7().to_string(),
                uuid::Uuid::now_v7().to_string(),
                uuid::Uuid::now_v7().to_string(),
            ],
        )
        .expect("seed outcome");

        conn.execute(
            "UPDATE effect_outcomes SET consumed = 1 WHERE id = ?1",
            [&outcome_id],
        )
        .expect("consumption is the sole mutable field");
        let consumed: bool = conn
            .query_row(
                "SELECT consumed FROM effect_outcomes WHERE id = ?1",
                [&outcome_id],
                |row| row.get(0),
            )
            .expect("read consumed flag");
        assert!(consumed);

        for column in ["attempt_id", "run_id"] {
            let error = conn
                .execute(
                    &format!("UPDATE effect_outcomes SET {column} = NULL WHERE id = ?1"),
                    [&outcome_id],
                )
                .expect_err("exact outcome lineage must reject tampering");
            assert!(error.to_string().contains("lineage are immutable"));
        }
    }

    #[test]
    fn lineage_migration_is_registered_with_a_nonempty_checksum() {
        let conn = open_mem();
        migrate(&conn).expect("migrate");
        let (description, checksum): (String, String) = conn
            .query_row(
                "SELECT description, checksum FROM schema_migrations WHERE version = 16",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("load v16 registration");
        assert_eq!(description, "immutable effect outcome lineage");
        assert!(!checksum.is_empty());
    }

    #[test]
    fn origin_migration_preserves_legacy_unknown_and_guards_new_rows() {
        let conn = open_mem();
        conn.execute_batch("CREATE TABLE conversations (id TEXT PRIMARY KEY);")
            .expect("create referenced legacy conversation table");
        conn.execute_batch(SCHEMA_V14)
            .expect("apply legacy interaction session schema");
        let legacy_id = uuid::Uuid::now_v7().to_string();
        conn.execute("INSERT INTO conversations (id) VALUES (?1)", [&legacy_id])
            .expect("seed legacy conversation");
        conn.execute(
            "INSERT INTO interaction_sessions
                (conversation_id, config_json, state, created_at, updated_at)
             VALUES (?1, '{}', 'active', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
            [&legacy_id],
        )
        .expect("seed pre-provenance interaction");

        conn.execute_batch(SCHEMA_V17)
            .expect("apply interaction origin migration");
        let legacy_origin: Option<String> = conn
            .query_row(
                "SELECT origin_working_directory FROM interaction_sessions
                 WHERE conversation_id = ?1",
                [&legacy_id],
                |row| row.get(0),
            )
            .expect("read legacy origin");
        assert_eq!(legacy_origin, None, "migration must not invent provenance");

        let new_id = uuid::Uuid::now_v7().to_string();
        conn.execute("INSERT INTO conversations (id) VALUES (?1)", [&new_id])
            .expect("seed current conversation");
        conn.execute(
            "INSERT INTO interaction_sessions
                (conversation_id, config_json, state, created_at, updated_at,
                 origin_working_directory)
             VALUES (?1, '{}', 'active', '2024-01-01T00:00:00Z',
                     '2024-01-01T00:00:00Z', '/workspace/exact')",
            [&new_id],
        )
        .expect("new interaction records exact origin");

        let missing_id = uuid::Uuid::now_v7().to_string();
        conn.execute("INSERT INTO conversations (id) VALUES (?1)", [&missing_id])
            .expect("seed missing-origin conversation");
        let missing_origin_error = conn
            .execute(
                "INSERT INTO interaction_sessions
                    (conversation_id, config_json, state, created_at, updated_at)
                 VALUES (?1, '{}', 'active', '2024-01-01T00:00:00Z',
                         '2024-01-01T00:00:00Z')",
                [&missing_id],
            )
            .expect_err("new interaction without provenance must fail");
        assert!(missing_origin_error
            .to_string()
            .contains("origin working directory"));

        let mutation_error = conn
            .execute(
                "UPDATE interaction_sessions SET origin_working_directory = '/workspace/other'
                 WHERE conversation_id = ?1",
                [&new_id],
            )
            .expect_err("durable interaction origin must be immutable");
        assert!(mutation_error.to_string().contains("immutable"));
    }

    #[test]
    fn interaction_origin_migration_is_registered_with_a_nonempty_checksum() {
        let conn = open_mem();
        migrate(&conn).expect("migrate");
        let (description, checksum): (String, String) = conn
            .query_row(
                "SELECT description, checksum FROM schema_migrations WHERE version = 17",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("load v17 registration");
        assert_eq!(description, "immutable interaction origin cwd");
        assert!(!checksum.is_empty());
    }

    #[test]
    fn interaction_schema_enforces_ordinals_and_one_terminal_event() {
        let conn = open_mem();
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("enable foreign keys");
        migrate(&conn).expect("migrate");

        let now = Utc::now().to_rfc3339();
        let agent_id = uuid::Uuid::now_v7().to_string();
        let conversation_id = uuid::Uuid::now_v7().to_string();
        let message_id = uuid::Uuid::now_v7().to_string();
        let turn_id = uuid::Uuid::now_v7().to_string();
        let run_id = uuid::Uuid::now_v7().to_string();

        conn.execute(
            "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at)
             VALUES (?1, 'agent', 'active', '{}', ?2, ?2)",
            rusqlite::params![agent_id, now],
        )
        .expect("insert agent");
        conn.execute(
            "INSERT INTO conversations
                 (id, agent_id, message_count, metadata_json, created_at, updated_at)
             VALUES (?1, ?2, 1, '{}', ?3, ?3)",
            rusqlite::params![conversation_id, agent_id, now],
        )
        .expect("insert conversation");
        conn.execute(
            "INSERT INTO conversation_messages
                 (id, conversation_id, role, content_json, created_at)
             VALUES (?1, ?2, 'user', '{\"type\":\"text\",\"text\":\"hello\"}', ?3)",
            rusqlite::params![message_id, conversation_id, now],
        )
        .expect("insert message");
        conn.execute(
            "INSERT INTO runs
                 (id, agent_id, conversation_id, state, params_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'created', '{}', ?4, ?4)",
            rusqlite::params![run_id, agent_id, conversation_id, now],
        )
        .expect("insert run");
        conn.execute(
            "INSERT INTO interaction_turns
                 (id, conversation_id, ordinal, state, target_json, config_json,
                  user_message_id, started_at)
             VALUES (?1, ?2, 1, 'pending', '{}', '{}', ?3, ?4)",
            rusqlite::params![turn_id, conversation_id, message_id, now],
        )
        .expect("insert interaction turn");
        conn.execute(
            "INSERT INTO interaction_turn_runs (turn_id, run_id, role_json, ordinal)
             VALUES (?1, ?2, '\"primary\"', 1)",
            rusqlite::params![turn_id, run_id],
        )
        .expect("link run");

        let insert_terminal = |event_id: String, sequence: i64| {
            conn.execute(
                "INSERT INTO interaction_events
                     (id, conversation_id, turn_id, sequence, kind, payload_json,
                      is_terminal, created_at)
                 VALUES (?1, ?2, ?3, ?4, 'turn_completed', '{}', 1, ?5)",
                rusqlite::params![event_id, conversation_id, turn_id, sequence, now],
            )
        };
        insert_terminal(uuid::Uuid::now_v7().to_string(), 1).expect("insert first terminal event");
        assert!(
            insert_terminal(uuid::Uuid::now_v7().to_string(), 2).is_err(),
            "a second terminal event for one turn must be rejected"
        );

        let duplicate_ordinal = conn.execute(
            "INSERT INTO interaction_turns
                 (id, conversation_id, ordinal, state, target_json, config_json,
                  user_message_id, started_at)
             VALUES (?1, ?2, 1, 'pending', '{}', '{}', ?3, ?4)",
            rusqlite::params![
                uuid::Uuid::now_v7().to_string(),
                conversation_id,
                message_id,
                now
            ],
        );
        assert!(
            duplicate_ordinal.is_err(),
            "turn ordinals must be unique within a conversation"
        );
    }
}
