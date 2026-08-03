//! [`EventStore`] implementation for [`SqlitePool`].
//!
//! This module bridges the async [`EventStore`] trait (from
//! `polkagent-store-trait`) to the synchronous rusqlite operations provided by
//! [`SqlitePool`].  Every trait method clones the pool, spawns the query on a
//! blocking thread via [`tokio::task::spawn_blocking`], and awaits the result.
//!
//! ## Schema
//!
//! Events are stored in the `run_events` table with columns:
//!
//!   `id, run_id, sequence, kind, data_json, timestamp, correlation_id, schema_version`
//!
//! Diagnostic events are stored in the same table; the `kind` column value
//! distinguishes them (prefixed with `diagnostic:` by convention).
//!
//! Global ordering uses SQLite's `rowid` as a monotonic proxy since no
//! dedicated `global_sequence` column exists in the production schema.
//!
//! ## Terminal-event invariant
//!
//! The four terminal event types (`run_completed`, `run_failed`,
//! `run_cancelled`, `run_timed_out`) are tracked: before appending a durable
//! event whose `kind` is terminal, the implementation checks whether the
//! run already has a terminal event and rejects duplicates with
//! [`EventStoreError::DuplicateTerminalEvent`].

use async_trait::async_trait;
use polkagent_core::RunId;
use polkagent_store_trait::event::{EventFilter, EventStore, EventStoreError, StoredEvent};
use rusqlite::{params, Connection};

