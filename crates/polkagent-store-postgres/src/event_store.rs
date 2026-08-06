use async_trait::async_trait;
use polkagent_core::RunId;
use polkagent_store_trait::event::{EventFilter, EventStore, EventStoreError, StoredEvent};
use sqlx::Row;

use crate::pool::PgPool;

const TERMINAL_EVENT_TYPES: &[&str] = &[
    "run_completed",
    "run_failed",
    "run_cancelled",
    "run_timed_out",
];

fn is_terminal(event_type: &str) -> bool {
    TERMINAL_EVENT_TYPES.contains(&event_type)
}

fn map_pg_err(e: sqlx::Error) -> EventStoreError {
    EventStoreError::Backend(Box::new(e))
}

fn db_i64_to_u64(value: i64, field: &str) -> Result<u64, EventStoreError> {
    u64::try_from(value).map_err(|_| {
        EventStoreError::Serialisation(format!(
            "negative PostgreSQL BIGINT for event {field}: {value}"
        ))
    })
}

fn db_i32_to_u32(value: i32, field: &str) -> Result<u32, EventStoreError> {
    u32::try_from(value).map_err(|_| {
        EventStoreError::Serialisation(format!(
            "negative PostgreSQL INTEGER for event {field}: {value}"
        ))
    })
}

fn u64_to_db_i64(value: u64, field: &str) -> Result<i64, EventStoreError> {
    i64::try_from(value).map_err(|_| {
        EventStoreError::Serialisation(format!(
            "event {field} {value} exceeds PostgreSQL BIGINT capacity"
        ))
    })
}

fn u32_to_db_i32(value: u32, field: &str) -> Result<i32, EventStoreError> {
    i32::try_from(value).map_err(|_| {
        EventStoreError::Serialisation(format!(
            "event {field} {value} exceeds PostgreSQL INTEGER capacity"
        ))
    })
}

fn usize_to_db_i64(value: usize, field: &str) -> Result<i64, EventStoreError> {
    i64::try_from(value).map_err(|_| {
        EventStoreError::Serialisation(format!(
            "event {field} {value} exceeds PostgreSQL BIGINT capacity"
        ))
    })
}

fn row_to_event(row: &sqlx::postgres::PgRow) -> Result<StoredEvent, EventStoreError> {
    let global_sequence: i64 = row.get("global_sequence");
    let sequence: i64 = row.get("sequence");
    let schema_version: i32 = row.get("schema_version");
    let data_json: serde_json::Value = row.get("data_json");
    let correlation_id: Option<String> = row.get("correlation_id");

    Ok(StoredEvent {
        id: row.get("id"),
        run_id: row.get("run_id"),
        sequence: db_i64_to_u64(sequence, "sequence")?,
        event_type: row.get("kind"),
        payload: data_json,
        timestamp: row
            .get::<chrono::DateTime<chrono::Utc>, _>("timestamp")
            .to_rfc3339(),
        correlation_id: correlation_id.unwrap_or_default(),
        schema_version: db_i32_to_u32(schema_version, "schema_version")?,
        global_sequence: db_i64_to_u64(global_sequence, "global_sequence")?,
        turn_id: row.get("turn_id"),
        step_id: row.get("step_id"),
        effect_intent_id: row.get("effect_intent_id"),
        effect_attempt_id: row.get("effect_attempt_id"),
        conversation_id: row.get("conversation_id"),
        causation_id: row.get("causation_id"),
        scope_id: row.get("scope_id"),
        durability: row.get("durability"),
        trace_id: row.get("trace_id"),
        span_id: row.get("span_id"),
    })
}

const EVENT_COLS: &str = "global_sequence, id, run_id, sequence, kind, data_json, \
    timestamp, correlation_id, schema_version, conversation_id, causation_id, \
    scope_id, durability, trace_id, span_id, turn_id, step_id, effect_intent_id, \
    effect_attempt_id";

#[async_trait]
impl EventStore for PgPool {
    async fn append_durable(&self, event: StoredEvent) -> Result<StoredEvent, EventStoreError> {
        let tenant = self.tenant_id().to_string();
        let sequence = u64_to_db_i64(event.sequence, "sequence")?;
        let schema_version = u32_to_db_i32(event.schema_version, "schema_version")?;

        let mut tx = self.pool().begin().await.map_err(map_pg_err)?;
        self.set_tenant(&mut tx).await.map_err(map_pg_err)?;

        // 1. Non-monotonic sequence check.
        let current_max: Option<i64> =
            sqlx::query_scalar("SELECT MAX(sequence) FROM durable_events WHERE run_id = $1")
                .bind(&event.run_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(map_pg_err)?;

        let current_max = db_i64_to_u64(current_max.unwrap_or(0), "sequence")?;
        if event.sequence <= current_max {
            return Err(EventStoreError::NonMonotonicSequence {
                run_id: event.run_id.clone(),
                current: current_max,
                proposed: event.sequence,
            });
        }

        // 2. Terminal-event uniqueness check.
        if is_terminal(&event.event_type) {
            let has_terminal: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM durable_events WHERE run_id = $1 AND kind = ANY($2))",
            )
            .bind(&event.run_id)
            .bind(TERMINAL_EVENT_TYPES)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_pg_err)?;

