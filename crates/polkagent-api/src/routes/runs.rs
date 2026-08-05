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
    if state
        .agents
        .get(agent_id)
        .await
        .map_err(ApiError::from)?
        .is_none()
    {
        return Err(ApiError::AgentNotFound(agent_id.to_string()));
    }

    // Increment the active gauge before the run_manager call so that if
    // we're fast-completing in-process the gauge is never below the true
    // active count.
    state.metrics.increment_active_runs();

    let result = state
        .run_manager
        .create_run(agent_id, body.input)
        .await
        .map_err(ApiError::from);

    match &result {
        Ok(record) => {
            // The run is now in Running state; record the start metric.
            state.metrics.runs_started();
            info!(run_id = %record.id, agent_id = %agent_id, "run created");
        }
        Err(_) => {
            state.metrics.runs_failed();
        }
    }

    // The gauge will be decremented when the run reaches a terminal state
    // via the event bus. Decrement here only on create failure.
    if result.is_err() {
        state.metrics.decrement_active_runs();
    }

    let record = result?;
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

/// List turns for a run, ordered by sequence ascending.
///
/// Returns 404 if the run does not exist.
#[instrument(skip(state), fields(run_id = %id))]
pub async fn list_run_turns(
    State(state): State<AppState>,
    Path(id): Path<RunId>,
) -> Result<impl IntoResponse, ApiError> {
    let turns = state
        .run_manager
        .list_turns(id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(crate::dto::ListTurnsResponse {
        version: API_VERSION.to_owned(),
        data: turns,
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
        .map_err(|error| crate::routes::artifacts::map_artifact_store_error(error, id))?;

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
/// Attempts to transition the run from `AwaitingApproval` → `Running`.
/// Returns 404 if the run does not exist, or 409 if the run is not in
/// the `AwaitingApproval` state.
#[instrument(skip(state), fields(run_id = %id))]
pub async fn resume_run(
    State(state): State<AppState>,
    Path(id): Path<RunId>,
) -> Result<impl IntoResponse, ApiError> {
    let record = state
        .run_manager
        .resume_run(id)
        .await
        .map_err(ApiError::from)?;

    info!(run_id = %id, "run resumed");

    let response: RunResponse = record.into();
    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// POST /agents/:id/start
// ---------------------------------------------------------------------------

/// Start an agent by creating a new run.
///
/// Creates a run for the agent in `Running` state and returns an
/// `accepted` lifecycle response. Returns 404 if the agent does not exist.
#[instrument(skip(state), fields(agent_id = %id))]
pub async fn start_agent(
    State(state): State<AppState>,
    Path(id): Path<AgentId>,
) -> Result<impl IntoResponse, ApiError> {
    // Verify the agent exists.
    state
        .agents
        .get(id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::AgentNotFound(id.to_string()))?;

    state
        .run_manager
        .create_run(id, serde_json::Value::Null)
        .await
        .map_err(ApiError::from)?;

    info!(agent_id = %id, "agent started");
    Ok(Json(AgentLifecycleResponse {
        version: API_VERSION.to_owned(),
        agent_id: id.to_string(),
        action: "start".to_owned(),
        status: "accepted".to_owned(),
    }))
}

// ---------------------------------------------------------------------------
// POST /agents/:id/stop
// ---------------------------------------------------------------------------

/// Stop a running agent by cancelling all its active runs.
///
/// Transitions every non-terminal run belonging to the agent to `Cancelled`.
/// Returns 404 if the agent does not exist.
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
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::AgentNotFound(id.to_string()))?;

    let cancelled = state
        .run_manager
        .stop_agent_runs(id)
        .await
        .map_err(ApiError::from)?;

    info!(agent_id = %id, cancelled, "agent stopped");
    Ok(Json(AgentLifecycleResponse {
        version: API_VERSION.to_owned(),
        agent_id: id.to_string(),
        action: "stop".to_owned(),
        status: format!("stopped ({cancelled} runs cancelled)"),
    }))
}

// ---------------------------------------------------------------------------
// POST /agents/:id/pause
// ---------------------------------------------------------------------------

/// Pause a running agent by pausing all its running runs.
///
/// Transitions every `Running` run belonging to the agent to
/// `AwaitingApproval`. Returns 404 if the agent does not exist.
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
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::AgentNotFound(id.to_string()))?;

    let paused = state
        .run_manager
        .pause_agent_runs(id)
        .await
        .map_err(ApiError::from)?;

    info!(agent_id = %id, paused, "agent paused");
    Ok(Json(AgentLifecycleResponse {
        version: API_VERSION.to_owned(),
        agent_id: id.to_string(),
        action: "pause".to_owned(),
        status: format!("paused ({paused} runs paused)"),
    }))
}

// ---------------------------------------------------------------------------
// POST /agents/:id/resume
// ---------------------------------------------------------------------------

/// Resume a paused agent by resuming all its paused runs.
///
/// Transitions every `AwaitingApproval` run belonging to the agent back
/// to `Running`. Returns 404 if the agent does not exist.
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
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::AgentNotFound(id.to_string()))?;

    let resumed = state
        .run_manager
        .resume_agent_runs(id)
        .await
        .map_err(ApiError::from)?;

    info!(agent_id = %id, resumed, "agent resumed");
    Ok(Json(AgentLifecycleResponse {
        version: API_VERSION.to_owned(),
        agent_id: id.to_string(),
        action: "resume".to_owned(),
        status: format!("resumed ({resumed} runs resumed)"),
    }))
}
