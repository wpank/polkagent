//! Run lifecycle endpoints.
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `POST` | `/agents/:agent_id/runs` | [`create_run`] |
//! | `GET` | `/runs` | [`list_runs`] |
//! | `GET` | `/runs/:id` | [`get_run`] |
//! | `POST` | `/runs/:id/cancel` | [`cancel_run`] |
//! | `GET` | `/runs/:id/turns` | [`list_run_turns`] |
//! | `GET` | `/runs/:id/events` | [`list_run_events`] |
//! | `GET` | `/runs/:id/artifacts` | [`list_run_artifacts`] |
//! | `GET` | `/runs/:id/effects` | [`list_run_effects`] |
//! | `POST` | `/runs/:id/resume` | [`resume_run`] |

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use polkagent_core::{AgentId, RunId};
use tracing::{info, instrument};

use crate::run::{ListRunsParams, RunFilter};

use crate::{
    dto::{
        AgentLifecycleResponse, CreateRunRequest, CursorInfo, ListRunsQuery, ListRunsResponse,
        PageMeta, RunResponse, API_VERSION,
    },
    error::ApiError,
    state::AppState,
};

// ---------------------------------------------------------------------------
// POST /agents/:agent_id/runs
// ---------------------------------------------------------------------------

/// Create and immediately start a run for the specified agent.
///
/// Returns 201 Created with the full `RunResponse` on success.
/// Returns 404 if the agent does not exist.
/// Returns 422 if the request body fails validation.
#[instrument(skip(state, body), fields(agent_id = %agent_id))]
pub async fn create_run(
    State(state): State<AppState>,
    Path(agent_id): Path<AgentId>,
    Json(body): Json<CreateRunRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // Verify the agent exists.
    if state.agents.get(agent_id).await.is_none() {
        return Err(ApiError::AgentNotFound(agent_id.to_string()));
    }

    let record = state
        .run_manager
        .create_run(agent_id, body.input)
        .await
        .map_err(ApiError::from)?;

    info!(run_id = %record.id, agent_id = %agent_id, "run created");

    let response: RunResponse = record.into();
    Ok((StatusCode::CREATED, Json(response)))
}

// ---------------------------------------------------------------------------
// GET /runs
// ---------------------------------------------------------------------------

