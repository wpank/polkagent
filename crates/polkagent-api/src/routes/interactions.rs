//! Durable headless interaction endpoints.
//!
//! These routes are the HTTP agent-execution surface. The separate
//! `/conversations` routes manipulate transcript records only and never start
//! model execution.

use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::{
    extract::{rejection::JsonRejection, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{sse::Event, sse::KeepAlive, IntoResponse, Sse},
    Json,
};
use futures::Stream;
use polkagent_core::ConversationId;
use polkagent_interaction::{
    BoxInteractionEventStream, ClientContext, ConfigOption, ConfigOptionValue, ConfigUpdate,
    InteractionConfig, InteractionContent, InteractionEventEnvelope, InteractionOverrides,
    InteractionService, InteractionStore, InteractionTurnId, ListInteractionsRequest,
    PromptRequest, StreamError, SubscriptionRequest,
};
use tracing::{debug, instrument, warn};

use crate::{
    dto::{
        CancelHttpInteractionTurnResponse, CreateHttpInteractionRequest, CursorInfo,
        HttpInteractionConfigResponse, HttpInteractionResponse, InteractionReplayCheckpoint,
        ListHttpInteractionTurnsResponse, ListHttpInteractionsQuery, ListHttpInteractionsResponse,
        PageMeta, PromptHttpInteractionRequest, PromptHttpInteractionResponse,
        ReplayInteractionEventsQuery, ReplayInteractionEventsResponse,
        StreamInteractionEventsQuery, UpdateHttpInteractionConfigRequest,
        UpdateHttpInteractionTargetRequest, UpdateHttpInteractionTargetResponse, API_VERSION,
    },
    error::ApiError,
    state::AppState,
};

const DEFAULT_REPLAY_LIMIT: u32 = 100;
const MAX_REPLAY_LIMIT: u32 = 999;
const SSE_STREAM_CAPACITY: usize = 256;
const SSE_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const LAST_EVENT_ID_HEADER: &str = "last-event-id";
const INTERACTION_SSE_EVENT_NAME: &str = "interaction_event";

fn interaction_service(state: &AppState) -> Result<Arc<dyn InteractionService>, ApiError> {
    state
        .interaction_service
        .clone()
        .ok_or_else(|| ApiError::NotImplemented("interaction service not configured".to_owned()))
}

fn interaction_store(state: &AppState) -> Result<Arc<dyn InteractionStore>, ApiError> {
    state.interaction_store.clone().ok_or_else(|| {
        ApiError::NotImplemented("interaction event store not configured".to_owned())
    })
}

fn interaction_config_json_error(rejection: &JsonRejection) -> ApiError {
    ApiError::ValidationError(format!(
        "invalid interaction configuration request: {}",
        rejection.body_text()
    ))
}

async fn set_interaction_config(
    state: &AppState,
    conversation_id: ConversationId,
    update: ConfigUpdate,
) -> Result<InteractionConfig, ApiError> {
    interaction_service(state)?
        .set_config_option(conversation_id, update)
        .await
        .map_err(ApiError::from)
}

fn client_context(
    working_directory: std::path::PathBuf,
    client_name: Option<String>,
    client_session_id: Option<String>,
) -> Result<ClientContext, ApiError> {
    let mut context = ClientContext::new(working_directory).map_err(ApiError::from)?;
    context.client_name = client_name;
    context.client_session_id = client_session_id;
    Ok(context)
}

/// Create a durable interaction without starting execution.
#[instrument(skip(state, body))]
pub async fn create_interaction(
    State(state): State<AppState>,
    Json(body): Json<CreateHttpInteractionRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let service = interaction_service(&state)?;
    let context = client_context(
        body.working_directory,
        body.client_name,
        body.client_session_id,
    )?;
    let interaction = service
        .new_interaction(polkagent_interaction::CreateInteractionRequest {
            title: body.title,
            config: InteractionConfig::new(body.target),
            client_context: context,
        })
        .await
        .map_err(ApiError::from)?;
    Ok((
        StatusCode::CREATED,
        Json(HttpInteractionResponse {
            version: API_VERSION.to_owned(),
            interaction,
        }),
    ))
}

/// List durable interactions in stable recency order.
pub async fn list_interactions(
    State(state): State<AppState>,
    Query(query): Query<ListHttpInteractionsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let service = interaction_service(&state)?;
    let limit = query.limit.unwrap_or(50).clamp(1, 100);
    let offset = query.offset.unwrap_or(0);
    let mut interactions = service
        .list_interactions(ListInteractionsRequest {
            limit: limit.saturating_add(1),
            offset,
            state: query.state,
        })
        .await
        .map_err(ApiError::from)?;
    let has_more = interactions.len() > limit as usize;
    interactions.truncate(limit as usize);
    let page_size = interactions.len();
    let next = has_more.then(|| offset.saturating_add(limit).to_string());
    Ok(Json(ListHttpInteractionsResponse {
        version: API_VERSION.to_owned(),
        data: interactions,
        cursor: CursorInfo { next, has_more },
        meta: PageMeta { page_size },
    }))
}

/// Load one durable interaction projection.
pub async fn get_interaction(
    State(state): State<AppState>,
    Path(conversation_id): Path<ConversationId>,
) -> Result<impl IntoResponse, ApiError> {
    let interaction = interaction_service(&state)?
        .load_interaction(conversation_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(HttpInteractionResponse {
        version: API_VERSION.to_owned(),
        interaction,
    }))
}

/// List durable turns for one interaction.
pub async fn list_interaction_turns(
    State(state): State<AppState>,
    Path(conversation_id): Path<ConversationId>,
) -> Result<impl IntoResponse, ApiError> {
    let turns = interaction_service(&state)?
        .list_turns(conversation_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(ListHttpInteractionTurnsResponse {
        version: API_VERSION.to_owned(),
        data: turns,
    }))
}

/// Archive a terminal interaction while preserving its durable history.
#[instrument(skip(state), fields(%conversation_id))]
pub async fn archive_interaction(
    State(state): State<AppState>,
    Path(conversation_id): Path<ConversationId>,
) -> Result<impl IntoResponse, ApiError> {
    interaction_service(&state)?
        .delete_interaction(conversation_id)
        .await
        .map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Persist a user prompt, attach its event stream, and start execution.
#[instrument(skip(state, body), fields(%conversation_id))]
pub async fn prompt_interaction(
    State(state): State<AppState>,
    Path(conversation_id): Path<ConversationId>,
    Json(body): Json<PromptHttpInteractionRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let context = client_context(
        body.working_directory,
        body.client_name,
        body.client_session_id,
    )?;
    let started = interaction_service(&state)?
        .prompt(PromptRequest {
            turn_id: body.turn_id,
            conversation_id,
            content: vec![InteractionContent::Text { text: body.prompt }],
            config_overrides: InteractionOverrides::default(),
            client_context: context,
        })
        .await
        .map_err(ApiError::from)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(PromptHttpInteractionResponse {
            version: API_VERSION.to_owned(),
            handle: started.handle,
        }),
    ))
}

