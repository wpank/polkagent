//! Route registration for all API endpoints.
//!
//! The public surface is mounted under `/api/v1alpha1` by [`register`].
//! Health endpoints are mounted at the root so that load-balancers can reach
//! them without the versioned prefix.
//!
//! # Layout
//!
//! ```text
//! /api/v1alpha1
//!   POST   /agents
//!   GET    /agents
//!   GET    /agents/:id
//!   DELETE /agents/:id
//!
//!   POST   /agents/:agent_id/runs
//!   GET    /runs
//!   GET    /runs/:id
//!   POST   /runs/:id/cancel
//!   GET    /runs/:run_id/effects
//!
//!   GET    /effects/:id
//!   POST   /effects/:id/approve
//!   POST   /effects/:id/deny
//!
//!   GET    /events/stream          (WebSocket)
//!
//!   GET    /system/info
//!
//! /health
//!   GET    /health/live
//!   GET    /health/ready
//!   GET    /health/startup
//! ```

pub mod agents;
pub mod effects;
pub mod events;
pub mod health;
pub mod runs;
pub mod system;

use axum::{
    routing::{get, post},
    Router,
};

use crate::state::AppState;

/// Register all routes and return the complete `Router`.
///
/// The returned router is ready to be bound to a listener; the caller in
/// `ApiServer::serve` wraps it with `tower-http` middleware (CORS, tracing).
#[must_use]
pub fn register(state: AppState) -> Router {
    // -----------------------------------------------------------------------
    // Health routes (no version prefix — reachable by load balancer probes)
    // -----------------------------------------------------------------------
    let health_routes = Router::new()
        .route("/health/live", get(health::liveness))
        .route("/health/ready", get(health::readiness))
        .route("/health/startup", get(health::startup));

    // -----------------------------------------------------------------------
    // v1alpha1 API routes
    // -----------------------------------------------------------------------
    let api_routes = Router::new()
        // Agents
        .route("/agents", post(agents::create_agent).get(agents::list_agents))
        .route(
            "/agents/{id}",
            get(agents::get_agent).delete(agents::delete_agent),
        )
        // Runs (agent-scoped creation + top-level query)
        .route("/agents/{agent_id}/runs", post(runs::create_run))
        .route("/runs", get(runs::list_runs))
        .route("/runs/{id}", get(runs::get_run))
        .route("/runs/{id}/cancel", post(runs::cancel_run))
        // Effects
        .route("/runs/{run_id}/effects", get(effects::list_effects))
        .route("/effects/{id}", get(effects::get_effect))
        .route("/effects/{id}/approve", post(effects::approve_effect))
        .route("/effects/{id}/deny", post(effects::deny_effect))
        // Events (WebSocket)
        .route("/events/stream", get(events::event_stream))
        // System
        .route("/system/info", get(system::system_info));

    Router::new()
        .merge(health_routes)
        .nest("/api/v1alpha1", api_routes)
        .with_state(state)
}
