//! Durable [`InteractionStore`] implementation backed by [`SqlitePool`].

use std::str::FromStr;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use polkagent_conversation::types::{MessageContent, MessageRole};
use polkagent_core::ids::{ConversationId, RunId};
use polkagent_interaction::{
    InteractionError, InteractionErrorCode, InteractionEvent, InteractionEventEnvelope,
    InteractionEventId, InteractionRunLink, InteractionState, InteractionStore, InteractionSummary,
    InteractionTurnId, ListInteractionsRequest, NewAssistantMessage, NewInteraction,
    NewInteractionEvent, NewInteractionTurn, StoredInteractionTurn, StoredTranscriptMessage,
    StoredTranscriptRole, StoredTranscriptTurn, TranscriptRequest, TurnHandle, TurnState,
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

    /// Create a turn and report whether this call won the durable insert.
    ///
    /// A deterministic retry with the same caller turn identity returns the
    /// stored winner and `false`, even though its locally sampled start time
    /// differs. Callers use the outcome to ensure only the winner activates
    /// the correlated run.
    pub async fn create_turn_once(
        &self,
        turn: NewInteractionTurn,
    ) -> Result<(StoredInteractionTurn, bool), InteractionError> {
        turn.validate()?;
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || create_turn_blocking(&pool, &turn))
            .await
            .map_err(join_error)?
    }

    /// Remove correlated `created` runs that were never attached to a durable
    /// interaction turn because the process stopped between preparation
    /// phases. Runtime startup calls this before accepting prompts.
    pub async fn discard_orphan_prepared_runs(&self) -> Result<u32, InteractionError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let changed = writer
                .execute(
                    "DELETE FROM runs
                     WHERE state = 'created' AND conversation_id IS NOT NULL
                       AND NOT EXISTS (
                           SELECT 1 FROM interaction_turn_runs links
                           WHERE links.run_id = runs.id
                       )",
                    [],
                )
                .map_err(|error| backend_error("discard orphan prepared runs", &error))?;
            u32::try_from(changed)
                .map_err(|_| invariant_error("orphan prepared run count exceeds supported range"))
        })
        .await
        .map_err(join_error)?
    }
}

