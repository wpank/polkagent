//! Tool registry endpoints.
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `GET` | `/tools` | [`list_tools`] |
//! | `GET` | `/tools/:tool_id` | [`get_tool`] |
//! | `GET` | `/tools/:tool_id/grants` | [`get_tool_grants`] |
//!
//! All endpoints return 501 Not Implemented when no [`ToolRegistryStore`] has
//! been configured in [`AppState`].

use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Json,
};
use tracing::instrument;

use crate::{
    dto::{ListToolsResponse, ToolGrantsResponse, ToolResponse, API_VERSION},
    error::ApiError,
    state::AppState,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn spec_to_response(spec: polkagent_tool::ToolSpec) -> ToolResponse {
    let classification = format!("{:?}", spec.output_classification);
    ToolResponse {
        version: API_VERSION.to_owned(),
        id: spec.name,
        description: spec.description,
        input_schema: spec.input_schema,
        required_grant: spec.required_grant,
        output_classification: classification,
    }
}

// ---------------------------------------------------------------------------
// GET /tools
// ---------------------------------------------------------------------------

/// List all registered tools.
///
/// Returns 501 Not Implemented when no tool registry is configured.
pub async fn list_tools(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    let registry = state
        .tool_registry
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("tool registry not configured".to_owned()))?;

    let specs = registry.list_tools().await;
    let data: Vec<ToolResponse> = specs.into_iter().map(spec_to_response).collect();

    Ok(Json(ListToolsResponse {
        version: API_VERSION.to_owned(),
        data,
    }))
}

// ---------------------------------------------------------------------------
// GET /tools/:tool_id
// ---------------------------------------------------------------------------

/// Get the definition and schema for a single tool.
///
/// Returns 501 when no registry is configured, 404 when the tool is not found.
#[instrument(skip(state), fields(tool_id = %tool_id))]
pub async fn get_tool(
    State(state): State<AppState>,
    Path(tool_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let registry = state
        .tool_registry
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("tool registry not configured".to_owned()))?;

    let spec = registry
        .get_tool(&tool_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("tool '{tool_id}'")))?;

    Ok(Json(spec_to_response(spec)))
}

// ---------------------------------------------------------------------------
// GET /tools/:tool_id/grants
// ---------------------------------------------------------------------------

/// Get the required grant for a single tool.
///
/// Returns 501 when no registry is configured, 404 when the tool is not found.
#[instrument(skip(state), fields(tool_id = %tool_id))]
pub async fn get_tool_grants(
    State(state): State<AppState>,
    Path(tool_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let registry = state
        .tool_registry
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("tool registry not configured".to_owned()))?;

    let spec = registry
        .get_tool(&tool_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("tool '{tool_id}'")))?;

    Ok(Json(ToolGrantsResponse {
        version: API_VERSION.to_owned(),
        tool_id,
        required_grant: spec.required_grant,
    }))
}
