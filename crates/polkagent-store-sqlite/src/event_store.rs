//! [`EventStore`] implementation for [`SqlitePool`].
//!
//! This module bridges the async [`EventStore`] trait (from
//! `polkagent-store-trait`) to the synchronous rusqlite operations provided by
//! [`SqlitePool`].  Every trait method clones the pool, spawns the query on a
//! blocking thread via [`tokio::task::spawn_blocking`], and awaits the result.
//!
//! ## Schema
//!
//! Events are stored in the `run_events` table. Migration V19 preserves the
//! complete [`StoredEvent`] metadata envelope plus the component identifiers
//! from `RunEvent::correlation`; legacy rows retain their original `rowid`.
//!
//! Diagnostic events are stored in the same table; the `kind` column value
//! distinguishes them (prefixed with `diagnostic:` by convention).
//!
//! Global ordering uses `SQLite`'s `rowid` as a monotonic proxy since no
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
use rusqlite::{params, Connection, OptionalExtension};

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

const MAX_EVENT_TYPE_BYTES: usize = 128;
const MAX_CORRELATION_ID_BYTES: usize = 256;
const MAX_SCOPE_ID_BYTES: usize = 256;

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

fn invalid_metadata(message: impl Into<String>) -> EventStoreError {
    EventStoreError::Serialisation(message.into())
}

fn validate_bounded_text(
    field: &str,
    value: &str,
    max_bytes: usize,
) -> Result<(), EventStoreError> {
    if value.len() > max_bytes {
        return Err(invalid_metadata(format!(
            "event {field} exceeds {max_bytes} bytes"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(invalid_metadata(format!(
            "event {field} contains control characters"
        )));
    }
    Ok(())
}

fn validate_uuid(field: &str, value: &str) -> Result<(), EventStoreError> {
    uuid::Uuid::parse_str(value)
        .map(|_| ())
        .map_err(|_| invalid_metadata(format!("event {field} is not a UUID")))
}

fn validate_optional_uuid(field: &str, value: Option<&str>) -> Result<(), EventStoreError> {
    value.map_or(Ok(()), |value| validate_uuid(field, value))
}

fn validate_w3c_id(field: &str, value: Option<&str>, width: usize) -> Result<(), EventStoreError> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.len() != width
        || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        || value.bytes().all(|byte| byte == b'0')
    {
        return Err(invalid_metadata(format!(
            "event {field} must be a non-zero {width}-character hexadecimal identifier"
        )));
    }
    Ok(())
}

fn validate_common_event(event: &StoredEvent) -> Result<(), EventStoreError> {
    validate_uuid("id", &event.id)?;
    validate_uuid("run_id", &event.run_id)?;
    validate_optional_uuid("turn_id", event.turn_id.as_deref())?;
    validate_optional_uuid("step_id", event.step_id.as_deref())?;
    validate_optional_uuid("effect_intent_id", event.effect_intent_id.as_deref())?;
    validate_optional_uuid("effect_attempt_id", event.effect_attempt_id.as_deref())?;
    validate_optional_uuid("conversation_id", event.conversation_id.as_deref())?;
    validate_optional_uuid("causation_id", event.causation_id.as_deref())?;
    if event.causation_id.as_deref() == Some(event.id.as_str()) {
        return Err(invalid_metadata(
            "event causation_id cannot reference itself",
        ));
    }
    validate_bounded_text("event_type", &event.event_type, MAX_EVENT_TYPE_BYTES)?;
    if event.event_type.is_empty() {
        return Err(invalid_metadata("event event_type cannot be empty"));
    }
    if event.event_type.starts_with("diagnostic:") {
        return Err(invalid_metadata(
            "event event_type uses the reserved diagnostic storage prefix",
        ));
    }
    validate_bounded_text(
        "correlation_id",
        &event.correlation_id,
        MAX_CORRELATION_ID_BYTES,
    )?;
    validate_bounded_text("scope_id", &event.scope_id, MAX_SCOPE_ID_BYTES)?;
    validate_w3c_id("trace_id", event.trace_id.as_deref(), 32)?;
    validate_w3c_id("span_id", event.span_id.as_deref(), 16)?;
    chrono::DateTime::parse_from_rfc3339(&event.timestamp)
        .map_err(|_| invalid_metadata("event timestamp is not RFC 3339"))?;
    if event.sequence == 0 {
        return Err(invalid_metadata("event sequence must be greater than zero"));
    }
    if event.schema_version == 0 {
        return Err(invalid_metadata(
            "event schema_version must be greater than zero",
        ));
    }
    if !matches!(
        event.durability.as_str(),
        "durable" | "diagnostic" | "ephemeral"
    ) {
        return Err(invalid_metadata("event durability is invalid"));
    }
    Ok(())
}

fn validate_new_event(event: &StoredEvent, durability: &str) -> Result<(), EventStoreError> {
    validate_common_event(event)?;
    if event.global_sequence != 0 {
        return Err(invalid_metadata(
            "new event global_sequence must be zero before assignment",
        ));
    }
    if event.durability != durability {
        return Err(invalid_metadata(format!(
            "event durability must be {durability} for this append operation"
        )));
    }
    Ok(())
}

fn invalid_row(error: EventStoreError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error.to_string(),
        )),
    )
}