            if has_terminal {
                return Err(EventStoreError::DuplicateTerminalEvent {
                    run_id: event.run_id.clone(),
                });
            }
        }

        // 3. Serialise payload.
        let payload = &event.payload;

        let ts = chrono::DateTime::parse_from_rfc3339(&event.timestamp)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .map_err(|e| EventStoreError::Serialisation(format!("invalid timestamp: {e}")))?;

        // 4. INSERT and get assigned global_sequence.
        let row = sqlx::query(&format!(
            "INSERT INTO durable_events
             (id, tenant_id, run_id, sequence, kind, data_json, timestamp,
              correlation_id, schema_version, conversation_id, causation_id,
              scope_id, durability, trace_id, span_id, turn_id, step_id,
              effect_intent_id, effect_attempt_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                     $13, $14, $15, $16, $17, $18, $19)
             RETURNING {EVENT_COLS}"
        ))
        .bind(&event.id)
        .bind(&tenant)
        .bind(&event.run_id)
        .bind(sequence)
        .bind(&event.event_type)
        .bind(payload)
        .bind(ts)
        .bind(&event.correlation_id)
        .bind(schema_version)
        .bind(&event.conversation_id)
        .bind(&event.causation_id)
        .bind(&event.scope_id)
        .bind(&event.durability)
        .bind(&event.trace_id)
        .bind(&event.span_id)
        .bind(&event.turn_id)
        .bind(&event.step_id)
        .bind(&event.effect_intent_id)
        .bind(&event.effect_attempt_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| {
            if matches!(&e, sqlx::Error::Database(db_err) if db_err.is_unique_violation()) {
                EventStoreError::Conflict(format!("event with id {} already exists", event.id))
            } else {
                map_pg_err(e)
            }
        })?;

        let stored = row_to_event(&row)?;

        tx.commit().await.map_err(map_pg_err)?;
        Ok(stored)
    }

    async fn append_diagnostic(
        &self,
        event: StoredEvent,
        expires_at: String,
    ) -> Result<(), EventStoreError> {
        let tenant = self.tenant_id().to_string();
        let sequence = u64_to_db_i64(event.sequence, "sequence")?;
        let schema_version = u32_to_db_i32(event.schema_version, "schema_version")?;

        let ts = chrono::DateTime::parse_from_rfc3339(&event.timestamp)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .map_err(|e| EventStoreError::Serialisation(format!("invalid timestamp: {e}")))?;

        let expires = chrono::DateTime::parse_from_rfc3339(&expires_at)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .map_err(|e| EventStoreError::Serialisation(format!("invalid expires_at: {e}")))?;

        let mut tx = self.pool().begin().await.map_err(map_pg_err)?;
        self.set_tenant(&mut tx).await.map_err(map_pg_err)?;

        sqlx::query(
            "INSERT INTO diagnostic_events
             (id, tenant_id, run_id, sequence, kind, data_json, timestamp,
              correlation_id, schema_version, conversation_id, causation_id,
              scope_id, durability, trace_id, span_id, turn_id, step_id,
              effect_intent_id, effect_attempt_id, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12,
                     $13, $14, $15, $16, $17, $18, $19, $20)",
        )
        .bind(&event.id)
        .bind(&tenant)
        .bind(&event.run_id)
        .bind(sequence)
        .bind(&event.event_type)
        .bind(&event.payload)
        .bind(ts)
        .bind(&event.correlation_id)
        .bind(schema_version)
        .bind(&event.conversation_id)
        .bind(&event.causation_id)
        .bind(&event.scope_id)
        .bind(&event.durability)
        .bind(&event.trace_id)
        .bind(&event.span_id)
        .bind(&event.turn_id)
        .bind(&event.step_id)
        .bind(&event.effect_intent_id)
        .bind(&event.effect_attempt_id)
        .bind(expires)
        .execute(&mut *tx)
        .await
        .map_err(map_pg_err)?;

        tx.commit().await.map_err(map_pg_err)?;
        Ok(())
    }

    async fn read_from_cursor(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        let cursor = u64_to_db_i64(cursor, "cursor")?;
        let limit = usize_to_db_i64(limit, "limit")?;
        let mut tx = self.pool().begin().await.map_err(map_pg_err)?;
        self.set_tenant(&mut tx).await.map_err(map_pg_err)?;

        let rows = sqlx::query(&format!(
            "SELECT {EVENT_COLS} FROM durable_events
             WHERE global_sequence > $1
             ORDER BY global_sequence ASC
             LIMIT $2"
        ))
        .bind(cursor)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(map_pg_err)?;

        rows.iter().map(row_to_event).collect()
    }

    async fn read_run_events(&self, run_id: RunId) -> Result<Vec<StoredEvent>, EventStoreError> {
        let run_str = run_id.to_string();

        let mut tx = self.pool().begin().await.map_err(map_pg_err)?;
        self.set_tenant(&mut tx).await.map_err(map_pg_err)?;

        let rows = sqlx::query(&format!(
            "SELECT {EVENT_COLS} FROM durable_events
             WHERE run_id = $1
             ORDER BY sequence ASC"
        ))
        .bind(&run_str)
        .fetch_all(&mut *tx)
        .await
        .map_err(map_pg_err)?;

        rows.iter().map(row_to_event).collect()
    }

    async fn query(&self, filter: EventFilter) -> Result<Vec<StoredEvent>, EventStoreError> {
        let mut conditions: Vec<String> = Vec::new();
        let mut param_idx = 1u32;

        // We will build up bind values with a dynamic query approach
        let mut tx = self.pool().begin().await.map_err(map_pg_err)?;
        self.set_tenant(&mut tx).await.map_err(map_pg_err)?;

        let mut binds_run_id = None;
        let mut binds_conversation_id = None;
        let mut binds_scope_id = None;
        let mut binds_event_types = Vec::new();
        let mut binds_since: Option<i64> = None;
        let mut binds_limit: Option<i64> = None;

        if let Some(ref run_id) = filter.run_id {
            conditions.push(format!("run_id = ${param_idx}"));
            binds_run_id = Some(run_id.to_string());
            param_idx += 1;
        }

        if let Some(ref conversation_id) = filter.conversation_id {
            conditions.push(format!("conversation_id = ${param_idx}"));
            binds_conversation_id = Some(conversation_id.clone());
            param_idx += 1;
        }

        if let Some(ref scope_id) = filter.scope_id {
            conditions.push(format!("scope_id = ${param_idx}"));
            binds_scope_id = Some(scope_id.clone());
            param_idx += 1;
        }

        if !filter.event_types.is_empty() {
            conditions.push(format!("kind = ANY(${param_idx})"));
            binds_event_types = filter.event_types.clone();
            param_idx += 1;
        }

        if let Some(since) = filter.since_global_sequence {
            conditions.push(format!("global_sequence >= ${param_idx}"));
            binds_since = Some(u64_to_db_i64(since, "since_global_sequence")?);
            param_idx += 1;
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let limit_clause = if let Some(limit) = filter.limit {
            binds_limit = Some(usize_to_db_i64(limit, "limit")?);
            format!("LIMIT ${param_idx}")
        } else {
            String::new()
        };

        let sql = format!(
            "SELECT {EVENT_COLS} FROM durable_events {where_clause} ORDER BY global_sequence ASC {limit_clause}"
        );

        // Build query dynamically using sqlx::query
        let mut query = sqlx::query(&sql);

        if let Some(ref run_id) = binds_run_id {
            query = query.bind(run_id);
        }
        if let Some(ref conversation_id) = binds_conversation_id {
            query = query.bind(conversation_id);
        }
        if let Some(ref scope_id) = binds_scope_id {
            query = query.bind(scope_id);
        }
        if !binds_event_types.is_empty() {
            query = query.bind(&binds_event_types);
        }
        if let Some(since) = binds_since {
            query = query.bind(since);
        }
        if let Some(limit) = binds_limit {
            query = query.bind(limit);
        }

        let rows = query.fetch_all(&mut *tx).await.map_err(map_pg_err)?;

        rows.iter().map(row_to_event).collect()
    }

    async fn max_sequence(&self, run_id: RunId) -> Result<u64, EventStoreError> {
        let run_str = run_id.to_string();

        let mut tx = self.pool().begin().await.map_err(map_pg_err)?;
        self.set_tenant(&mut tx).await.map_err(map_pg_err)?;

        let max: Option<i64> =
            sqlx::query_scalar("SELECT MAX(sequence) FROM durable_events WHERE run_id = $1")
                .bind(&run_str)
                .fetch_one(&mut *tx)
                .await
                .map_err(map_pg_err)?;

        db_i64_to_u64(max.unwrap_or(0), "sequence")
    }

    async fn has_terminal_event(&self, run_id: RunId) -> Result<bool, EventStoreError> {
        let run_str = run_id.to_string();

        let mut tx = self.pool().begin().await.map_err(map_pg_err)?;
        self.set_tenant(&mut tx).await.map_err(map_pg_err)?;

        let has: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM durable_events WHERE run_id = $1 AND kind = ANY($2))",
        )
        .bind(&run_str)
        .bind(TERMINAL_EVENT_TYPES)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_pg_err)?;

        Ok(has)
    }
}