#[async_trait]
impl InteractionStore for SqliteInteractionStore {
    async fn create_interaction(
        &self,
        interaction: NewInteraction,
    ) -> Result<InteractionSummary, InteractionError> {
        interaction.validate()?;
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let mut writer = pool.writer();
            let transaction = writer
                .transaction()
                .map_err(|error| backend_error("begin interaction session transaction", &error))?;
            if let Some(existing) =
                load_interaction_conn_optional(&transaction, interaction.conversation_id)?
            {
                if existing.config == interaction.config
                    && existing.created_at == interaction.created_at
                {
                    return Ok(existing);
                }
                return Err(conflict_error(
                    "interaction is already bound to different durable defaults",
                ));
            }
            let config_json = encode_json("interaction config", &interaction.config)?;
            transaction
                .execute(
                    "INSERT INTO interaction_sessions
                         (conversation_id, config_json, state, created_at, updated_at)
                     VALUES (?1, ?2, 'active', ?3, ?3)",
                    rusqlite::params![
                        interaction.conversation_id.to_string(),
                        config_json,
                        interaction.created_at.to_rfc3339(),
                    ],
                )
                .map_err(|error| write_error("create interaction session", &error))?;
            let summary = load_interaction_conn(&transaction, interaction.conversation_id)?;
            transaction
                .commit()
                .map_err(|error| backend_error("commit interaction session", &error))?;
            Ok(summary)
        })
        .await
        .map_err(join_error)?
    }

    async fn list_interactions(
        &self,
        request: ListInteractionsRequest,
    ) -> Result<Vec<InteractionSummary>, InteractionError> {
        if request.limit == 0 || request.limit > 1_000 {
            return Err(InteractionError::invalid_request(
                "interaction list limit must be between 1 and 1000",
            ));
        }
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let state = request.state.map(encode_interaction_state);
            let mut statement = writer
                .prepare(
                    "SELECT s.conversation_id
                     FROM interaction_sessions s
                     WHERE (?1 IS NULL OR s.state = ?1)
                     ORDER BY s.updated_at DESC, s.conversation_id DESC
                     LIMIT ?2 OFFSET ?3",
                )
                .map_err(|error| backend_error("prepare interaction list", &error))?;
            let ids = statement
                .query_map(
                    rusqlite::params![state, i64::from(request.limit), i64::from(request.offset)],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|error| backend_error("query interaction list", &error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| backend_error("decode interaction list", &error))?;
            ids.into_iter()
                .map(|id| {
                    parse_id("conversation", &id).and_then(|id| load_interaction_conn(&writer, id))
                })
                .collect()
        })
        .await
        .map_err(join_error)?
    }

    async fn load_interaction(
        &self,
        conversation_id: ConversationId,
    ) -> Result<InteractionSummary, InteractionError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            load_interaction_conn(&writer, conversation_id)
        })
        .await
        .map_err(join_error)?
    }

    async fn update_interaction_config(
        &self,
        conversation_id: ConversationId,
        config: polkagent_interaction::InteractionConfig,
    ) -> Result<InteractionSummary, InteractionError> {
        config.validate()?;
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let config_json = encode_json("interaction config", &config)?;
            let changed = writer
                .execute(
                    "UPDATE interaction_sessions
                     SET config_json = ?1, updated_at = ?2
                     WHERE conversation_id = ?3 AND state = 'active'",
                    rusqlite::params![
                        config_json,
                        Utc::now().to_rfc3339(),
                        conversation_id.to_string()
                    ],
                )
                .map_err(|error| write_error("update interaction config", &error))?;
            if changed != 1 {
                return Err(conflict_or_not_found(&writer, conversation_id));
            }
            load_interaction_conn(&writer, conversation_id)
        })
        .await
        .map_err(join_error)?
    }

    async fn set_interaction_state(
        &self,
        conversation_id: ConversationId,
        state: InteractionState,
    ) -> Result<InteractionSummary, InteractionError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let changed = writer
                .execute(
                    "UPDATE interaction_sessions SET state = ?1, updated_at = ?2
                     WHERE conversation_id = ?3",
                    rusqlite::params![
                        encode_interaction_state(state),
                        Utc::now().to_rfc3339(),
                        conversation_id.to_string()
                    ],
                )
                .map_err(|error| write_error("update interaction state", &error))?;
            if changed != 1 {
                return Err(not_found_error("interaction was not found"));
            }
            load_interaction_conn(&writer, conversation_id)
        })
        .await
        .map_err(join_error)?
    }

    async fn create_turn(
        &self,
        turn: NewInteractionTurn,
    ) -> Result<StoredInteractionTurn, InteractionError> {
        self.create_turn_once(turn).await.map(|(stored, _)| stored)
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

    async fn load_transcript_turns(
        &self,
        request: TranscriptRequest,
    ) -> Result<Vec<StoredTranscriptTurn>, InteractionError> {
        request.validate()?;
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            // Preserve NotFound for an empty or out-of-range page without
            // computing the interaction's total turn count.
            let exists = writer
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM interaction_sessions WHERE conversation_id = ?1
                     )",
                    [request.conversation_id.to_string()],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(|error| backend_error("check transcript interaction", &error))?;
            if !exists {
                return Err(not_found_error("interaction was not found"));
            }
            let mut statement = writer
                .prepare(
                    "SELECT id FROM interaction_turns
                     WHERE conversation_id = ?1 ORDER BY ordinal ASC
                     LIMIT ?2 OFFSET ?3",
                )
                .map_err(|error| backend_error("prepare transcript turn page", &error))?;
            let ids = statement
                .query_map(
                    rusqlite::params![
                        request.conversation_id.to_string(),
                        i64::from(request.limit),
                        i64::from(request.offset),
                    ],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|error| backend_error("query transcript turn page", &error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| backend_error("decode transcript turn page", &error))?;
            ids.into_iter()
                .map(|id| {
                    let turn_id = parse_id("interaction turn", &id)?;
                    let turn = load_turn_conn(&writer, turn_id)?;
                    let user_message = load_transcript_message_conn(&writer, turn.user_message_id)?;
                    let assistant_message = turn
                        .assistant_message_id
                        .map(|message_id| load_transcript_message_conn(&writer, message_id))
                        .transpose()?
                        .flatten();
                    Ok(StoredTranscriptTurn {
                        turn,
                        user_message,
                        assistant_message,
                    })
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

    async fn finish_turn(
        &self,
        event: NewInteractionEvent,
        assistant_message: NewAssistantMessage,
    ) -> Result<InteractionEventEnvelope, InteractionError> {
        if !event.event.is_terminal() {
            return Err(InteractionError::invalid_request(
                "finish_turn requires a terminal interaction event",
            ));
        }
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || finish_turn_blocking(&pool, &event, &assistant_message))
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

    async fn latest_event_sequence(
        &self,
        conversation_id: ConversationId,
    ) -> Result<u64, InteractionError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let sequence: i64 = writer
                .query_row(
                    "SELECT COALESCE(MAX(sequence), 0) FROM interaction_events
                     WHERE conversation_id = ?1",
                    [conversation_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|error| backend_error("load latest interaction sequence", &error))?;
            u64::try_from(sequence)
                .map_err(|_| invariant_error("stored interaction event sequence is invalid"))
        })
        .await
        .map_err(join_error)?
    }
}

fn load_interaction_conn(
    connection: &Connection,
    conversation_id: ConversationId,
) -> Result<InteractionSummary, InteractionError> {
    load_interaction_conn_optional(connection, conversation_id)?
        .ok_or_else(|| not_found_error("interaction was not found"))
}

fn load_interaction_conn_optional(
    connection: &Connection,
    conversation_id: ConversationId,
) -> Result<Option<InteractionSummary>, InteractionError> {
    connection
        .query_row(
            "SELECT c.title, s.config_json, s.state,
                    (SELECT COUNT(*) FROM interaction_turns t
                     WHERE t.conversation_id = s.conversation_id),
                    s.created_at, s.updated_at
             FROM interaction_sessions s
             JOIN conversations c ON c.id = s.conversation_id
             WHERE s.conversation_id = ?1",
            [conversation_id.to_string()],
            |row| {
                Ok(RawInteraction {
                    title: row.get(0)?,
                    config_json: row.get(1)?,
                    state: row.get(2)?,
                    turn_count: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(|error| backend_error("load interaction session", &error))?
        .map(|row| {
            Ok(InteractionSummary {
                conversation_id,
                title: row.title,
                config: decode_json("interaction config", &row.config_json)?,
                state: decode_interaction_state(&row.state)?,
                turn_count: u32::try_from(row.turn_count)
                    .map_err(|_| invariant_error("stored interaction turn count is invalid"))?,
                created_at: parse_timestamp("interaction created_at", &row.created_at)?,
                updated_at: parse_timestamp("interaction updated_at", &row.updated_at)?,
            })
        })
        .transpose()
}

fn conflict_or_not_found(
    connection: &Connection,
    conversation_id: ConversationId,
) -> InteractionError {
    match load_interaction_conn_optional(connection, conversation_id) {
        Ok(Some(_)) => conflict_error("archived interaction configuration cannot be changed"),
        Ok(None) => not_found_error("interaction was not found"),
        Err(error) => error,
    }
}

const fn encode_interaction_state(state: InteractionState) -> &'static str {
    match state {
        InteractionState::Active => "active",
        InteractionState::Archived => "archived",
    }
}

fn decode_interaction_state(value: &str) -> Result<InteractionState, InteractionError> {
    match value {
        "active" => Ok(InteractionState::Active),
        "archived" => Ok(InteractionState::Archived),
        _ => Err(invariant_error("stored interaction state is invalid")),
    }
}

fn create_turn_blocking(
    pool: &SqlitePool,
    turn: &NewInteractionTurn,
) -> Result<(StoredInteractionTurn, bool), InteractionError> {
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
            && user_message_matches(
                &transaction,
                turn.user_message_id,
                turn.conversation_id,
                &turn.user_message_text,
            )?
            && existing.runs == turn.runs
            && initial_event_id == turn.initial_event_id
        {
            return Ok((existing, false));
        }
        return Err(conflict_error(
            "interaction turn id is already bound to different durable input",
        ));
    }

    insert_or_validate_user_message(&transaction, turn)?;
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
    Ok((stored, true))
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

fn finish_turn_blocking(
    pool: &SqlitePool,
    event: &NewInteractionEvent,
    assistant_message: &NewAssistantMessage,
) -> Result<InteractionEventEnvelope, InteractionError> {
    let mut writer = pool.writer();
    let transaction = writer
        .transaction()
        .map_err(|error| backend_error("begin interaction completion transaction", &error))?;

    if let Some(existing) = load_event_by_id(&transaction, event.event_id)? {
        let stored = load_turn_conn(&transaction, event.turn_id)?;
        if existing.conversation_id == event.conversation_id
            && existing.turn_id == event.turn_id
            && existing.timestamp == event.timestamp
            && existing.event == event.event
            && stored.assistant_message_id == Some(assistant_message.message_id)
            && transcript_message_matches(
                &transaction,
                assistant_message.message_id,
                event.conversation_id,
                MessageRole::Assistant,
                &assistant_message.text,
                assistant_message.created_at,
            )?
        {
            return Ok(existing);
        }
        return Err(conflict_error(
            "terminal interaction event id is already bound to different durable input",
        ));
    }

    insert_or_validate_assistant_message(&transaction, event.conversation_id, assistant_message)?;
    let state = transaction
        .query_row(
            "SELECT state FROM interaction_turns WHERE id = ?1 AND conversation_id = ?2",
            rusqlite::params![event.turn_id.to_string(), event.conversation_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| backend_error("load completing interaction turn", &error))?
        .ok_or_else(|| not_found_error("interaction turn was not found for this interaction"))?;
    if decode_turn_state(&state)?.is_terminal() {
        return Err(conflict_error(
            "interaction turn already has a different terminal event",
        ));
    }

    let changed = transaction
        .execute(
            "UPDATE interaction_turns SET assistant_message_id = ?1
             WHERE id = ?2 AND assistant_message_id IS NULL",
            rusqlite::params![
                assistant_message.message_id.to_string(),
                event.turn_id.to_string()
            ],
        )
        .map_err(|error| write_error("attach interaction assistant message", &error))?;
    if changed != 1 {
        return Err(conflict_error(
            "interaction turn already has a different assistant message",
        ));
    }

    let envelope = insert_event(&transaction, event)?;
    project_turn_state(&transaction, event)?;
    transaction
        .commit()
        .map_err(|error| backend_error("commit interaction completion", &error))?;
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

fn insert_or_validate_user_message(
    transaction: &Transaction<'_>,
    turn: &NewInteractionTurn,
) -> Result<(), InteractionError> {
    if message_exists(transaction, turn.user_message_id)? {
        if transcript_message_matches(
            transaction,
            turn.user_message_id,
            turn.conversation_id,
            MessageRole::User,
            &turn.user_message_text,
            turn.started_at,
        )? {
            return Ok(());
        }
        return Err(conflict_error(
            "interaction user message identity is bound to different content",
        ));
    }
    insert_transcript_message(
        transaction,
        turn.user_message_id,
        turn.conversation_id,
        MessageRole::User,
        &turn.user_message_text,
        turn.started_at,
    )
}

fn insert_or_validate_assistant_message(
    transaction: &Transaction<'_>,
    conversation_id: ConversationId,
    message: &NewAssistantMessage,
) -> Result<(), InteractionError> {
    if message_exists(transaction, message.message_id)? {
        if transcript_message_matches(
            transaction,
            message.message_id,
            conversation_id,
            MessageRole::Assistant,
            &message.text,
            message.created_at,
        )? {
            return Ok(());
        }
        return Err(conflict_error(
            "interaction assistant message identity is bound to different content",
        ));
    }
    insert_transcript_message(
        transaction,
        message.message_id,
        conversation_id,
        MessageRole::Assistant,
        &message.text,
        message.created_at,
    )
}

fn message_exists(connection: &Connection, message_id: Uuid) -> Result<bool, InteractionError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM conversation_messages WHERE id = ?1)",
            [message_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|error| backend_error("check interaction transcript message", &error))
}

fn user_message_matches(
    connection: &Connection,
    message_id: Uuid,
    conversation_id: ConversationId,
    text: &str,
) -> Result<bool, InteractionError> {
    let stored = connection
        .query_row(
            "SELECT conversation_id, role, content_json
             FROM conversation_messages WHERE id = ?1",
            [message_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|error| backend_error("load interaction user message", &error))?;
    let Some((stored_conversation, stored_role, content_json)) = stored else {
        return Ok(false);
    };
    Ok(stored_conversation == conversation_id.to_string()
        && stored_role == encode_message_role(MessageRole::User)
        && decode_json::<MessageContent>("interaction transcript content", &content_json)?
            == MessageContent::Text {
                text: text.to_owned(),
            })
}

fn transcript_message_matches(
    connection: &Connection,
    message_id: Uuid,
    conversation_id: ConversationId,
    role: MessageRole,
    text: &str,
    created_at: DateTime<Utc>,
) -> Result<bool, InteractionError> {
    let stored = connection
        .query_row(
            "SELECT conversation_id, role, content_json, created_at
             FROM conversation_messages WHERE id = ?1",
            [message_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|error| backend_error("load interaction transcript message", &error))?;
    let Some((stored_conversation, stored_role, content_json, stored_created_at)) = stored else {
        return Ok(false);
    };
    let expected_content = MessageContent::Text {
        text: text.to_owned(),
    };
    Ok(stored_conversation == conversation_id.to_string()
        && stored_role == encode_message_role(role)
        && decode_json::<MessageContent>("interaction transcript content", &content_json)?
            == expected_content
        && parse_timestamp("interaction transcript timestamp", &stored_created_at)? == created_at)
}

fn insert_transcript_message(
    transaction: &Transaction<'_>,
    message_id: Uuid,
    conversation_id: ConversationId,
    role: MessageRole,
    text: &str,
    created_at: DateTime<Utc>,
) -> Result<(), InteractionError> {
    let content = encode_json(
        "interaction transcript content",
        &MessageContent::Text {
            text: text.to_owned(),
        },
    )?;
    let changed = transaction
        .execute(
            "INSERT INTO conversation_messages
                 (id, conversation_id, role, content_json, token_count, created_at)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
            rusqlite::params![
                message_id.to_string(),
                conversation_id.to_string(),
                encode_message_role(role),
                content,
                created_at.to_rfc3339(),
            ],
        )
        .map_err(|error| write_error("insert interaction transcript message", &error))?;
    if changed != 1 {
        return Err(invariant_error(
            "interaction transcript insert changed an unexpected number of rows",
        ));
    }
    let updated = transaction
        .execute(
            "UPDATE conversations
             SET message_count = message_count + 1, updated_at = ?1 WHERE id = ?2",
            rusqlite::params![created_at.to_rfc3339(), conversation_id.to_string()],
        )
        .map_err(|error| write_error("update interaction transcript projection", &error))?;
    if updated != 1 {
        return Err(not_found_error("interaction conversation was not found"));
    }
    Ok(())
}

const fn encode_message_role(role: MessageRole) -> &'static str {
    match role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::System => "system",
        MessageRole::Tool => "tool",
    }
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

fn load_transcript_message_conn(
    connection: &Connection,
    message_id: Uuid,
) -> Result<Option<StoredTranscriptMessage>, InteractionError> {
    let row = connection
        .query_row(
            "SELECT conversation_id, role, content_json, token_count
             FROM conversation_messages WHERE id = ?1",
            [message_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|error| backend_error("load correlated transcript message", &error))?;
    let Some((conversation_id, role, content_json, token_count)) = row else {
        return Ok(None);
    };
    let role = match role.as_str() {
        "user" => StoredTranscriptRole::User,
        "assistant" => StoredTranscriptRole::Assistant,
        "system" => StoredTranscriptRole::System,
        "tool" => StoredTranscriptRole::Tool,
        _ => return Err(invariant_error("stored transcript message role is invalid")),
    };
    let content: MessageContent = decode_json("transcript message content", &content_json)?;
    let text = match content {
        MessageContent::Text { text } => Some(text),
        MessageContent::ToolCall { .. }
        | MessageContent::ToolResult { .. }
        | MessageContent::Mixed { .. } => None,
    };
    let token_count = token_count
        .map(|value| {
            u32::try_from(value)
                .map_err(|_| invariant_error("stored transcript token count is invalid"))
        })
        .transpose()?;
    Ok(Some(StoredTranscriptMessage {
        message_id,
        conversation_id: parse_id("conversation", &conversation_id)?,
        role,
        text,
        token_count,
    }))
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

struct RawInteraction {
    title: Option<String>,
    config_json: String,
    state: String,
    turn_count: i64,
    created_at: String,
    updated_at: String,
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
    use polkagent_store_trait::{RunStatus, RunStore};

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
                     VALUES (?1, ?2, 0, '{}', ?3, ?3)",
                    rusqlite::params![conversation_id.to_string(), agent_id.to_string(), now],
                )
                .expect("insert conversation");
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
            user_message_text: "hello".to_owned(),
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
    async fn interaction_session_create_list_update_and_archive_round_trip() {
        let fixture = fixture();
        let target = InteractionTarget::Agent(AgentId::new());
        let created_at = Utc::now();
        let new_interaction = NewInteraction {
            conversation_id: fixture.conversation_id,
            config: InteractionConfig::new(target),
            created_at,
        };
        let created = fixture
            .store
            .create_interaction(new_interaction.clone())
            .await
            .expect("create interaction session");
        assert_eq!(created.state, InteractionState::Active);
        assert_eq!(created.turn_count, 0);
        assert_eq!(
            fixture
                .store
                .create_interaction(new_interaction)
                .await
                .expect("retry interaction creation"),
            created
        );
        assert_eq!(
            fixture
                .store
                .list_interactions(ListInteractionsRequest::default())
                .await
                .expect("list active interactions"),
            vec![created.clone()]
        );

        let mut config = created.config;
        config.model = Some("model-x".to_owned());
        let updated = fixture
            .store
            .update_interaction_config(fixture.conversation_id, config)
            .await
            .expect("update interaction config");
        assert_eq!(updated.config.model.as_deref(), Some("model-x"));
        let archived = fixture
            .store
            .set_interaction_state(fixture.conversation_id, InteractionState::Archived)
            .await
            .expect("archive interaction");
        assert_eq!(archived.state, InteractionState::Archived);
        assert!(fixture
            .store
            .list_interactions(ListInteractionsRequest {
                state: Some(InteractionState::Active),
                ..ListInteractionsRequest::default()
            })
            .await
            .expect("list active interactions")
            .is_empty());
        let error = fixture
            .store
            .update_interaction_config(fixture.conversation_id, archived.config)
            .await
            .expect_err("archived config update must fail");
        assert_eq!(error.code, InteractionErrorCode::Conflict);
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
    async fn deterministic_retry_uses_durable_winner_despite_new_timestamp() {
        let fixture = fixture();
        let first = new_turn(&fixture);
        let (winner, created) = fixture
            .store
            .create_turn_once(first.clone())
            .await
            .expect("create winner");
        assert!(created);

        let mut retry = first;
        retry.started_at += chrono::Duration::seconds(1);
        let (stored, created) = fixture
            .store
            .create_turn_once(retry)
            .await
            .expect("load durable winner");
        assert!(!created);
        assert_eq!(stored, winner);

        let message_count: i64 = {
            let writer = fixture.store.pool().writer();
            writer
                .query_row(
                    "SELECT COUNT(*) FROM conversation_messages WHERE conversation_id = ?1",
                    [fixture.conversation_id.to_string()],
                    |row| row.get(0),
                )
                .expect("count transcript messages")
        };
        assert_eq!(message_count, 1);

        RunStore::delete_prepared(fixture.store.pool(), fixture.run_id)
            .await
            .expect("linked winner makes compensation a safe no-op");
        let run = RunStore::get(fixture.store.pool(), fixture.run_id)
            .await
            .expect("linked winner run remains durable");
        assert_eq!(run.status, RunStatus::new("created"));
    }

    #[tokio::test]
    async fn finish_turn_is_atomic_idempotent_and_conflicts_on_content() {
        let fixture = fixture();
        let turn = new_turn(&fixture);
        fixture
            .store
            .create_turn(turn.clone())
            .await
            .expect("create turn");
        let timestamp = Utc::now();
        let terminal = NewInteractionEvent {
            event_id: InteractionEventId::new(),
            conversation_id: fixture.conversation_id,
            turn_id: turn.turn_id,
            timestamp,
            event: InteractionEvent::TurnCompleted {
                result: TurnResult {
                    text: "answer".to_owned(),
                    run_ids: vec![fixture.run_id],
                    usage: UsageView::default(),
                },
            },
        };
        let assistant = NewAssistantMessage {
            message_id: Uuid::now_v7(),
            text: "answer".to_owned(),
            created_at: timestamp,
        };
        let first = fixture
            .store
            .finish_turn(terminal.clone(), assistant.clone())
            .await
            .expect("finish turn");
        let retry = fixture
            .store
            .finish_turn(terminal.clone(), assistant.clone())
            .await
            .expect("retry terminal write");
        assert_eq!(retry, first);

        let mut conflicting = assistant.clone();
        conflicting.text = "different answer".to_owned();
        let error = fixture
            .store
            .finish_turn(terminal, conflicting)
            .await
            .expect_err("same terminal identities cannot change transcript content");
        assert_eq!(error.code, InteractionErrorCode::Conflict);

        let stored = fixture
            .store
            .load_turn(turn.turn_id)
            .await
            .expect("load completed turn");
        assert_eq!(stored.assistant_message_id, Some(assistant.message_id));
        assert_eq!(stored.summary.state, TurnState::Completed);
        let writer = fixture.store.pool().writer();
        let (messages, terminal_events): (i64, i64) = (
            writer
                .query_row(
                    "SELECT COUNT(*) FROM conversation_messages WHERE conversation_id = ?1",
                    [fixture.conversation_id.to_string()],
                    |row| row.get(0),
                )
                .expect("count transcript messages"),
            writer
                .query_row(
                    "SELECT COUNT(*) FROM interaction_events
                     WHERE turn_id = ?1 AND is_terminal = 1",
                    [turn.turn_id.to_string()],
                    |row| row.get(0),
                )
                .expect("count terminal events"),
        );
        assert_eq!(messages, 2);
        assert_eq!(terminal_events, 1);
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
        let writer = fixture.store.pool().writer();
        let messages: i64 = writer
            .query_row(
                "SELECT COUNT(*) FROM conversation_messages WHERE conversation_id = ?1",
                [fixture.conversation_id.to_string()],
                |row| row.get(0),
            )
            .expect("count messages after rolled-back turn");
        assert_eq!(messages, 0);
    }
}