/// Read a single `StoredEvent` from the current row of a `rusqlite::Row`.
///
/// Expected column order (matching `RUN_EVENTS_COLS`):
///   0: id, 1: `run_id`, 2: sequence, 3: kind, 4: `data_json`,
///   5: timestamp, 6: `correlation_id`, 7: `schema_version`, 8: rowid,
///   9..18: optional envelope and component-correlation metadata
fn row_to_stored_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredEvent> {
    let data_json_str: String = row.get(4)?;
    let payload: serde_json::Value = serde_json::from_str(&data_json_str).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let correlation_id: Option<String> = row.get(6)?;
    let stored_kind: String = row.get(3)?;
    let durability: String = row.get(12)?;
    let event_type = match (durability.as_str(), stored_kind.strip_prefix("diagnostic:")) {
        ("diagnostic", Some(event_type)) => event_type.to_owned(),
        ("diagnostic", None) => {
            return Err(invalid_row(invalid_metadata(
                "stored diagnostic event kind is missing its storage prefix",
            )))
        }
        ("durable", Some(_)) => {
            return Err(invalid_row(invalid_metadata(
                "stored durable event kind has a diagnostic storage prefix",
            )))
        }
        ("durable", None) => stored_kind,
        _ => {
            return Err(invalid_row(invalid_metadata(
                "SQLite event durability must be durable or diagnostic",
            )))
        }
    };
    // Use rowid as global_sequence proxy.
    let rowid: u64 = row.get(8)?;

    let event = StoredEvent {
        id: row.get(0)?,
        run_id: row.get(1)?,
        sequence: row.get(2)?,
        event_type,
        payload,
        timestamp: row.get(5)?,
        correlation_id: correlation_id.unwrap_or_default(),
        schema_version: row.get(7)?,
        global_sequence: rowid,
        conversation_id: row.get(9)?,
        causation_id: row.get(10)?,
        scope_id: row.get(11)?,
        durability,
        trace_id: row.get(13)?,
        span_id: row.get(14)?,
        turn_id: row.get(15)?,
        step_id: row.get(16)?,
        effect_intent_id: row.get(17)?,
        effect_attempt_id: row.get(18)?,
    };
    validate_common_event(&event).map_err(invalid_row)?;
    if event.global_sequence == 0 {
        return Err(invalid_row(invalid_metadata(
            "stored event global_sequence must be greater than zero",
        )));
    }
    Ok(event)
}

