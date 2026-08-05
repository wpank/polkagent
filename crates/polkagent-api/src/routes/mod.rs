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
//!   POST   /agents/:id/start
//!   POST   /agents/:id/stop
//!   POST   /agents/:id/pause
//!   POST   /agents/:id/resume
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
//!   GET    /providers/:provider_id/models
//!
//!   GET    /models
//!   GET    /models/:model_id
//!
//!   GET    /skills
//!   GET    /skills/:skill_id
//!   POST   /skills/install
//!   POST   /skills/:skill_id/uninstall
//!   PUT    /skills/:skill_id/config
//!
//!   GET    /tools
//!   GET    /tools/:tool_id
//!   GET    /tools/:tool_id/grants
//!
//!   GET    /payments/balance
//!   GET    /payments/usage
//!   GET    /payments/receipts
//!   GET    /payments/receipts/:receipt_id
//!
//!   POST   /memory/query
//!   GET    /memory/stats
//!   POST   /memory/forget
//!   GET    /memory/entries/:entry_id
//!
//!   GET    /audit
//!   GET    /audit/verify
//!   GET    /audit/:id
//!
//!   POST   /conversations
//!   GET    /conversations
//!   GET    /conversations/:id
//!   POST   /conversations/:id/messages
//!   DELETE /conversations/:id
//!
//!   POST   /interactions
//!   GET    /interactions
//!   GET    /interactions/:id
//!   DELETE /interactions/:id
//!   GET    /interactions/:id/turns
//!   POST   /interactions/:id/prompt
//!   POST   /interactions/:id/turns/:turn_id/cancel
//!   GET    /interactions/:id/config
//!   PUT    /interactions/:id/config
//!   PUT    /interactions/:id/target (compatibility alias)
//!   GET    /interactions/:id/events
//!   GET    /interactions/:id/events/stream (SSE)
//!
//!   POST   /registry/listings
//!   GET    /registry/listings/:id
//!   GET    /registry/search
//!
//!   GET    /system/info
//!
//! /openapi.json                     (no version prefix)
//!   GET    /openapi.json
//!
//! /ws/v1alpha1                      (WebSocket, no version prefix in path)
//!   GET    /ws/v1alpha1
//!
//! /v1/compat/pca                    (C1 bridge, no version prefix)
//!   GET    /v1/compat/pca/health
//!   GET    /v1/compat/pca/inbound
//!   POST   /v1/compat/pca/inbound/ack
//!   POST   /v1/compat/pca/inbound/renew
//!   POST   /v1/compat/pca/send
//!
//! /health
//!   GET    /health/live
//!   GET    /health/ready
//!   GET    /health/startup
//! ```

pub mod agents;
pub mod artifacts;
pub mod audit;
pub mod bridge;
pub mod conversations;
pub mod effects;
pub mod events;
pub mod events_rest;
pub mod health;
pub mod interactions;
pub mod memory;
pub mod metrics;
pub mod models;
pub mod openapi;
pub mod payments;
pub mod providers;
pub mod registry;
pub mod runs;
pub mod skills;
pub mod system;
pub mod tools;
pub mod ws;

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post, put},
    Router,
};

use crate::state::AppState;

/// Default request body size limit: 1 MiB.
const BODY_LIMIT_DEFAULT: usize = 1_048_576;

/// Larger body limit for artifact upload routes: 10 MiB.
const BODY_LIMIT_ARTIFACT: usize = 10_485_760;