/// List runs with optional filters and cursor-based pagination.
///
/// Query parameters:
/// - `agent_id`: restrict to a specific agent.
/// - `state`: restrict to a specific run state.
/// - `limit`: page size (default 50, max 100).
/// - `after`: opaque cursor from a previous response.
pub async fn list_runs(
    State(state): State<AppState>,
    Query(query): Query<ListRunsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let limit = query.limit.unwrap_or(50).min(100);

    let params = ListRunsParams {
        after: query.after,
        limit,
        filter: RunFilter {
            agent_id: query.agent_id,
            state: query.state,
        },
    };

    let (records, has_more) = state
        .run_manager
        .list_runs(params)
        .await
        .map_err(ApiError::from)?;

    let next_cursor = if has_more {
        records.last().map(|r| r.id.to_string())
    } else {
        None
    };

    let page_size = records.len();
    let data: Vec<RunResponse> = records.into_iter().map(RunResponse::from).collect();

    Ok(Json(ListRunsResponse {
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
// GET /runs/:id
// ---------------------------------------------------------------------------

/// Get detailed information about a single run.
///
/// Returns 404 if no run with the given ID exists.
pub async fn get_run(
    State(state): State<AppState>,
    Path(id): Path<RunId>,
) -> Result<impl IntoResponse, ApiError> {
    let record = state
        .run_manager
        .get_run(id)
        .await
        .map_err(ApiError::from)?;

    let response: RunResponse = record.into();
    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// POST /runs/:id/cancel
// ---------------------------------------------------------------------------

/// Request cancellation of a run.
///
/// Returns 200 with the updated `RunResponse` on success.
/// Returns 404 if the run does not exist.
/// Returns 409 if the run is already in a terminal state.
#[instrument(skip(state), fields(run_id = %id))]
pub async fn cancel_run(
    State(state): State<AppState>,
    Path(id): Path<RunId>,
) -> Result<impl IntoResponse, ApiError> {
    let record = state
        .run_manager
        .cancel_run(id)
        .await
        .map_err(ApiError::from)?;

    info!(run_id = %id, "run cancelled");

    let response: RunResponse = record.into();
    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// GET /runs/:id/turns
// ---------------------------------------------------------------------------

/// List turns for a run.
///
/// **Stub:** Returns an empty list. Will be populated once the turn store
/// is integrated.
#[instrument(skip(state), fields(run_id = %id))]
pub async fn list_run_turns(
    State(state): State<AppState>,
    Path(id): Path<RunId>,
) -> Result<impl IntoResponse, ApiError> {
    // Verify the run exists.
    let _record = state
        .run_manager
        .get_run(id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(crate::dto::ListTurnsResponse {
        version: API_VERSION.to_owned(),
        data: vec![],
    }))
}

// ---------------------------------------------------------------------------
// GET /runs/:id/events
// ---------------------------------------------------------------------------

/// List events for a run from the durable event store.
///
/// Returns 501 if no event store is configured.
#[instrument(skip(state), fields(run_id = %id))]
pub async fn list_run_events(
    State(state): State<AppState>,
    Path(id): Path<RunId>,
) -> Result<impl IntoResponse, ApiError> {
    let event_store = state
        .event_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("event store not configured".to_owned()))?;

    // Verify the run exists.
    let _record = state
        .run_manager
        .get_run(id)
        .await
        .map_err(ApiError::from)?;

    let events = event_store
        .read_run_events(id)
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    let data: Vec<serde_json::Value> = events
        .into_iter()
        .map(|evt| serde_json::to_value(&evt).unwrap_or(serde_json::Value::Null))
        .collect();

    Ok(Json(serde_json::json!({
        "version": API_VERSION,
        "data": data,
    })))
}

// ---------------------------------------------------------------------------
// GET /runs/:id/artifacts
// ---------------------------------------------------------------------------

/// List artifacts produced by a run.
///
/// Returns 501 if no artifact store is configured.
#[instrument(skip(state), fields(run_id = %id))]
pub async fn list_run_artifacts(
    State(state): State<AppState>,
    Path(id): Path<RunId>,
) -> Result<impl IntoResponse, ApiError> {
    let artifact_store = state
        .artifact_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("artifact store not configured".to_owned()))?;

    // Verify the run exists.
    let _record = state
        .run_manager
        .get_run(id)
        .await
        .map_err(ApiError::from)?;

    let summaries = artifact_store
        .list_for_run(id)
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    let data: Vec<crate::dto::ArtifactResponse> = summaries
        .into_iter()
        .map(|s| crate::dto::ArtifactResponse {
            version: API_VERSION.to_owned(),
            id: s.id.to_string(),
            kind: s.kind,
            algorithm: s.algorithm,
            digest_hex: s.digest_hex,
            classification: s.classification,
            run_id: s.run_id,
            created_at: s.created_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(crate::dto::ListArtifactsResponse {
        version: API_VERSION.to_owned(),
        data,
    }))
}

// ---------------------------------------------------------------------------
// GET /runs/:id/effects  (alias for the effects module endpoint)
// ---------------------------------------------------------------------------

/// List effects for a run (alias for `GET /runs/:run_id/effects`).
///
/// Delegates to the same underlying effect store query.
#[instrument(skip(state), fields(run_id = %id))]
pub async fn list_run_effects(
    State(state): State<AppState>,
    Path(id): Path<RunId>,
) -> Result<impl IntoResponse, ApiError> {
    let intents = state
        .effect_store
        .get_by_run(id)
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    let data: Vec<serde_json::Value> = intents
        .into_iter()
        .map(|intent| serde_json::to_value(&intent).unwrap_or(serde_json::Value::Null))
        .collect();

    Ok(Json(serde_json::json!({
        "version": API_VERSION,
        "data": data,
    })))
}

// ---------------------------------------------------------------------------
// POST /runs/:id/resume
// ---------------------------------------------------------------------------

/// Resume a paused run.
///
/// Attempts to transition the run from `Paused` → `Running`. Returns 501 if
/// the run manager does not support pause/resume, or 409 if the run is not
/// in a resumable state.
#[instrument(skip(state), fields(run_id = %id))]
pub async fn resume_run(
    State(state): State<AppState>,
    Path(id): Path<RunId>,
) -> Result<impl IntoResponse, ApiError> {
    // Verify the run exists first; return 404 if not.
    let record = state
        .run_manager
        .get_run(id)
        .await
        .map_err(ApiError::from)?;

    // The current RunManagerTrait does not expose a resume operation.
    // Return a descriptive 501 that includes the current state for clarity.
    let state_name = format!("{:?}", record.state);
    Err::<Json<()>, _>(ApiError::NotImplemented(format!(
        "resume run {id} (current state: {state_name}) is not yet implemented"
    )))
}

// ---------------------------------------------------------------------------
// POST /agents/:id/start
// ---------------------------------------------------------------------------

/// Start an agent (transition to active/running state).
///
/// **Stub:** Returns 501 Not Implemented. Will be wired to the agent
/// lifecycle subsystem once long-running agent processes are supported.
#[instrument(skip(_state), fields(agent_id = %id))]
pub async fn start_agent(
    State(_state): State<AppState>,
    Path(id): Path<AgentId>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(AgentLifecycleResponse {
        version: API_VERSION.to_owned(),
        agent_id: id.to_string(),
        action: "start".to_owned(),
        status: "not_implemented".to_owned(),
    }))
}

// ---------------------------------------------------------------------------
// POST /agents/:id/stop
// ---------------------------------------------------------------------------

/// Stop a running agent.
///
/// **Stub:** Returns a lifecycle response. Will be wired to the agent
/// lifecycle subsystem once long-running agent processes are supported.
#[instrument(skip(state), fields(agent_id = %id))]
pub async fn stop_agent(
    State(state): State<AppState>,
    Path(id): Path<AgentId>,
) -> Result<impl IntoResponse, ApiError> {
    // Verify the agent exists.
    state
        .agents
        .get(id)
        .await
        .ok_or_else(|| ApiError::AgentNotFound(id.to_string()))?;

    info!(agent_id = %id, "agent stop requested");
    Ok(Json(AgentLifecycleResponse {
        version: API_VERSION.to_owned(),
        agent_id: id.to_string(),
        action: "stop".to_owned(),
        status: "not_implemented".to_owned(),
    }))
}

// ---------------------------------------------------------------------------
// POST /agents/:id/pause
// ---------------------------------------------------------------------------

/// Pause a running agent.
///
/// **Stub:** Returns a lifecycle response. Will be wired to the agent
/// lifecycle subsystem once pause/resume is supported.
#[instrument(skip(state), fields(agent_id = %id))]
pub async fn pause_agent(
    State(state): State<AppState>,
    Path(id): Path<AgentId>,
) -> Result<impl IntoResponse, ApiError> {
    // Verify the agent exists.
    state
        .agents
        .get(id)
        .await
        .ok_or_else(|| ApiError::AgentNotFound(id.to_string()))?;

    info!(agent_id = %id, "agent pause requested");
    Ok(Json(AgentLifecycleResponse {
        version: API_VERSION.to_owned(),
        agent_id: id.to_string(),
        action: "pause".to_owned(),
        status: "not_implemented".to_owned(),
    }))
}

// ---------------------------------------------------------------------------
// POST /agents/:id/resume
// ---------------------------------------------------------------------------

/// Resume a paused agent.
///
/// **Stub:** Returns a lifecycle response. Will be wired to the agent
/// lifecycle subsystem once pause/resume is supported.
#[instrument(skip(state), fields(agent_id = %id))]
pub async fn resume_agent(
    State(state): State<AppState>,
    Path(id): Path<AgentId>,
) -> Result<impl IntoResponse, ApiError> {
    // Verify the agent exists.
    state
        .agents
        .get(id)
        .await
        .ok_or_else(|| ApiError::AgentNotFound(id.to_string()))?;

    info!(agent_id = %id, "agent resume requested");
    Ok(Json(AgentLifecycleResponse {
        version: API_VERSION.to_owned(),
        agent_id: id.to_string(),
        action: "resume".to_owned(),
        status: "not_implemented".to_owned(),
    }))
}
