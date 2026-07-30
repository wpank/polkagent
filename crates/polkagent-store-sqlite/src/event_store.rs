//! [`EventStore`] implementation for [`SqlitePool`].
//!
//! This module bridges the async [`EventStore`] trait (from
//! `polkagent-store-trait`) to the synchronous rusqlite operations provided by
//! [`SqlitePool`].  Every trait method clones the pool, spawns the query on a
//! blocking thread via [`tokio::task::spawn_blocking`], and awaits the result.
//!
//! ## Schema
//!
//! Events are stored in two tables created by migration V2:
//!
//! - `durable_events` — the authoritative, globally-ordered event log.
//! - `diagnostic_events` — shorter-retention diagnostic events with an
//!   `expires_at` column for retention enforcement.
//!
//! A single-row `global_sequence_counter` table provides a monotonic counter
//! that is incremented inside the same transaction as each INSERT, so no
//! `SELECT MAX()` race is possible.
//!
//! ## Terminal-event invariant
//!
//! The four terminal event types (`run_completed`, `run_failed`,
//! `run_cancelled`, `run_timed_out`) are tracked: before appending a durable
//! event whose `event_type` is terminal, the implementation checks whether the
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
fn row_to_stored_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredEvent> {
    let payload_str: String = row.get(11)?;
    let payload: serde_json::Value = serde_json::from_str(&payload_str).unwrap_or_default();

    Ok(StoredEvent {
        id: row.get(0)?,
        event_type: row.get(1)?,
        sequence: row.get(2)?,
        global_sequence: row.get(3)?,
        run_id: row.get(4)?,
        conversation_id: row.get(5)?,
        correlation_id: row.get(6)?,
        causation_id: row.get(7)?,
        scope_id: row.get(8)?,
        timestamp: row.get(9)?,
        durability: row.get(10)?,
        payload,
        trace_id: row.get(12)?,
        span_id: row.get(13)?,
        schema_version: row.get(14)?,
    })
}

/// The standard SELECT column list for `durable_events`.
const DURABLE_COLS: &str = "\
    id, event_type, sequence, global_sequence, run_id, \
    conversation_id, correlation_id, causation_id, scope_id, \
    timestamp, durability, payload, trace_id, span_id, schema_version";

