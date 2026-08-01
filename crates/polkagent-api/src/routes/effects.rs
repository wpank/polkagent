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
use chrono::Utc;
use polkagent_core::{EffectId, RunId};
use polkagent_store_trait::StoreError;
use serde::Serialize;
use tracing::{info, instrument};

use crate::{
    dto::{ApproveEffectRequest, ApproveEffectResponse, DenyEffectRequest, DenyEffectResponse, API_VERSION},
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
/// - Looks up the effect intent in the store.
/// - Verifies it is in a state that permits approval (`"pending"` or
///   `"waiting_approval"`).
/// - Transitions the intent to `"approved"` via the store.
/// - Returns 200 with the updated effect state.
///
/// # Errors
///
/// - 404 if no intent with `id` exists.
/// - 409 if the intent is in a state that cannot be approved.
#[instrument(skip(state, body), fields(effect_id = %id))]
pub async fn approve_effect(
    State(state): State<AppState>,
    Path(id): Path<EffectId>,
    body: Option<Json<ApproveEffectRequest>>,
) -> Result<impl IntoResponse, ApiError> {
    let _body = body.map(|b| b.0).unwrap_or_default();

    // Look up the intent, returning 404 if it doesn't exist.
    let intent = state
        .effect_store
        .get_intent(id)
        .await
        .map_err(|e| match e {
            StoreError::NotFound { .. } => {
                ApiError::NotFound(format!("effect intent {id}"))
            }
            other => ApiError::InternalError(other.to_string()),
        })?;

    // Verify the intent is in an approvable state.
    let approvable_states = ["pending", "waiting_approval"];
    if !approvable_states.contains(&intent.state.as_str()) {
        return Err(ApiError::InvalidState(format!(
            "effect intent {id} is in state '{}' which cannot be approved; \
             only pending or waiting_approval intents may be approved",
            intent.state
        )));
    }

    // Transition to "approved" via the store.
    let updated = state
        .effect_store
        .update_intent_state(id, "approved")
        .await
        .map_err(|e| match e {
            StoreError::NotFound { .. } => ApiError::NotFound(format!("effect intent {id}")),
            StoreError::InvalidTransition { message } => ApiError::InvalidState(message),
            other => ApiError::InternalError(other.to_string()),
        })?;

    let approved_at = Utc::now();
    state.metrics.effects_approved();
    info!(effect_id = %id, "effect intent approved");

    Ok(Json(ApproveEffectResponse {
        version: API_VERSION.to_owned(),
        effect_id: updated.id.to_string(),
        new_state: updated.state.clone(),
        approved_at,
    }))
}

// ---------------------------------------------------------------------------
// POST /effects/:id/deny
// ---------------------------------------------------------------------------

/// Deny a pending effect intent.
///
/// - Looks up the effect intent in the store.
/// - Verifies it is in a state that permits denial (`"pending"` or
///   `"waiting_approval"`).
/// - Transitions the intent to `"denied"` via the store.
/// - Records the denial reason from the request body (if provided).
/// - Returns 200 with the updated effect state.
///
/// # Errors
///
/// - 404 if no intent with `id` exists.
/// - 409 if the intent is in a state that cannot be denied.
#[instrument(skip(state, body), fields(effect_id = %id))]
pub async fn deny_effect(
    State(state): State<AppState>,
    Path(id): Path<EffectId>,
    body: Option<Json<DenyEffectRequest>>,
) -> Result<impl IntoResponse, ApiError> {
    let body = body.map(|b| b.0).unwrap_or_default();
    let reason = body.reason.clone();

    // Look up the intent, returning 404 if it doesn't exist.
    let intent = state
        .effect_store
        .get_intent(id)
        .await
        .map_err(|e| match e {
            StoreError::NotFound { .. } => {
                ApiError::NotFound(format!("effect intent {id}"))
            }
            other => ApiError::InternalError(other.to_string()),
        })?;

    // Verify the intent is in a deniable state.
    let deniable_states = ["pending", "waiting_approval"];
    if !deniable_states.contains(&intent.state.as_str()) {
        return Err(ApiError::InvalidState(format!(
            "effect intent {id} is in state '{}' which cannot be denied; \
             only pending or waiting_approval intents may be denied",
            intent.state
        )));
    }

    // Transition to "denied" via the store.
    let updated = state
        .effect_store
        .update_intent_state(id, "denied")
        .await
        .map_err(|e| match e {
            StoreError::NotFound { .. } => ApiError::NotFound(format!("effect intent {id}")),
            StoreError::InvalidTransition { message } => ApiError::InvalidState(message),
            other => ApiError::InternalError(other.to_string()),
        })?;

    let denied_at = Utc::now();
    state.metrics.effects_denied();
    info!(effect_id = %id, "effect intent denied");

    Ok(Json(DenyEffectResponse {
        version: API_VERSION.to_owned(),
        effect_id: updated.id.to_string(),
        new_state: updated.state.clone(),
        reason,
        denied_at,
    }))
}