/// Register all routes and return the complete `Router`.
///
/// The returned router is ready to be bound to a listener; the caller in
/// `ApiServer::serve` wraps it with `tower-http` middleware (CORS, tracing).
///
/// # Body size limits
///
/// A default body limit of 1 MiB is applied to all routes via
/// [`DefaultBodyLimit::max`].  The `/artifacts/{id}/content` route allows up
/// to 10 MiB to accommodate larger artifact payloads.
pub fn register(state: AppState) -> Router {
    // -----------------------------------------------------------------------
    // Health routes (no version prefix — reachable by load balancer probes)
    // -----------------------------------------------------------------------
    // Liveness is stateless (always 200). Readiness and startup delegate to
    // the full HealthState-based probes in the health module. The
    // `Arc<HealthState>` is extracted from `AppState` via `FromRef`.
    let health_routes = Router::new()
        .route("/health/live", get(health::liveness))
        .route("/health/ready", get(health::readiness))
        .route("/health/startup", get(health::startup));

    // -----------------------------------------------------------------------
    // Prometheus metrics (no version prefix — scrapeable by collectors)
    // -----------------------------------------------------------------------
    let metrics_route = Router::new().route("/metrics", get(metrics::prometheus_metrics));

    // -----------------------------------------------------------------------
    // OpenAPI spec route (no version prefix, no auth required)
    // -----------------------------------------------------------------------
    let openapi_route = Router::new().route("/openapi.json", get(openapi::serve_openapi));

    // -----------------------------------------------------------------------
    // Artifact content route with a larger body limit (10 MiB).
    // -----------------------------------------------------------------------
    let artifact_content_route = Router::new()
        .route(
            "/artifacts/{id}/content",
            get(artifacts::get_artifact_content),
        )
        .layer(DefaultBodyLimit::max(BODY_LIMIT_ARTIFACT));

    // -----------------------------------------------------------------------
    // v1alpha1 API routes
    // -----------------------------------------------------------------------
    let api_routes = Router::new()
        // Agents CRUD
        .route(
            "/agents",
            post(agents::create_agent).get(agents::list_agents),
        )
        .route(
            "/agents/{id}",
            get(agents::get_agent).delete(agents::delete_agent),
        )
        // Agent lifecycle
        .route("/agents/{id}/start", post(runs::start_agent))
        .route("/agents/{id}/stop", post(runs::stop_agent))
        .route("/agents/{id}/pause", post(runs::pause_agent))
        .route("/agents/{id}/resume", post(runs::resume_agent))
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
        // Artifacts (content served from the higher-limit sub-router)
        .route("/artifacts/{id}", get(artifacts::get_artifact))
        .route(
            "/artifacts/{id}/provenance",
            get(artifacts::get_artifact_provenance),
        )
        .merge(artifact_content_route)
        // Events (REST + WebSocket)
        .route("/events", get(events_rest::list_events))
        .route("/events/stream", get(events::event_stream))
        .route("/events/{id}", get(events_rest::get_event))
        // Providers
        .route("/providers", get(providers::list_providers))
        .route("/providers/{id}", get(providers::get_provider))
        .route(
            "/providers/{provider_id}/models",
            get(models::list_provider_models),
        )
        // Models
        .route("/models", get(models::list_all_models))
        .route("/models/{model_id}", get(models::get_model))
        // Skills
        .route("/skills", get(skills::list_skills))
        .route("/skills/install", post(skills::install_skill))
        .route("/skills/{skill_id}", get(skills::get_skill))
        .route(
            "/skills/{skill_id}/uninstall",
            post(skills::uninstall_skill),
        )
        .route(
            "/skills/{skill_id}/config",
            put(skills::update_skill_config),
        )
        // Tools
        .route("/tools", get(tools::list_tools))
        .route("/tools/{tool_id}", get(tools::get_tool))
        .route("/tools/{tool_id}/grants", get(tools::get_tool_grants))
        // Payments
        .route("/payments/balance", get(payments::get_balance))
        .route("/payments/usage", get(payments::get_usage))
        .route("/payments/receipts", get(payments::list_receipts))
        .route(
            "/payments/receipts/{receipt_id}",
            get(payments::get_receipt),
        )
        // Memory
        .route("/memory/query", post(memory::query_memory))
        .route("/memory/stats", get(memory::memory_stats))
        .route("/memory/forget", post(memory::forget_memory))
        .route("/memory/entries/{entry_id}", get(memory::get_memory_entry))
        // Audit
        .route("/audit", get(audit::list_audit_entries))
        .route("/audit/verify", get(audit::verify_audit_integrity))
        .route("/audit/{id}", get(audit::get_audit_entry))
        // Conversations
        .route(
            "/conversations",
            post(conversations::create_conversation).get(conversations::list_conversations),
        )
        .route(
            "/conversations/{id}",
            get(conversations::get_conversation).delete(conversations::delete_conversation),
        )
        .route(
            "/conversations/{id}/messages",
            post(conversations::add_message),
        )
        // Durable agent interactions (execution + replay)
        .route(
            "/interactions",
            post(interactions::create_interaction).get(interactions::list_interactions),
        )
        .route(
            "/interactions/{id}",
            get(interactions::get_interaction).delete(interactions::archive_interaction),
        )
        .route(
            "/interactions/{id}/turns",
            get(interactions::list_interaction_turns),
        )
        .route(
            "/interactions/{id}/prompt",
            post(interactions::prompt_interaction),
        )
        .route(
            "/interactions/{id}/turns/{turn_id}/cancel",
            post(interactions::cancel_interaction_turn),
        )
        .route(
            "/interactions/{id}/config",
            get(interactions::get_interaction_config).put(interactions::update_interaction_config),
        )
        .route(
            "/interactions/{id}/target",
            put(interactions::update_interaction_target),
        )
        .route(
            "/interactions/{id}/events",
            get(interactions::replay_interaction_events),
        )
        .route(
            "/interactions/{id}/events/stream",
            get(interactions::stream_interaction_events),
        )
        // Registry (PRD-12 §5.5 — agent-service listings)
        .route("/registry/listings", post(registry::create_listing))
        .route("/registry/listings/{id}", get(registry::get_listing))
        .route("/registry/search", get(registry::search_listings))
        // System
        .route("/system/info", get(system::system_info))
        // Apply default 1 MiB body limit to all routes in this sub-router.
        .layer(DefaultBodyLimit::max(BODY_LIMIT_DEFAULT));

    // -----------------------------------------------------------------------
    // WebSocket v1alpha1 route (no version prefix in the path segment; the
    // `v1alpha1` is part of the path literal per PRD-14 §4).
    // -----------------------------------------------------------------------
    let ws_route = Router::new().route("/ws/v1alpha1", get(ws::ws_handler));

    // -----------------------------------------------------------------------
    // PCA C1 bridge compatibility routes (PRD-06 §12, §20.4).
    //
    // These are mounted at `/v1/compat/pca/` without the v1alpha1 prefix so
    // that existing PCA tooling can interact without changes.
    // -----------------------------------------------------------------------
    let bridge_routes = Router::new()
        .route("/v1/compat/pca/health", get(bridge::bridge_health))
        .route("/v1/compat/pca/inbound", get(bridge::bridge_inbound))
        .route("/v1/compat/pca/inbound/ack", post(bridge::bridge_ack))
        .route("/v1/compat/pca/inbound/renew", post(bridge::bridge_renew))
        .route("/v1/compat/pca/send", post(bridge::bridge_send));

    Router::new()
        .merge(health_routes)
        .merge(metrics_route)
        .merge(openapi_route)
        .merge(ws_route)
        .merge(bridge_routes)
        .nest("/api/v1alpha1", api_routes)
        .with_state(state)
}