/// The standard SELECT column list for `run_events`, including `rowid` for
/// global ordering.
const RUN_EVENTS_COLS: &str = "\
    id, run_id, sequence, kind, data_json, \
    timestamp, correlation_id, schema_version, rowid, \
    conversation_id, causation_id, scope_id, durability, trace_id, span_id, \
    turn_id, step_id, effect_intent_id, effect_attempt_id";

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
        validate_new_event(&event, "durable")?;
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
                      timestamp, correlation_id, schema_version, \
                      conversation_id, causation_id, scope_id, durability, \
                      trace_id, span_id, turn_id, step_id, effect_intent_id, \
                      effect_attempt_id) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, \
                             ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
                    params![
                        event.id,
                        event.run_id,
                        event.sequence,
                        event.event_type,
                        payload_str,
                        event.timestamp,
                        event.correlation_id,
                        event.schema_version,
                        event.conversation_id,
                        event.causation_id,
                        event.scope_id,
                        event.durability,
                        event.trace_id,
                        event.span_id,
                        event.turn_id,
                        event.step_id,
                        event.effect_intent_id,
                        event.effect_attempt_id,
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
            let raw_rowid = writer.last_insert_rowid();
            let rowid = u64::try_from(raw_rowid).map_err(|_| {
                EventStoreError::Backend(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("SQLite returned invalid rowid {raw_rowid}"),
                )))
            })?;

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
        validate_new_event(&event, "diagnostic")?;
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
                      timestamp, correlation_id, schema_version, \
                      conversation_id, causation_id, scope_id, durability, \
                      trace_id, span_id, turn_id, step_id, effect_intent_id, \
                      effect_attempt_id) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, \
                             ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
                    params![
                        event.id,
                        event.run_id,
                        event.sequence,
                        kind,
                        payload_str,
                        event.timestamp,
                        event.correlation_id,
                        event.schema_version,
                        event.conversation_id,
                        event.causation_id,
                        event.scope_id,
                        event.durability,
                        event.trace_id,
                        event.span_id,
                        event.turn_id,
                        event.step_id,
                        event.effect_intent_id,
                        event.effect_attempt_id,
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

    async fn get_event_by_id(&self, id: &str) -> Result<StoredEvent, EventStoreError> {
        let pool = self.clone();
        let id = id.to_owned();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let event = writer
                .query_row(
                    &format!(
                        "SELECT {RUN_EVENTS_COLS} FROM run_events \
                         WHERE id = ?1 AND kind NOT LIKE 'diagnostic:%'"
                    ),
                    [&id],
                    row_to_stored_event,
                )
                .optional()
                .map_err(map_rusqlite)?;
            event.ok_or_else(|| EventStoreError::NotFound(format!("event {id}")))
        })
        .await
        .map_err(map_join)?
    }

    async fn read_run_events(&self, run_id: RunId) -> Result<Vec<StoredEvent>, EventStoreError> {
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

            if let Some(ref conversation_id) = filter.conversation_id {
                conditions.push(format!("conversation_id = ?{param_idx}"));
                param_values.push(Box::new(conversation_id.clone()));
                param_idx += 1;
            }

            if let Some(ref scope_id) = filter.scope_id {
                conditions.push(format!("scope_id = ?{param_idx}"));
                param_values.push(Box::new(scope_id.clone()));
                param_idx += 1;
            }

            // Filter canonical event types against both durable kinds and the
            // prefixed diagnostic storage representation.
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
                let durable_clause = format!("kind IN ({})", placeholders.join(","));
                if filter.include_diagnostic {
                    let diagnostic_placeholders: Vec<String> = filter
                        .event_types
                        .iter()
                        .map(|event_type| {
                            let placeholder = format!("?{param_idx}");
                            param_values.push(Box::new(format!("diagnostic:{event_type}")));
                            param_idx += 1;
                            placeholder
                        })
                        .collect();
                    conditions.push(format!(
                        "({durable_clause} OR kind IN ({}))",
                        diagnostic_placeholders.join(",")
                    ));
                } else {
                    conditions.push(durable_clause);
                }
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

            let params_for_query: Vec<&dyn rusqlite::types::ToSql> = param_values
                .iter()
                .map(std::convert::AsRef::as_ref)
                .collect();

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
// These contract tests use `expect` to pinpoint the exact database setup or
// event-store operation that violated the fixture's asserted invariant.
#[allow(clippy::expect_used)]
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
            turn_id: None,
            step_id: None,
            effect_intent_id: None,
            effect_attempt_id: None,
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
                "INSERT INTO agents (id, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
                params![agent_id, format!("agent-{agent_id}"), now],
            )
            .expect("insert agent");
        writer
            .execute(
                "INSERT INTO runs (id, agent_id, state, created_at, updated_at) VALUES (?1, ?2, 'created', ?3, ?3)",
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
    async fn durable_metadata_round_trips_for_null_and_populated_envelopes() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_dummy_run(&pool, &run_id);

        let null_event = make_event(&run_id, 1, "run_created");
        let null_id = null_event.id.clone();
        let null_stored = pool
            .append_durable(null_event)
            .await
            .expect("append null-metadata event");
        assert!(null_stored.global_sequence > 0);
        let null_read = pool
            .get_event_by_id(&null_id)
            .await
            .expect("read null-metadata event");
        assert_eq!(null_read.turn_id, None);
        assert_eq!(null_read.step_id, None);
        assert_eq!(null_read.effect_intent_id, None);
        assert_eq!(null_read.effect_attempt_id, None);
        assert_eq!(null_read.conversation_id, None);
        assert_eq!(null_read.causation_id, None);
        assert_eq!(null_read.trace_id, None);
        assert_eq!(null_read.span_id, None);
        assert_eq!(null_read.scope_id, "scope-1");
        assert_eq!(null_read.durability, "durable");

        let turn_id = uuid::Uuid::now_v7().to_string();
        let step_id = uuid::Uuid::now_v7().to_string();
        let effect_intent_id = uuid::Uuid::now_v7().to_string();
        let effect_attempt_id = uuid::Uuid::now_v7().to_string();
        let conversation_id = uuid::Uuid::now_v7().to_string();
        let causation_id = uuid::Uuid::now_v7().to_string();
        let mut populated = make_event(&run_id, 2, "turn_started");
        populated.turn_id = Some(turn_id.clone());
        populated.step_id = Some(step_id.clone());
        populated.effect_intent_id = Some(effect_intent_id.clone());
        populated.effect_attempt_id = Some(effect_attempt_id.clone());
        populated.conversation_id = Some(conversation_id.clone());
        populated.correlation_id = "workflow-correlation-1".to_owned();
        populated.causation_id = Some(causation_id.clone());
        populated.scope_id = "workspace-local".to_owned();
        populated.trace_id = Some("4bf92f3577b34da6a3ce929d0e0e4736".to_owned());
        populated.span_id = Some("00f067aa0ba902b7".to_owned());
        let populated_id = populated.id.clone();
        let populated_stored = pool
            .append_durable(populated)
            .await
            .expect("append populated-metadata event");
        let populated_read = pool
            .get_event_by_id(&populated_id)
            .await
            .expect("read populated-metadata event");

        assert_eq!(
            populated_read.global_sequence,
            populated_stored.global_sequence
        );
        assert_eq!(populated_read.turn_id.as_deref(), Some(turn_id.as_str()));
        assert_eq!(populated_read.step_id.as_deref(), Some(step_id.as_str()));
        assert_eq!(
            populated_read.effect_intent_id.as_deref(),
            Some(effect_intent_id.as_str())
        );
        assert_eq!(
            populated_read.effect_attempt_id.as_deref(),
            Some(effect_attempt_id.as_str())
        );
        assert_eq!(
            populated_read.conversation_id.as_deref(),
            Some(conversation_id.as_str())
        );
        assert_eq!(populated_read.correlation_id, "workflow-correlation-1");
        assert_eq!(
            populated_read.causation_id.as_deref(),
            Some(causation_id.as_str())
        );
        assert_eq!(populated_read.scope_id, "workspace-local");
        assert_eq!(
            populated_read.trace_id.as_deref(),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
        assert_eq!(populated_read.span_id.as_deref(), Some("00f067aa0ba902b7"));
        assert_eq!(populated_read.durability, "durable");

        let filtered = pool
            .query(EventFilter {
                conversation_id: Some(conversation_id),
                scope_id: Some("workspace-local".to_owned()),
                ..Default::default()
            })
            .await
            .expect("filter populated envelope metadata");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, populated_id);
    }

    #[tokio::test]
    async fn diagnostic_event_type_and_metadata_round_trip_without_storage_prefix() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_dummy_run(&pool, &run_id);

        let mut diagnostic = make_event(&run_id, 1, "diagnostic_log");
        diagnostic.durability = "diagnostic".to_owned();
        diagnostic.turn_id = Some(uuid::Uuid::now_v7().to_string());
        diagnostic.step_id = Some(uuid::Uuid::now_v7().to_string());
        diagnostic.effect_intent_id = Some(uuid::Uuid::now_v7().to_string());
        diagnostic.effect_attempt_id = Some(uuid::Uuid::now_v7().to_string());
        diagnostic.conversation_id = Some(uuid::Uuid::now_v7().to_string());
        diagnostic.correlation_id = "diagnostic-correlation".to_owned();
        diagnostic.causation_id = Some(uuid::Uuid::now_v7().to_string());
        diagnostic.scope_id = "diagnostic-scope".to_owned();
        diagnostic.trace_id = Some("4bf92f3577b34da6a3ce929d0e0e4736".to_owned());
        diagnostic.span_id = Some("00f067aa0ba902b7".to_owned());
        let expected = diagnostic.clone();

        pool.append_diagnostic(diagnostic, "2030-01-01T00:00:00Z".to_owned())
            .await
            .expect("append diagnostic event");
        assert!(pool
            .read_from_cursor(0, 16)
            .await
            .expect("read durable cursor")
            .is_empty());

        let events = pool
            .query(EventFilter {
                event_types: vec!["diagnostic_log".to_owned()],
                include_diagnostic: true,
                ..Default::default()
            })
            .await
            .expect("query diagnostic event by canonical event type");
        assert_eq!(events.len(), 1);
        let stored = &events[0];
        assert!(stored.global_sequence > 0);
        assert_eq!(stored.id, expected.id);
        assert_eq!(stored.event_type, expected.event_type);
        assert_eq!(stored.run_id, expected.run_id);
        assert_eq!(stored.turn_id, expected.turn_id);
        assert_eq!(stored.step_id, expected.step_id);
        assert_eq!(stored.effect_intent_id, expected.effect_intent_id);
        assert_eq!(stored.effect_attempt_id, expected.effect_attempt_id);
        assert_eq!(stored.conversation_id, expected.conversation_id);
        assert_eq!(stored.correlation_id, expected.correlation_id);
        assert_eq!(stored.causation_id, expected.causation_id);
        assert_eq!(stored.scope_id, expected.scope_id);
        assert_eq!(stored.timestamp, expected.timestamp);
        assert_eq!(stored.durability, "diagnostic");
        assert_eq!(stored.payload, expected.payload);
        assert_eq!(stored.trace_id, expected.trace_id);
        assert_eq!(stored.span_id, expected.span_id);
        assert_eq!(stored.schema_version, expected.schema_version);
    }

    #[tokio::test]
    async fn invalid_new_metadata_is_rejected_before_insert() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_dummy_run(&pool, &run_id);

        let mut cases = Vec::new();
        let mut invalid_turn = make_event(&run_id, 1, "run_created");
        invalid_turn.turn_id = Some("not-a-uuid".to_owned());
        cases.push(invalid_turn);

        let mut invalid_trace = make_event(&run_id, 1, "run_created");
        invalid_trace.trace_id = Some("0".repeat(32));
        cases.push(invalid_trace);

        let mut invalid_scope = make_event(&run_id, 1, "run_created");
        invalid_scope.scope_id = "s".repeat(MAX_SCOPE_ID_BYTES + 1);
        cases.push(invalid_scope);

        let mut invalid_timestamp = make_event(&run_id, 1, "run_created");
        invalid_timestamp.timestamp = "not-a-timestamp".to_owned();
        cases.push(invalid_timestamp);

        let mut assigned_cursor = make_event(&run_id, 1, "run_created");
        assigned_cursor.global_sequence = 7;
        cases.push(assigned_cursor);

        for event in cases {
            let error = pool
                .append_durable(event)
                .await
                .expect_err("invalid metadata must fail before insertion");
            assert!(matches!(error, EventStoreError::Serialisation(_)));
        }
        assert_eq!(
            pool.max_sequence(run_id.parse().expect("run ID"))
                .await
                .expect("max"),
            0
        );
    }

    #[tokio::test]
    async fn corrupted_persisted_payload_timestamp_and_typed_id_fail_closed() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_dummy_run(&pool, &run_id);
        let invalid_json_id = uuid::Uuid::now_v7().to_string();
        let invalid_timestamp_id = uuid::Uuid::now_v7().to_string();
        let invalid_turn_id = uuid::Uuid::now_v7().to_string();
        {
            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO run_events
                        (id, run_id, sequence, kind, data_json, timestamp,
                         correlation_id, schema_version)
                     VALUES (?1, ?2, 1, 'run_created', '{',
                             '2024-01-01T00:00:00Z', '', 1)",
                    params![invalid_json_id, run_id],
                )
                .expect("seed malformed JSON");
            writer
                .execute(
                    "INSERT INTO run_events
                        (id, run_id, sequence, kind, data_json, timestamp,
                         correlation_id, schema_version)
                     VALUES (?1, ?2, 2, 'run_created', '\"run_created\"',
                             'not-a-timestamp', '', 1)",
                    params![invalid_timestamp_id, run_id],
                )
                .expect("seed malformed timestamp");
            writer
                .execute(
                    "INSERT INTO run_events
                        (id, run_id, sequence, kind, data_json, timestamp,
                         correlation_id, schema_version, turn_id)
                     VALUES (?1, ?2, 3, 'run_created', '\"run_created\"',
                             '2024-01-01T00:00:00Z', '', 1, 'not-a-uuid')",
                    params![invalid_turn_id, run_id],
                )
                .expect("seed malformed typed correlation ID");
        }

        for id in [invalid_json_id, invalid_timestamp_id, invalid_turn_id] {
            let error = pool
                .get_event_by_id(&id)
                .await
                .expect_err("corrupt persisted metadata must not be synthesized");
            assert!(matches!(error, EventStoreError::Backend(_)));
        }
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
    async fn indexed_id_lookup_finds_event_after_ten_thousand_rows() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_dummy_run(&pool, &run_id);
        let target_id = uuid::Uuid::now_v7().to_string();
        {
            let mut writer = pool.writer();
            let transaction = writer.transaction().expect("begin bulk event insert");
            {
                let mut statement = transaction
                    .prepare(
                        "INSERT INTO run_events \
                         (id, run_id, sequence, kind, data_json, timestamp, correlation_id, schema_version) \
                         VALUES (?1, ?2, ?3, 'turn_started', '{}', '2024-01-01T00:00:00Z', 'lookup', 1)",
                    )
                    .expect("prepare bulk event insert");
                for sequence in 1..=10_002_u64 {
                    let id = if sequence == 10_002 {
                        target_id.clone()
                    } else {
                        format!("earlier-{sequence}")
                    };
                    statement
                        .execute(params![id, run_id, sequence])
                        .expect("insert bulk event");
                }
            }
            transaction.commit().expect("commit bulk event insert");
        }

        let found = pool
            .get_event_by_id(&target_id)
            .await
            .expect("indexed event lookup");
        assert_eq!(found.id, target_id);
        assert_eq!(found.global_sequence, 10_002);
        let missing = pool
            .get_event_by_id("missing-event")
            .await
            .expect_err("missing event must be typed not found");
        assert!(matches!(missing, EventStoreError::NotFound(_)));

        let plan: String = pool
            .writer()
            .query_row(
                "EXPLAIN QUERY PLAN SELECT id FROM run_events \
                 WHERE id = ?1 AND kind NOT LIKE 'diagnostic:%'",
                [target_id],
                |row| row.get(3),
            )
            .expect("explain indexed lookup");
        assert!(
            plan.contains("INDEX"),
            "unexpected SQLite query plan: {plan}"
        );
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
        pool.append_diagnostic(evt, expires)
            .await
            .expect("append diagnostic");
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

        let results = pool.query(EventFilter::default()).await.expect("query");

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
