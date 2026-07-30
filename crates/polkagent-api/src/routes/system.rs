//! System information endpoint.
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `GET` | `/system/info` | [`system_info`] |

use axum::{extract::State, response::IntoResponse, Json};

use crate::{
    dto::{ConfigSummary, SystemInfoResponse, API_VERSION},
    error::ApiError,
    state::AppState,
};

// ---------------------------------------------------------------------------
// GET /system/info
// ---------------------------------------------------------------------------

/// Return version, uptime, and a non-secret configuration summary.
///
/// This endpoint exposes only fields that are safe to show to authenticated
/// clients: platform version, uptime, and non-sensitive config values. No
/// secrets, API keys, or database URLs are included.
pub async fn system_info(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    let config = &state.config;

    let config_summary = ConfigSummary {
        bind_address: config.api.bind_address.clone(),
        database_backend: format!("{:?}", config.database.backend).to_lowercase(),
        max_concurrent_runs: config.execution.max_concurrent_runs,
    };

    Ok(Json(SystemInfoResponse {
        version: API_VERSION.to_owned(),
        platform_version: env!("CARGO_PKG_VERSION").to_owned(),
        uptime_secs: state.uptime_secs(),
        config_summary,
    }))
}
