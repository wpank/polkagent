//! Run lifecycle endpoints.
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `POST` | `/agents/:agent_id/runs` | [`create_run`] |
//! | `GET` | `/runs` | [`list_runs`] |
//! | `GET` | `/runs/:id` | [`get_run`] |
//! | `POST` | `/runs/:id/cancel` | [`cancel_run`] |

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
        CreateRunRequest, CursorInfo, ListRunsQuery, ListRunsResponse, PageMeta, RunResponse,
        API_VERSION,
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