use crate::pool::SqlitePool;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The four canonical terminal event type names.
const TERMINAL_EVENT_TYPES: &[&str] = &[
    "run_completed",
    "run_failed",
    "run_cancelled",
    "run_timed_out",
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map a `rusqlite::Error` into an [`EventStoreError`].
fn map_rusqlite(err: rusqlite::Error) -> EventStoreError {
    EventStoreError::Backend(Box::new(err))
}

/// Map a `tokio::task::JoinError` into an [`EventStoreError`].
fn map_join(err: tokio::task::JoinError) -> EventStoreError {
    EventStoreError::Backend(Box::new(err))
}

/// Returns `true` if `event_type` is one of the four terminal event types.
fn is_terminal(event_type: &str) -> bool {
    TERMINAL_EVENT_TYPES.contains(&event_type)
}

/// Read a single `StoredEvent` from the current row of a `rusqlite::Row`.
///
/// Expected column order (matching `RUN_EVENTS_COLS`):
///   0: id, 1: run_id, 2: sequence, 3: kind, 4: data_json,
///   5: timestamp, 6: correlation_id, 7: schema_version, 8: rowid
fn row_to_stored_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredEvent> {
    let data_json_str: String = row.get(4)?;
    let payload: serde_json::Value = serde_json::from_str(&data_json_str).unwrap_or_default();
    let correlation_id: Option<String> = row.get(6)?;
    // Use rowid as global_sequence proxy.
    let rowid: u64 = row.get(8)?;

    Ok(StoredEvent {
        id: row.get(0)?,
        run_id: row.get(1)?,
        sequence: row.get(2)?,
        event_type: row.get(3)?,
        payload,
        timestamp: row.get(5)?,
        correlation_id: correlation_id.unwrap_or_default(),
        schema_version: row.get(7)?,
        // Fields not stored in run_events -- use defaults.
        global_sequence: rowid,
        conversation_id: None,
        causation_id: None,
        scope_id: String::new(),
        durability: String::new(),
        trace_id: None,
        span_id: None,
    })
}

/// The standard SELECT column list for `run_events`, including `rowid` for
/// global ordering.
const RUN_EVENTS_COLS: &str = "\
    id, run_id, sequence, kind, data_json, \
    timestamp, correlation_id, schema_version, rowid";

/// Check whether a run already has a terminal event in `run_events`.
fn check_terminal(conn: &Connection, run_id: &str) -> Result<bool, EventStoreError> {
    let count: u64 = conn
        .query_row(
            "SELECT COUNT(*) FROM run_events \
             WHERE run_id = ?1 AND kind IN ('run_completed','run_failed','run_cancelled','run_timed_out')",
            params![run_id],
            |r| r.get(0),
        )
        .map_err(map_rusqlite)?;

    Ok(count > 0)
}

/// Fetch the current max sequence for a run from `run_events`.
fn current_max_sequence(conn: &Connection, run_id: &str) -> Result<u64, EventStoreError> {
    let max: Option<u64> = conn
        .query_row(
            "SELECT MAX(sequence) FROM run_events WHERE run_id = ?1",
            params![run_id],
            |r| r.get(0),
        )
        .map_err(map_rusqlite)?;

    Ok(max.unwrap_or(0))
}

// ---------------------------------------------------------------------------
// EventStore impl for SqlitePool
// ---------------------------------------------------------------------------

#[async_trait]
impl EventStore for SqlitePool {
    async fn append_durable(&self, event: StoredEvent) -> Result<StoredEvent, EventStoreError> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();

            // --- Invariant checks inside the writer lock ---

            // 1. Non-monotonic sequence check.
            let current_max = current_max_sequence(&writer, &event.run_id)?;
            if event.sequence <= current_max {
                return Err(EventStoreError::NonMonotonicSequence {
                    run_id: event.run_id.clone(),
                    current: current_max,
                    proposed: event.sequence,
                });
            }

            // 2. Terminal-event uniqueness check.
            if is_terminal(&event.event_type) && check_terminal(&writer, &event.run_id)? {
                return Err(EventStoreError::DuplicateTerminalEvent {
                    run_id: event.run_id.clone(),
                });
            }

            // 3. Serialise payload.
            let payload_str = serde_json::to_string(&event.payload)
                .map_err(|e| EventStoreError::Serialisation(e.to_string()))?;

            // 4. INSERT into run_events.
            writer
                .execute(
                    "INSERT INTO run_events \
                     (id, run_id, sequence, kind, data_json, \
                      timestamp, correlation_id, schema_version) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        event.id,
                        event.run_id,
                        event.sequence,
                        event.event_type,
                        payload_str,
                        event.timestamp,
                        event.correlation_id,
                        event.schema_version,
                    ],
                )
                .map_err(|e| {
                    if crate::error::StoreError::is_unique_violation(&e) {
                        EventStoreError::Conflict(format!(
                            "event with id {} already exists",
                            event.id
                        ))
                    } else if crate::error::StoreError::is_fk_violation(&e) {
                        EventStoreError::Backend(Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!(
                                "foreign key constraint failed for run_id {}; \
                                 ensure the run exists before appending events",
                                event.run_id
                            ),
                        )))
                    } else {
                        map_rusqlite(e)
                    }
                })?;

            // 5. Retrieve the rowid assigned by SQLite to use as global_sequence.
            let rowid = writer.last_insert_rowid() as u64;

            // 6. Return the completed event with assigned global_sequence.
            Ok(StoredEvent {
                global_sequence: rowid,
                ..event
            })
        })
        .await
        .map_err(map_join)?
    }

    async fn append_diagnostic(
        &self,
        event: StoredEvent,
        _expires_at: String,
    ) -> Result<(), EventStoreError> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();

            let payload_str = serde_json::to_string(&event.payload)
                .map_err(|e| EventStoreError::Serialisation(e.to_string()))?;

            // Store diagnostic events in run_events with a `diagnostic:` kind
            // prefix so they can be distinguished from durable events.
            let kind = format!("diagnostic:{}", event.event_type);

            writer
                .execute(
                    "INSERT INTO run_events \
                     (id, run_id, sequence, kind, data_json, \
                      timestamp, correlation_id, schema_version) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        event.id,
                        event.run_id,
                        event.sequence,
                        kind,
                        payload_str,
                        event.timestamp,
                        event.correlation_id,
                        event.schema_version,
                    ],
                )
                .map_err(map_rusqlite)?;

            Ok(())
        })
        .await
        .map_err(map_join)?
    }

    async fn read_from_cursor(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let mut stmt = writer
                .prepare(&format!(
                    "SELECT {RUN_EVENTS_COLS} FROM run_events \
                     WHERE rowid > ?1 AND kind NOT LIKE 'diagnostic:%' \
                     ORDER BY rowid ASC \
                     LIMIT ?2"
                ))
                .map_err(map_rusqlite)?;

            let rows = stmt
                .query_map(params![cursor, limit as u64], row_to_stored_event)
                .map_err(map_rusqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_rusqlite)?;

            Ok(rows)
        })
        .await
        .map_err(map_join)?
    }

    async fn read_run_events(
        &self,
        run_id: RunId,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        let pool = self.clone();
        let run_id_str = run_id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let mut stmt = writer
                .prepare(&format!(
                    "SELECT {RUN_EVENTS_COLS} FROM run_events \
                     WHERE run_id = ?1 AND kind NOT LIKE 'diagnostic:%' \
                     ORDER BY sequence ASC"
                ))
                .map_err(map_rusqlite)?;

            let rows = stmt
                .query_map(params![run_id_str], row_to_stored_event)
                .map_err(map_rusqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_rusqlite)?;

            Ok(rows)
        })
        .await
        .map_err(map_join)?
    }

    async fn query(&self, filter: EventFilter) -> Result<Vec<StoredEvent>, EventStoreError> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();

            // Build the query dynamically based on which filter fields are set.
            let mut conditions: Vec<String> = Vec::new();
            let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
            let mut param_idx = 1u32;

            // Filter: run_id
            if let Some(ref run_id) = filter.run_id {
                conditions.push(format!("run_id = ?{param_idx}"));
                param_values.push(Box::new(run_id.to_string()));
                param_idx += 1;
            }

            // Filter: event_types (IN clause) -- maps to `kind` column
            if !filter.event_types.is_empty() {
                let placeholders: Vec<String> = filter
                    .event_types
                    .iter()
                    .map(|t| {
                        let p = format!("?{param_idx}");
                        param_values.push(Box::new(t.clone()));
                        param_idx += 1;
                        p
                    })
                    .collect();
                conditions.push(format!("kind IN ({})", placeholders.join(",")));
            }

            // Filter: since_global_sequence -- maps to rowid
            if let Some(since) = filter.since_global_sequence {
                conditions.push(format!("rowid >= ?{param_idx}"));
                param_values.push(Box::new(since));
                param_idx += 1;
            }

            // Exclude diagnostic events unless requested.
            if !filter.include_diagnostic {
                conditions.push("kind NOT LIKE 'diagnostic:%'".to_string());
            }

            let where_clause = if conditions.is_empty() {
                String::new()
            } else {
                format!("WHERE {}", conditions.join(" AND "))
            };

            let limit_clause = if let Some(limit) = filter.limit {
                let clause = format!("LIMIT ?{param_idx}");
                param_values.push(Box::new(limit as u64));
                // param_idx is no longer used after this, suppress the warning.
                let _ = param_idx;
                clause
            } else {
                String::new()
            };

            let sql = format!(
                "SELECT {RUN_EVENTS_COLS} FROM run_events \
                 {where_clause} ORDER BY rowid ASC {limit_clause}"
            );

            let params_for_query: Vec<&dyn rusqlite::types::ToSql> =
                param_values.iter().map(|b| b.as_ref()).collect();

            let mut stmt = writer.prepare(&sql).map_err(map_rusqlite)?;

            let rows = stmt
                .query_map(
                    rusqlite::params_from_iter(params_for_query.iter()),
                    row_to_stored_event,
                )
                .map_err(map_rusqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_rusqlite)?;

            Ok(rows)
        })
        .await
        .map_err(map_join)?
    }

    async fn max_sequence(&self, run_id: RunId) -> Result<u64, EventStoreError> {
        let pool = self.clone();
        let run_id_str = run_id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            current_max_sequence(&writer, &run_id_str)
        })
        .await
        .map_err(map_join)?
    }

    async fn has_terminal_event(&self, run_id: RunId) -> Result<bool, EventStoreError> {
        let pool = self.clone();
        let run_id_str = run_id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            check_terminal(&writer, &run_id_str)
        })
        .await
        .map_err(map_join)?
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations;

    /// Create an in-memory pool with all migrations applied.
    fn setup_pool() -> SqlitePool {
        let pool = SqlitePool::open_in_memory().expect("open in-memory pool");
        {
            let writer = pool.writer();
            migrations::migrate(&writer).expect("migrate");
        }
        pool
    }

    /// Build a minimal `StoredEvent` for testing.
    fn make_event(run_id: &str, sequence: u64, event_type: &str) -> StoredEvent {
        StoredEvent {
            id: uuid::Uuid::now_v7().to_string(),
            event_type: event_type.to_string(),
            sequence,
            global_sequence: 0, // assigned by the store
            run_id: run_id.to_string(),
            conversation_id: None,
            correlation_id: "corr-1".to_string(),
            causation_id: None,
            scope_id: "scope-1".to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            durability: "durable".to_string(),
            payload: serde_json::json!({"key": "value"}),
            trace_id: None,
            span_id: None,
            schema_version: 1,
        }
    }

    /// Insert a dummy run row into the `runs` table so that the FK constraint
    /// on `run_events.run_id` is satisfied. Also inserts a dummy agent.
    fn insert_dummy_run(pool: &SqlitePool, run_id: &str) {
        let writer = pool.writer();
        let now = chrono::Utc::now().to_rfc3339();
        let agent_id = uuid::Uuid::now_v7().to_string();
        writer
            .execute(
                "INSERT OR IGNORE INTO agents (id, name, created_at, updated_at) VALUES (?1, 'test-agent', ?2, ?2)",
                params![agent_id, now],
            )
            .expect("insert agent");
        writer
            .execute(
                "INSERT OR IGNORE INTO runs (id, agent_id, state, created_at, updated_at) VALUES (?1, ?2, 'created', ?3, ?3)",
                params![run_id, agent_id, now],
            )
            .expect("insert run");
    }

    #[tokio::test]
    async fn append_durable_assigns_global_sequence() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_dummy_run(&pool, &run_id);

        let evt = make_event(&run_id, 1, "run_started");
        let stored = pool.append_durable(evt).await.expect("append");

        assert!(stored.global_sequence > 0);
        assert_eq!(stored.sequence, 1);
    }

    #[tokio::test]
    async fn append_durable_increments_global_sequence() {
        let pool = setup_pool();
        let run_a = uuid::Uuid::now_v7().to_string();
        let run_b = uuid::Uuid::now_v7().to_string();
        insert_dummy_run(&pool, &run_a);
        insert_dummy_run(&pool, &run_b);

        let e1 = pool
            .append_durable(make_event(&run_a, 1, "run_started"))
            .await
            .expect("e1");
        let e2 = pool
            .append_durable(make_event(&run_b, 1, "run_started"))
            .await
            .expect("e2");
        let e3 = pool
            .append_durable(make_event(&run_a, 2, "turn_started"))
            .await
            .expect("e3");

        // Global sequence (rowid) must be strictly increasing.
        assert!(e1.global_sequence < e2.global_sequence);
        assert!(e2.global_sequence < e3.global_sequence);
    }

    #[tokio::test]
    async fn append_durable_rejects_non_monotonic_sequence() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_dummy_run(&pool, &run_id);

        pool.append_durable(make_event(&run_id, 1, "run_started"))
            .await
            .expect("first");

        // Same sequence number should fail.
        let err = pool
            .append_durable(make_event(&run_id, 1, "turn_started"))
            .await
            .expect_err("should fail");

        assert!(
            matches!(err, EventStoreError::NonMonotonicSequence { .. }),
            "expected NonMonotonicSequence, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn append_durable_rejects_lower_sequence() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_dummy_run(&pool, &run_id);

        pool.append_durable(make_event(&run_id, 5, "run_started"))
            .await
            .expect("first");

        let err = pool
            .append_durable(make_event(&run_id, 3, "turn_started"))
            .await
            .expect_err("should fail");

        assert!(matches!(err, EventStoreError::NonMonotonicSequence { .. }));
    }

    #[tokio::test]
    async fn append_durable_rejects_duplicate_terminal_event() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_dummy_run(&pool, &run_id);

        pool.append_durable(make_event(&run_id, 1, "run_started"))
            .await
            .expect("start");

        pool.append_durable(make_event(&run_id, 2, "run_completed"))
            .await
            .expect("first terminal");

        let err = pool
            .append_durable(make_event(&run_id, 3, "run_failed"))
            .await
            .expect_err("should reject second terminal");

        assert!(
            matches!(err, EventStoreError::DuplicateTerminalEvent { .. }),
            "expected DuplicateTerminalEvent, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn append_durable_rejects_duplicate_id() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_dummy_run(&pool, &run_id);

        let evt = make_event(&run_id, 1, "run_started");
        let dup_id = evt.id.clone();
        pool.append_durable(evt).await.expect("first");

        let mut second = make_event(&run_id, 2, "turn_started");
        second.id = dup_id;
        let err = pool
            .append_durable(second)
            .await
            .expect_err("should reject duplicate id");

        assert!(
            matches!(err, EventStoreError::Conflict(_)),
            "expected Conflict, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn read_run_events_returns_ordered_events() {
        let pool = setup_pool();
        let run_id_str = uuid::Uuid::now_v7().to_string();
        let run_id: RunId = run_id_str.parse().expect("parse RunId");
        insert_dummy_run(&pool, &run_id_str);

        pool.append_durable(make_event(&run_id_str, 1, "run_started"))
            .await
            .expect("e1");
        pool.append_durable(make_event(&run_id_str, 2, "turn_started"))
            .await
            .expect("e2");
        pool.append_durable(make_event(&run_id_str, 3, "run_completed"))
            .await
            .expect("e3");

        let events = pool.read_run_events(run_id).await.expect("read");
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].sequence, 1);
        assert_eq!(events[1].sequence, 2);
        assert_eq!(events[2].sequence, 3);
    }

    #[tokio::test]
    async fn read_from_cursor_paginates() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_dummy_run(&pool, &run_id);

        let mut rowids = Vec::new();
        for seq in 1..=5 {
            let stored = pool
                .append_durable(make_event(&run_id, seq, "turn_started"))
                .await
                .expect("append");
            rowids.push(stored.global_sequence);
        }

        // Read after cursor = rowid of event 2, limit 2.
        let page = pool.read_from_cursor(rowids[1], 2).await.expect("read");
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].global_sequence, rowids[2]);
        assert_eq!(page[1].global_sequence, rowids[3]);
    }

    #[tokio::test]
    async fn read_from_cursor_returns_empty_past_end() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_dummy_run(&pool, &run_id);

        pool.append_durable(make_event(&run_id, 1, "run_started"))
            .await
            .expect("append");

        let page = pool.read_from_cursor(100_000, 10).await.expect("read");
        assert!(page.is_empty());
    }

    #[tokio::test]
    async fn max_sequence_returns_zero_for_new_run() {
        let pool = setup_pool();
        let run_id = RunId::new();

        let max = pool.max_sequence(run_id).await.expect("max_sequence");
        assert_eq!(max, 0);
    }

    #[tokio::test]
    async fn max_sequence_reflects_appended_events() {
        let pool = setup_pool();
        let run_id_str = uuid::Uuid::now_v7().to_string();
        let run_id: RunId = run_id_str.parse().expect("parse");
        insert_dummy_run(&pool, &run_id_str);

        pool.append_durable(make_event(&run_id_str, 1, "run_started"))
            .await
            .expect("e1");
        pool.append_durable(make_event(&run_id_str, 2, "turn_started"))
            .await
            .expect("e2");

        let max = pool.max_sequence(run_id).await.expect("max_sequence");
        assert_eq!(max, 2);
    }

    #[tokio::test]
    async fn has_terminal_event_false_initially() {
        let pool = setup_pool();
        let run_id_str = uuid::Uuid::now_v7().to_string();
        let run_id: RunId = run_id_str.parse().expect("parse");
        insert_dummy_run(&pool, &run_id_str);

        pool.append_durable(make_event(&run_id_str, 1, "run_started"))
            .await
            .expect("append");

        let has = pool.has_terminal_event(run_id).await.expect("check");
        assert!(!has);
    }

    #[tokio::test]
    async fn has_terminal_event_true_after_terminal() {
        let pool = setup_pool();
        let run_id_str = uuid::Uuid::now_v7().to_string();
        let run_id: RunId = run_id_str.parse().expect("parse");
        insert_dummy_run(&pool, &run_id_str);

        pool.append_durable(make_event(&run_id_str, 1, "run_started"))
            .await
            .expect("e1");
        pool.append_durable(make_event(&run_id_str, 2, "run_completed"))
            .await
            .expect("terminal");

        let has = pool.has_terminal_event(run_id).await.expect("check");
        assert!(has);
    }

    #[tokio::test]
    async fn append_diagnostic_succeeds() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_dummy_run(&pool, &run_id);

        let evt = StoredEvent {
            durability: "diagnostic".to_string(),
            ..make_event(&run_id, 1, "debug_trace")
        };

        let expires = "2099-12-31T23:59:59Z".to_string();
        pool.append_diagnostic(evt, expires).await.expect("append diagnostic");
    }

    #[tokio::test]
    async fn query_filters_by_run_id() {
        let pool = setup_pool();
        let run_a = uuid::Uuid::now_v7().to_string();
        let run_b = uuid::Uuid::now_v7().to_string();
        insert_dummy_run(&pool, &run_a);
        insert_dummy_run(&pool, &run_b);

        pool.append_durable(make_event(&run_a, 1, "run_started"))
            .await
            .expect("a1");
        pool.append_durable(make_event(&run_b, 1, "run_started"))
            .await
            .expect("b1");
        pool.append_durable(make_event(&run_a, 2, "turn_started"))
            .await
            .expect("a2");

        let run_a_id: RunId = run_a.parse().expect("parse");
        let results = pool
            .query(EventFilter {
                run_id: Some(run_a_id),
                ..Default::default()
            })
            .await
            .expect("query");

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.run_id == run_a_id.to_string()));
    }

    #[tokio::test]
    async fn query_filters_by_event_types() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_dummy_run(&pool, &run_id);

        pool.append_durable(make_event(&run_id, 1, "run_started"))
            .await
            .expect("e1");
        pool.append_durable(make_event(&run_id, 2, "turn_started"))
            .await
            .expect("e2");
        pool.append_durable(make_event(&run_id, 3, "run_completed"))
            .await
            .expect("e3");

        let results = pool
            .query(EventFilter {
                event_types: vec!["run_started".to_string(), "run_completed".to_string()],
                ..Default::default()
            })
            .await
            .expect("query");

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].event_type, "run_started");
        assert_eq!(results[1].event_type, "run_completed");
    }

    #[tokio::test]
    async fn query_filters_by_since_global_sequence() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_dummy_run(&pool, &run_id);

        let mut rowids = Vec::new();
        for seq in 1..=5 {
            let stored = pool
                .append_durable(make_event(&run_id, seq, "turn_started"))
                .await
                .expect("append");
            rowids.push(stored.global_sequence);
        }

        let results = pool
            .query(EventFilter {
                since_global_sequence: Some(rowids[2]),
                ..Default::default()
            })
            .await
            .expect("query");

        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|e| e.global_sequence >= rowids[2]));
    }

    #[tokio::test]
    async fn query_with_limit() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_dummy_run(&pool, &run_id);

        for seq in 1..=10 {
            pool.append_durable(make_event(&run_id, seq, "turn_started"))
                .await
                .expect("append");
        }

        let results = pool
            .query(EventFilter {
                limit: Some(3),
                ..Default::default()
            })
            .await
            .expect("query");

        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn query_empty_filter_returns_all() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_dummy_run(&pool, &run_id);

        pool.append_durable(make_event(&run_id, 1, "run_started"))
            .await
            .expect("e1");
        pool.append_durable(make_event(&run_id, 2, "turn_started"))
            .await
            .expect("e2");

        let results = pool
            .query(EventFilter::default())
            .await
            .expect("query");

        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn payload_round_trips_through_store() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_dummy_run(&pool, &run_id);

        let payload = serde_json::json!({
            "nested": {"array": [1, 2, 3]},
            "text": "hello"
        });

        let mut evt = make_event(&run_id, 1, "run_started");
        evt.payload = payload.clone();

        pool.append_durable(evt).await.expect("append");

        let run_id_parsed: RunId = run_id.parse().expect("parse");
        let events = pool.read_run_events(run_id_parsed).await.expect("read");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload, payload);
    }

    #[tokio::test]
    async fn all_terminal_event_types_detected() {
        for terminal_type in TERMINAL_EVENT_TYPES {
            let pool = setup_pool();
            let run_id_str = uuid::Uuid::now_v7().to_string();
            let run_id: RunId = run_id_str.parse().expect("parse");
            insert_dummy_run(&pool, &run_id_str);

            pool.append_durable(make_event(&run_id_str, 1, terminal_type))
                .await
                .expect("terminal");

            let has = pool.has_terminal_event(run_id).await.expect("check");
            assert!(has, "{terminal_type} should be detected as terminal");
        }
    }
}
