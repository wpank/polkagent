//! Health-check endpoints.
//!
//! | Path | Purpose |
//! |---|---|
//! | `GET /health/live` | Always 200; proves the process is alive. |
//! | `GET /health/ready` | 200 when the database is reachable; 503 otherwise. |
//! | `GET /health/startup` | 200 after one-time initialisation is complete. |

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde_json::json;

use crate::state::AppState;

// ---------------------------------------------------------------------------
// Liveness
// ---------------------------------------------------------------------------

/// `GET /health/live`
///
/// Always returns 200 OK with `{"status": "ok"}`. The load-balancer uses this
/// to determine whether the process is alive and should receive traffic. If
/// this endpoint fails it means the process is dead or completely unresponsive.
pub async fn liveness() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

// ---------------------------------------------------------------------------
// Readiness
// ---------------------------------------------------------------------------

/// `GET /health/ready`
///
/// Returns 200 when the server is ready to serve traffic: the database is
/// reachable and the effect store can be queried. Returns 503 with a JSON
/// body when any dependency is unavailable.
pub async fn readiness(State(state): State<AppState>) -> impl IntoResponse {
    // Probe the effect store with a lightweight call. We attempt to fetch a
    // sentinel (non-existent) intent; `StoreError::NotFound` is the expected
    // healthy response, meaning the store is reachable. Any other error
    // variant indicates a backend problem.
    use polkagent_core::EffectId;
    use polkagent_store_trait::StoreError;

    let sentinel_id = EffectId::new();
    match state.effect_store.get_intent(sentinel_id).await {
        Ok(_) | Err(StoreError::NotFound { .. }) => (
            StatusCode::OK,
            Json(json!({ "status": "ok", "checks": { "effect_store": "ok" } })),
        ),
        Err(err) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "status": "degraded",
                "checks": {
                    "effect_store": format!("unavailable: {err}")
                }
            })),
        ),
    }
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

/// `GET /health/startup`
///
/// Returns 200 once one-time initialisation (configuration loading, migration,
/// registry warm-up) has completed. In this implementation, presence of the
/// `AppState` implies init is done, so we always return 200.
pub async fn startup(State(_state): State<AppState>) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({ "status": "ok", "detail": "initialisation complete" })),
    )
}
