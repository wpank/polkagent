//! Durable headless interaction endpoints.
//!
//! These routes are the HTTP agent-execution surface. The separate
//! `/conversations` routes manipulate transcript records only and never start
//! model execution.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use polkagent_core::ConversationId;
use polkagent_interaction::{
    ClientContext, ConfigOption, ConfigOptionValue, ConfigUpdate, InteractionConfig,
    InteractionContent, InteractionEventEnvelope, InteractionOverrides, InteractionService,
    InteractionStore, ListInteractionsRequest, PromptRequest,
};
use tracing::instrument;

use crate::{
    dto::{
        CancelHttpInteractionTurnResponse, CreateHttpInteractionRequest, CursorInfo,
        HttpInteractionResponse, InteractionReplayCheckpoint, ListHttpInteractionTurnsResponse,
        ListHttpInteractionsQuery, ListHttpInteractionsResponse, PageMeta,
        PromptHttpInteractionRequest, PromptHttpInteractionResponse, ReplayInteractionEventsQuery,
        ReplayInteractionEventsResponse, UpdateHttpInteractionTargetRequest,
        UpdateHttpInteractionTargetResponse, API_VERSION,
    },
    error::ApiError,
    state::AppState,
};

const DEFAULT_REPLAY_LIMIT: u32 = 100;
const MAX_REPLAY_LIMIT: u32 = 999;

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

/// Change the supported interaction target configuration.
#[instrument(skip(state, body), fields(%conversation_id))]
pub async fn update_interaction_target(
    State(state): State<AppState>,
    Path(conversation_id): Path<ConversationId>,
    Json(body): Json<UpdateHttpInteractionTargetRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let config = interaction_service(&state)?
        .set_config_option(
            conversation_id,
            ConfigUpdate {
                option: ConfigOption::Target,
                value: ConfigOptionValue::Target(body.target),
            },
        )
        .await
        .map_err(ApiError::from)?;
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
