//! Durable [`InteractionStore`] implementation backed by [`SqlitePool`].

use std::str::FromStr;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use polkagent_core::ids::{ConversationId, RunId};
use polkagent_interaction::{
    InteractionError, InteractionErrorCode, InteractionEvent, InteractionEventEnvelope,
    InteractionEventId, InteractionRunLink, InteractionStore, InteractionTurnId,
    NewInteractionEvent, NewInteractionTurn, StoredInteractionTurn, TurnHandle, TurnState,
    TurnSummary,
};
use rusqlite::{Connection, OptionalExtension, Transaction};
use uuid::Uuid;

use crate::pool::SqlitePool;

/// SQLite-backed persistence for durable interactions, turn correlations, and replay.
#[derive(Debug, Clone)]
pub struct SqliteInteractionStore {
    pool: SqlitePool,
}

impl SqliteInteractionStore {
    /// Wrap an initialized and migrated `SQLite` pool.
    #[must_use]
    pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Access the underlying pool used by the store.
    #[must_use]
    pub const fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

#[async_trait]
impl InteractionStore for SqliteInteractionStore {
    async fn create_turn(
        &self,
        turn: NewInteractionTurn,
    ) -> Result<StoredInteractionTurn, InteractionError> {
        turn.validate()?;
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || create_turn_blocking(&pool, &turn))
            .await
            .map_err(join_error)?
    }

    async fn load_turn(
        &self,
        turn_id: InteractionTurnId,
    ) -> Result<StoredInteractionTurn, InteractionError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            load_turn_conn(&writer, turn_id)
        })
        .await
        .map_err(join_error)?
    }

    async fn list_turns(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<StoredInteractionTurn>, InteractionError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let mut statement = writer
                .prepare(
                    "SELECT id FROM interaction_turns
                     WHERE conversation_id = ?1 ORDER BY ordinal ASC",
                )
                .map_err(|error| backend_error("prepare turn list", &error))?;
            let ids = statement
                .query_map([conversation_id.to_string()], |row| row.get::<_, String>(0))
                .map_err(|error| backend_error("query turn list", &error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| backend_error("decode turn list", &error))?;
            ids.into_iter()
                .map(|id| {
                    parse_id("interaction turn", &id).and_then(|id| load_turn_conn(&writer, id))
                })
                .collect()
        })
        .await
        .map_err(join_error)?
    }

    async fn append_event(
        &self,
        event: NewInteractionEvent,
    ) -> Result<InteractionEventEnvelope, InteractionError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || append_event_blocking(&pool, &event))
            .await
            .map_err(join_error)?
    }

    async fn load_events(
        &self,
        conversation_id: ConversationId,
        after_sequence: u64,
        limit: u32,
    ) -> Result<Vec<InteractionEventEnvelope>, InteractionError> {
        if limit == 0 || limit > 1_000 {
            return Err(InteractionError::invalid_request(
                "interaction event replay limit must be between 1 and 1000",
            ));
        }
        let after_sequence = i64::try_from(after_sequence).map_err(|_| {
            InteractionError::invalid_request("interaction event checkpoint is too large")
        })?;
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let mut statement = writer
                .prepare(
                    "SELECT id, conversation_id, turn_id, sequence, payload_json, created_at
                     FROM interaction_events
                     WHERE conversation_id = ?1 AND sequence > ?2
                     ORDER BY sequence ASC LIMIT ?3",
                )
                .map_err(|error| backend_error("prepare event replay", &error))?;
            let rows = statement
                .query_map(
                    rusqlite::params![
                        conversation_id.to_string(),
                        after_sequence,
                        i64::from(limit)
                    ],
                    |row| {
                        Ok(RawEvent {
                            id: row.get(0)?,
                            conversation_id: row.get(1)?,
                            turn_id: row.get(2)?,
                            sequence: row.get(3)?,
                            payload_json: row.get(4)?,
                            created_at: row.get(5)?,
                        })
                    },
                )
                .map_err(|error| backend_error("query event replay", &error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| backend_error("decode event replay", &error))?;
            rows.iter().map(decode_event).collect()
        })
        .await
        .map_err(join_error)?
    }
}

