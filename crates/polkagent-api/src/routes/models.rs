//! Model discovery endpoints.
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `GET` | `/providers/:provider_id/models` | [`list_provider_models`] |
//! | `GET` | `/models` | [`list_all_models`] |
//! | `GET` | `/models/:model_id` | [`get_model`] |
//!
//! Model metadata is derived from the configured providers in [`AppState::config`].
//! Each provider's `default_model` is exposed as the canonical model entry.
//! No external network calls are made.

use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Json,
};
use tracing::instrument;

use crate::{
    dto::{ListModelsResponse, ModelCapabilities, ModelResponse, API_VERSION},
    error::ApiError,
    state::AppState,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a [`ModelResponse`] from a provider's metadata.
fn model_response_for_provider(provider_id: &str, model_id: &str) -> ModelResponse {
    // Derive capabilities and context window from known model identifiers.
    // This is a best-effort static mapping; a real implementation would query
    // a model registry or provider capability API.
    let (context_window, capabilities) = model_capabilities(model_id);

    ModelResponse {
        version: API_VERSION.to_owned(),
        id: model_id.to_owned(),
        name: model_id.to_owned(),
        provider: provider_id.to_owned(),
        context_window,
        capabilities,
    }
}

/// Static capability/context-window lookup by model name prefix.
fn model_capabilities(model_id: &str) -> (u32, ModelCapabilities) {
    if model_id.contains("claude") {
        let context_window = if model_id.contains("opus")
            || model_id.contains("sonnet")
            || model_id.contains("haiku")
        {
            200_000
        } else {
            100_000
        };
        (
            context_window,
            ModelCapabilities {
                vision: true,
                tool_use: true,
                streaming: true,
            },
        )
    } else if model_id.contains("gpt-4") {
        (
            128_000,
            ModelCapabilities {
                vision: model_id.contains("vision") || model_id.contains("4o"),
                tool_use: true,
                streaming: true,
            },
        )
    } else if model_id.contains("gpt-3.5") {
        (
            16_385,
            ModelCapabilities {
                vision: false,
                tool_use: true,
                streaming: true,
            },
        )
    } else {
        // Unknown model: conservative defaults.
        (
            4_096,
            ModelCapabilities {
                vision: false,
                tool_use: false,
                streaming: false,
            },
        )
    }
}

// ---------------------------------------------------------------------------
// GET /providers/:provider_id/models
// ---------------------------------------------------------------------------

/// List all models available under a specific provider.
///
/// Returns 404 if the provider ID is not configured.
#[instrument(skip(state), fields(provider_id = %provider_id))]
pub async fn list_provider_models(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let provider = state
        .config
        .providers
        .iter()
        .find(|p| p.id == provider_id)
        .ok_or_else(|| ApiError::NotFound(format!("provider '{provider_id}'")))?;

    // Expose the default model as the sole known model for this provider.
    let model = model_response_for_provider(&provider.id, &provider.default_model);

    Ok(Json(ListModelsResponse {
        version: API_VERSION.to_owned(),
        data: vec![model],
    }))
}

// ---------------------------------------------------------------------------
// GET /models
// ---------------------------------------------------------------------------

/// List all models across every configured provider.
///
/// Each provider contributes its `default_model` to the list.
pub async fn list_all_models(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let data: Vec<ModelResponse> = state
        .config
        .providers
        .iter()
        .map(|p| model_response_for_provider(&p.id, &p.default_model))
        .collect();

    Ok(Json(ListModelsResponse {
        version: API_VERSION.to_owned(),
        data,
    }))
}

// ---------------------------------------------------------------------------
// GET /models/:model_id
// ---------------------------------------------------------------------------

/// Get metadata for a single model by its identifier.
///
/// Searches all configured providers for a matching `default_model`.
/// Returns 404 if no provider exposes a model with the given ID.
#[instrument(skip(state), fields(model_id = %model_id))]
pub async fn get_model(
    State(state): State<AppState>,
    Path(model_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let entry = state
        .config
        .providers
        .iter()
        .find(|p| p.default_model == model_id)
        .ok_or_else(|| ApiError::NotFound(format!("model '{model_id}'")))?;

    Ok(Json(model_response_for_provider(&entry.id, &model_id)))
}