/// Cancel one turn after verifying that it belongs to the path interaction.
#[instrument(skip(state), fields(%conversation_id, %turn_id))]
pub async fn cancel_interaction_turn(
    State(state): State<AppState>,
    Path((conversation_id, turn_id)): Path<(
        ConversationId,
        polkagent_interaction::InteractionTurnId,
    )>,
) -> Result<impl IntoResponse, ApiError> {
    let service = interaction_service(&state)?;
    let turns = service
        .list_turns(conversation_id)
        .await
        .map_err(ApiError::from)?;
    if !turns.iter().any(|turn| turn.handle.turn_id == turn_id) {
        return Err(ApiError::NotFound(
            "interaction turn was not found".to_owned(),
        ));
    }
    service.cancel_turn(turn_id).await.map_err(ApiError::from)?;
    let turn = service
        .list_turns(conversation_id)
        .await
        .map_err(ApiError::from)?
        .into_iter()
        .find(|turn| turn.handle.turn_id == turn_id)
        .ok_or_else(|| {
            ApiError::InternalError("cancelled turn projection is missing".to_owned())
        })?;
    Ok(Json(CancelHttpInteractionTurnResponse {
        version: API_VERSION.to_owned(),
        turn,
    }))
}

/// Read the supported durable interaction configuration.
#[instrument(skip(state), fields(%conversation_id))]
pub async fn get_interaction_config(
    State(state): State<AppState>,
    Path(conversation_id): Path<ConversationId>,
) -> Result<impl IntoResponse, ApiError> {
    let config = interaction_service(&state)?
        .load_interaction(conversation_id)
        .await
        .map_err(ApiError::from)?
        .config;
    Ok(Json(HttpInteractionConfigResponse {
        version: API_VERSION.to_owned(),
        config: config.into(),
    }))
}