fn create_turn_blocking(
    pool: &SqlitePool,
    turn: &NewInteractionTurn,
) -> Result<StoredInteractionTurn, InteractionError> {
    let mut writer = pool.writer();
    let transaction = writer
        .transaction()
        .map_err(|error| backend_error("begin interaction turn transaction", &error))?;

    if turn_exists(&transaction, turn.turn_id)? {
        let existing = load_turn_conn(&transaction, turn.turn_id)?;
        let initial_event_id = transaction
            .query_row(
                "SELECT id FROM interaction_events
                 WHERE turn_id = ?1 ORDER BY sequence ASC LIMIT 1",
                [turn.turn_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .map_err(|error| backend_error("load initial interaction event", &error))?;
        let initial_event_id: InteractionEventId =
            parse_id("interaction event", &initial_event_id)?;
        if existing.summary.handle.conversation_id == turn.conversation_id
            && existing.summary.ordinal == turn.ordinal
            && existing.target == turn.target
            && existing.config == turn.config
            && existing.user_message_id == turn.user_message_id
            && existing.runs == turn.runs
            && existing.summary.started_at == turn.started_at
            && initial_event_id == turn.initial_event_id
        {
            return Ok(existing);
        }
        return Err(conflict_error(
            "interaction turn id is already bound to different durable input",
        ));
    }

    ensure_message_correlation(&transaction, turn.user_message_id, turn.conversation_id)?;
    for run in &turn.runs {
        ensure_run_correlation(&transaction, run.run_id, turn.conversation_id)?;
    }

    let target_json = encode_json("interaction target", &turn.target)?;
    let config_json = encode_json("interaction config", &turn.config)?;
    transaction
        .execute(
            "INSERT INTO interaction_turns
                 (id, conversation_id, ordinal, state, target_json, config_json,
                  user_message_id, started_at)
             VALUES (?1, ?2, ?3, 'running', ?4, ?5, ?6, ?7)",
            rusqlite::params![
                turn.turn_id.to_string(),
                turn.conversation_id.to_string(),
                i64::from(turn.ordinal),
                target_json,
                config_json,
                turn.user_message_id.to_string(),
                turn.started_at.to_rfc3339(),
            ],
        )
        .map_err(|error| write_error("create interaction turn", &error))?;

    let mut ordered_runs = turn.runs.clone();
    ordered_runs.sort_unstable_by_key(|run| run.ordinal);
    for run in &ordered_runs {
        let role_json = encode_json("interaction run role", &run.role)?;
        transaction
            .execute(
                "INSERT INTO interaction_turn_runs (turn_id, run_id, role_json, ordinal)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    turn.turn_id.to_string(),
                    run.run_id.to_string(),
                    role_json,
                    i64::from(run.ordinal),
                ],
            )
            .map_err(|error| write_error("link interaction run", &error))?;
    }

    let initial = InteractionEvent::TurnStarted {
        target: turn.target.clone(),
        runs: ordered_runs
            .iter()
            .map(|run| (run.run_id, run.role.clone()))
            .collect(),
    };
    insert_event(
        &transaction,
        &NewInteractionEvent {
            event_id: turn.initial_event_id,
            conversation_id: turn.conversation_id,
            turn_id: turn.turn_id,
            timestamp: turn.started_at,
            event: initial,
        },
    )?;
    let stored = load_turn_conn(&transaction, turn.turn_id)?;
    transaction
        .commit()
        .map_err(|error| backend_error("commit interaction turn", &error))?;
    Ok(stored)
}

fn append_event_blocking(
    pool: &SqlitePool,
    event: &NewInteractionEvent,
) -> Result<InteractionEventEnvelope, InteractionError> {
    if matches!(event.event, InteractionEvent::TurnStarted { .. }) {
        return Err(InteractionError::invalid_request(
            "turn_started must be written atomically with interaction turn creation",
        ));
    }

    let mut writer = pool.writer();
    let transaction = writer
        .transaction()
        .map_err(|error| backend_error("begin interaction event transaction", &error))?;
    if let Some(existing) = load_event_by_id(&transaction, event.event_id)? {
        if existing.event_id == event.event_id
            && existing.conversation_id == event.conversation_id
            && existing.turn_id == event.turn_id
            && existing.timestamp == event.timestamp
            && existing.event == event.event
        {
            return Ok(existing);
        }
        return Err(conflict_error(
            "interaction event id is already bound to different durable input",
        ));
    }

    let state = transaction
        .query_row(
            "SELECT state FROM interaction_turns WHERE id = ?1 AND conversation_id = ?2",
            rusqlite::params![event.turn_id.to_string(), event.conversation_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| backend_error("load interaction turn state", &error))?
        .ok_or_else(|| not_found_error("interaction turn was not found for this interaction"))?;
    let state = decode_turn_state(&state)?;
    if state.is_terminal() {
        return Err(conflict_error(
            "cannot append an event after the interaction turn is terminal",
        ));
    }

    let envelope = insert_event(&transaction, event)?;
    project_turn_state(&transaction, event)?;
    transaction
        .commit()
        .map_err(|error| backend_error("commit interaction event", &error))?;
    Ok(envelope)
}

fn insert_event(
    transaction: &Transaction<'_>,
    event: &NewInteractionEvent,
) -> Result<InteractionEventEnvelope, InteractionError> {
    let next_sequence: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM interaction_events
             WHERE conversation_id = ?1",
            [event.conversation_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|error| backend_error("allocate interaction event sequence", &error))?;
    let payload_json = encode_json("interaction event", &event.event)?;
    let is_terminal = i64::from(event.event.is_terminal());
    transaction
        .execute(
            "INSERT INTO interaction_events
                 (id, conversation_id, turn_id, sequence, kind, payload_json,
                  is_terminal, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                event.event_id.to_string(),
                event.conversation_id.to_string(),
                event.turn_id.to_string(),
                next_sequence,
                event_kind(&event.event),
                payload_json,
                is_terminal,
                event.timestamp.to_rfc3339(),
            ],
        )
        .map_err(|error| write_error("append interaction event", &error))?;
    Ok(InteractionEventEnvelope {
        event_id: event.event_id,
        conversation_id: event.conversation_id,
        turn_id: event.turn_id,
        sequence: u64::try_from(next_sequence)
            .map_err(|_| invariant_error("stored interaction event sequence is invalid"))?,
        timestamp: event.timestamp,
        event: event.event.clone(),
    })
}

fn project_turn_state(
    transaction: &Transaction<'_>,
    event: &NewInteractionEvent,
) -> Result<(), InteractionError> {
    let (state, terminal, error_json) = match &event.event {
        InteractionEvent::ApprovalRequested { .. } => (Some("awaiting_approval"), false, None),
        InteractionEvent::ApprovalResolved { .. } => (Some("running"), false, None),
        InteractionEvent::TurnCompleted { .. } => (Some("completed"), true, None),
        InteractionEvent::TurnFailed { error } => (
            Some("failed"),
            true,
            Some(encode_json("interaction failure", error)?),
        ),
        InteractionEvent::TurnCancelled { .. } => (Some("cancelled"), true, None),
        InteractionEvent::TurnTimedOut => (Some("timed_out"), true, None),
        _ => (None, false, None),
    };
    let Some(state) = state else {
        return Ok(());
    };
    let completed_at = terminal.then(|| event.timestamp.to_rfc3339());
    let changed = transaction
        .execute(
            "UPDATE interaction_turns
             SET state = ?1, completed_at = COALESCE(?2, completed_at),
                 error_json = COALESCE(?3, error_json)
             WHERE id = ?4 AND state IN ('running', 'awaiting_approval')",
            rusqlite::params![state, completed_at, error_json, event.turn_id.to_string()],
        )
        .map_err(|error| write_error("project interaction turn state", &error))?;
    if changed != 1 {
        return Err(conflict_error(
            "interaction turn state changed while appending its event",
        ));
    }
    Ok(())
}

fn turn_exists(
    connection: &Connection,
    turn_id: InteractionTurnId,
) -> Result<bool, InteractionError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM interaction_turns WHERE id = ?1)",
            [turn_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|error| backend_error("check interaction turn", &error))
}

