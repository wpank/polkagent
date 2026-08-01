//! Agent CRUD endpoints.
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `POST` | `/agents` | [`create_agent`] |
//! | `GET` | `/agents` | [`list_agents`] |
//! | `GET` | `/agents/:id` | [`get_agent`] |
//! | `DELETE` | `/agents/:id` | [`delete_agent`] |

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::Utc;
use polkagent_core::{AgentId, agent::AgentSpec};
use tracing::{info, instrument};

use crate::{
    dto::{
        AgentResponse, CreateAgentRequest, CursorInfo, ListAgentsQuery, ListAgentsResponse,
        PageMeta, API_VERSION,
    },
    error::ApiError,
    state::AppState,
};

// ---------------------------------------------------------------------------
// POST /agents
// ---------------------------------------------------------------------------

/// Create a new agent from the request body.
///
/// The spec is stored in the in-process agent registry. Returns 201 Created
/// with the full `AgentResponse` on success.
#[instrument(skip(state, body), fields(name = %body.name))]
pub async fn create_agent(
    State(state): State<AppState>,
    Json(body): Json<CreateAgentRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // Validate required fields.
    if body.name.trim().is_empty() {
        return Err(ApiError::ValidationError(
            "name must not be empty".to_owned(),
        ));
    }
    if body.model.trim().is_empty() {
        return Err(ApiError::ValidationError(
            "model must not be empty".to_owned(),
        ));
    }

    let id = AgentId::new();
    let now = Utc::now();
    let mut spec = AgentSpec::new(id, body.name.clone(), body.model.clone());
    spec.description = body.description;
    spec.tools = body.tools;
    spec.system_prompt = body.system_prompt;
    spec.created_at = now;
    spec.updated_at = now;

    // PRD-03 fields — wire through from request, using defaults when absent.
    spec.declared_capabilities = body.declared_capabilities;
    spec.policy_refs = body.policy_refs;
    spec.resource_limits = body.resource_limits;
    spec.model_preference = body.model_preference;
    spec.surface_bindings = body.surface_bindings;

    state.agents.insert(spec.clone()).await;

    info!(agent_id = %id, name = %body.name, "agent created");

    let response = AgentResponse::from_spec(spec);
    Ok((StatusCode::CREATED, Json(response)))
}

// ---------------------------------------------------------------------------
// GET /agents
// ---------------------------------------------------------------------------

/// List all agents with cursor-based pagination.
///
/// Query parameters: `after` (cursor), `limit` (default 50, max 100).
pub async fn list_agents(
    State(state): State<AppState>,
    Query(query): Query<ListAgentsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let limit = query.limit.unwrap_or(50).min(100) as usize;
    let after = query.after;

    let (specs, has_more) = state.agents.list_page(after, limit).await;

    let next_cursor = if has_more {
        specs.last().map(|s| s.id.to_string())
    } else {
        None
    };

    let page_size = specs.len();
    let data: Vec<AgentResponse> = specs.into_iter().map(AgentResponse::from_spec).collect();

    Ok(Json(ListAgentsResponse {
        version: API_VERSION.to_owned(),
        data,
        cursor: CursorInfo {
            next: next_cursor,
            has_more,
        },
        meta: PageMeta { page_size },
    }))
}

// ---------------------------------------------------------------------------
// GET /agents/:id
// ---------------------------------------------------------------------------

/// Get a single agent by its UUID.
///
/// Returns 404 if no agent with the given ID exists.
pub async fn get_agent(
    State(state): State<AppState>,
    Path(id): Path<AgentId>,
) -> Result<impl IntoResponse, ApiError> {
    let spec = state
        .agents
        .get(id)
        .await
        .ok_or_else(|| ApiError::AgentNotFound(id.to_string()))?;

    Ok(Json(AgentResponse::from_spec(spec)))
}

// ---------------------------------------------------------------------------
// DELETE /agents/:id
// ---------------------------------------------------------------------------

/// Delete (archive) an agent by its UUID.
///
/// Returns 204 No Content on success, 404 if not found.
pub async fn delete_agent(
    State(state): State<AppState>,
    Path(id): Path<AgentId>,
) -> Result<impl IntoResponse, ApiError> {
    let found = state.agents.remove(id).await;
    if !found {
        return Err(ApiError::AgentNotFound(id.to_string()));
    }

    info!(agent_id = %id, "agent deleted");
    Ok(StatusCode::NO_CONTENT)
}