/// Atomically change one supported durable interaction configuration option.
#[instrument(skip(state, body), fields(%conversation_id))]
pub async fn update_interaction_config(
    State(state): State<AppState>,
    Path(conversation_id): Path<ConversationId>,
    body: Result<Json<UpdateHttpInteractionConfigRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(body) = body.map_err(|error| interaction_config_json_error(&error))?;
    let update = match body {
        UpdateHttpInteractionConfigRequest::Target(target) => ConfigUpdate {
            option: ConfigOption::Target,
            value: ConfigOptionValue::Target(target),
        },
        UpdateHttpInteractionConfigRequest::Model(model) => ConfigUpdate {
            option: ConfigOption::Model,
            value: ConfigOptionValue::Model(model),
        },
    };
    let config = set_interaction_config(&state, conversation_id, update).await?;
    Ok(Json(HttpInteractionConfigResponse {
        version: API_VERSION.to_owned(),
        config: config.into(),
    }))
}

/// Compatibility delegate for the original target-only mutation route.
#[instrument(skip(state, body), fields(%conversation_id))]
pub async fn update_interaction_target(
    State(state): State<AppState>,
    Path(conversation_id): Path<ConversationId>,
    body: Result<Json<UpdateHttpInteractionTargetRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(body) = body.map_err(|error| interaction_config_json_error(&error))?;
    let config = set_interaction_config(
        &state,
        conversation_id,
        ConfigUpdate {
            option: ConfigOption::Target,
            value: ConfigOptionValue::Target(body.target),
        },
    )
    .await?;
    Ok(Json(UpdateHttpInteractionTargetResponse {
        version: API_VERSION.to_owned(),
        config,
    }))
}

/// Return a finite page of durable interaction events after a checkpoint.
///
/// This GET-only projection reads `InteractionStore` because the shared
/// `InteractionService` does not yet expose paged replay. Every execution or
/// mutation handler above goes through the service itself.
#[instrument(skip(state), fields(%conversation_id))]
pub async fn replay_interaction_events(
    State(state): State<AppState>,
    Path(conversation_id): Path<ConversationId>,
    Query(query): Query<ReplayInteractionEventsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    interaction_service(&state)?
        .load_interaction(conversation_id)
        .await
        .map_err(ApiError::from)?;
    let limit = query
        .limit
        .unwrap_or(DEFAULT_REPLAY_LIMIT)
        .clamp(1, MAX_REPLAY_LIMIT);
    let after_sequence = query.after_sequence.unwrap_or(0);
    let (events, checkpoint, has_more) = replay_page(
        interaction_store(&state)?.as_ref(),
        conversation_id,
        query.turn_id,
        after_sequence,
        limit,
    )
    .await?;
    Ok(Json(ReplayInteractionEventsResponse {
        version: API_VERSION.to_owned(),
        data: events,
        checkpoint: InteractionReplayCheckpoint {
            next_after_sequence: checkpoint,
            has_more,
        },
    }))
}

struct InteractionSseState {
    service: Arc<dyn InteractionService>,
    events: BoxInteractionEventStream,
    conversation_id: ConversationId,
    turn_id: Option<InteractionTurnId>,
    last_delivered_sequence: u64,
}

fn requested_sse_checkpoint(
    headers: &HeaderMap,
    query_checkpoint: Option<u64>,
) -> Result<u64, ApiError> {
    let Some(last_event_id) = headers.get(LAST_EVENT_ID_HEADER) else {
        return Ok(query_checkpoint.unwrap_or(0));
    };
    let value = last_event_id.to_str().map_err(|_| {
        ApiError::ValidationError("Last-Event-ID must be an ASCII decimal sequence".to_owned())
    })?;
    value.trim().parse::<u64>().map_err(|_| {
        ApiError::ValidationError(
            "Last-Event-ID must be a non-negative decimal sequence".to_owned(),
        )
    })
}

async fn resubscribe_after_lag(state: &mut InteractionSseState) -> bool {
    // The service's lag marker may point at the durable tail. Resuming there
    // would skip events, so the HTTP adapter always replays after the last
    // sequence it actually emitted to this client.
    tokio::task::yield_now().await;
    match state
        .service
        .subscribe(SubscriptionRequest {
            conversation_id: state.conversation_id,
            turn_id: state.turn_id,
            after_sequence: Some(state.last_delivered_sequence),
            capacity: SSE_STREAM_CAPACITY,
        })
        .await
    {
        Ok(events) => {
            state.events = events;
            true
        }
        Err(error) => {
            warn!(
                error_code = %error.code,
                "interaction SSE resubscription failed"
            );
            false
        }
    }
}