/// Allocate the next global sequence number inside an existing transaction.
///
/// This increments the single-row `global_sequence_counter` and returns the
/// new value. Must be called while the writer lock is held.
fn next_global_sequence(conn: &Connection) -> Result<u64, EventStoreError> {
    conn.execute(
        "UPDATE global_sequence_counter SET value = value + 1 WHERE id = 1",
        [],
    )
    .map_err(map_rusqlite)?;

    let seq: u64 = conn
        .query_row(
            "SELECT value FROM global_sequence_counter WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .map_err(map_rusqlite)?;

    Ok(seq)
}

/// Check whether a run already has a terminal event in `durable_events`.
fn check_terminal(conn: &Connection, run_id: &str) -> Result<bool, EventStoreError> {
    let count: u64 = conn
        .query_row(
            "SELECT COUNT(*) FROM durable_events \
             WHERE run_id = ?1 AND event_type IN ('run_completed','run_failed','run_cancelled','run_timed_out')",
            params![run_id],
            |r| r.get(0),
        )
        .map_err(map_rusqlite)?;

    Ok(count > 0)
}

/// Fetch the current max sequence for a run from `durable_events`.
fn current_max_sequence(conn: &Connection, run_id: &str) -> Result<u64, EventStoreError> {
    let max: Option<u64> = conn
        .query_row(
            "SELECT MAX(sequence) FROM durable_events WHERE run_id = ?1",
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

            // 3. Allocate global_sequence.
            let global_seq = next_global_sequence(&writer)?;

            // 4. Serialise payload.
            let payload_str = serde_json::to_string(&event.payload)
                .map_err(|e| EventStoreError::Serialisation(e.to_string()))?;

            // 5. INSERT.
            writer
                .execute(
                    "INSERT INTO durable_events \
                     (id, event_type, sequence, global_sequence, run_id, \
                      conversation_id, correlation_id, causation_id, scope_id, \
                      timestamp, durability, payload, trace_id, span_id, schema_version) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                    params![
                        event.id,
                        event.event_type,
                        event.sequence,
                        global_seq,
                        event.run_id,
                        event.conversation_id,
                        event.correlation_id,
                        event.causation_id,
                        event.scope_id,
                        event.timestamp,
                        event.durability,
                        payload_str,
                        event.trace_id,
                        event.span_id,
                        event.schema_version,
                    ],
                )
                .map_err(|e| {
                    if crate::error::StoreError::is_unique_violation(&e) {
                        EventStoreError::Conflict(format!(
                            "event with id {} already exists",
                            event.id
                        ))
                    } else {
                        map_rusqlite(e)
                    }
                })?;

            // 6. Return the completed event with assigned global_sequence.
            Ok(StoredEvent {
                global_sequence: global_seq,
                ..event
            })
        })
        .await
        .map_err(map_join)?
    }

    async fn append_diagnostic(
        &self,
        event: StoredEvent,
        expires_at: String,
    ) -> Result<(), EventStoreError> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();

            // Allocate a global_sequence for ordering consistency even though
            // diagnostic events have relaxed invariants.
            let global_seq = next_global_sequence(&writer)?;

            let payload_str = serde_json::to_string(&event.payload)
                .map_err(|e| EventStoreError::Serialisation(e.to_string()))?;

            writer
                .execute(
                    "INSERT INTO diagnostic_events \
                     (id, event_type, sequence, global_sequence, run_id, \
                      conversation_id, correlation_id, causation_id, scope_id, \
                      timestamp, durability, payload, trace_id, span_id, \
                      schema_version, expires_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                    params![
                        event.id,
                        event.event_type,
                        event.sequence,
                        global_seq,
                        event.run_id,
                        event.conversation_id,
                        event.correlation_id,
                        event.causation_id,
                        event.scope_id,
                        event.timestamp,
                        event.durability,
                        payload_str,
                        event.trace_id,
                        event.span_id,
                        event.schema_version,
                        expires_at,
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
                    "SELECT {DURABLE_COLS} FROM durable_events \
                     WHERE global_sequence > ?1 \
                     ORDER BY global_sequence ASC \
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
                    "SELECT {DURABLE_COLS} FROM durable_events \
                     WHERE run_id = ?1 \
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
                conditions.push(format!("d.run_id = ?{param_idx}"));
                param_values.push(Box::new(run_id.to_string()));
                param_idx += 1;
            }

            // Filter: conversation_id
            if let Some(ref conv_id) = filter.conversation_id {
                conditions.push(format!("d.conversation_id = ?{param_idx}"));
                param_values.push(Box::new(conv_id.clone()));
                param_idx += 1;
            }

            // Filter: scope_id
            if let Some(ref scope_id) = filter.scope_id {
                conditions.push(format!("d.scope_id = ?{param_idx}"));
                param_values.push(Box::new(scope_id.clone()));
                param_idx += 1;
            }

            // Filter: event_types (IN clause)
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
                conditions.push(format!("d.event_type IN ({})", placeholders.join(",")));
            }

            // Filter: since_global_sequence
            if let Some(since) = filter.since_global_sequence {
                conditions.push(format!("d.global_sequence >= ?{param_idx}"));
                param_values.push(Box::new(since));
                param_idx += 1;
            }

            // Base query.  When `include_diagnostic` is true we UNION both
            // tables; otherwise we only query durable_events.
            let where_clause = if conditions.is_empty() {
                String::new()
            } else {
                format!("WHERE {}", conditions.join(" AND "))
            };

            let limit_clause = if let Some(limit) = filter.limit {
                format!("LIMIT ?{param_idx}")
                    .to_string()
                    .tap(|_| {
                        param_values.push(Box::new(limit as u64));
                    })
            } else {
                String::new()
            };

            // We always need a well-defined column set alias.
            let durable_cols_prefixed = "\
                d.id, d.event_type, d.sequence, d.global_sequence, d.run_id, \
                d.conversation_id, d.correlation_id, d.causation_id, d.scope_id, \
                d.timestamp, d.durability, d.payload, d.trace_id, d.span_id, d.schema_version";

            let sql = if filter.include_diagnostic {
                // UNION both tables, re-apply the same WHERE to diagnostic.
                // For simplicity we build two SELECTs with the same filters.
                format!(
                    "SELECT {durable_cols_prefixed} FROM durable_events d {where_clause} \
                     UNION ALL \
                     SELECT {durable_cols_prefixed} FROM diagnostic_events d {where_clause} \
                     ORDER BY global_sequence ASC {limit_clause}"
                )
            } else {
                format!(
                    "SELECT {durable_cols_prefixed} FROM durable_events d \
                     {where_clause} ORDER BY global_sequence ASC {limit_clause}"
                )
            };

            // For the UNION ALL variant, we duplicate the params.
            let params_for_query: Vec<&dyn rusqlite::types::ToSql> = if filter.include_diagnostic {
                // Double the params: once for durable, once for diagnostic.
                let base: Vec<&dyn rusqlite::types::ToSql> =
                    param_values.iter().map(|b| b.as_ref()).collect();
                let mut doubled = base.clone();
                doubled.extend(base);
                doubled
            } else {
                param_values.iter().map(|b| b.as_ref()).collect()
            };

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
// Utility trait for inline side-effects (avoids a let binding)
// ---------------------------------------------------------------------------

