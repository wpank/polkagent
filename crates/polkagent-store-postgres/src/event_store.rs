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

fn row_to_event(row: &sqlx::postgres::PgRow) -> Result<StoredEvent, EventStoreError> {
    let global_sequence: i64 = row.get("global_sequence");
    let data_json: serde_json::Value = row.get("data_json");
    let correlation_id: Option<String> = row.get("correlation_id");

    Ok(StoredEvent {
        id: row.get("id"),
        run_id: row.get("run_id"),
        sequence: row.get::<i64, _>("sequence") as u64,
        event_type: row.get("kind"),
        payload: data_json,
        timestamp: row
            .get::<chrono::DateTime<chrono::Utc>, _>("timestamp")
            .to_rfc3339(),
        correlation_id: correlation_id.unwrap_or_default(),
        schema_version: row.get::<i32, _>("schema_version") as u32,
        global_sequence: global_sequence as u64,
        conversation_id: None,
        causation_id: None,
        scope_id: String::new(),
        durability: String::new(),
        trace_id: None,
        span_id: None,
    })
}

const EVENT_COLS: &str =
    "global_sequence, id, run_id, sequence, kind, data_json, timestamp, correlation_id, schema_version";

#[async_trait]
impl EventStore for PgPool {
    async fn append_durable(&self, event: StoredEvent) -> Result<StoredEvent, EventStoreError> {
        let tenant = self.tenant_id().to_string();

        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(map_pg_err)?;
        self.set_tenant(&mut *tx).await.map_err(map_pg_err)?;

        // 1. Non-monotonic sequence check.
        let current_max: Option<i64> = sqlx::query_scalar(
            "SELECT MAX(sequence) FROM durable_events WHERE run_id = $1",
        )
        .bind(&event.run_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_pg_err)?;

        let current_max = current_max.unwrap_or(0) as u64;
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
             (id, tenant_id, run_id, sequence, kind, data_json, timestamp, correlation_id, schema_version)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             RETURNING {EVENT_COLS}"
        ))
        .bind(&event.id)
        .bind(&tenant)
        .bind(&event.run_id)
        .bind(event.sequence as i64)
        .bind(&event.event_type)
        .bind(payload)
        .bind(ts)
        .bind(&event.correlation_id)
        .bind(event.schema_version as i32)
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

        let ts = chrono::DateTime::parse_from_rfc3339(&event.timestamp)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .map_err(|e| EventStoreError::Serialisation(format!("invalid timestamp: {e}")))?;

        let expires = chrono::DateTime::parse_from_rfc3339(&expires_at)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .map_err(|e| EventStoreError::Serialisation(format!("invalid expires_at: {e}")))?;

        let mut tx = self.pool().begin().await.map_err(map_pg_err)?;
        self.set_tenant(&mut *tx).await.map_err(map_pg_err)?;

        sqlx::query(
            "INSERT INTO diagnostic_events
             (id, tenant_id, run_id, sequence, kind, data_json, timestamp, correlation_id, schema_version, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(&event.id)
        .bind(&tenant)
        .bind(&event.run_id)
        .bind(event.sequence as i64)
        .bind(&event.event_type)
        .bind(&event.payload)
        .bind(ts)
        .bind(&event.correlation_id)
        .bind(event.schema_version as i32)
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
        let mut tx = self.pool().begin().await.map_err(map_pg_err)?;
        self.set_tenant(&mut *tx).await.map_err(map_pg_err)?;

        let rows = sqlx::query(&format!(
            "SELECT {EVENT_COLS} FROM durable_events
             WHERE global_sequence > $1
             ORDER BY global_sequence ASC
             LIMIT $2"
        ))
        .bind(cursor as i64)
        .bind(limit as i64)
        .fetch_all(&mut *tx)
        .await
        .map_err(map_pg_err)?;

        rows.iter().map(row_to_event).collect()
    }

    async fn read_run_events(
        &self,
        run_id: RunId,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        let run_str = run_id.to_string();

        let mut tx = self.pool().begin().await.map_err(map_pg_err)?;
        self.set_tenant(&mut *tx).await.map_err(map_pg_err)?;

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
        self.set_tenant(&mut *tx).await.map_err(map_pg_err)?;

        let mut binds_run_id = None;
        let mut binds_event_types = Vec::new();
        let mut binds_since: Option<i64> = None;
        let mut binds_limit: Option<i64> = None;

        if let Some(ref run_id) = filter.run_id {
            conditions.push(format!("run_id = ${param_idx}"));
            binds_run_id = Some(run_id.to_string());
            param_idx += 1;
        }

        if !filter.event_types.is_empty() {
            conditions.push(format!("kind = ANY(${param_idx})"));
            binds_event_types = filter.event_types.clone();
            param_idx += 1;
        }

        if let Some(since) = filter.since_global_sequence {
            conditions.push(format!("global_sequence >= ${param_idx}"));
            binds_since = Some(since as i64);
            param_idx += 1;
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let limit_clause = if let Some(limit) = filter.limit {
            binds_limit = Some(limit as i64);
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
        self.set_tenant(&mut *tx).await.map_err(map_pg_err)?;

        let max: Option<i64> = sqlx::query_scalar(
            "SELECT MAX(sequence) FROM durable_events WHERE run_id = $1",
        )
        .bind(&run_str)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_pg_err)?;

        Ok(max.unwrap_or(0) as u64)
    }

    async fn has_terminal_event(&self, run_id: RunId) -> Result<bool, EventStoreError> {
        let run_str = run_id.to_string();

        let mut tx = self.pool().begin().await.map_err(map_pg_err)?;
        self.set_tenant(&mut *tx).await.map_err(map_pg_err)?;

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
