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
//!   GET    /runs/:id/turns
//!   GET    /runs/:id/events
//!   GET    /runs/:id/artifacts
//!   GET    /runs/:id/effects
//!   POST   /runs/:id/resume
//!
//!   GET    /effects/:id
//!   POST   /effects/:id/approve
//!   POST   /effects/:id/deny
//!
//!   GET    /artifacts/:id
//!   GET    /artifacts/:id/content
//!   GET    /artifacts/:id/provenance
//!
//!   GET    /events
//!   GET    /events/:id
//!   GET    /events/stream          (WebSocket)
//!
//!   GET    /providers
//!   GET    /providers/:id
//!
//!   POST   /memory/query
//!   GET    /memory/stats
//!
//!   GET    /system/info
//!
//! /health
//!   GET    /health/live
//!   GET    /health/ready
//!   GET    /health/startup
//! ```

pub mod agents;
pub mod artifacts;
pub mod effects;
pub mod events;
pub mod events_rest;
pub mod health;
pub mod memory;
pub mod providers;
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
        .route("/runs/{id}/turns", get(runs::list_run_turns))
        .route("/runs/{id}/events", get(runs::list_run_events))
        .route("/runs/{id}/artifacts", get(runs::list_run_artifacts))
        .route("/runs/{id}/effects", get(runs::list_run_effects))
        .route("/runs/{id}/resume", post(runs::resume_run))
        // Effects
        .route("/effects/{id}", get(effects::get_effect))
        .route("/effects/{id}/approve", post(effects::approve_effect))
        .route("/effects/{id}/deny", post(effects::deny_effect))
        // Artifacts
        .route("/artifacts/{id}", get(artifacts::get_artifact))
        .route("/artifacts/{id}/content", get(artifacts::get_artifact_content))
        .route("/artifacts/{id}/provenance", get(artifacts::get_artifact_provenance))
        // Events (REST + WebSocket)
        .route("/events", get(events_rest::list_events))
        .route("/events/{id}", get(events_rest::get_event))
        .route("/events/stream", get(events::event_stream))
        // Providers
        .route("/providers", get(providers::list_providers))
        .route("/providers/{id}", get(providers::get_provider))
        // Memory
        .route("/memory/query", post(memory::query_memory))
        .route("/memory/stats", get(memory::memory_stats))
        // System
        .route("/system/info", get(system::system_info));

    Router::new()
        .merge(health_routes)
        .nest("/api/v1alpha1", api_routes)
        .with_state(state)
}
