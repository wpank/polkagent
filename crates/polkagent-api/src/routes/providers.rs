//! Provider configuration endpoints.
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `GET` | `/providers` | [`list_providers`] |
//! | `GET` | `/providers/:id` | [`get_provider`] |
//!
//! These endpoints expose the configured AI model providers from the platform
//! configuration. Secret fields (API keys) are never returned.

use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Json,
};
use tracing::instrument;

use crate::{
    dto::{ListProvidersResponse, ProviderResponse, API_VERSION},
    error::ApiError,
    state::AppState,
};

// ---------------------------------------------------------------------------
// GET /providers
// ---------------------------------------------------------------------------

/// List all configured AI model providers.
///
/// Returns provider metadata from the platform configuration. API keys
/// and other secrets are never included in the response.
pub async fn list_providers(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    let providers: Vec<ProviderResponse> = state
        .config
        .providers
        .iter()
        .map(|p| ProviderResponse {
            version: API_VERSION.to_owned(),
            id: p.id.clone(),
            provider_type: p.provider_type.clone(),
            base_url: p.base_url.clone(),
            default_model: p.default_model.clone(),
            timeout_secs: p.timeout_secs,
            max_retries: p.max_retries,
        })
        .collect();

    Ok(Json(ListProvidersResponse {
        version: API_VERSION.to_owned(),
        data: providers,
    }))
}

// ---------------------------------------------------------------------------
// GET /providers/:id
// ---------------------------------------------------------------------------

/// Get detailed information about a single provider.
///
/// Returns 404 if no provider with the given ID is configured.
#[instrument(skip(state), fields(provider_id = %id))]
pub async fn get_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let provider = state
        .config
        .providers
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| ApiError::NotFound(format!("provider '{id}'")))?;

    Ok(Json(ProviderResponse {
        version: API_VERSION.to_owned(),
        id: provider.id.clone(),
        provider_type: provider.provider_type.clone(),
        base_url: provider.base_url.clone(),
        default_model: provider.default_model.clone(),
        timeout_secs: provider.timeout_secs,
        max_retries: provider.max_retries,
    }))
}