fn sse_event_stream(
    state: InteractionSseState,
) -> impl Stream<Item = Result<Event, Infallible>> + Send {
    futures::stream::unfold(state, |mut state| async move {
        loop {
            match state.events.recv().await {
                Ok(envelope) => {
                    if envelope.sequence <= state.last_delivered_sequence {
                        continue;
                    }
                    if envelope.conversation_id != state.conversation_id {
                        warn!("interaction SSE received a cross-interaction event");
                        return None;
                    }
                    if state
                        .turn_id
                        .is_some_and(|turn_id| turn_id != envelope.turn_id)
                    {
                        warn!("interaction SSE service returned an event outside its turn filter");
                        continue;
                    }
                    let event = match Event::default()
                        .event(INTERACTION_SSE_EVENT_NAME)
                        .id(envelope.sequence.to_string())
                        .json_data(&envelope)
                    {
                        Ok(event) => event,
                        Err(error) => {
                            warn!(%error, "interaction SSE envelope serialization failed");
                            return None;
                        }
                    };
                    state.last_delivered_sequence = envelope.sequence;
                    return Some((Ok(event), state));
                }
                Err(StreamError::Lagged {
                    last_seen_sequence,
                    resume_after_sequence,
                }) => {
                    warn!(
                        ?last_seen_sequence,
                        resume_after_sequence,
                        delivered_sequence = state.last_delivered_sequence,
                        "interaction SSE receiver lagged; replaying from delivered checkpoint"
                    );
                    if !resubscribe_after_lag(&mut state).await {
                        return None;
                    }
                }
                Err(StreamError::Closed) => {
                    debug!("interaction SSE source closed");
                    return None;
                }
                Err(StreamError::Backend(error)) => {
                    warn!(
                        error_code = %error.code,
                        "interaction SSE backend failed"
                    );
                    return None;
                }
            }
        }
    })
}

/// Replay durable events and then follow the bounded interaction stream.
///
/// A valid `Last-Event-ID` header takes precedence over `after_sequence`.
/// Every data event is named `interaction_event`, carries a complete typed
/// [`InteractionEventEnvelope`], and uses its durable conversation sequence as
/// the SSE `id`.
#[instrument(skip(state, headers), fields(%conversation_id))]
pub async fn stream_interaction_events(
    State(state): State<AppState>,
    Path(conversation_id): Path<ConversationId>,
    Query(query): Query<StreamInteractionEventsQuery>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let after_sequence = requested_sse_checkpoint(&headers, query.after_sequence)?;
    let service = interaction_service(&state)?;
    let events = service
        .subscribe(SubscriptionRequest {
            conversation_id,
            turn_id: query.turn_id,
            after_sequence: Some(after_sequence),
            capacity: SSE_STREAM_CAPACITY,
        })
        .await
        .map_err(ApiError::from)?;
    let stream = sse_event_stream(InteractionSseState {
        service,
        events,
        conversation_id,
        turn_id: query.turn_id,
        last_delivered_sequence: after_sequence,
    });
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(SSE_KEEPALIVE_INTERVAL)
            .text("keepalive"),
    ))
}

async fn replay_page(
    store: &dyn InteractionStore,
    conversation_id: ConversationId,
    turn_id: Option<polkagent_interaction::InteractionTurnId>,
    after_sequence: u64,
    limit: u32,
) -> Result<(Vec<InteractionEventEnvelope>, u64, bool), ApiError> {
    let mut cursor = after_sequence;
    let mut events = Vec::with_capacity(limit as usize + 1);
    loop {
        let page = store
            .load_events(conversation_id, cursor, 1_000)
            .await
            .map_err(ApiError::from)?;
        if page.is_empty() {
            return Ok((events, cursor, false));
        }
        let full_page = page.len() == 1_000;
        for event in page {
            if event.sequence <= cursor {
                return Err(ApiError::InternalError(
                    "interaction event replay did not advance".to_owned(),
                ));
            }
            cursor = event.sequence;
            if turn_id.is_none_or(|filter| filter == event.turn_id) {
                events.push(event);
                if events.len() > limit as usize {
                    events.pop();
                    let checkpoint = events.last().map_or(after_sequence, |event| event.sequence);
                    return Ok((events, checkpoint, true));
                }
            }
        }
        if !full_page {
            return Ok((events, cursor, false));
        }
    }
}