fn ensure_message_correlation(
    connection: &Connection,
    message_id: Uuid,
    conversation_id: ConversationId,
) -> Result<(), InteractionError> {
    let stored = connection
        .query_row(
            "SELECT conversation_id FROM conversation_messages WHERE id = ?1",
            [message_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| backend_error("load interaction user message", &error))?
        .ok_or_else(|| not_found_error("interaction user message was not found"))?;
    if stored != conversation_id.to_string() {
        return Err(conflict_error(
            "interaction user message belongs to another conversation",
        ));
    }
    Ok(())
}

fn ensure_run_correlation(
    connection: &Connection,
    run_id: RunId,
    conversation_id: ConversationId,
) -> Result<(), InteractionError> {
    let stored = connection
        .query_row(
            "SELECT conversation_id FROM runs WHERE id = ?1",
            [run_id.to_string()],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|error| backend_error("load linked interaction run", &error))?
        .ok_or_else(|| not_found_error("linked interaction run was not found"))?;
    let expected = conversation_id.to_string();
    if stored.as_deref() != Some(expected.as_str()) {
        return Err(conflict_error(
            "linked run must carry the interaction conversation at creation time",
        ));
    }
    Ok(())
}

fn load_turn_conn(
    connection: &Connection,
    turn_id: InteractionTurnId,
) -> Result<StoredInteractionTurn, InteractionError> {
    let row = connection
        .query_row(
            "SELECT conversation_id, ordinal, state, target_json, config_json,
                    user_message_id, assistant_message_id, started_at, completed_at,
                    error_json,
                    (SELECT MIN(sequence) FROM interaction_events WHERE turn_id = ?1)
             FROM interaction_turns WHERE id = ?1",
            [turn_id.to_string()],
            |row| {
                Ok(RawTurn {
                    conversation_id: row.get(0)?,
                    ordinal: row.get(1)?,
                    state: row.get(2)?,
                    target_json: row.get(3)?,
                    config_json: row.get(4)?,
                    user_message_id: row.get(5)?,
                    assistant_message_id: row.get(6)?,
                    started_at: row.get(7)?,
                    completed_at: row.get(8)?,
                    error_json: row.get(9)?,
                    first_event_sequence: row.get(10)?,
                })
            },
        )
        .optional()
        .map_err(|error| backend_error("load interaction turn", &error))?
        .ok_or_else(|| not_found_error("interaction turn was not found"))?;

    let mut statement = connection
        .prepare(
            "SELECT run_id, role_json, ordinal FROM interaction_turn_runs
             WHERE turn_id = ?1 ORDER BY ordinal ASC",
        )
        .map_err(|error| backend_error("prepare interaction run links", &error))?;
    let raw_runs = statement
        .query_map([turn_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|error| backend_error("load interaction run links", &error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| backend_error("decode interaction run links", &error))?;
    let runs = raw_runs
        .into_iter()
        .map(|(run_id, role_json, ordinal)| {
            Ok(InteractionRunLink {
                run_id: parse_id("run", &run_id)?,
                role: decode_json("interaction run role", &role_json)?,
                ordinal: u32::try_from(ordinal)
                    .map_err(|_| invariant_error("stored interaction run ordinal is invalid"))?,
            })
        })
        .collect::<Result<Vec<_>, InteractionError>>()?;
    let conversation_id = parse_id("conversation", &row.conversation_id)?;
    let first_event_sequence = row
        .first_event_sequence
        .ok_or_else(|| invariant_error("interaction turn has no durable initial event"))?;
    let first_event_sequence = u64::try_from(first_event_sequence)
        .map_err(|_| invariant_error("stored initial event sequence is invalid"))?;
    let summary = TurnSummary {
        handle: TurnHandle {
            turn_id,
            conversation_id,
            run_ids: runs.iter().map(|run| run.run_id).collect(),
            first_event_sequence,
        },
        ordinal: u32::try_from(row.ordinal)
            .map_err(|_| invariant_error("stored interaction turn ordinal is invalid"))?,
        state: decode_turn_state(&row.state)?,
        started_at: parse_timestamp("interaction turn started_at", &row.started_at)?,
        completed_at: row
            .completed_at
            .as_deref()
            .map(|value| parse_timestamp("interaction turn completed_at", value))
            .transpose()?,
    };
    Ok(StoredInteractionTurn {
        summary,
        runs,
        target: decode_json("interaction target", &row.target_json)?,
        config: decode_json("interaction config", &row.config_json)?,
        user_message_id: parse_id("conversation message", &row.user_message_id)?,
        assistant_message_id: row
            .assistant_message_id
            .as_deref()
            .map(|id| parse_id("conversation message", id))
            .transpose()?,
        error: row
            .error_json
            .as_deref()
            .map(|value| decode_json("interaction failure", value))
            .transpose()?,
    })
}

fn load_event_by_id(
    connection: &Connection,
    event_id: InteractionEventId,
) -> Result<Option<InteractionEventEnvelope>, InteractionError> {
    connection
        .query_row(
            "SELECT id, conversation_id, turn_id, sequence, payload_json, created_at
             FROM interaction_events WHERE id = ?1",
            [event_id.to_string()],
            |row| {
                Ok(RawEvent {
                    id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    turn_id: row.get(2)?,
                    sequence: row.get(3)?,
                    payload_json: row.get(4)?,
                    created_at: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(|error| backend_error("load interaction event", &error))?
        .map(|row| decode_event(&row))
        .transpose()
}

fn decode_event(row: &RawEvent) -> Result<InteractionEventEnvelope, InteractionError> {
    Ok(InteractionEventEnvelope {
        event_id: parse_id("interaction event", &row.id)?,
        conversation_id: parse_id("conversation", &row.conversation_id)?,
        turn_id: parse_id("interaction turn", &row.turn_id)?,
        sequence: u64::try_from(row.sequence)
            .map_err(|_| invariant_error("stored interaction event sequence is invalid"))?,
        timestamp: parse_timestamp("interaction event timestamp", &row.created_at)?,
        event: decode_json("interaction event", &row.payload_json)?,
    })
}

fn decode_turn_state(value: &str) -> Result<TurnState, InteractionError> {
    match value {
        "pending" => Ok(TurnState::Pending),
        "running" => Ok(TurnState::Running),
        "awaiting_approval" => Ok(TurnState::AwaitingApproval),
        "completed" => Ok(TurnState::Completed),
        "failed" => Ok(TurnState::Failed),
        "cancelled" => Ok(TurnState::Cancelled),
        "timed_out" => Ok(TurnState::TimedOut),
        _ => Err(invariant_error("stored interaction turn state is invalid")),
    }
}

const fn event_kind(event: &InteractionEvent) -> &'static str {
    match event {
        InteractionEvent::TurnStarted { .. } => "turn_started",
        InteractionEvent::AgentMessageDelta { .. } => "agent_message_delta",
        InteractionEvent::ThoughtDelta { .. } => "thought_delta",
        InteractionEvent::ToolCallStarted { .. } => "tool_call_started",
        InteractionEvent::ToolCallUpdated { .. } => "tool_call_updated",
        InteractionEvent::PlanUpdated { .. } => "plan_updated",
        InteractionEvent::ApprovalRequested { .. } => "approval_requested",
        InteractionEvent::ApprovalResolved { .. } => "approval_resolved",
        InteractionEvent::UsageUpdated { .. } => "usage_updated",
        InteractionEvent::RunStateChanged { .. } => "run_state_changed",
        InteractionEvent::TurnCompleted { .. } => "turn_completed",
        InteractionEvent::TurnFailed { .. } => "turn_failed",
        InteractionEvent::TurnCancelled { .. } => "turn_cancelled",
        InteractionEvent::TurnTimedOut => "turn_timed_out",
    }
}

fn encode_json<T: serde::Serialize>(context: &str, value: &T) -> Result<String, InteractionError> {
    serde_json::to_string(value).map_err(|_| invariant_error(&format!("cannot encode {context}")))
}

fn decode_json<T: serde::de::DeserializeOwned>(
    context: &str,
    value: &str,
) -> Result<T, InteractionError> {
    serde_json::from_str(value)
        .map_err(|_| invariant_error(&format!("stored {context} is invalid")))
}

fn parse_id<T: FromStr>(context: &str, value: &str) -> Result<T, InteractionError> {
    T::from_str(value).map_err(|_| invariant_error(&format!("stored {context} id is invalid")))
}

fn parse_timestamp(context: &str, value: &str) -> Result<DateTime<Utc>, InteractionError> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|_| invariant_error(&format!("stored {context} is invalid")))
}

fn backend_error(context: &str, error: &rusqlite::Error) -> InteractionError {
    tracing::error!(%error, context, "interaction SQLite operation failed");
    InteractionError::new(
        InteractionErrorCode::Internal,
        format!("{context}: durable interaction store failed"),
    )
}

fn write_error(context: &str, error: &rusqlite::Error) -> InteractionError {
    if crate::StoreError::is_unique_violation(error) {
        return conflict_error(&format!("{context}: durable identity already exists"));
    }
    backend_error(context, error)
}

fn join_error(_: tokio::task::JoinError) -> InteractionError {
    invariant_error("durable interaction store worker stopped unexpectedly")
}

fn not_found_error(message: &str) -> InteractionError {
    InteractionError::new(InteractionErrorCode::NotFound, message)
}

fn conflict_error(message: &str) -> InteractionError {
    InteractionError::new(InteractionErrorCode::Conflict, message)
}

fn invariant_error(message: &str) -> InteractionError {
    InteractionError::new(InteractionErrorCode::Internal, message)
}

struct RawTurn {
    conversation_id: String,
    ordinal: i64,
    state: String,
    target_json: String,
    config_json: String,
    user_message_id: String,
    assistant_message_id: Option<String>,
    started_at: String,
    completed_at: Option<String>,
    error_json: Option<String>,
    first_event_sequence: Option<i64>,
}

struct RawEvent {
    id: String,
    conversation_id: String,
    turn_id: String,
    sequence: i64,
    payload_json: String,
    created_at: String,
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "SQLite interaction tests use fixed fixtures whose setup failures must identify the broken boundary"
)]
mod tests {
    use polkagent_core::ids::AgentId;
    use polkagent_interaction::{
        InteractionConfig, InteractionEventId, InteractionTarget, RunRole, TurnResult, UsageView,
    };

    use super::*;
    use crate::migrations;

    struct Fixture {
        store: SqliteInteractionStore,
        conversation_id: ConversationId,
        run_id: RunId,
        message_id: Uuid,
    }

    fn fixture() -> Fixture {
        let pool = SqlitePool::open_in_memory().expect("open SQLite");
        {
            let writer = pool.writer();
            migrations::migrate(&writer).expect("migrate SQLite");
        }
        let conversation_id = ConversationId::new();
        let agent_id = AgentId::new();
        let run_id = RunId::new();
        let message_id = Uuid::now_v7();
        let now = Utc::now().to_rfc3339();
        {
            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at)
                     VALUES (?1, 'interaction-test', 'active', '{}', ?2, ?2)",
                    rusqlite::params![agent_id.to_string(), now],
                )
                .expect("insert agent");
            writer
                .execute(
                    "INSERT INTO conversations
                         (id, agent_id, message_count, metadata_json, created_at, updated_at)
                     VALUES (?1, ?2, 1, '{}', ?3, ?3)",
                    rusqlite::params![conversation_id.to_string(), agent_id.to_string(), now],
                )
                .expect("insert conversation");
            writer
                .execute(
                    "INSERT INTO conversation_messages
                         (id, conversation_id, role, content_json, created_at)
                     VALUES (?1, ?2, 'user',
                             '{\"type\":\"text\",\"text\":\"hello\"}', ?3)",
                    rusqlite::params![message_id.to_string(), conversation_id.to_string(), now],
                )
                .expect("insert user message");
            writer
                .execute(
                    "INSERT INTO runs
                         (id, agent_id, conversation_id, state, params_json,
                          created_at, updated_at)
                     VALUES (?1, ?2, ?3, 'created', '{}', ?4, ?4)",
                    rusqlite::params![
                        run_id.to_string(),
                        agent_id.to_string(),
                        conversation_id.to_string(),
                        now
                    ],
                )
                .expect("insert correlated run");
        }
        Fixture {
            store: SqliteInteractionStore::new(pool),
            conversation_id,
            run_id,
            message_id,
        }
    }

    fn new_turn(fixture: &Fixture) -> NewInteractionTurn {
        let target = InteractionTarget::Agent(AgentId::new());
        NewInteractionTurn {
            turn_id: InteractionTurnId::new(),
            conversation_id: fixture.conversation_id,
            ordinal: 1,
            target: target.clone(),
            config: InteractionConfig::new(target),
            user_message_id: fixture.message_id,
            runs: vec![InteractionRunLink {
                run_id: fixture.run_id,
                role: RunRole::Primary,
                ordinal: 1,
            }],
            initial_event_id: InteractionEventId::new(),
            started_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn create_is_idempotent_and_replays_terminal_lifecycle() {
        let fixture = fixture();
        let turn = new_turn(&fixture);
        let created = fixture
            .store
            .create_turn(turn.clone())
            .await
            .expect("create turn");
        assert_eq!(created.summary.handle.first_event_sequence, 1);
        assert_eq!(created.summary.state, TurnState::Running);
        assert_eq!(created.runs, turn.runs);
        assert_eq!(
            fixture
                .store
                .create_turn(turn.clone())
                .await
                .expect("retry create"),
            created
        );

        let delta = NewInteractionEvent {
            event_id: InteractionEventId::new(),
            conversation_id: fixture.conversation_id,
            turn_id: turn.turn_id,
            timestamp: Utc::now(),
            event: InteractionEvent::AgentMessageDelta {
                run_id: fixture.run_id,
                text: "hello".to_owned(),
            },
        };
        let delta_envelope = fixture
            .store
            .append_event(delta.clone())
            .await
            .expect("append delta");
        assert_eq!(delta_envelope.sequence, 2);
        assert_eq!(
            fixture
                .store
                .append_event(delta)
                .await
                .expect("retry delta")
                .sequence,
            2
        );

        let completed = NewInteractionEvent {
            event_id: InteractionEventId::new(),
            conversation_id: fixture.conversation_id,
            turn_id: turn.turn_id,
            timestamp: Utc::now(),
            event: InteractionEvent::TurnCompleted {
                result: TurnResult {
                    text: "hello".to_owned(),
                    run_ids: vec![fixture.run_id],
                    usage: UsageView::default(),
                },
            },
        };
        let terminal_envelope = fixture
            .store
            .append_event(completed.clone())
            .await
            .expect("append terminal event");
        assert_eq!(terminal_envelope.sequence, 3);
        assert_eq!(
            fixture
                .store
                .append_event(completed)
                .await
                .expect("retry terminal event")
                .sequence,
            3
        );

        let stored = fixture
            .store
            .load_turn(turn.turn_id)
            .await
            .expect("load turn");
        assert_eq!(stored.summary.state, TurnState::Completed);
        assert!(stored.summary.completed_at.is_some());
        let replay = fixture
            .store
            .load_events(fixture.conversation_id, 1, 10)
            .await
            .expect("replay events");
        assert_eq!(replay, vec![delta_envelope, terminal_envelope]);

        let late = NewInteractionEvent {
            event_id: InteractionEventId::new(),
            conversation_id: fixture.conversation_id,
            turn_id: turn.turn_id,
            timestamp: Utc::now(),
            event: InteractionEvent::TurnTimedOut,
        };
        let error = fixture
            .store
            .append_event(late)
            .await
            .expect_err("terminal turn rejects later event");
        assert_eq!(error.code, InteractionErrorCode::Conflict);
    }

    #[tokio::test]
    async fn creation_requires_run_correlation_at_run_creation_time() {
        let fixture = fixture();
        let other_conversation = ConversationId::new();
        {
            let writer = fixture.store.pool().writer();
            writer
                .execute(
                    "UPDATE runs SET conversation_id = ?1 WHERE id = ?2",
                    rusqlite::params![other_conversation.to_string(), fixture.run_id.to_string()],
                )
                .expect("move run correlation");
        }
        let error = fixture
            .store
            .create_turn(new_turn(&fixture))
            .await
            .expect_err("mismatched run correlation must fail");
        assert_eq!(error.code, InteractionErrorCode::Conflict);
        assert!(fixture
            .store
            .list_turns(fixture.conversation_id)
            .await
            .expect("list turns")
            .is_empty());
    }
}
