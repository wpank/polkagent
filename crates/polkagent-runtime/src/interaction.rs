//! Durable headless interaction service over one shared runtime.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use polkagent_artifact::ArtifactStore;
use polkagent_conversation::{
    types::{Conversation, MessageContent, MessageRole},
    ConversationStore,
};
use polkagent_core::event::{EventCorrelation, EventKind, RunEvent};
use polkagent_core::{AgentId, ConversationId, EventId, RunId, RunState};
use polkagent_interaction::{
    BoxInteractionEventStream, ConfigOptionValue, ConfigUpdate, CreateInteractionRequest,
    InteractionConfig, InteractionContent, InteractionError, InteractionErrorCode,
    InteractionEvent, InteractionEventHub, InteractionEventId, InteractionOverrides,
    InteractionRunLink, InteractionService, InteractionState, InteractionStore, InteractionSummary,
    InteractionTarget, InteractionTurnId, ListInteractionsRequest, NewAssistantMessage,
    NewInteraction, NewInteractionEvent, NewInteractionTurn, OverrideValue, PromptRequest, RunRole,
    StartedTurn, SubscriptionRequest, TurnResult, TurnSummary, UsageView,
};
use polkagent_service::AppService;
use polkagent_store_sqlite::{SqliteInteractionStore, SqlitePool, SqliteRunStore};
use polkagent_store_trait::event::{EventStore, StoredEvent};
use polkagent_store_trait::RunStore;
use uuid::Uuid;

const INTERACTION_STREAM_CAPACITY: usize = 256;
const USER_MESSAGE_DISCRIMINATOR: u8 = 0x31;
const ASSISTANT_MESSAGE_DISCRIMINATOR: u8 = 0x52;
const INITIAL_EVENT_DISCRIMINATOR: u8 = 0x73;
const TERMINAL_EVENT_DISCRIMINATOR: u8 = 0x94;
const RUN_DISCRIMINATOR: u8 = 0xb5;
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
                .map_err(|error| store_error("resolve interaction agent", &error))?,
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
            .prepare_interaction_run(run_id, input.agent_id, input.conversation_id)
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
        if let Err(error) = self.app.execute_prepared_run(prepared, &input.prompt).await {
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

    /// Rebuild durable lifecycle projections for one linked run. Returns
    /// `true` when a terminal event was found and projected.
    async fn backfill_run(
        &self,
        conversation_id: ConversationId,
        turn_id: InteractionTurnId,
        run_id: RunId,
        output_complete: bool,
    ) -> Result<bool, InteractionError> {
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
        if !request.client_context.working_directory.is_absolute() {
            return Err(InteractionError::invalid_request(
                "working_directory must be absolute",
            ));
        }
        let agent_id = self.resolve_target(&request.config.target)?;
        let conversation_id = ConversationId::new();
        let mut conversation = Conversation::new(conversation_id, agent_id);
        conversation.title = request.title;
        let created_at = conversation.created_at;
        ConversationStore::create(&self.pool, conversation)
            .await
            .map_err(|error| conversation_error("create interaction transcript", &error))?;

        let mut config = request.config;
        config.target = InteractionTarget::Agent(agent_id);
        match self
            .store
            .create_interaction(NewInteraction {
                conversation_id,
                config,
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
        if !request.client_context.working_directory.is_absolute() {
            return Err(InteractionError::invalid_request(
                "working_directory must be absolute",
            ));
        }
        let prompt = prompt_text(&request.content)?;
        let interaction = self.store.load_interaction(request.conversation_id).await?;
        if interaction.state != InteractionState::Active {
            return Err(InteractionError::new(
                InteractionErrorCode::Conflict,
                "archived interactions cannot accept prompts",
            ));
        }
        let mut config = request.config_overrides.apply_to(&interaction.config)?;
        let agent_id = self.resolve_target(&config.target)?;
        config.target = InteractionTarget::Agent(agent_id);

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
        self.start_new_turn(NewTurnStart {
            conversation_id: request.conversation_id,
            turn_id,
            ordinal,
            agent_id,
            config,
            prompt,
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
        let ConfigOptionValue::Target(target) = update.value else {
            return Err(unsupported_config_error());
        };
        let agent_id = self.resolve_target(&target)?;
        config.target = InteractionTarget::Agent(agent_id);
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
    if config.model.is_some()
        || config.provider.is_some()
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
    if !matches!(overrides.model, OverrideValue::Inherit)
        || !matches!(overrides.provider, OverrideValue::Inherit)
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
        "the shared runtime currently composes only interaction target configuration",
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

fn conversation_error(context: &str, error: &impl std::fmt::Display) -> InteractionError {
    tracing::error!(%error, context, "durable interaction transcript operation failed");
    internal_error(&format!("{context}: durable transcript operation failed"))
}

fn service_error(context: &str, error: &impl std::fmt::Display) -> InteractionError {
    tracing::error!(%error, context, "interaction runtime operation failed");
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

    use polkagent_config::Config;
    use polkagent_conversation::{types::Conversation, ConversationStore};
    use polkagent_core::{AgentSpec, ApprovalId, EventId};
    use polkagent_event::{EventBus, EventRecorder};
    use polkagent_executor_fake::FakeExecutor;
    use polkagent_interaction::{
        ClientContext, ConfigOption, CreateInteractionRequest, InteractionOverrides,
        InteractionTurnId, TurnState,
    };
    use polkagent_store_sqlite::migrations;
    use polkagent_store_trait::{RunStatus, StoreError};

    use super::*;

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
                created_at: Utc::now(),
            })
            .await
            .expect("seed interaction");
        store
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
                             (conversation_id, config_json, state, created_at, updated_at)
                         VALUES (?1, ?2, 'active', ?3, ?3)",
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