trait Tap: Sized {
    fn tap(self, f: impl FnOnce(&Self)) -> Self {
        f(&self);
        self
    }
}

impl<T> Tap for T {}

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

    #[tokio::test]
    async fn append_durable_assigns_global_sequence() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();

        let evt = make_event(&run_id, 1, "run_started");
        let stored = pool.append_durable(evt).await.expect("append");

        assert_eq!(stored.global_sequence, 1);
        assert_eq!(stored.sequence, 1);
    }

    #[tokio::test]
    async fn append_durable_increments_global_sequence() {
        let pool = setup_pool();
        let run_a = uuid::Uuid::now_v7().to_string();
        let run_b = uuid::Uuid::now_v7().to_string();

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

        assert_eq!(e1.global_sequence, 1);
        assert_eq!(e2.global_sequence, 2);
        assert_eq!(e3.global_sequence, 3);
    }

    #[tokio::test]
    async fn append_durable_rejects_non_monotonic_sequence() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();

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

        for seq in 1..=5 {
            pool.append_durable(make_event(&run_id, seq, "turn_started"))
                .await
                .expect("append");
        }

        // Read after cursor 2, limit 2.
        let page = pool.read_from_cursor(2, 2).await.expect("read");
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].global_sequence, 3);
        assert_eq!(page[1].global_sequence, 4);
    }

    #[tokio::test]
    async fn read_from_cursor_returns_empty_past_end() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();

        pool.append_durable(make_event(&run_id, 1, "run_started"))
            .await
            .expect("append");

        let page = pool.read_from_cursor(100, 10).await.expect("read");
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

        for seq in 1..=5 {
            pool.append_durable(make_event(&run_id, seq, "turn_started"))
                .await
                .expect("append");
        }

        let results = pool
            .query(EventFilter {
                since_global_sequence: Some(3),
                ..Default::default()
            })
            .await
            .expect("query");

        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|e| e.global_sequence >= 3));
    }

    #[tokio::test]
    async fn query_with_limit() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();

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

            pool.append_durable(make_event(&run_id_str, 1, terminal_type))
                .await
                .expect("terminal");

            let has = pool.has_terminal_event(run_id).await.expect("check");
            assert!(has, "{terminal_type} should be detected as terminal");
        }
    }
}
