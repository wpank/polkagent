//! Effect pipeline REST endpoints.
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `GET` | `/runs/:run_id/effects` | [`list_effects`] |
//! | `GET` | `/effects/:id` | [`get_effect`] |
//! | `POST` | `/effects/:id/approve` | [`approve_effect`] |
//! | `POST` | `/effects/:id/deny` | [`deny_effect`] |

use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Json,
};
use polkagent_core::{EffectId, RunId};
use serde::Serialize;
use tracing::instrument;

use crate::{
    dto::API_VERSION,
    error::ApiError,
    state::AppState,
};

// ---------------------------------------------------------------------------
// Response DTOs
// ---------------------------------------------------------------------------

/// Response body for a single stored effect intent.
#[derive(Debug, Clone, Serialize)]
pub struct EffectResponse {
    /// API version.
    pub version: String,
    /// The stored intent serialised from the effect store.
    pub data: serde_json::Value,
}

/// Response body for a list of stored effect intents.
#[derive(Debug, Clone, Serialize)]
pub struct ListEffectsResponse {
    /// API version.
    pub version: String,
    /// List of stored intents.
    pub data: Vec<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// GET /runs/:run_id/effects
// ---------------------------------------------------------------------------

/// List all effect intents associated with a run.
///
/// Returns the raw stored intent payloads from the effect store.
#[instrument(skip(state), fields(run_id = %run_id))]
pub async fn list_effects(
    State(state): State<AppState>,
    Path(run_id): Path<RunId>,
) -> Result<impl IntoResponse, ApiError> {
    let intents = state
        .effect_store
        .get_by_run(run_id)
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    let data: Vec<serde_json::Value> = intents
        .into_iter()
        .map(|intent| {
            serde_json::to_value(&intent)
                .unwrap_or_else(|_| serde_json::Value::Null)
        })
        .collect();

    Ok(Json(ListEffectsResponse {
        version: API_VERSION.to_owned(),
        data,
    }))
}

// ---------------------------------------------------------------------------
// GET /effects/:id
// ---------------------------------------------------------------------------

/// Get detailed information about a single effect intent.
///
/// Returns 404 if no intent with the given ID exists.
#[instrument(skip(state), fields(effect_id = %id))]
pub async fn get_effect(
    State(state): State<AppState>,
    Path(id): Path<EffectId>,
) -> Result<impl IntoResponse, ApiError> {
    let intent = state
        .effect_store
        .get_intent(id)
        .await
        .map_err(|e| match e {
            polkagent_store_trait::StoreError::NotFound { .. } => {
                ApiError::NotFound(format!("effect intent {id}"))
            }
            other => ApiError::InternalError(other.to_string()),
        })?;

    let data = serde_json::to_value(&intent)
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    Ok(Json(EffectResponse {
        version: API_VERSION.to_owned(),
        data,
    }))
}

// ---------------------------------------------------------------------------
// POST /effects/:id/approve
// ---------------------------------------------------------------------------

/// Approve a pending effect intent.
///
/// **Stub:** Returns 501 Not Implemented. Will be wired to the approval
/// subsystem once it is available.
#[instrument(skip(_state), fields(effect_id = %id))]
pub async fn approve_effect(
    State(_state): State<AppState>,
    Path(id): Path<EffectId>,
) -> Result<impl IntoResponse, ApiError> {
    Err::<Json<()>, _>(ApiError::NotImplemented(format!(
        "approve effect {id} is not yet implemented"
    )))
}

// ---------------------------------------------------------------------------
// POST /effects/:id/deny
// ---------------------------------------------------------------------------

/// Deny a pending effect intent.
///
/// **Stub:** Returns 501 Not Implemented. Will be wired to the approval
/// subsystem once it is available.
#[instrument(skip(_state), fields(effect_id = %id))]
pub async fn deny_effect(
    State(_state): State<AppState>,
    Path(id): Path<EffectId>,
) -> Result<impl IntoResponse, ApiError> {
    Err::<Json<()>, _>(ApiError::NotImplemented(format!(
        "deny effect {id} is not yet implemented"
    )))
}
