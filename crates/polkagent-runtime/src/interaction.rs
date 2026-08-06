//! Durable headless interaction service over one shared runtime.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use polkagent_artifact::ArtifactStore;
use polkagent_conversation::{
    types::{Conversation, MessageContent, MessageRole},
    ConversationStore,
};
use polkagent_core::event::{EventCorrelation, EventKind, RunEvent};
use polkagent_core::{AgentId, ConversationId, EffectId, EventId, RunId, RunState};
use polkagent_executor_trait::{
    ContentBlock, InferenceMessage, MessageRole as InferenceMessageRole,
};
use polkagent_interaction::{
    BoxInteractionEventStream, ConfigOptionValue, ConfigUpdate, CreateInteractionRequest,
    InteractionConfig, InteractionContent, InteractionError, InteractionErrorCode,
    InteractionEvent, InteractionEventHub, InteractionEventId, InteractionOverrides,
    InteractionRunLink, InteractionService, InteractionState, InteractionStore, InteractionSummary,
    InteractionTarget, InteractionTranscriptTurn, InteractionTurnId, ListInteractionsRequest,
    NewAssistantMessage, NewInteraction, NewInteractionEvent, NewInteractionTurn, OverrideValue,
    PromptRequest, RunRole, StartedTurn, StoredTranscriptMessage, StoredTranscriptRole,
    StoredTranscriptTurn, SubscriptionRequest, ToolCallId, ToolCallKind, ToolCallStatus,
    ToolCallView, TranscriptRequest, TurnResult, TurnState, TurnSummary, UsageView,
};
use polkagent_service::{AppService, ServiceError};
use polkagent_store_sqlite::{
    DurableToolCall, DurableToolOutcome, SqliteInteractionStore, SqlitePool, SqliteRunStore,
    StoreError as SqliteStoreError,
};
use polkagent_store_trait::event::{EventStore, StoredEvent};
use polkagent_store_trait::RunStore;
use uuid::Uuid;

const INTERACTION_STREAM_CAPACITY: usize = 256;
/// Maximum number of completed user/assistant pairs sent to a model executor.
const MAX_MODEL_CONTEXT_TURNS: usize = 32;
/// Maximum recent durable turn records inspected when selecting context.
const MAX_MODEL_CONTEXT_SCAN_TURNS: u32 = 1_000;
const USER_MESSAGE_DISCRIMINATOR: u8 = 0x31;
const ASSISTANT_MESSAGE_DISCRIMINATOR: u8 = 0x52;
const INITIAL_EVENT_DISCRIMINATOR: u8 = 0x73;
const TERMINAL_EVENT_DISCRIMINATOR: u8 = 0x94;
const RUN_DISCRIMINATOR: u8 = 0xb5;
const TOOL_STARTED_EVENT_DISCRIMINATOR: u8 = 0xd6;
const TOOL_UPDATED_EVENT_DISCRIMINATOR: u8 = 0xf7;
const PREPARED_RUN_RECOVERY_REASON: &str =
    "interaction run was interrupted before activation and cannot be resumed safely";
const UNRECOVERABLE_OUTPUT_REASON: &str =
    "completed run output is unavailable for durable transcript recovery";

struct NewTurnStart {
    conversation_id: ConversationId,
    turn_id: InteractionTurnId,
    ordinal: u32,
    agent_id: AgentId,
    config: InteractionConfig,
    prompt: String,
    messages: Vec<InferenceMessage>,
}

/// Production [`InteractionService`] backed by the shared `AppService`,
/// `SQLite` interaction/event store, and conversation transcript store.
#[derive(Clone)]
pub struct DurableInteractionService {
    app: Arc<AppService>,
    pool: SqlitePool,
    store: Arc<SqliteInteractionStore>,
    hub: InteractionEventHub,
    prompt_lock: Arc<tokio::sync::Mutex<()>>,
}

impl std::fmt::Debug for DurableInteractionService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DurableInteractionService")
            .field("pool", &self.pool)
            .finish_non_exhaustive()
    }
}

impl DurableInteractionService {
    /// Compose the durable service over the exact stores and event bus used by
    /// the process-wide runtime.
    pub fn new(app: Arc<AppService>, pool: SqlitePool) -> Self {
        let store = Arc::new(SqliteInteractionStore::new(pool.clone()));
        let erased: Arc<dyn InteractionStore> = store.clone();
        Self {
            app,
            pool,
            store,
            hub: InteractionEventHub::new(erased),
            prompt_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Reconcile non-terminal interaction turns with durable terminal run
    /// state after process restart.
    pub async fn recover(&self) -> Result<u32, InteractionError> {
        let discarded = self.store.discard_orphan_prepared_runs().await?;
        if discarded > 0 {
            tracing::warn!(discarded, "discarded orphan prepared interaction runs");
        }
        let mut recovered = 0_u32;
        let mut offset = 0_u32;
        let mut conversation_ids = Vec::new();
        loop {
            let interactions = self
                .store
                .list_interactions(ListInteractionsRequest {
                    limit: 1_000,
                    offset,
                    state: None,
                })
                .await?;
            if interactions.is_empty() {
                break;
            }
            let page_len = u32::try_from(interactions.len())
                .map_err(|_| internal_error("interaction recovery page is too large"))?;
            conversation_ids.extend(
                interactions
                    .into_iter()
                    .map(|interaction| interaction.conversation_id),
            );
            let next = offset
                .checked_add(page_len)
                .ok_or_else(|| internal_error("interaction recovery offset overflowed"))?;
            if next <= offset {
                return Err(internal_error("interaction recovery did not advance"));
            }
            offset = next;
            if page_len < 1_000 {
                break;
            }
        }
        // Mutate only after the paginated recency-ordered scan is complete;
        // terminal projection may change ordering metadata in future schemas.
        for conversation_id in conversation_ids {
            for turn in self.store.list_turns(conversation_id).await? {
                if turn.summary.state.is_terminal() {
                    continue;
                }
                let Some(run) = turn.runs.first() else {
                    return Err(internal_error("stored interaction turn has no linked run"));
                };
                let summary = RunStore::get(&self.pool, run.run_id)
                    .await
                    .map_err(|error| store_error("load linked run during recovery", &error))?;
                if summary.status.0 == "created" {
                    self.app
                        .fail_prepared_run(run.run_id, PREPARED_RUN_RECOVERY_REASON)
                        .await
                        .map_err(|error| {
                            service_error("terminalize interrupted prepared run", &error)
                        })?;
                }
                if self
                    .backfill_run(
                        conversation_id,
                        turn.summary.handle.turn_id,
                        run.run_id,
                        false,
                    )
                    .await?
                {
                    recovered = recovered.saturating_add(1);
                }
            }
        }
        Ok(recovered)
    }

    fn resolve_target(&self, target: &InteractionTarget) -> Result<AgentId, InteractionError> {
        let store = SqliteRunStore::new(self.pool.clone());
        let row = match target {
            InteractionTarget::Agent(agent_id) => store
                .get_agent(&agent_id.to_string())
                .map_err(interaction_target_store_error)?,
            InteractionTarget::Auto => store
                .list_agents(Some("active"), false)
                .map_err(|error| store_error("resolve automatic interaction agent", &error))?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    InteractionError::new(
                        InteractionErrorCode::Unavailable,
                        "no active agent is available for automatic interaction targeting",
                    )
                })?,
            InteractionTarget::Group(_) => {
                return Err(InteractionError::new(
                    InteractionErrorCode::Unsupported,
                    "group interactions are not composed by the shared runtime",
                ));
            }
        };
        if row.state != "active" {
            return Err(InteractionError::new(
                InteractionErrorCode::Conflict,
                "the selected interaction agent is not active",
            ));
        }
        row.id
            .parse()
            .map_err(|_| internal_error("stored interaction agent identity is invalid"))
    }

    fn resolve_model_config(
        &self,
        agent_id: AgentId,
        requested: Option<&str>,
    ) -> Result<Option<String>, InteractionError> {
        self.app
            .resolve_interaction_model(agent_id, requested)
            .map_err(model_selection_error)
    }

    async fn finish_turn(
        &self,
        turn_id: InteractionTurnId,
        conversation_id: ConversationId,
        timestamp: DateTime<Utc>,
        mut event: InteractionEvent,
        text: String,
    ) -> Result<(), InteractionError> {
        if let InteractionEvent::TurnCompleted { result } = &mut event {
            result.text.clone_from(&text);
        }
        let assistant_message_id = derived_uuid(turn_id, ASSISTANT_MESSAGE_DISCRIMINATOR);
        self.hub
            .publish_terminal(
                NewInteractionEvent {
                    event_id: InteractionEventId::from_uuid(derived_uuid(
                        turn_id,
                        TERMINAL_EVENT_DISCRIMINATOR,
                    )),
                    conversation_id,
                    turn_id,
                    timestamp,
                    event,
                },
                NewAssistantMessage {
                    message_id: assistant_message_id,
                    text,
                    created_at: timestamp,
                },
            )
            .await?;
        Ok(())
    }

    async fn completed_text(
        &self,
        conversation_id: ConversationId,
        turn_id: InteractionTurnId,
        output_artifact_id: Option<polkagent_core::ArtifactId>,
        output_complete: bool,
    ) -> Result<String, InteractionError> {
        if let Some(artifact_id) = output_artifact_id {
            let body = ArtifactStore::get_body(&self.pool, artifact_id)
                .await
                .map_err(|error| store_error("load completed run output artifact", &error))?;
            return String::from_utf8(body).map_err(|error| {
                tracing::error!(%artifact_id, %error, "run output artifact is not UTF-8 text");
                InteractionError::new(
                    InteractionErrorCode::Unavailable,
                    "completed run output artifact is not a text transcript",
                )
            });
        }
        if output_complete {
            return self.assistant_text(conversation_id, turn_id).await;
        }
        Err(InteractionError::new(
            InteractionErrorCode::Unavailable,
            UNRECOVERABLE_OUTPUT_REASON,
        ))
    }

    async fn assistant_text(
        &self,
        conversation_id: ConversationId,
        turn_id: InteractionTurnId,
    ) -> Result<String, InteractionError> {
        let mut after_sequence = 0_u64;
        let mut text = String::new();
        loop {
            let events = self
                .store
                .load_events(conversation_id, after_sequence, 1_000)
                .await?;
            if events.is_empty() {
                break;
            }
            let page_len = events.len();
            let next = events
                .last()
                .map(|event| event.sequence)
                .ok_or_else(|| internal_error("interaction event page disappeared"))?;
            if next <= after_sequence {
                return Err(internal_error("interaction event replay did not advance"));
            }
            for envelope in events {
                if envelope.turn_id == turn_id {
                    if let InteractionEvent::AgentMessageDelta { text: delta, .. } = envelope.event
                    {
                        text.push_str(&delta);
                    }
                }
            }
            after_sequence = next;
            if page_len < 1_000 {
                break;
            }
        }
        Ok(text)
    }

    async fn user_message_text(
        &self,
        conversation_id: ConversationId,
        message_id: Uuid,
    ) -> Result<String, InteractionError> {
        let conversation = ConversationStore::get(&self.pool, conversation_id)
            .await
            .map_err(|error| conversation_error("load interaction transcript", &error))?;
        let limit = usize::try_from(conversation.message_count)
            .unwrap_or(usize::MAX)
            .saturating_add(1);
        let message = ConversationStore::get_messages(&self.pool, conversation_id, limit, 0)
            .await
            .map_err(|error| conversation_error("load interaction messages", &error))?
            .into_iter()
            .find(|message| message.id == message_id)
            .ok_or_else(|| internal_error("interaction user message is missing"))?;
        match (message.role, message.content) {
            (MessageRole::User, MessageContent::Text { text }) => Ok(text),
            _ => Err(internal_error(
                "interaction user message has incompatible role or content",
            )),
        }
    }

    async fn transcript_page(
        &self,
        request: TranscriptRequest,
    ) -> Result<Vec<InteractionTranscriptTurn>, InteractionError> {
        request.validate()?;
        let stored = self.store.load_transcript_turns(request).await?;
        let mut message_ids = HashSet::with_capacity(stored.len().saturating_mul(2));
        for item in &stored {
            if !message_ids.insert(item.turn.user_message_id) {
                return Err(internal_error(
                    "interaction transcript user message is linked more than once",
                ));
            }
            if item
                .turn
                .assistant_message_id
                .is_some_and(|message_id| !message_ids.insert(message_id))
            {
                return Err(internal_error(
                    "interaction transcript assistant message is linked more than once",
                ));
            }
        }
        stored
            .into_iter()
            .map(|stored| project_transcript_turn(stored, request.conversation_id))
            .collect()
    }

    /// Assemble bounded model context without rendering role-tagged strings.
    ///
    /// Only completed durable turns are eligible. Failed, cancelled, and
    /// timed-out turns are omitted as whole user/assistant pairs, including
    /// any partial assistant output. From the latest 1,000 prior records, the
    /// newest 32 completed pairs are retained in chronological order. The
    /// current user message is then appended exactly once.
    async fn model_context_messages(
        &self,
        conversation_id: ConversationId,
        prior_turn_count: usize,
        current_prompt: &str,
    ) -> Result<Vec<InferenceMessage>, InteractionError> {
        let prior_turn_count = u32::try_from(prior_turn_count)
            .map_err(|_| internal_error("interaction turn count exceeds supported range"))?;
        let scan_count = prior_turn_count.min(MAX_MODEL_CONTEXT_SCAN_TURNS);
        let transcript = if scan_count == 0 {
            Vec::new()
        } else {
            self.transcript_page(TranscriptRequest {
                conversation_id,
                limit: scan_count,
                offset: prior_turn_count.saturating_sub(scan_count),
            })
            .await?
        };
        let completed = transcript
            .into_iter()
            .filter(|turn| turn.turn.state == TurnState::Completed)
            .collect::<Vec<_>>();
        let retained_from = completed.len().saturating_sub(MAX_MODEL_CONTEXT_TURNS);
        let mut messages = Vec::with_capacity(
            completed
                .len()
                .saturating_sub(retained_from)
                .saturating_mul(2)
                .saturating_add(1),
        );
        for turn in completed.into_iter().skip(retained_from) {
            let assistant_text = turn.assistant_text.ok_or_else(|| {
                internal_error("completed interaction context is missing assistant output")
            })?;
            messages.push(inference_text_message(
                InferenceMessageRole::User,
                turn.user_text,
            ));
            messages.push(inference_text_message(
                InferenceMessageRole::Assistant,
                assistant_text,
            ));
        }
        messages.push(inference_text_message(
            InferenceMessageRole::User,
            current_prompt.to_owned(),
        ));
        Ok(messages)
    }

    async fn retry_existing_turn(
        &self,
        existing: &polkagent_interaction::StoredInteractionTurn,
        config: &InteractionConfig,
        prompt: &str,
    ) -> Result<StartedTurn, InteractionError> {
        let conversation_id = existing.summary.handle.conversation_id;
        let turn_id = existing.summary.handle.turn_id;
        let existing_text = self
            .user_message_text(conversation_id, existing.user_message_id)
            .await?;
        if existing.config != *config || existing_text != prompt {
            return Err(InteractionError::new(
                InteractionErrorCode::Conflict,
                "interaction turn id is already bound to different prompt or configuration",
            ));
        }
        let events = self
            .hub
            .subscribe(SubscriptionRequest {
                conversation_id,
                turn_id: Some(turn_id),
                after_sequence: None,
                capacity: INTERACTION_STREAM_CAPACITY,
            })
            .await?;
        Ok(StartedTurn {
            handle: existing.summary.handle.clone(),
            events,
        })
    }

    async fn start_new_turn(&self, input: NewTurnStart) -> Result<StartedTurn, InteractionError> {
        let run_id = RunId::from(derived_uuid(input.turn_id, RUN_DISCRIMINATOR));
        let user_message_id = derived_uuid(input.turn_id, USER_MESSAGE_DISCRIMINATOR);
        let started_at = Utc::now();
        let run_events = self.app.subscribe_events();
        let prepared = match self
            .app
            .prepare_interaction_run_with_model(
                run_id,
                input.agent_id,
                input.conversation_id,
                input.config.model.as_deref(),
            )
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                if let Ok(existing) = self.store.load_turn(input.turn_id).await {
                    return self
                        .retry_existing_turn(&existing, &input.config, &input.prompt)
                        .await;
                }
                return Err(service_error("prepare interaction run", &error));
            }
        };
        let (stored, created) = match self
            .store
            .create_turn_once(NewInteractionTurn {
                turn_id: input.turn_id,
                conversation_id: input.conversation_id,
                ordinal: input.ordinal,
                target: input.config.target.clone(),
                config: input.config,
                user_message_id,
                user_message_text: input.prompt.clone(),
                runs: vec![InteractionRunLink {
                    run_id,
                    role: RunRole::Primary,
                    ordinal: 1,
                }],
                initial_event_id: InteractionEventId::from_uuid(derived_uuid(
                    input.turn_id,
                    INITIAL_EVENT_DISCRIMINATOR,
                )),
                started_at,
            })
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                if let Err(compensation) = self.app.discard_prepared_run(run_id).await {
                    tracing::error!(%run_id, %compensation, "prepared run compensation failed");
                }
                return Err(error);
            }
        };
        let interaction_events = match self
            .hub
            .subscribe(SubscriptionRequest {
                conversation_id: input.conversation_id,
                turn_id: Some(input.turn_id),
                after_sequence: None,
                capacity: INTERACTION_STREAM_CAPACITY,
            })
            .await
        {
            Ok(events) => events,
            Err(error) => {
                if created {
                    if let Err(compensation) = self
                        .app
                        .fail_prepared_run(run_id, "interaction event subscription failed")
                        .await
                    {
                        tracing::error!(%run_id, %compensation, "prepared run failure compensation failed");
                        self.finish_turn(
                            input.turn_id,
                            input.conversation_id,
                            Utc::now(),
                            InteractionEvent::TurnFailed {
                                error: error.clone(),
                            },
                            String::new(),
                        )
                        .await?;
                    } else {
                        self.backfill_run(input.conversation_id, input.turn_id, run_id, false)
                            .await?;
                    }
                }
                return Err(error);
            }
        };
        if !created {
            return Ok(StartedTurn {
                handle: stored.summary.handle,
                events: interaction_events,
            });
        }

        let projector = tokio::spawn(self.clone().project_run(
            run_events,
            input.conversation_id,
            input.turn_id,
            run_id,
        ));
        if let Err(error) = self
            .app
            .execute_prepared_run_with_messages(prepared, input.messages)
            .await
        {
            match self
                .app
                .terminalize_run_failure(run_id, "interaction run activation failed")
                .await
            {
                Ok(()) => {
                    self.backfill_run(input.conversation_id, input.turn_id, run_id, false)
                        .await?;
                }
                Err(terminalization_error) => {
                    tracing::error!(%run_id, %terminalization_error, "run activation failure compensation failed");
                    let failure = InteractionEvent::TurnFailed {
                        error: service_error("start interaction run", &error),
                    };
                    let text = self
                        .assistant_text(input.conversation_id, input.turn_id)
                        .await?;
                    self.finish_turn(
                        input.turn_id,
                        input.conversation_id,
                        Utc::now(),
                        failure,
                        text,
                    )
                    .await?;
                }
            }
            projector.abort();
            return Err(service_error("start interaction run", &error));
        }
        Ok(StartedTurn {
            handle: stored.summary.handle,
            events: interaction_events,
        })
    }

    async fn project_run(
        self,
        mut events: polkagent_event::EventReceiver,
        conversation_id: ConversationId,
        turn_id: InteractionTurnId,
        run_id: RunId,
    ) {
        let mut output_complete = true;
        loop {
            match events.recv().await {
                Ok(event) if event.run_id == run_id => {
                    let terminal = event.kind.is_terminal_for_interaction();
                    if let Err(error) = self
                        .project_run_event(conversation_id, turn_id, event, output_complete)
                        .await
                    {
                        tracing::error!(%turn_id, %run_id, %error, "interaction projection failed");
                        return;
                    }
                    if terminal {
                        return;
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(%turn_id, %run_id, skipped, "run event projection lagged");
                    output_complete = false;
                    // Attach the replacement before durable replay so a
                    // lifecycle event committed during backfill is queued.
                    let replacement = self.app.subscribe_events();
                    match self
                        .backfill_run(conversation_id, turn_id, run_id, output_complete)
                        .await
                    {
                        Ok(true) => return,
                        Ok(false) => events = replacement,
                        Err(error) => {
                            tracing::error!(%turn_id, %run_id, %error, "interaction lag backfill failed");
                            return;
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    if let Err(error) = self
                        .backfill_run(conversation_id, turn_id, run_id, false)
                        .await
                    {
                        tracing::error!(%turn_id, %run_id, %error, "interaction close backfill failed");
                    }
                    return;
                }
            }
        }
    }

    async fn project_run_event(
        &self,
        conversation_id: ConversationId,
        turn_id: InteractionTurnId,
        event: RunEvent,
        output_complete: bool,
    ) -> Result<(), InteractionError> {
        if matches!(
            &event.kind,
            EventKind::ToolCallStarted { .. } | EventKind::ToolCallCompleted { .. }
        ) {
            return self
                .project_tool_run_event(conversation_id, turn_id, &event)
                .await;
        }
        let event_id = InteractionEventId::from_uuid(event.id.as_uuid());
        let projected = match event.kind {
            EventKind::StreamingToken { text } => Some(InteractionEvent::AgentMessageDelta {
                run_id: event.run_id,
                text,
            }),
            EventKind::RunCreated => Some(InteractionEvent::RunStateChanged {
                run_id: event.run_id,
                state: RunState::Created,
            }),
            EventKind::RunQueued => Some(InteractionEvent::RunStateChanged {
                run_id: event.run_id,
                state: RunState::Queued,
            }),
            EventKind::RunStarted => Some(InteractionEvent::RunStateChanged {
                run_id: event.run_id,
                state: RunState::Running,
            }),
            EventKind::RunCompleting => Some(InteractionEvent::RunStateChanged {
                run_id: event.run_id,
                state: RunState::Completing,
            }),
            EventKind::RunCompleted {
                output_artifact_id,
                input_tokens,
                output_tokens,
            } => {
                match self
                    .completed_text(
                        conversation_id,
                        turn_id,
                        output_artifact_id,
                        output_complete,
                    )
                    .await
                {
                    Ok(text) => {
                        self.finish_turn(
                            turn_id,
                            conversation_id,
                            event.timestamp,
                            InteractionEvent::TurnCompleted {
                                result: TurnResult {
                                    text: text.clone(),
                                    run_ids: vec![event.run_id],
                                    usage: UsageView {
                                        input_tokens,
                                        output_tokens,
                                        ..UsageView::default()
                                    },
                                },
                            },
                            text,
                        )
                        .await?;
                    }
                    Err(error) => {
                        let partial = self.assistant_text(conversation_id, turn_id).await?;
                        self.finish_turn(
                            turn_id,
                            conversation_id,
                            event.timestamp,
                            InteractionEvent::TurnFailed { error },
                            partial,
                        )
                        .await?;
                    }
                }
                None
            }
            EventKind::RunFailed { reason } => {
                let text = self.assistant_text(conversation_id, turn_id).await?;
                self.finish_turn(
                    turn_id,
                    conversation_id,
                    event.timestamp,
                    InteractionEvent::TurnFailed {
                        error: InteractionError::new(InteractionErrorCode::Unavailable, reason),
                    },
                    text,
                )
                .await?;
                None
            }
            EventKind::RunCancelled { reason } => {
                let text = self.assistant_text(conversation_id, turn_id).await?;
                self.finish_turn(
                    turn_id,
                    conversation_id,
                    event.timestamp,
                    InteractionEvent::TurnCancelled {
                        reason: Some(reason),
                    },
                    text,
                )
                .await?;
                None
            }
            EventKind::RunTimedOut => {
                let text = self.assistant_text(conversation_id, turn_id).await?;
                self.finish_turn(
                    turn_id,
                    conversation_id,
                    event.timestamp,
                    InteractionEvent::TurnTimedOut,
                    text,
                )
                .await?;
                None
            }
            _ => None,
        };
        if let Some(projected) = projected {
            self.hub
                .publish(NewInteractionEvent {
                    event_id,
                    conversation_id,
                    turn_id,
                    timestamp: event.timestamp,
                    event: projected,
                })
                .await?;
        }
        Ok(())
    }

    async fn project_tool_run_event(
        &self,
        conversation_id: ConversationId,
        turn_id: InteractionTurnId,
        event: &RunEvent,
    ) -> Result<(), InteractionError> {
        let intent_id = event.correlation.effect_intent_id.ok_or_else(|| {
            internal_error("tool run event has no durable effect intent correlation")
        })?;
        let attempt_id = event.correlation.effect_attempt_id.ok_or_else(|| {
            internal_error("tool run event has no durable effect attempt correlation")
        })?;
        let calls = self
            .pool
            .durable_tool_calls(event.run_id)
            .await
            .map_err(|error| store_error("load durable tool projection", &error))?;
        let call = calls
            .iter()
            .find(|call| call.intent_id == intent_id)
            .ok_or_else(|| internal_error("tool run event intent is not durable"))?;
        if !call
            .attempts
            .iter()
            .any(|attempt| attempt.attempt_id == attempt_id)
        {
            return Err(internal_error(
                "tool run event attempt is not durable for its intent",
            ));
        }
        match &event.kind {
            EventKind::ToolCallStarted { .. } => {
                self.publish_tool_started(conversation_id, turn_id, call)
                    .await?;
            }
            EventKind::ToolCallCompleted { .. } => {
                let outcome = call.outcome.as_ref().ok_or_else(|| {
                    internal_error("tool completion event has no durable effect outcome")
                })?;
                if outcome.attempt_id != attempt_id {
                    return Err(internal_error(
                        "tool completion outcome does not match its correlated attempt",
                    ));
                }
                // A diagnostic start notification may have been dropped. The
                // durable read model makes this idempotent and preserves order.
                self.publish_tool_started(conversation_id, turn_id, call)
                    .await?;
                self.publish_tool_updated(conversation_id, turn_id, call)
                    .await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn publish_tool_started(
        &self,
        conversation_id: ConversationId,
        turn_id: InteractionTurnId,
        call: &DurableToolCall,
    ) -> Result<(), InteractionError> {
        let first_attempt = call
            .attempts
            .first()
            .ok_or_else(|| internal_error("durable tool intent has no execution attempt"))?;
        self.hub
            .publish(NewInteractionEvent {
                event_id: tool_event_id(call.intent_id, TOOL_STARTED_EVENT_DISCRIMINATOR),
                conversation_id,
                turn_id,
                timestamp: first_attempt.started_at,
                event: InteractionEvent::ToolCallStarted {
                    call: tool_view(call, ToolCallStatus::InProgress),
                },
            })
            .await?;
        Ok(())
    }

    async fn publish_tool_updated(
        &self,
        conversation_id: ConversationId,
        turn_id: InteractionTurnId,
        call: &DurableToolCall,
    ) -> Result<(), InteractionError> {
        let outcome = call
            .outcome
            .as_ref()
            .ok_or_else(|| internal_error("durable tool intent has no outcome"))?;
        let status = match outcome.result {
            DurableToolOutcome::Success => ToolCallStatus::Succeeded,
            DurableToolOutcome::Failure { .. } | DurableToolOutcome::Timeout => {
                ToolCallStatus::Failed
            }
            DurableToolOutcome::Cancelled => ToolCallStatus::Cancelled,
            DurableToolOutcome::Unknown => ToolCallStatus::Unknown,
        };
        self.hub
            .publish(NewInteractionEvent {
                event_id: tool_event_id(call.intent_id, TOOL_UPDATED_EVENT_DISCRIMINATOR),
                conversation_id,
                turn_id,
                timestamp: outcome.observed_at,
                event: InteractionEvent::ToolCallUpdated {
                    call: tool_view(call, status),
                },
            })
            .await?;
        Ok(())
    }

    async fn backfill_tool_calls(
        &self,
        conversation_id: ConversationId,
        turn_id: InteractionTurnId,
        run_id: RunId,
    ) -> Result<(), InteractionError> {
        let calls = self
            .pool
            .durable_tool_calls(run_id)
            .await
            .map_err(|error| store_error("backfill durable tool projections", &error))?;
        for call in calls.iter().filter(|call| !call.attempts.is_empty()) {
            self.publish_tool_started(conversation_id, turn_id, call)
                .await?;
            if call.outcome.is_some() {
                self.publish_tool_updated(conversation_id, turn_id, call)
                    .await?;
            }
        }
        Ok(())
    }

    /// Rebuild durable lifecycle projections for one linked run. Returns
    /// `true` when a terminal event was found and projected.
    async fn backfill_run(
        &self,
        conversation_id: ConversationId,
        turn_id: InteractionTurnId,
        run_id: RunId,
        output_complete: bool,
    ) -> Result<bool, InteractionError> {
        // Tool diagnostics are not part of lifecycle EventStore replay. Rebuild
        // them from the authoritative effect tables before a run terminal can
        // close the interaction turn.
        self.backfill_tool_calls(conversation_id, turn_id, run_id)
            .await?;
        let stored = EventStore::read_run_events(&self.pool, run_id)
            .await
            .map_err(|error| store_error("replay linked run events", &error))?;
        for event in stored {
            let event = decode_stored_run_event(event)?;
            let terminal = event.kind.is_terminal_for_interaction();
            self.project_run_event(conversation_id, turn_id, event, output_complete)
                .await?;
            if terminal {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[async_trait]
impl InteractionService for DurableInteractionService {
    async fn new_interaction(
        &self,
        request: CreateInteractionRequest,
    ) -> Result<InteractionSummary, InteractionError> {
        request.config.validate()?;
        ensure_supported_config(&request.config)?;
        request.client_context.validate()?;
        let origin_working_directory = request.client_context.working_directory.clone();
        let mut config = request.config;
        let agent_id = self.resolve_target(&config.target)?;
        config.target = InteractionTarget::Agent(agent_id);
        config.model = self.resolve_model_config(agent_id, config.model.as_deref())?;
        let conversation_id = ConversationId::new();
        let mut conversation = Conversation::new(conversation_id, agent_id);
        conversation.title = request.title;
        let created_at = conversation.created_at;
        ConversationStore::create(&self.pool, conversation)
            .await
            .map_err(|error| conversation_error("create interaction transcript", &error))?;
        match self
            .store
            .create_interaction(NewInteraction {
                conversation_id,
                config,
                origin_working_directory,
                created_at,
            })
            .await
        {
            Ok(summary) => Ok(summary),
            Err(error) => {
                if let Err(compensation) =
                    ConversationStore::delete(&self.pool, conversation_id).await
                {
                    tracing::error!(%conversation_id, %compensation, "interaction create compensation failed");
                }
                Err(error)
            }
        }
    }

    async fn list_interactions(
        &self,
        request: ListInteractionsRequest,
    ) -> Result<Vec<InteractionSummary>, InteractionError> {
        self.store.list_interactions(request).await
    }

    async fn load_interaction(
        &self,
        conversation_id: ConversationId,
    ) -> Result<InteractionSummary, InteractionError> {
        self.store.load_interaction(conversation_id).await
    }

    async fn verify_interaction_origin(
        &self,
        conversation_id: ConversationId,
        working_directory: &std::path::Path,
    ) -> Result<(), InteractionError> {
        self.store
            .verify_interaction_origin(conversation_id, working_directory)
            .await
    }

    async fn list_turns(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<TurnSummary>, InteractionError> {
        Ok(self
            .store
            .list_turns(conversation_id)
            .await?
            .into_iter()
            .map(|turn| turn.summary)
            .collect())
    }

    async fn load_transcript(
        &self,
        request: TranscriptRequest,
    ) -> Result<Vec<InteractionTranscriptTurn>, InteractionError> {
        self.transcript_page(request).await
    }

    async fn delete_interaction(
        &self,
        conversation_id: ConversationId,
    ) -> Result<(), InteractionError> {
        if self
            .store
            .list_turns(conversation_id)
            .await?
            .iter()
            .any(|turn| !turn.summary.state.is_terminal())
        {
            return Err(InteractionError::new(
                InteractionErrorCode::Busy,
                "an interaction with active work cannot be archived",
            ));
        }
        self.store
            .set_interaction_state(conversation_id, InteractionState::Archived)
            .await?;
        Ok(())
    }

    async fn prompt(&self, request: PromptRequest) -> Result<StartedTurn, InteractionError> {
        let _prompt_guard = self.prompt_lock.lock().await;
        request.validate()?;
        ensure_supported_overrides(&request.config_overrides)?;
        request.client_context.validate()?;
        self.store
            .verify_interaction_origin(
                request.conversation_id,
                &request.client_context.working_directory,
            )
            .await?;
        let prompt = prompt_text(&request.content)?;
        let interaction = self.store.load_interaction(request.conversation_id).await?;
        if interaction.state != InteractionState::Active {
            return Err(InteractionError::new(
                InteractionErrorCode::Conflict,
                "archived interactions cannot accept prompts",
            ));
        }
        // Model precedence is prompt override > persisted interaction model >
        // registered agent default. `apply_to` resolves the first two; a
        // resulting `None` remains inheritance and is applied only to the
        // cloned prepared-run spec.
        let mut config = request.config_overrides.apply_to(&interaction.config)?;
        let agent_id = self.resolve_target(&config.target)?;
        config.target = InteractionTarget::Agent(agent_id);
        config.model = self.resolve_model_config(agent_id, config.model.as_deref())?;

        let turns = self.store.list_turns(request.conversation_id).await?;
        let turn_id = request.turn_id.unwrap_or_default();
        if let Some(existing) = turns
            .iter()
            .find(|turn| turn.summary.handle.turn_id == turn_id)
        {
            return self.retry_existing_turn(existing, &config, &prompt).await;
        }
        if turns.iter().any(|turn| !turn.summary.state.is_terminal()) {
            return Err(InteractionError::new(
                InteractionErrorCode::Busy,
                "the interaction already has an active turn",
            ));
        }
        let ordinal = u32::try_from(turns.len())
            .map_err(|_| internal_error("interaction turn count exceeds supported range"))?
            .saturating_add(1);
        let messages = self
            .model_context_messages(request.conversation_id, turns.len(), &prompt)
            .await?;
        self.start_new_turn(NewTurnStart {
            conversation_id: request.conversation_id,
            turn_id,
            ordinal,
            agent_id,
            config,
            prompt,
            messages,
        })
        .await
    }

    async fn cancel_turn(&self, turn_id: InteractionTurnId) -> Result<(), InteractionError> {
        let turn = self.store.load_turn(turn_id).await?;
        if turn.summary.state.is_terminal() {
            return Ok(());
        }
        for run in turn.runs {
            let summary = RunStore::get(&self.pool, run.run_id)
                .await
                .map_err(|error| store_error("load run for interaction cancellation", &error))?;
            if !run_status_is_terminal(summary.status.as_str()) {
                self.app
                    .cancel_run(run.run_id)
                    .await
                    .map_err(|error| service_error("cancel interaction run", &error))?;
            }
            self.backfill_run(
                turn.summary.handle.conversation_id,
                turn_id,
                run.run_id,
                false,
            )
            .await?;
        }
        Ok(())
    }

    async fn set_config_option(
        &self,
        conversation_id: ConversationId,
        update: ConfigUpdate,
    ) -> Result<InteractionConfig, InteractionError> {
        update.validate()?;
        let mut config = self.store.load_interaction(conversation_id).await?.config;
        match update.value {
            ConfigOptionValue::Target(target) => config.target = target,
            ConfigOptionValue::Model(model) => config.model = model,
            ConfigOptionValue::Provider(_)
            | ConfigOptionValue::Harness(_)
            | ConfigOptionValue::Autonomy(_)
            | ConfigOptionValue::MaxTurns(_)
            | ConfigOptionValue::Budget(_) => return Err(unsupported_config_error()),
        }
        let agent_id = self.resolve_target(&config.target)?;
        config.target = InteractionTarget::Agent(agent_id);
        config.model = self.resolve_model_config(agent_id, config.model.as_deref())?;
        config.validate()?;
        Ok(self
            .store
            .update_interaction_config(conversation_id, config)
            .await?
            .config)
    }

    async fn approve(
        &self,
        _conversation_id: ConversationId,
        _approval_id: polkagent_core::ApprovalId,
    ) -> Result<(), InteractionError> {
        Err(InteractionError::new(
            InteractionErrorCode::Unavailable,
            "interaction approval identity is not yet wired to a durable effect request",
        ))
    }

    async fn deny(
        &self,
        _conversation_id: ConversationId,
        _approval_id: polkagent_core::ApprovalId,
        _reason: Option<String>,
    ) -> Result<(), InteractionError> {
        Err(InteractionError::new(
            InteractionErrorCode::Unavailable,
            "interaction approval identity is not yet wired to a durable effect request",
        ))
    }

    async fn subscribe(
        &self,
        request: SubscriptionRequest,
    ) -> Result<BoxInteractionEventStream, InteractionError> {
        self.store.load_interaction(request.conversation_id).await?;
        self.hub.subscribe(request).await
    }
}

fn project_transcript_turn(
    stored: StoredTranscriptTurn,
    conversation_id: ConversationId,
) -> Result<InteractionTranscriptTurn, InteractionError> {
    let turn = stored.turn;
    let user = stored.user_message.ok_or_else(|| {
        internal_error("interaction transcript is missing its correlated user message")
    })?;
    if user.message_id != turn.user_message_id {
        return Err(internal_error(
            "interaction transcript returned the wrong user message identity",
        ));
    }
    let (user_text, user_token_count) =
        correlated_plain_text(user, conversation_id, StoredTranscriptRole::User, "user")?;
    let (assistant_text, assistant_token_count) =
        match (turn.summary.state.is_terminal(), turn.assistant_message_id) {
            (true, Some(message_id)) => {
                let assistant = stored.assistant_message.ok_or_else(|| {
                    internal_error("terminal interaction turn is missing its assistant transcript")
                })?;
                if assistant.message_id != message_id {
                    return Err(internal_error(
                        "interaction transcript returned the wrong assistant message identity",
                    ));
                }
                let (text, tokens) = correlated_plain_text(
                    assistant,
                    conversation_id,
                    StoredTranscriptRole::Assistant,
                    "assistant",
                )?;
                (Some(text), tokens)
            }
            (true, None) => {
                return Err(internal_error(
                    "terminal interaction turn is missing its assistant transcript",
                ));
            }
            (false, Some(_) | None) if stored.assistant_message.is_some() => {
                return Err(internal_error(
                    "active interaction turn already has an assistant transcript",
                ));
            }
            (false, Some(_)) => {
                return Err(internal_error(
                    "active interaction turn has a durable assistant message identity",
                ));
            }
            (false, None) => (None, None),
        };
    Ok(InteractionTranscriptTurn {
        turn: turn.summary,
        user_text,
        user_token_count,
        assistant_text,
        assistant_token_count,
    })
}

fn correlated_plain_text(
    message: StoredTranscriptMessage,
    conversation_id: ConversationId,
    expected_role: StoredTranscriptRole,
    label: &str,
) -> Result<(String, Option<u32>), InteractionError> {
    if message.conversation_id != conversation_id {
        return Err(internal_error(
            "interaction transcript message belongs to another conversation",
        ));
    }
    if message.role != expected_role {
        return Err(internal_error(&format!(
            "interaction transcript {label} message has a mismatched role"
        )));
    }
    let text = message.text.ok_or_else(|| {
        internal_error(&format!(
            "interaction transcript {label} message contains unsupported rich content"
        ))
    })?;
    Ok((text, message.token_count))
}

fn inference_text_message(role: InferenceMessageRole, text: String) -> InferenceMessage {
    InferenceMessage {
        role,
        content: vec![ContentBlock::Text { text }],
    }
}

fn prompt_text(content: &[InteractionContent]) -> Result<String, InteractionError> {
    let mut text = Vec::with_capacity(content.len());
    for block in content {
        match block {
            InteractionContent::Text { text: block } => text.push(block.as_str()),
            InteractionContent::ResourceLink { .. } | InteractionContent::Artifact { .. } => {
                return Err(InteractionError::new(
                    InteractionErrorCode::Unsupported,
                    "resource and artifact prompt blocks are not composed by the runtime",
                ));
            }
        }
    }
    Ok(text.join("\n"))
}

fn ensure_supported_config(config: &InteractionConfig) -> Result<(), InteractionError> {
    if config.provider.is_some()
        || config.harness.is_some()
        || config.autonomy != polkagent_core::AutonomyLevel::default()
        || config.max_turns.is_some()
        || config.budget.is_some()
    {
        return Err(unsupported_config_error());
    }
    Ok(())
}

fn ensure_supported_overrides(overrides: &InteractionOverrides) -> Result<(), InteractionError> {
    if !matches!(overrides.provider, OverrideValue::Inherit)
        || !matches!(overrides.harness, OverrideValue::Inherit)
        || overrides.autonomy.is_some()
        || !matches!(overrides.max_turns, OverrideValue::Inherit)
        || !matches!(overrides.budget, OverrideValue::Inherit)
    {
        return Err(unsupported_config_error());
    }
    Ok(())
}

fn unsupported_config_error() -> InteractionError {
    InteractionError::new(
        InteractionErrorCode::Unsupported,
        "the shared runtime currently composes only interaction target and model configuration",
    )
}

fn run_status_is_terminal(status: &str) -> bool {
    status == "completed"
        || status == "failed"
        || status.starts_with("failed:")
        || status == "cancelled"
        || status.starts_with("cancelled:")
        || status == "timed_out"
}

fn derived_uuid(turn_id: InteractionTurnId, discriminator: u8) -> Uuid {
    let mut bytes = *turn_id.as_uuid().as_bytes();
    bytes[0] ^= discriminator;
    bytes[15] ^= discriminator.rotate_left(1);
    Uuid::from_bytes(bytes)
}

fn tool_event_id(intent_id: EffectId, discriminator: u8) -> InteractionEventId {
    let mut bytes = *intent_id.as_uuid().as_bytes();
    bytes[0] ^= discriminator;
    bytes[15] ^= discriminator.rotate_left(1);
    InteractionEventId::from_uuid(Uuid::from_bytes(bytes))
}

fn tool_view(call: &DurableToolCall, status: ToolCallStatus) -> ToolCallView {
    let (summary, error) = match (
        &status,
        call.outcome.as_ref().map(|outcome| &outcome.result),
    ) {
        (ToolCallStatus::InProgress, _) => (
            Some("Arguments withheld by interaction safety policy".to_owned()),
            None,
        ),
        (ToolCallStatus::Succeeded, _) => (
            Some("Completed; output withheld by interaction safety policy".to_owned()),
            None,
        ),
        (ToolCallStatus::Failed, Some(DurableToolOutcome::Failure { error_class })) => (
            Some("Failed; output withheld by interaction safety policy".to_owned()),
            Some(format!("Tool execution failed ({error_class})")),
        ),
        (ToolCallStatus::Failed, Some(DurableToolOutcome::Timeout)) => (
            Some("Timed out; output withheld by interaction safety policy".to_owned()),
            Some("Tool execution timed out".to_owned()),
        ),
        (ToolCallStatus::Cancelled, _) => (
            Some("Cancelled; output withheld by interaction safety policy".to_owned()),
            Some("Tool execution was cancelled".to_owned()),
        ),
        (ToolCallStatus::Unknown, _) => (
            Some("Outcome unknown; output withheld by interaction safety policy".to_owned()),
            Some("Tool execution outcome is unknown".to_owned()),
        ),
        _ => (None, None),
    };
    ToolCallView {
        call_id: ToolCallId::from_uuid(call.intent_id.as_uuid()),
        run_id: call.run_id,
        name: call.tool_name.clone(),
        title: call.tool_name.clone(),
        kind: ToolCallKind::Other,
        status,
        arguments: None,
        summary,
        output: None,
        locations: Vec::new(),
        diff: None,
        error,
    }
}

fn decode_stored_run_event(stored: StoredEvent) -> Result<RunEvent, InteractionError> {
    let run_id = stored
        .run_id
        .parse::<RunId>()
        .map_err(|_| internal_error("stored run event has an invalid run identity"))?;
    let id = stored
        .id
        .parse::<EventId>()
        .map_err(|_| internal_error("stored run event has an invalid event identity"))?;
    let kind = serde_json::from_value::<EventKind>(stored.payload)
        .map_err(|_| internal_error("stored run event payload is invalid"))?;
    let timestamp = DateTime::parse_from_rfc3339(&stored.timestamp)
        .map_err(|_| internal_error("stored run event timestamp is invalid"))?
        .with_timezone(&Utc);
    let causation_id = stored
        .causation_id
        .map(|value| {
            value
                .parse::<EventId>()
                .map_err(|_| internal_error("stored run event causation identity is invalid"))
        })
        .transpose()?;
    let mut event = RunEvent::new_durable(
        id,
        run_id,
        stored.sequence,
        kind,
        EventCorrelation {
            run_id,
            ..EventCorrelation::default()
        },
    );
    event.causation_id = causation_id;
    event.timestamp = timestamp;
    Ok(event)
}

trait TerminalRunEvent {
    fn is_terminal_for_interaction(&self) -> bool;
}

impl TerminalRunEvent for EventKind {
    fn is_terminal_for_interaction(&self) -> bool {
        matches!(
            self,
            Self::RunCompleted { .. }
                | Self::RunFailed { .. }
                | Self::RunCancelled { .. }
                | Self::RunTimedOut
        )
    }
}

fn internal_error(message: &str) -> InteractionError {
    InteractionError::new(InteractionErrorCode::Internal, message)
}

fn store_error(context: &str, error: &impl std::fmt::Display) -> InteractionError {
    tracing::error!(%error, context, "durable interaction store operation failed");
    internal_error(&format!("{context}: durable store operation failed"))
}

fn interaction_target_store_error(error: SqliteStoreError) -> InteractionError {
    match error {
        SqliteStoreError::NotFound(_) => InteractionError::new(
            InteractionErrorCode::NotFound,
            "the selected interaction agent is unavailable",
        ),
        other => store_error("resolve interaction agent", &other),
    }
}

fn conversation_error(context: &str, error: &impl std::fmt::Display) -> InteractionError {
    tracing::error!(%error, context, "durable interaction transcript operation failed");
    internal_error(&format!("{context}: durable transcript operation failed"))
}

fn model_selection_error(error: ServiceError) -> InteractionError {
    match error {
        ServiceError::Config { message } => InteractionError::invalid_request(message),
        ServiceError::Unsupported { message } => {
            InteractionError::new(InteractionErrorCode::Unsupported, message)
        }
        ServiceError::AgentNotFound { .. } => InteractionError::new(
            InteractionErrorCode::NotFound,
            "the selected interaction agent is unavailable",
        ),
        other => service_error("resolve interaction model", &other),
    }
}

fn service_error(context: &str, error: &ServiceError) -> InteractionError {
    tracing::error!(%error, context, "interaction runtime operation failed");
    if let ServiceError::Unsupported { message } = error {
        return InteractionError::new(
            InteractionErrorCode::Unsupported,
            format!("{context}: {message}"),
        );
    }
    InteractionError::new(
        InteractionErrorCode::Unavailable,
        format!("{context}: shared runtime operation failed"),
    )
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "durable runtime fixtures fail fast at the exact persistence boundary under test"
)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    use futures::Stream;
    use polkagent_config::Config;
    use polkagent_conversation::{types::Conversation, ConversationStore};
    use polkagent_core::{
        AgentSpec, ApprovalId, EffectAttemptId, EffectId, EffectOutcomeId, EventId, StepId, TurnId,
        WorkerId,
    };
    use polkagent_event::{EventBus, EventRecorder};
    use polkagent_executor_fake::FakeExecutor;
    use polkagent_executor_trait::{
        ExecutorError, InferenceRequest, InferenceResponse, ModelExecutor, StreamEvent,
        TokenUsage as ExecutorTokenUsage,
    };
    use polkagent_harness_trait::{
        CancelMode, Harness, HarnessCapabilities, HarnessError, HarnessEvent, HarnessId,
        HarnessStatus, McpMode, SessionConfig, SessionId, SessionResumeMode, ToolInjection,
    };
    use polkagent_interaction::{
        ClientContext, ConfigOption, CreateInteractionRequest, InteractionOverrides,
        InteractionTurnId,
    };
    use polkagent_store_sqlite::migrations;
    use polkagent_store_trait::{
        EffectStore, RunStatus, StoreError, StoreRetryClass, StoredIntent, StoredOutcome,
    };

    use super::*;

    #[derive(Default)]
    struct CapturingExecutor {
        requests: Mutex<Vec<InferenceRequest>>,
    }

    impl CapturingExecutor {
        fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        fn requests(&self) -> Vec<InferenceRequest> {
            self.requests.lock().expect("capture lock").clone()
        }
    }

    #[async_trait]
    impl ModelExecutor for CapturingExecutor {
        async fn complete(
            &self,
            request: InferenceRequest,
        ) -> Result<InferenceResponse, ExecutorError> {
            self.requests.lock().expect("capture lock").push(request);
            Ok(InferenceResponse {
                text: "captured assistant".to_owned(),
                tool_calls: Vec::new(),
                stop_reason: "end_turn".to_owned(),
                usage: ExecutorTokenUsage::default(),
                provider_request_id: None,
            })
        }

        async fn stream(
            &self,
            _request: InferenceRequest,
        ) -> Result<
            Box<dyn Stream<Item = Result<StreamEvent, ExecutorError>> + Send + Unpin>,
            ExecutorError,
        > {
            Err(ExecutorError::Internal {
                message: "streaming is not used by interaction tests".to_owned(),
            })
        }

        async fn health(&self) -> Result<(), ExecutorError> {
            Ok(())
        }
    }

    struct NeverCalledHarness {
        id: HarnessId,
        calls: AtomicU64,
        models: Vec<String>,
        model_override: Option<String>,
    }

    impl NeverCalledHarness {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                id: HarnessId::new("never-called"),
                calls: AtomicU64::new(0),
                models: Vec::new(),
                model_override: None,
            })
        }

        fn with_model_evidence(models: Vec<String>, model_override: Option<String>) -> Arc<Self> {
            Arc::new(Self {
                id: HarnessId::new("model-evidence"),
                calls: AtomicU64::new(0),
                models,
                model_override,
            })
        }

        fn call_count(&self) -> u64 {
            self.calls.load(Ordering::SeqCst)
        }

        fn called(&self) {
            self.calls.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl Harness for NeverCalledHarness {
        fn id(&self) -> &HarnessId {
            &self.id
        }

        fn capabilities(&self) -> HarnessCapabilities {
            HarnessCapabilities {
                supports_streaming: false,
                supports_tools: false,
                supports_sessions: false,
                max_context_tokens: 0,
                models: self.models.clone(),
                transport: None,
                model_override: self.model_override.clone(),
                session_resume: SessionResumeMode::default(),
                mcp_passthrough: McpMode::default(),
                tool_injection: ToolInjection::default(),
                cancel: CancelMode::default(),
                multiplex_safe: false,
            }
        }

        fn status(&self) -> HarnessStatus {
            HarnessStatus::Idle
        }

        async fn start_session(&self, _config: SessionConfig) -> Result<SessionId, HarnessError> {
            self.called();
            Err(HarnessError::Internal {
                message: "must not start contextual harness session".to_owned(),
            })
        }

        async fn send_message(
            &self,
            _session_id: SessionId,
            _message: &str,
        ) -> Result<(), HarnessError> {
            self.called();
            Err(HarnessError::Internal {
                message: "must not send contextual harness message".to_owned(),
            })
        }

        async fn receive_events(
            &self,
            _session_id: SessionId,
        ) -> Result<std::pin::Pin<Box<dyn Stream<Item = HarnessEvent> + Send>>, HarnessError>
        {
            self.called();
            Err(HarnessError::Internal {
                message: "must not receive contextual harness events".to_owned(),
            })
        }

        async fn end_session(&self, _session_id: SessionId) -> Result<(), HarnessError> {
            self.called();
            Err(HarnessError::Internal {
                message: "must not end contextual harness session".to_owned(),
            })
        }

        async fn health(&self) -> Result<bool, HarnessError> {
            self.called();
            Ok(true)
        }
    }

    fn test_pool() -> SqlitePool {
        let pool = SqlitePool::open_in_memory().expect("open SQLite fixture");
        {
            let writer = pool.writer();
            migrations::migrate(&writer).expect("migrate SQLite fixture");
        }
        pool
    }

    fn seed_agent(pool: &SqlitePool) -> (AgentId, AgentSpec) {
        let agent_id = AgentId::new();
        let spec = AgentSpec::new(agent_id, "interaction-agent", "fake/model");
        let now = Utc::now().to_rfc3339();
        pool.writer()
            .execute(
                "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at)
                 VALUES (?1, ?2, 'active', ?3, ?4, ?4)",
                rusqlite::params![
                    agent_id.to_string(),
                    &spec.name,
                    serde_json::to_string(&spec).expect("encode agent"),
                    now
                ],
            )
            .expect("seed agent");
        (agent_id, spec)
    }

    fn test_service(
        pool: &SqlitePool,
        spec: AgentSpec,
        with_executor: bool,
    ) -> (Arc<AppService>, DurableInteractionService, EventBus) {
        let bus = EventBus::new(32);
        let shared = Arc::new(pool.clone());
        let recorder = EventRecorder::new(shared.clone(), bus.clone());
        let mut builder = AppService::builder()
            .with_config(Config::default())
            .with_run_store(shared.clone())
            .with_effect_store(shared.clone())
            .with_conversation_store(shared.clone())
            .with_payment_store(shared)
            .with_event_bus(bus.clone())
            .with_event_recorder(recorder);
        if with_executor {
            builder = builder.with_executor(FakeExecutor::new());
        }
        let app = Arc::new(builder.build().expect("build app service"));
        app.create_agent(spec).expect("register agent");
        let service = DurableInteractionService::new(Arc::clone(&app), pool.clone());
        (app, service, bus)
    }

    fn test_service_with_executor(
        pool: &SqlitePool,
        spec: AgentSpec,
        executor: Arc<dyn ModelExecutor>,
    ) -> (Arc<AppService>, DurableInteractionService, EventBus) {
        test_service_with_executor_and_config(pool, spec, executor, Config::default())
    }

    fn test_service_with_executor_and_config(
        pool: &SqlitePool,
        spec: AgentSpec,
        executor: Arc<dyn ModelExecutor>,
        config: Config,
    ) -> (Arc<AppService>, DurableInteractionService, EventBus) {
        let bus = EventBus::new(32);
        let shared = Arc::new(pool.clone());
        let recorder = EventRecorder::new(shared.clone(), bus.clone());
        let app = Arc::new(
            AppService::builder()
                .with_config(config)
                .with_executor(executor)
                .with_run_store(shared.clone())
                .with_effect_store(shared.clone())
                .with_conversation_store(shared.clone())
                .with_payment_store(shared)
                .with_event_bus(bus.clone())
                .with_event_recorder(recorder)
                .build()
                .expect("build app service"),
        );
        app.create_agent(spec).expect("register agent");
        let service = DurableInteractionService::new(Arc::clone(&app), pool.clone());
        (app, service, bus)
    }

    fn model_test_config() -> Config {
        Config {
            models: ["model-a", "model-b"]
                .into_iter()
                .map(|slug| polkagent_config::ModelOverrideConfig {
                    slug: slug.to_owned(),
                    provider: "fake".to_owned(),
                    ..polkagent_config::ModelOverrideConfig::default()
                })
                .collect(),
            ..Config::default()
        }
    }

    fn test_service_with_harness(
        pool: &SqlitePool,
        spec: AgentSpec,
        harness: Arc<dyn Harness>,
    ) -> (Arc<AppService>, DurableInteractionService, EventBus) {
        let bus = EventBus::new(32);
        let shared = Arc::new(pool.clone());
        let recorder = EventRecorder::new(shared.clone(), bus.clone());
        let app = Arc::new(
            AppService::builder()
                .with_config(Config::default())
                .with_harness(harness)
                .with_run_store(shared.clone())
                .with_effect_store(shared.clone())
                .with_conversation_store(shared.clone())
                .with_payment_store(shared)
                .with_event_bus(bus.clone())
                .with_event_recorder(recorder)
                .build()
                .expect("build harness app service"),
        );
        app.create_agent(spec).expect("register agent");
        let service = DurableInteractionService::new(Arc::clone(&app), pool.clone());
        (app, service, bus)
    }

    fn prompt_request(
        conversation_id: ConversationId,
        turn_id: InteractionTurnId,
        text: &str,
    ) -> PromptRequest {
        PromptRequest {
            turn_id: Some(turn_id),
            conversation_id,
            content: vec![InteractionContent::Text {
                text: text.to_owned(),
            }],
            config_overrides: InteractionOverrides::default(),
            client_context: ClientContext::new(PathBuf::from("/tmp")).expect("client context"),
        }
    }

    fn prompt_request_with_model(
        conversation_id: ConversationId,
        turn_id: InteractionTurnId,
        text: &str,
        model: OverrideValue<String>,
    ) -> PromptRequest {
        let mut request = prompt_request(conversation_id, turn_id, text);
        request.config_overrides.model = model;
        request
    }

    async fn create_model_interaction(
        service: &DurableInteractionService,
        agent_id: AgentId,
        model: Option<&str>,
    ) -> InteractionSummary {
        let mut config = InteractionConfig::new(InteractionTarget::Agent(agent_id));
        config.model = model.map(str::to_owned);
        service
            .new_interaction(CreateInteractionRequest {
                title: None,
                config,
                client_context: ClientContext::new(PathBuf::from("/tmp")).expect("client context"),
            })
            .await
            .expect("create model interaction")
    }

    async fn wait_for_terminal(started: &mut StartedTurn) -> InteractionEvent {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let event = started.events.recv().await.expect("interaction event");
                if event.event.is_terminal() {
                    return event.event;
                }
            }
        })
        .await
        .expect("terminal interaction event")
    }

    fn inference_message_text(message: &InferenceMessage) -> &str {
        match message.content.as_slice() {
            [ContentBlock::Text { text }] => text,
            other => panic!("expected one text block, got {other:?}"),
        }
    }

    fn captured_model_for_prompt(requests: &[InferenceRequest], prompt: &str) -> Option<String> {
        requests.iter().find_map(|request| {
            request
                .messages
                .last()
                .filter(|message| inference_message_text(message) == prompt)
                .map(|_| request.model_id.clone())
        })
    }

    fn durable_work_counts(pool: &SqlitePool, conversation_id: ConversationId) -> (i64, i64, i64) {
        let writer = pool.writer();
        let conversation_id = conversation_id.to_string();
        let turns = writer
            .query_row(
                "SELECT COUNT(*) FROM interaction_turns WHERE conversation_id = ?1",
                [&conversation_id],
                |row| row.get(0),
            )
            .expect("count interaction turns");
        let runs = writer
            .query_row(
                "SELECT COUNT(*) FROM runs WHERE conversation_id = ?1",
                [&conversation_id],
                |row| row.get(0),
            )
            .expect("count interaction runs");
        let events = writer
            .query_row(
                "SELECT COUNT(*) FROM interaction_events WHERE conversation_id = ?1",
                [&conversation_id],
                |row| row.get(0),
            )
            .expect("count interaction events");
        (turns, runs, events)
    }

    async fn seed_interaction(
        pool: &SqlitePool,
        agent_id: AgentId,
        conversation_id: ConversationId,
    ) -> SqliteInteractionStore {
        ConversationStore::create(pool, Conversation::new(conversation_id, agent_id))
            .await
            .expect("seed conversation");
        let store = SqliteInteractionStore::new(pool.clone());
        store
            .create_interaction(NewInteraction {
                conversation_id,
                config: InteractionConfig::new(InteractionTarget::Agent(agent_id)),
                origin_working_directory: std::path::PathBuf::from("/tmp"),
                created_at: Utc::now(),
            })
            .await
            .expect("seed interaction");
        store
    }

    #[derive(Clone, Copy)]
    enum TranscriptOutcome {
        Active,
        Completed,
        Failed,
        Cancelled,
        TimedOut,
    }

    async fn seed_transcript_turn(
        pool: &SqlitePool,
        store: &SqliteInteractionStore,
        agent_id: AgentId,
        conversation_id: ConversationId,
        ordinal: u32,
        outcome: TranscriptOutcome,
        assistant_text: &str,
    ) -> polkagent_interaction::StoredInteractionTurn {
        let run_id = RunId::new();
        RunStore::create_correlated(
            pool,
            run_id,
            &agent_id.to_string(),
            Some(&conversation_id.to_string()),
            RunStatus::new("created"),
        )
        .await
        .expect("prepare transcript run");
        let turn_id = InteractionTurnId::new();
        let user_message_id = Uuid::now_v7();
        store
            .create_turn(NewInteractionTurn {
                turn_id,
                conversation_id,
                ordinal,
                target: InteractionTarget::Agent(agent_id),
                config: InteractionConfig::new(InteractionTarget::Agent(agent_id)),
                user_message_id,
                user_message_text: format!("user-{ordinal}"),
                runs: vec![InteractionRunLink {
                    run_id,
                    role: RunRole::Primary,
                    ordinal: 1,
                }],
                initial_event_id: InteractionEventId::new(),
                started_at: Utc::now(),
            })
            .await
            .expect("seed transcript turn");
        let assistant_message_id = if matches!(outcome, TranscriptOutcome::Active) {
            None
        } else {
            let event = match outcome {
                TranscriptOutcome::Completed => InteractionEvent::TurnCompleted {
                    result: TurnResult {
                        text: assistant_text.to_owned(),
                        run_ids: vec![run_id],
                        usage: UsageView {
                            input_tokens: 11,
                            output_tokens: 13,
                            ..UsageView::default()
                        },
                    },
                },
                TranscriptOutcome::Failed => InteractionEvent::TurnFailed {
                    error: InteractionError::new(
                        InteractionErrorCode::Unavailable,
                        "fixture failure",
                    ),
                },
                TranscriptOutcome::Cancelled => InteractionEvent::TurnCancelled {
                    reason: Some("fixture cancellation".to_owned()),
                },
                TranscriptOutcome::TimedOut => InteractionEvent::TurnTimedOut,
                TranscriptOutcome::Active => unreachable!("active turns do not finish"),
            };
            let message_id = Uuid::now_v7();
            store
                .finish_turn(
                    NewInteractionEvent {
                        event_id: InteractionEventId::new(),
                        conversation_id,
                        turn_id,
                        timestamp: Utc::now(),
                        event,
                    },
                    NewAssistantMessage {
                        message_id,
                        text: assistant_text.to_owned(),
                        created_at: Utc::now(),
                    },
                )
                .await
                .expect("finish transcript turn");
            Some(message_id)
        };
        {
            let writer = pool.writer();
            writer
                .execute(
                    "UPDATE conversation_messages SET token_count = ?1 WHERE id = ?2",
                    rusqlite::params![i64::from(ordinal), user_message_id.to_string()],
                )
                .expect("set user transcript tokens");
            if let Some(message_id) = assistant_message_id {
                writer
                    .execute(
                        "UPDATE conversation_messages SET token_count = ?1 WHERE id = ?2",
                        rusqlite::params![i64::from(ordinal) + 10, message_id.to_string()],
                    )
                    .expect("set assistant transcript tokens");
            }
        }
        store.load_turn(turn_id).await.expect("reload seeded turn")
    }

    #[tokio::test]
    async fn concurrent_sessions_keep_execution_scoped_models_isolated() {
        let pool = test_pool();
        let (agent_id, spec) = seed_agent(&pool);
        let executor = CapturingExecutor::new();
        let erased: Arc<dyn ModelExecutor> = executor.clone();
        let (app, first_service, _bus) =
            test_service_with_executor_and_config(&pool, spec, erased, model_test_config());
        let second_service = DurableInteractionService::new(app, pool.clone());
        let first = create_model_interaction(&first_service, agent_id, Some("model-a")).await;
        let second = create_model_interaction(&second_service, agent_id, Some("model-b")).await;

        let (first_started, second_started) = tokio::join!(
            first_service.prompt(prompt_request(
                first.conversation_id,
                InteractionTurnId::new(),
                "session-a"
            )),
            second_service.prompt(prompt_request(
                second.conversation_id,
                InteractionTurnId::new(),
                "session-b"
            )),
        );
        let mut first_started = first_started.expect("start first model session");
        let mut second_started = second_started.expect("start second model session");
        let (first_terminal, second_terminal) = tokio::join!(
            wait_for_terminal(&mut first_started),
            wait_for_terminal(&mut second_started)
        );
        assert!(matches!(
            first_terminal,
            InteractionEvent::TurnCompleted { .. }
        ));
        assert!(matches!(
            second_terminal,
            InteractionEvent::TurnCompleted { .. }
        ));

        let default = create_model_interaction(&first_service, agent_id, None).await;
        let mut default_started = first_service
            .prompt(prompt_request(
                default.conversation_id,
                InteractionTurnId::new(),
                "agent-default",
            ))
            .await
            .expect("start default model session");
        wait_for_terminal(&mut default_started).await;

        let requests = executor.requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(
            captured_model_for_prompt(&requests, "session-a").as_deref(),
            Some("fake/model-a")
        );
        assert_eq!(
            captured_model_for_prompt(&requests, "session-b").as_deref(),
            Some("fake/model-b")
        );
        assert_eq!(
            captured_model_for_prompt(&requests, "agent-default").as_deref(),
            Some("fake/model")
        );
    }

    #[tokio::test]
    async fn persisted_model_option_survives_service_restart() {
        let pool = test_pool();
        let (agent_id, spec) = seed_agent(&pool);
        let executor = CapturingExecutor::new();
        let erased: Arc<dyn ModelExecutor> = executor.clone();
        let (app, service, _bus) =
            test_service_with_executor_and_config(&pool, spec, erased, model_test_config());
        let interaction = create_model_interaction(&service, agent_id, None).await;
        let config = service
            .set_config_option(
                interaction.conversation_id,
                ConfigUpdate {
                    option: ConfigOption::Model,
                    value: ConfigOptionValue::Model(Some("model-a".to_owned())),
                },
            )
            .await
            .expect("persist model option");
        assert_eq!(config.model.as_deref(), Some("fake/model-a"));

        let restarted = DurableInteractionService::new(app, pool);
        assert_eq!(
            restarted
                .load_interaction(interaction.conversation_id)
                .await
                .expect("load restarted interaction")
                .config
                .model
                .as_deref(),
            Some("fake/model-a")
        );
        let mut started = restarted
            .prompt(prompt_request(
                interaction.conversation_id,
                InteractionTurnId::new(),
                "after-restart",
            ))
            .await
            .expect("start restarted model prompt");
        wait_for_terminal(&mut started).await;
        let requests = executor.requests();
        assert_eq!(
            captured_model_for_prompt(&requests, "after-restart").as_deref(),
            Some("fake/model-a")
        );
    }

    #[tokio::test]
    async fn prompt_model_override_does_not_leak_and_retry_uses_effective_config() {
        let pool = test_pool();
        let (agent_id, spec) = seed_agent(&pool);
        let executor = CapturingExecutor::new();
        let erased: Arc<dyn ModelExecutor> = executor.clone();
        let (_app, service, _bus) =
            test_service_with_executor_and_config(&pool, spec, erased, model_test_config());
        let interaction = create_model_interaction(&service, agent_id, Some("model-a")).await;
        let override_turn_id = InteractionTurnId::new();
        let override_request = prompt_request_with_model(
            interaction.conversation_id,
            override_turn_id,
            "override-b",
            OverrideValue::Set("model-b".to_owned()),
        );
        let mut override_started = service
            .prompt(override_request.clone())
            .await
            .expect("start model override");
        wait_for_terminal(&mut override_started).await;
        let retry = service
            .prompt(override_request)
            .await
            .expect("retry identical effective config");
        assert_eq!(retry.handle, override_started.handle);
        assert_eq!(executor.requests().len(), 1);

        let conflict = service
            .prompt(prompt_request_with_model(
                interaction.conversation_id,
                override_turn_id,
                "override-b",
                OverrideValue::Set("model-a".to_owned()),
            ))
            .await
            .expect_err("retry with a different effective model must conflict");
        assert_eq!(conflict.code, InteractionErrorCode::Conflict);
        assert_eq!(executor.requests().len(), 1);

        let mut inherited = service
            .prompt(prompt_request(
                interaction.conversation_id,
                InteractionTurnId::new(),
                "inherit-a",
            ))
            .await
            .expect("start inherited model prompt");
        wait_for_terminal(&mut inherited).await;
        let mut cleared = service
            .prompt(prompt_request_with_model(
                interaction.conversation_id,
                InteractionTurnId::new(),
                "clear-to-agent-default",
                OverrideValue::Clear,
            ))
            .await
            .expect("start cleared model prompt");
        wait_for_terminal(&mut cleared).await;
        let requests = executor.requests();
        assert_eq!(
            captured_model_for_prompt(&requests, "override-b").as_deref(),
            Some("fake/model-b")
        );
        assert_eq!(
            captured_model_for_prompt(&requests, "inherit-a").as_deref(),
            Some("fake/model-a")
        );
        assert_eq!(
            captured_model_for_prompt(&requests, "clear-to-agent-default").as_deref(),
            Some("fake/model")
        );
        assert_eq!(
            service
                .load_interaction(interaction.conversation_id)
                .await
                .expect("load session after override")
                .config
                .model
                .as_deref(),
            Some("fake/model-a")
        );
    }

    #[tokio::test]
    async fn unknown_model_is_refused_before_turn_or_executor_invocation() {
        let pool = test_pool();
        let (agent_id, spec) = seed_agent(&pool);
        let executor = CapturingExecutor::new();
        let erased: Arc<dyn ModelExecutor> = executor.clone();
        let (_app, service, _bus) =
            test_service_with_executor_and_config(&pool, spec, erased, model_test_config());
        let interaction = create_model_interaction(&service, agent_id, None).await;
        let error = service
            .prompt(prompt_request_with_model(
                interaction.conversation_id,
                InteractionTurnId::new(),
                "unknown",
                OverrideValue::Set("not-in-catalog".to_owned()),
            ))
            .await
            .expect_err("unknown model must fail before activation");
        assert_eq!(error.code, InteractionErrorCode::InvalidRequest);
        assert!(error.message.contains("unknown model"));
        assert!(executor.requests().is_empty());
        assert!(service
            .list_turns(interaction.conversation_id)
            .await
            .expect("load untouched turns")
            .is_empty());
    }

    #[tokio::test]
    async fn harness_model_policy_accepts_only_noop_or_fixed_override_evidence() {
        let pool = test_pool();
        let (agent_id, spec) = seed_agent(&pool);
        let harness = NeverCalledHarness::with_model_evidence(
            vec!["listed-but-not-selectable".to_owned()],
            Some("fixed-model".to_owned()),
        );
        let erased: Arc<dyn Harness> = harness.clone();
        let (_app, service, _bus) = test_service_with_harness(&pool, spec, erased);
        let fixed = create_model_interaction(&service, agent_id, Some("fixed-model")).await;
        assert_eq!(fixed.config.model.as_deref(), Some("fake/fixed-model"));

        let error = service
            .set_config_option(
                fixed.conversation_id,
                ConfigUpdate {
                    option: ConfigOption::Model,
                    value: ConfigOptionValue::Model(Some("listed-but-not-selectable".to_owned())),
                },
            )
            .await
            .expect_err("harness discovery list must not imply dynamic selection");
        assert_eq!(error.code, InteractionErrorCode::Unsupported);
        assert!(error.message.contains("discovery-only"));
        assert_eq!(harness.call_count(), 0);
        assert_eq!(
            service
                .load_interaction(fixed.conversation_id)
                .await
                .expect("load unchanged harness interaction")
                .config
                .model
                .as_deref(),
            Some("fake/fixed-model")
        );
    }

    #[tokio::test]
    async fn follow_up_after_restart_preserves_roles_and_retry_does_not_duplicate_prompt() {
        let pool = test_pool();
        let (agent_id, spec) = seed_agent(&pool);
        let executor = FakeExecutor::new();
        let erased: Arc<dyn ModelExecutor> = executor.clone();
        let (app, service, _bus) = test_service_with_executor(&pool, spec, erased);
        let conversation_id = ConversationId::new();
        seed_interaction(&pool, agent_id, conversation_id).await;

        let first_turn_id = InteractionTurnId::new();
        let first_request = prompt_request(conversation_id, first_turn_id, "first user");
        let mut first = service
            .prompt(first_request.clone())
            .await
            .expect("start first prompt");
        assert!(matches!(
            wait_for_terminal(&mut first).await,
            InteractionEvent::TurnCompleted { .. }
        ));
        assert_eq!(executor.call_count(), 1);

        let restarted = DurableInteractionService::new(app, pool);
        let retry = restarted
            .prompt(first_request)
            .await
            .expect("retry completed prompt");
        assert_eq!(retry.handle, first.handle);
        assert_eq!(executor.call_count(), 1, "retry must not call the model");

        let mut follow_up = restarted
            .prompt(prompt_request(
                conversation_id,
                InteractionTurnId::new(),
                "second user",
            ))
            .await
            .expect("start follow-up");
        assert!(matches!(
            wait_for_terminal(&mut follow_up).await,
            InteractionEvent::TurnCompleted { .. }
        ));
        assert_eq!(executor.call_count(), 2);
        let request = executor.last_request().expect("captured follow-up request");
        assert_eq!(
            request
                .messages
                .iter()
                .map(|message| message.role)
                .collect::<Vec<_>>(),
            vec![
                InferenceMessageRole::User,
                InferenceMessageRole::Assistant,
                InferenceMessageRole::User,
            ]
        );
        assert_eq!(inference_message_text(&request.messages[0]), "first user");
        assert_eq!(
            inference_message_text(&request.messages[1]),
            "I am a fake assistant"
        );
        assert_eq!(inference_message_text(&request.messages[2]), "second user");
        assert_eq!(
            request
                .messages
                .iter()
                .filter(|message| inference_message_text(message) == "second user")
                .count(),
            1,
            "the current prompt must be appended exactly once"
        );
    }

    #[tokio::test]
    async fn contextual_execution_excludes_failed_and_cancelled_partial_turns() {
        let pool = test_pool();
        let (agent_id, spec) = seed_agent(&pool);
        let executor = FakeExecutor::new();
        let erased: Arc<dyn ModelExecutor> = executor.clone();
        let (_app, service, _bus) = test_service_with_executor(&pool, spec, erased);
        let conversation_id = ConversationId::new();
        let store = seed_interaction(&pool, agent_id, conversation_id).await;
        for (ordinal, outcome, assistant) in [
            (1, TranscriptOutcome::Completed, "assistant-1"),
            (2, TranscriptOutcome::Failed, "failed partial"),
            (3, TranscriptOutcome::Cancelled, "cancelled partial"),
            (4, TranscriptOutcome::Completed, "assistant-4"),
        ] {
            seed_transcript_turn(
                &pool,
                &store,
                agent_id,
                conversation_id,
                ordinal,
                outcome,
                assistant,
            )
            .await;
        }

        let mut started = service
            .prompt(prompt_request(
                conversation_id,
                InteractionTurnId::new(),
                "current user",
            ))
            .await
            .expect("start contextual prompt");
        wait_for_terminal(&mut started).await;
        let request = executor
            .last_request()
            .expect("captured contextual request");
        assert_eq!(
            request
                .messages
                .iter()
                .map(inference_message_text)
                .collect::<Vec<_>>(),
            vec![
                "user-1",
                "assistant-1",
                "user-4",
                "assistant-4",
                "current user",
            ]
        );
        assert_eq!(
            request
                .messages
                .iter()
                .map(|message| message.role)
                .collect::<Vec<_>>(),
            vec![
                InferenceMessageRole::User,
                InferenceMessageRole::Assistant,
                InferenceMessageRole::User,
                InferenceMessageRole::Assistant,
                InferenceMessageRole::User,
            ]
        );
    }

    #[tokio::test]
    async fn harness_follow_up_is_unsupported_and_terminalizes_without_invocation() {
        let pool = test_pool();
        let (agent_id, spec) = seed_agent(&pool);
        let harness = NeverCalledHarness::new();
        let erased: Arc<dyn Harness> = harness.clone();
        let (_app, service, _bus) = test_service_with_harness(&pool, spec, erased);
        let conversation_id = ConversationId::new();
        let store = seed_interaction(&pool, agent_id, conversation_id).await;
        seed_transcript_turn(
            &pool,
            &store,
            agent_id,
            conversation_id,
            1,
            TranscriptOutcome::Completed,
            "prior assistant",
        )
        .await;

        let error = service
            .prompt(prompt_request(
                conversation_id,
                InteractionTurnId::new(),
                "contextual follow-up",
            ))
            .await
            .expect_err("harness history must fail explicitly");
        assert_eq!(error.code, InteractionErrorCode::Unsupported);
        assert!(error.message.contains("role-safely"));
        assert_eq!(harness.call_count(), 0, "harness must not be invoked");

        let turns = store
            .list_turns(conversation_id)
            .await
            .expect("load durable turns");
        assert_eq!(turns.len(), 2);
        let failed = &turns[1];
        assert_eq!(failed.summary.state, TurnState::Failed);
        let run = RunStore::get(&pool, failed.runs[0].run_id)
            .await
            .expect("load linked rejected run");
        assert!(
            run.status.as_str().starts_with("failed"),
            "rejected run must not remain created: {:?}",
            run.status
        );
    }

    #[tokio::test]
    async fn contextual_execution_truncates_only_on_complete_turn_boundaries() {
        let pool = test_pool();
        let (agent_id, spec) = seed_agent(&pool);
        let executor = FakeExecutor::new();
        let erased: Arc<dyn ModelExecutor> = executor.clone();
        let (_app, service, _bus) = test_service_with_executor(&pool, spec, erased);
        let conversation_id = ConversationId::new();
        let store = seed_interaction(&pool, agent_id, conversation_id).await;
        for ordinal in 1..=35 {
            seed_transcript_turn(
                &pool,
                &store,
                agent_id,
                conversation_id,
                ordinal,
                TranscriptOutcome::Completed,
                &format!("assistant-{ordinal}"),
            )
            .await;
        }

        let mut started = service
            .prompt(prompt_request(
                conversation_id,
                InteractionTurnId::new(),
                "current user",
            ))
            .await
            .expect("start truncated prompt");
        wait_for_terminal(&mut started).await;
        let request = executor.last_request().expect("captured truncated request");
        assert_eq!(request.messages.len(), MAX_MODEL_CONTEXT_TURNS * 2 + 1);
        assert_eq!(inference_message_text(&request.messages[0]), "user-4");
        assert_eq!(inference_message_text(&request.messages[1]), "assistant-4");
        assert_eq!(
            inference_message_text(&request.messages[request.messages.len() - 2]),
            "assistant-35"
        );
        assert_eq!(
            inference_message_text(&request.messages[request.messages.len() - 1]),
            "current user"
        );
        assert!(request
            .messages
            .iter()
            .enumerate()
            .all(|(index, message)| message.role
                == if index % 2 == 0 {
                    InferenceMessageRole::User
                } else {
                    InferenceMessageRole::Assistant
                }));
    }

    #[tokio::test]
    async fn transcript_projection_survives_service_restart_and_preserves_turn_truth() {
        let pool = test_pool();
        let (agent_id, spec) = seed_agent(&pool);
        let (app, _service, _bus) = test_service(&pool, spec, false);
        let conversation_id = ConversationId::new();
        let store = seed_interaction(&pool, agent_id, conversation_id).await;
        for (ordinal, outcome, assistant) in [
            (1, TranscriptOutcome::Completed, "completed"),
            (2, TranscriptOutcome::Failed, "partial"),
            (3, TranscriptOutcome::Cancelled, ""),
            (4, TranscriptOutcome::TimedOut, "before timeout"),
            (5, TranscriptOutcome::Active, ""),
        ] {
            seed_transcript_turn(
                &pool,
                &store,
                agent_id,
                conversation_id,
                ordinal,
                outcome,
                assistant,
            )
            .await;
        }

        let restarted = DurableInteractionService::new(app, pool);
        let transcript = restarted
            .load_transcript(TranscriptRequest {
                conversation_id,
                limit: 100,
                offset: 0,
            })
            .await
            .expect("load restarted transcript");
        assert_eq!(transcript.len(), 5);
        assert_eq!(
            transcript
                .iter()
                .map(|turn| turn.turn.state)
                .collect::<Vec<_>>(),
            vec![
                TurnState::Completed,
                TurnState::Failed,
                TurnState::Cancelled,
                TurnState::TimedOut,
                TurnState::Running,
            ]
        );
        for (index, turn) in transcript.iter().enumerate() {
            let ordinal = u32::try_from(index + 1).expect("fixture ordinal");
            assert_eq!(turn.turn.ordinal, ordinal);
            assert_eq!(turn.user_text, format!("user-{ordinal}"));
            assert_eq!(turn.user_token_count, Some(ordinal));
            assert_eq!(
                turn.assistant_token_count,
                (ordinal < 5).then_some(ordinal + 10)
            );
        }
        assert_eq!(transcript[0].assistant_text.as_deref(), Some("completed"));
        assert_eq!(transcript[1].assistant_text.as_deref(), Some("partial"));
        assert_eq!(transcript[2].assistant_text.as_deref(), Some(""));
        assert_eq!(
            transcript[3].assistant_text.as_deref(),
            Some("before timeout")
        );
        assert_eq!(transcript[4].assistant_text, None);
    }

    #[tokio::test]
    async fn transcript_projection_fails_closed_on_missing_mismatched_and_rich_messages() {
        let pool = test_pool();
        let (agent_id, spec) = seed_agent(&pool);
        let (_app, service, _bus) = test_service(&pool, spec, false);

        for (case, corruption, expected) in [
            (
                "missing",
                "UPDATE interaction_turns SET state = 'completed', completed_at = started_at WHERE id = ?1",
                "missing its assistant transcript",
            ),
            (
                "mismatched",
                "UPDATE conversation_messages SET role = 'assistant' WHERE id = ?1",
                "mismatched role",
            ),
            (
                "rich",
                "UPDATE conversation_messages SET content_json = '{\"type\":\"tool_call\",\"name\":\"fixture\",\"arguments\":{}}' WHERE id = ?1",
                "unsupported rich content",
            ),
        ] {
            let conversation_id = ConversationId::new();
            let store = seed_interaction(&pool, agent_id, conversation_id).await;
            let turn = seed_transcript_turn(
                &pool,
                &store,
                agent_id,
                conversation_id,
                1,
                TranscriptOutcome::Active,
                "",
            )
            .await;
            let target = if case == "missing" {
                turn.summary.handle.turn_id.to_string()
            } else {
                turn.user_message_id.to_string()
            };
            pool.writer()
                .execute(corruption, [target])
                .expect("corrupt transcript correlation");
            let error = service
                .load_transcript(TranscriptRequest {
                    conversation_id,
                    limit: 100,
                    offset: 0,
                })
                .await
                .expect_err("corrupt transcript must fail closed");
            assert_eq!(error.code, InteractionErrorCode::Internal, "{case}");
            assert!(error.message.contains(expected), "{case}: {error}");
        }
    }

    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "the single transaction keeps the 1001-turn pagination fixture fast and auditable"
    )]
    async fn transcript_projection_pages_beyond_one_thousand_turns_and_messages() {
        let pool = test_pool();
        let (agent_id, spec) = seed_agent(&pool);
        let (_app, service, _bus) = test_service(&pool, spec, false);
        let conversation_id = ConversationId::new();
        seed_interaction(&pool, agent_id, conversation_id).await;
        let target = InteractionTarget::Agent(agent_id);
        let target_json = serde_json::to_string(&target).expect("encode target");
        let config_json =
            serde_json::to_string(&InteractionConfig::new(target.clone())).expect("encode config");
        {
            let mut writer = pool.writer();
            let transaction = writer.transaction().expect("begin large transcript seed");
            for ordinal in 1..=1_002_i64 {
                let turn_id = InteractionTurnId::new();
                let run_id = RunId::new();
                let user_id = Uuid::now_v7();
                let assistant_id = Uuid::now_v7();
                let event_id = InteractionEventId::new();
                let timestamp = (Utc::now() + chrono::Duration::milliseconds(ordinal)).to_rfc3339();
                for (message_id, role, text, tokens) in [
                    (user_id, "user", format!("user-{ordinal}"), ordinal),
                    (
                        assistant_id,
                        "assistant",
                        format!("assistant-{ordinal}"),
                        ordinal + 10,
                    ),
                ] {
                    let content = serde_json::to_string(&MessageContent::Text { text })
                        .expect("encode transcript message");
                    transaction
                        .execute(
                            "INSERT INTO conversation_messages
                             (id, conversation_id, role, content_json, token_count, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                            rusqlite::params![
                                message_id.to_string(),
                                conversation_id.to_string(),
                                role,
                                content,
                                tokens,
                                &timestamp,
                            ],
                        )
                        .expect("seed paged transcript message");
                }
                transaction
                .execute(
                    "INSERT INTO runs
                         (id, agent_id, conversation_id, state, params_json, created_at, updated_at, completed_at)
                     VALUES (?1, ?2, ?3, 'completed', '{}', ?4, ?4, ?4)",
                    rusqlite::params![
                        run_id.to_string(),
                        agent_id.to_string(),
                        conversation_id.to_string(),
                        &timestamp,
                    ],
                )
                .expect("seed paged transcript run");
                transaction
                    .execute(
                        "INSERT INTO interaction_turns
                         (id, conversation_id, ordinal, state, target_json, config_json,
                          user_message_id, assistant_message_id, started_at, completed_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                        rusqlite::params![
                            turn_id.to_string(),
                            conversation_id.to_string(),
                            ordinal,
                            match ordinal {
                                1_001 => "failed",
                                1_002 => "cancelled",
                                _ => "completed",
                            },
                            &target_json,
                            &config_json,
                            user_id.to_string(),
                            assistant_id.to_string(),
                            &timestamp,
                        ],
                    )
                    .expect("seed paged transcript turn");
                transaction
                    .execute(
                        "INSERT INTO interaction_turn_runs (turn_id, run_id, role_json, ordinal)
                     VALUES (?1, ?2, '\"primary\"', 1)",
                        rusqlite::params![turn_id.to_string(), run_id.to_string()],
                    )
                    .expect("seed paged transcript run link");
                let event = InteractionEvent::TurnStarted {
                    target: target.clone(),
                    runs: vec![(run_id, RunRole::Primary)],
                };
                transaction
                .execute(
                    "INSERT INTO interaction_events
                         (id, conversation_id, turn_id, sequence, kind, payload_json, is_terminal, created_at)
                     VALUES (?1, ?2, ?3, ?4, 'turn_started', ?5, 0, ?6)",
                    rusqlite::params![
                        event_id.to_string(),
                        conversation_id.to_string(),
                        turn_id.to_string(),
                        ordinal,
                        serde_json::to_string(&event).expect("encode turn event"),
                        &timestamp,
                    ],
                )
                .expect("seed paged transcript event");
            }
            let unlinked_content = serde_json::to_string(&MessageContent::Text {
                text: "unlinked-low-level-message".to_owned(),
            })
            .expect("encode unlinked message");
            transaction
                .execute(
                    "INSERT INTO conversation_messages
                     (id, conversation_id, role, content_json, token_count, created_at)
                 VALUES (?1, ?2, 'user', ?3, 999999, ?4)",
                    rusqlite::params![
                        Uuid::now_v7().to_string(),
                        conversation_id.to_string(),
                        unlinked_content,
                        Utc::now().to_rfc3339(),
                    ],
                )
                .expect("seed unlinked low-level message");
            transaction
                .execute(
                    "UPDATE conversations SET message_count = 2005 WHERE id = ?1",
                    [conversation_id.to_string()],
                )
                .expect("update paged transcript count");
            transaction.commit().expect("commit large transcript seed");
        }

        let tail = service
            .load_transcript(TranscriptRequest {
                conversation_id,
                limit: 3,
                offset: 999,
            })
            .await
            .expect("load transcript tail across message pages");
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[0].turn.ordinal, 1_000);
        assert_eq!(tail[0].turn.state, TurnState::Completed);
        assert_eq!(tail[0].user_text, "user-1000");
        assert_eq!(tail[0].assistant_text.as_deref(), Some("assistant-1000"));
        assert_eq!(tail[1].turn.ordinal, 1_001);
        assert_eq!(tail[1].turn.state, TurnState::Failed);
        assert_eq!(tail[1].user_token_count, Some(1_001));
        assert_eq!(tail[1].assistant_token_count, Some(1_011));
        assert_eq!(tail[2].turn.ordinal, 1_002);
        assert_eq!(tail[2].turn.state, TurnState::Cancelled);
        assert_eq!(tail[2].assistant_text.as_deref(), Some("assistant-1002"));
        assert!(tail
            .iter()
            .all(|turn| !turn.user_text.contains("unlinked-low-level-message")));
        assert!(service
            .load_transcript(TranscriptRequest {
                conversation_id,
                limit: 100,
                offset: 1_002,
            })
            .await
            .expect("load empty transcript tail")
            .is_empty());
    }

    #[tokio::test]
    async fn prompt_commits_correlation_before_first_event_and_retries_once() {
        let pool = test_pool();
        let (agent_id, spec) = seed_agent(&pool);
        let (app, service, _bus) = test_service(&pool, spec, true);
        let interaction = service
            .new_interaction(CreateInteractionRequest {
                title: Some("durable test".to_owned()),
                config: InteractionConfig::new(InteractionTarget::Agent(agent_id)),
                client_context: ClientContext::new(PathBuf::from("/tmp")).expect("client context"),
            })
            .await
            .expect("create interaction");
        let turn_id = InteractionTurnId::new();
        let request = PromptRequest {
            turn_id: Some(turn_id),
            conversation_id: interaction.conversation_id,
            content: vec![InteractionContent::Text {
                text: "hello".to_owned(),
            }],
            config_overrides: InteractionOverrides::default(),
            client_context: ClientContext::new(PathBuf::from("/tmp")).expect("client context"),
        };
        let mut run_events = app.subscribe_events();
        let mut started = service.prompt(request.clone()).await.expect("start prompt");
        let run_id = started.handle.run_ids[0];
        let first_run_event = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let event = run_events.recv().await.expect("run event bus");
                if event.run_id == run_id {
                    return event;
                }
            }
        })
        .await
        .expect("first run event");
        assert!(matches!(first_run_event.kind, EventKind::RunCreated));
        let (messages_at_first_event, links_at_first_event, run_conversation): (
            i64,
            i64,
            Option<String>,
        ) = {
            let writer = pool.writer();
            (
                writer
                    .query_row(
                        "SELECT COUNT(*) FROM conversation_messages WHERE conversation_id = ?1",
                        [interaction.conversation_id.to_string()],
                        |row| row.get(0),
                    )
                    .expect("count durable user messages"),
                writer
                    .query_row(
                        "SELECT COUNT(*) FROM interaction_turn_runs WHERE run_id = ?1",
                        [run_id.to_string()],
                        |row| row.get(0),
                    )
                    .expect("count durable run links"),
                writer
                    .query_row(
                        "SELECT conversation_id FROM runs WHERE id = ?1",
                        [run_id.to_string()],
                        |row| row.get(0),
                    )
                    .expect("load run conversation correlation"),
            )
        };
        assert_eq!((messages_at_first_event, links_at_first_event), (1, 1));
        assert_eq!(
            run_conversation,
            Some(interaction.conversation_id.to_string())
        );

        let first = started.events.recv().await.expect("turn started replay");
        assert!(matches!(first.event, InteractionEvent::TurnStarted { .. }));
        let terminal = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let event = started.events.recv().await.expect("interaction event");
                if event.event.is_terminal() {
                    return event;
                }
            }
        })
        .await
        .expect("terminal interaction event");
        let completed_text = match &terminal.event {
            InteractionEvent::TurnCompleted { result } => result.text.clone(),
            other => panic!("expected completed interaction, got {other:?}"),
        };
        assert!(!completed_text.is_empty());
        let transcript = ConversationStore::get_messages(&pool, interaction.conversation_id, 10, 0)
            .await
            .expect("load ordered transcript");
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[0].role, MessageRole::User);
        assert_eq!(transcript[1].role, MessageRole::Assistant);
        assert_eq!(
            transcript[0].content,
            MessageContent::Text {
                text: "hello".to_owned()
            }
        );
        assert_eq!(
            transcript[1].content,
            MessageContent::Text {
                text: completed_text
            }
        );

        let retry = service.prompt(request.clone()).await.expect("retry prompt");
        assert_eq!(retry.handle, started.handle);
        let mut conflicting = request;
        conflicting.content = vec![InteractionContent::Text {
            text: "different".to_owned(),
        }];
        let error = service
            .prompt(conflicting)
            .await
            .expect_err("conflicting turn retry");
        assert_eq!(error.code, InteractionErrorCode::Conflict);
        let counts: (i64, i64) = {
            let writer = pool.writer();
            (
                writer
                    .query_row(
                        "SELECT COUNT(*) FROM conversation_messages WHERE conversation_id = ?1",
                        [interaction.conversation_id.to_string()],
                        |row| row.get(0),
                    )
                    .expect("count final transcript"),
                writer
                    .query_row(
                        "SELECT COUNT(*) FROM interaction_events
                         WHERE conversation_id = ?1 AND is_terminal = 1",
                        [interaction.conversation_id.to_string()],
                        |row| row.get(0),
                    )
                    .expect("count terminal events"),
            )
        };
        let created_events = EventStore::read_run_events(&pool, run_id)
            .await
            .expect("read run events")
            .into_iter()
            .filter_map(|event| serde_json::from_value::<EventKind>(event.payload).ok())
            .filter(|kind| matches!(kind, EventKind::RunCreated))
            .count();
        assert_eq!(counts, (2, 1));
        assert_eq!(created_events, 1);
    }

    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "one proof keeps origin creation, retry, restart, and before-work rejection counts exact"
    )]
    async fn durable_origin_matches_across_retry_and_restart_and_rejects_before_work() {
        let pool = test_pool();
        let (agent_id, spec) = seed_agent(&pool);
        let (app, service, _bus) = test_service(&pool, spec, true);
        let origin = PathBuf::from("/workspace/alpha");
        let interaction = service
            .new_interaction(CreateInteractionRequest {
                title: Some("origin proof".to_owned()),
                config: InteractionConfig::new(InteractionTarget::Agent(agent_id)),
                client_context: ClientContext::new(origin.clone()).expect("exact origin"),
            })
            .await
            .expect("create origin-bound interaction");
        let stored_origin: String = pool
            .writer()
            .query_row(
                "SELECT origin_working_directory FROM interaction_sessions
                 WHERE conversation_id = ?1",
                [interaction.conversation_id.to_string()],
                |row| row.get(0),
            )
            .expect("load persisted origin");
        assert_eq!(stored_origin, "/workspace/alpha");

        let first_turn_id = InteractionTurnId::new();
        let mut first_request = prompt_request(
            interaction.conversation_id,
            first_turn_id,
            "first exact-origin turn",
        );
        first_request.client_context =
            ClientContext::new(origin.clone()).expect("exact prompt origin");
        let mut first = service
            .prompt(first_request.clone())
            .await
            .expect("exact origin starts work");
        let first_handle = first.handle.clone();
        let _ = wait_for_terminal(&mut first).await;
        let retry = service
            .prompt(first_request.clone())
            .await
            .expect("exact-origin retry returns durable winner");
        assert_eq!(retry.handle, first_handle);

        let before_rejections = durable_work_counts(&pool, interaction.conversation_id);
        let mut mismatched_retry = first_request;
        mismatched_retry.client_context =
            ClientContext::new(PathBuf::from("/workspace/beta")).expect("other workspace");
        let mismatch = service
            .prompt(mismatched_retry)
            .await
            .expect_err("retry from another workspace must fail");
        assert_eq!(mismatch.code, InteractionErrorCode::Conflict);

        for invalid in [
            PathBuf::from("relative/path"),
            PathBuf::from("/workspace/alpha/../beta"),
        ] {
            let mut request = prompt_request(
                interaction.conversation_id,
                InteractionTurnId::new(),
                "invalid-origin turn",
            );
            request.client_context.working_directory = invalid;
            let error = service
                .prompt(request)
                .await
                .expect_err("invalid lexical cwd must fail before durable work");
            assert_eq!(error.code, InteractionErrorCode::InvalidRequest);
        }
        assert_eq!(
            durable_work_counts(&pool, interaction.conversation_id),
            before_rejections,
            "mismatch, relative, and traversal cwd must not append events, turns, or runs"
        );

        let restarted = DurableInteractionService::new(Arc::clone(&app), pool.clone());
        restarted
            .verify_interaction_origin(interaction.conversation_id, &origin)
            .await
            .expect("restart verifies exact durable origin");
        let second_turn_id = InteractionTurnId::new();
        let mut second_request = prompt_request(
            interaction.conversation_id,
            second_turn_id,
            "second exact-origin turn after restart",
        );
        second_request.client_context = ClientContext::new(origin).expect("restart origin");
        let mut second = restarted
            .prompt(second_request)
            .await
            .expect("restart accepts exact-origin prompt");
        let _ = wait_for_terminal(&mut second).await;
        let after_restart = durable_work_counts(&pool, interaction.conversation_id);
        assert_eq!(after_restart.0, before_rejections.0 + 1);
        assert_eq!(after_restart.1, before_rejections.1 + 1);
        assert!(after_restart.2 > before_rejections.2);
    }

    #[tokio::test]
    async fn assistant_text_replays_more_than_one_store_page() {
        let pool = test_pool();
        let (agent_id, spec) = seed_agent(&pool);
        let (_app, service, _bus) = test_service(&pool, spec, false);
        let conversation_id = ConversationId::new();
        let store = seed_interaction(&pool, agent_id, conversation_id).await;
        let run_id = RunId::new();
        RunStore::create_correlated(
            &pool,
            run_id,
            &agent_id.to_string(),
            Some(&conversation_id.to_string()),
            RunStatus::new("created"),
        )
        .await
        .expect("prepare run");
        let turn_id = InteractionTurnId::new();
        store
            .create_turn(NewInteractionTurn {
                turn_id,
                conversation_id,
                ordinal: 1,
                target: InteractionTarget::Agent(agent_id),
                config: InteractionConfig::new(InteractionTarget::Agent(agent_id)),
                user_message_id: Uuid::now_v7(),
                user_message_text: "prompt".to_owned(),
                runs: vec![InteractionRunLink {
                    run_id,
                    role: RunRole::Primary,
                    ordinal: 1,
                }],
                initial_event_id: InteractionEventId::new(),
                started_at: Utc::now(),
            })
            .await
            .expect("seed turn");
        for _ in 0..1_005 {
            store
                .append_event(NewInteractionEvent {
                    event_id: InteractionEventId::new(),
                    conversation_id,
                    turn_id,
                    timestamp: Utc::now(),
                    event: InteractionEvent::AgentMessageDelta {
                        run_id,
                        text: "x".to_owned(),
                    },
                })
                .await
                .expect("append delta");
        }
        assert_eq!(
            service
                .assistant_text(conversation_id, turn_id)
                .await
                .expect("page assistant text"),
            "x".repeat(1_005)
        );
    }

    #[tokio::test]
    async fn recovery_reaches_oldest_page_and_closes_pre_activation_crash_window() {
        let pool = test_pool();
        let (agent_id, spec) = seed_agent(&pool);
        let (_app, service, _bus) = test_service(&pool, spec, false);
        let target = InteractionTarget::Agent(agent_id);
        let config_json = serde_json::to_string(&InteractionConfig::new(target.clone()))
            .expect("encode interaction config");
        let mut ids = Vec::with_capacity(1_001);
        {
            let mut writer = pool.writer();
            let transaction = writer.transaction().expect("begin interaction seed");
            for index in 0..1_001_i64 {
                let id = ConversationId::new();
                ids.push(id);
                let timestamp = (Utc::now() + chrono::Duration::seconds(index)).to_rfc3339();
                transaction
                    .execute(
                        "INSERT INTO conversations
                             (id, agent_id, message_count, metadata_json, created_at, updated_at)
                         VALUES (?1, ?2, 0, '{}', ?3, ?3)",
                        rusqlite::params![id.to_string(), agent_id.to_string(), &timestamp],
                    )
                    .expect("seed conversation page");
                transaction
                    .execute(
                        "INSERT INTO interaction_sessions
                             (conversation_id, config_json, state, created_at, updated_at,
                              origin_working_directory)
                         VALUES (?1, ?2, 'active', ?3, ?3, '/tmp')",
                        rusqlite::params![id.to_string(), &config_json, &timestamp],
                    )
                    .expect("seed interaction page");
            }
            transaction.commit().expect("commit interaction pages");
        }
        let conversation_id = ids[0];
        let run_id = RunId::new();
        RunStore::create_correlated(
            &pool,
            run_id,
            &agent_id.to_string(),
            Some(&conversation_id.to_string()),
            RunStatus::new("created"),
        )
        .await
        .expect("prepare linked run");
        let orphan = RunId::new();
        RunStore::create_correlated(
            &pool,
            orphan,
            &agent_id.to_string(),
            Some(&conversation_id.to_string()),
            RunStatus::new("created"),
        )
        .await
        .expect("prepare orphan run");
        let turn_id = InteractionTurnId::new();
        service
            .store
            .create_turn(NewInteractionTurn {
                turn_id,
                conversation_id,
                ordinal: 1,
                target: target.clone(),
                config: InteractionConfig::new(target),
                user_message_id: Uuid::now_v7(),
                user_message_text: "interrupted".to_owned(),
                runs: vec![InteractionRunLink {
                    run_id,
                    role: RunRole::Primary,
                    ordinal: 1,
                }],
                initial_event_id: InteractionEventId::new(),
                started_at: Utc::now(),
            })
            .await
            .expect("seed interrupted turn");

        assert_eq!(service.recover().await.expect("recover interactions"), 1);
        let recovered = service.store.load_turn(turn_id).await.expect("load turn");
        assert_eq!(recovered.summary.state, TurnState::Failed);
        assert!(recovered.assistant_message_id.is_some());
        assert!(matches!(
            RunStore::get(&pool, orphan).await,
            Err(StoreError::NotFound { .. })
        ));
        let run_events = EventStore::read_run_events(&pool, run_id)
            .await
            .expect("read recovery run events");
        assert_eq!(
            run_events
                .iter()
                .filter_map(|event| {
                    serde_json::from_value::<EventKind>(event.payload.clone()).ok()
                })
                .filter(|kind| matches!(kind, EventKind::RunFailed { .. }))
                .count(),
            1
        );
        assert_eq!(service.recover().await.expect("idempotent recovery"), 0);
        assert_eq!(
            EventStore::read_run_events(&pool, run_id)
                .await
                .expect("read idempotent events")
                .len(),
            run_events.len()
        );
    }

    #[tokio::test]
    async fn cancellation_survives_run_bus_lag_and_supports_durable_replay() {
        let pool = test_pool();
        let (agent_id, spec) = seed_agent(&pool);
        let (_app, service, bus) = test_service(&pool, spec, false);
        let interaction = service
            .new_interaction(CreateInteractionRequest {
                title: None,
                config: InteractionConfig::new(InteractionTarget::Agent(agent_id)),
                client_context: ClientContext::new(PathBuf::from("/tmp")).expect("client context"),
            })
            .await
            .expect("create interaction");
        let mut started = service
            .prompt(PromptRequest {
                turn_id: Some(InteractionTurnId::new()),
                conversation_id: interaction.conversation_id,
                content: vec![InteractionContent::Text {
                    text: "wait".to_owned(),
                }],
                config_overrides: InteractionOverrides::default(),
                client_context: ClientContext::new(PathBuf::from("/tmp")).expect("client context"),
            })
            .await
            .expect("start queued prompt");
        let run_id = started.handle.run_ids[0];
        for _ in 0..100 {
            bus.publish(RunEvent::new_ephemeral(
                EventId::new(),
                run_id,
                0,
                EventKind::StreamingToken {
                    text: "lost".to_owned(),
                },
            ));
        }
        service
            .cancel_turn(started.handle.turn_id)
            .await
            .expect("cancel lagged turn");
        service
            .cancel_turn(started.handle.turn_id)
            .await
            .expect("idempotent cancellation");
        let terminal = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let event = started.events.recv().await.expect("interaction event");
                if event.event.is_terminal() {
                    return event;
                }
            }
        })
        .await
        .expect("cancel terminal event");
        assert!(matches!(
            terminal.event,
            InteractionEvent::TurnCancelled { .. }
        ));
        let mut replay = service
            .subscribe(SubscriptionRequest {
                conversation_id: interaction.conversation_id,
                turn_id: Some(started.handle.turn_id),
                after_sequence: Some(started.handle.first_event_sequence),
                capacity: 8,
            })
            .await
            .expect("subscribe from durable checkpoint");
        let replayed_terminal = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let event = replay.recv().await.expect("replayed event");
                if event.event.is_terminal() {
                    return event;
                }
            }
        })
        .await
        .expect("replayed terminal");
        assert_eq!(replayed_terminal.event_id, terminal.event_id);
        let writer = pool.writer();
        let terminal_count: i64 = writer
            .query_row(
                "SELECT COUNT(*) FROM interaction_events WHERE turn_id = ?1 AND is_terminal = 1",
                [started.handle.turn_id.to_string()],
                |row| row.get(0),
            )
            .expect("count cancellation terminals");
        assert_eq!(terminal_count, 1);
    }

    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "the live/restart proof keeps exact effect, interaction, and redaction identities contiguous"
    )]
    async fn durable_tool_projection_keeps_identity_status_and_redaction_across_restart() {
        let pool = test_pool();
        let (agent_id, mut spec) = seed_agent(&pool);
        spec.tools.push("test.fails".to_owned());
        let (app, service, _bus) = test_service(&pool, spec, false);
        let interaction = service
            .new_interaction(CreateInteractionRequest {
                title: None,
                config: InteractionConfig::new(InteractionTarget::Agent(agent_id)),
                client_context: ClientContext::new(PathBuf::from("/tmp")).expect("client context"),
            })
            .await
            .expect("create interaction");
        let run_id = RunId::new();
        RunStore::create_correlated(
            &pool,
            run_id,
            &agent_id.to_string(),
            Some(&interaction.conversation_id.to_string()),
            RunStatus::new("running"),
        )
        .await
        .expect("seed linked run");
        let execution_turn_id = TurnId::new();
        let step_id = StepId::new();
        {
            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO turns (id, run_id, sequence, role, started_at)
                     VALUES (?1, ?2, 1, 'assistant', ?3)",
                    rusqlite::params![
                        execution_turn_id.to_string(),
                        run_id.to_string(),
                        Utc::now().to_rfc3339()
                    ],
                )
                .expect("seed execution turn");
            writer
                .execute(
                    "INSERT INTO steps (id, turn_id, sequence, kind, started_at)
                     VALUES (?1, ?2, 1, 'tool_call', ?3)",
                    rusqlite::params![
                        step_id.to_string(),
                        execution_turn_id.to_string(),
                        Utc::now().to_rfc3339()
                    ],
                )
                .expect("seed tool step");
        }
        let interaction_turn_id = InteractionTurnId::new();
        service
            .store
            .create_turn(NewInteractionTurn {
                turn_id: interaction_turn_id,
                conversation_id: interaction.conversation_id,
                ordinal: 1,
                target: InteractionTarget::Agent(agent_id),
                config: InteractionConfig::new(InteractionTarget::Agent(agent_id)),
                user_message_id: Uuid::now_v7(),
                user_message_text: "call a failing tool".to_owned(),
                runs: vec![InteractionRunLink {
                    run_id,
                    role: RunRole::Primary,
                    ordinal: 1,
                }],
                initial_event_id: InteractionEventId::new(),
                started_at: Utc::now(),
            })
            .await
            .expect("seed interaction turn");

        let intent_id = EffectId::new();
        EffectStore::propose_intent(
            &pool,
            StoredIntent {
                id: intent_id,
                run_id,
                step_id,
                state: "pending".to_owned(),
                lease_owner: None,
                lease_expires: None,
                retry_class: StoreRetryClass::CheckBeforeRetry,
                payload: serde_json::json!({
                    "kind": "tool_call",
                    "params": {
                        "tool_call_id": "provider-only-id",
                        "tool_name": "test.fails",
                        "arguments": {"secret": "argument-secret"},
                    },
                }),
                idempotency_key: format!("tool-{intent_id}"),
                created_at: Utc::now(),
            },
        )
        .await
        .expect("persist tool intent");
        let worker_id = WorkerId::new();
        let claimed = EffectStore::claim_intent(&pool, worker_id, Duration::from_secs(30))
            .await
            .expect("claim tool intent")
            .expect("seeded tool intent is claimable");
        assert_eq!(claimed.id, intent_id);
        let attempt_id = EffectAttemptId::new();
        EffectStore::record_attempt_start(
            &pool,
            attempt_id,
            intent_id,
            worker_id,
            serde_json::json!({"secret": "attempt-secret"}),
        )
        .await
        .expect("persist tool attempt");

        let mut started = RunEvent::new_ephemeral(
            EventId::new(),
            run_id,
            0,
            EventKind::ToolCallStarted {
                tool_name: "test.fails".to_owned(),
            },
        );
        started.correlation = EventCorrelation {
            run_id,
            turn_id: Some(execution_turn_id),
            step_id: Some(step_id),
            effect_intent_id: Some(intent_id),
            effect_attempt_id: Some(attempt_id),
        };
        service
            .project_run_event(
                interaction.conversation_id,
                interaction_turn_id,
                started,
                true,
            )
            .await
            .expect("project live tool start");

        EffectStore::record_outcome(
            &pool,
            StoredOutcome {
                id: EffectOutcomeId::new(),
                intent_id,
                attempt_id,
                run_id,
                consumed: false,
                payload: serde_json::json!({
                    "variant": "failure",
                    "error_class": "server_error",
                    "message": "handler-secret",
                    "retriable": false,
                }),
                observed_at: Utc::now(),
            },
        )
        .await
        .expect("persist tool outcome");
        let mut completed = RunEvent::new_ephemeral(
            EventId::new(),
            run_id,
            0,
            EventKind::ToolCallCompleted {
                tool_name: "test.fails".to_owned(),
            },
        );
        completed.correlation = EventCorrelation {
            run_id,
            turn_id: Some(execution_turn_id),
            step_id: Some(step_id),
            effect_intent_id: Some(intent_id),
            effect_attempt_id: Some(attempt_id),
        };
        service
            .project_run_event(
                interaction.conversation_id,
                interaction_turn_id,
                completed,
                true,
            )
            .await
            .expect("project live tool completion");

        let restarted = DurableInteractionService::new(app, pool.clone());
        restarted
            .backfill_tool_calls(interaction.conversation_id, interaction_turn_id, run_id)
            .await
            .expect("idempotent restart projection");
        let events = restarted
            .store
            .load_events(interaction.conversation_id, 0, 100)
            .await
            .expect("load durable interaction events");
        let tools = events
            .iter()
            .filter_map(|envelope| match &envelope.event {
                InteractionEvent::ToolCallStarted { call }
                | InteractionEvent::ToolCallUpdated { call } => Some((envelope, call)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(tools.len(), 2);
        let expected_call_id = ToolCallId::from_uuid(intent_id.as_uuid());
        assert_eq!(tools[0].1.call_id, expected_call_id);
        assert_eq!(tools[1].1.call_id, expected_call_id);
        assert_eq!(tools[0].1.status, ToolCallStatus::InProgress);
        assert_eq!(tools[1].1.status, ToolCallStatus::Failed);
        assert_eq!(
            tools[0].0.event_id,
            tool_event_id(intent_id, TOOL_STARTED_EVENT_DISCRIMINATOR)
        );
        assert_eq!(
            tools[1].0.event_id,
            tool_event_id(intent_id, TOOL_UPDATED_EVENT_DISCRIMINATOR)
        );
        assert!(tools.iter().all(|(_, call)| {
            call.arguments.is_none() && call.output.is_none() && call.diff.is_none()
        }));
        let encoded = serde_json::to_string(&tools).expect("encode safe projection");
        assert!(!encoded.contains("argument-secret"));
        assert!(!encoded.contains("attempt-secret"));
        assert!(!encoded.contains("handler-secret"));

        // Refused/malformed model calls and slash commands never create an
        // effect intent, so another backfill cannot fabricate a projection.
        restarted
            .backfill_tool_calls(
                interaction.conversation_id,
                interaction_turn_id,
                RunId::new(),
            )
            .await
            .expect("empty durable effect set");
        assert_eq!(
            restarted
                .store
                .load_events(interaction.conversation_id, 0, 100)
                .await
                .expect("reload events")
                .iter()
                .filter(|event| matches!(
                    event.event,
                    InteractionEvent::ToolCallStarted { .. }
                        | InteractionEvent::ToolCallUpdated { .. }
                ))
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn restart_completed_without_durable_output_fails_closed() {
        let pool = test_pool();
        let (agent_id, spec) = seed_agent(&pool);
        let (app, service, _bus) = test_service(&pool, spec, false);
        let conversation_id = ConversationId::new();
        let store = seed_interaction(&pool, agent_id, conversation_id).await;
        let run_id = RunId::new();
        RunStore::create_correlated(
            &pool,
            run_id,
            &agent_id.to_string(),
            Some(&conversation_id.to_string()),
            RunStatus::new("created"),
        )
        .await
        .expect("prepare run");
        let turn_id = InteractionTurnId::new();
        store
            .create_turn(NewInteractionTurn {
                turn_id,
                conversation_id,
                ordinal: 1,
                target: InteractionTarget::Agent(agent_id),
                config: InteractionConfig::new(InteractionTarget::Agent(agent_id)),
                user_message_id: Uuid::now_v7(),
                user_message_text: "prompt".to_owned(),
                runs: vec![InteractionRunLink {
                    run_id,
                    role: RunRole::Primary,
                    ordinal: 1,
                }],
                initial_event_id: InteractionEventId::new(),
                started_at: Utc::now(),
            })
            .await
            .expect("seed turn");
        RunStore::update_state(&pool, run_id, RunStatus::new("completed"))
            .await
            .expect("seed completed state");
        app.event_recorder()
            .record(RunEvent::new_durable(
                EventId::new(),
                run_id,
                0,
                EventKind::RunCompleted {
                    output_artifact_id: None,
                    input_tokens: 1,
                    output_tokens: 2,
                },
                EventCorrelation {
                    run_id,
                    ..EventCorrelation::default()
                },
            ))
            .await
            .expect("seed durable completion");
        assert_eq!(service.recover().await.expect("recover completion"), 1);
        let turn = store.load_turn(turn_id).await.expect("load recovered turn");
        assert_eq!(turn.summary.state, TurnState::Failed);
        assert_eq!(
            turn.error.expect("truthful recovery error").message,
            UNRECOVERABLE_OUTPUT_REASON
        );
    }

    #[tokio::test]
    async fn uncomposed_config_and_approval_capabilities_fail_explicitly() {
        let pool = test_pool();
        let (agent_id, spec) = seed_agent(&pool);
        let (_app, service, _bus) = test_service(&pool, spec, false);
        let mut unsupported = InteractionConfig::new(InteractionTarget::Agent(agent_id));
        unsupported.provider = Some("not-wired".to_owned());
        let error = service
            .new_interaction(CreateInteractionRequest {
                title: None,
                config: unsupported,
                client_context: ClientContext::new(PathBuf::from("/tmp")).expect("client context"),
            })
            .await
            .expect_err("provider config is not silently ignored");
        assert_eq!(error.code, InteractionErrorCode::Unsupported);

        let interaction = service
            .new_interaction(CreateInteractionRequest {
                title: None,
                config: InteractionConfig::new(InteractionTarget::Agent(agent_id)),
                client_context: ClientContext::new(PathBuf::from("/tmp")).expect("client context"),
            })
            .await
            .expect("create supported interaction");
        let error = service
            .set_config_option(
                interaction.conversation_id,
                ConfigUpdate {
                    option: ConfigOption::Model,
                    value: ConfigOptionValue::Model(Some("ignored".to_owned())),
                },
            )
            .await
            .expect_err("model update is not silently accepted");
        assert_eq!(error.code, InteractionErrorCode::Unsupported);
        assert_eq!(
            service
                .approve(interaction.conversation_id, ApprovalId::new())
                .await
                .expect_err("approval identity is not wired")
                .code,
            InteractionErrorCode::Unavailable
        );
        assert_eq!(
            service
                .deny(interaction.conversation_id, ApprovalId::new(), None)
                .await
                .expect_err("denial identity is not wired")
                .code,
            InteractionErrorCode::Unavailable
        );
    }
}
