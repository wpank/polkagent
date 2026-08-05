//! `ApiServer` — the top-level HTTP server entry point.
//!
//! Constructs the Axum router with all routes and middleware, binds to the
//! configured address, and drives the Tokio TCP listener.
//!
//! # Usage
//!
//! ```no_run
//! use std::sync::Arc;
//! use polkagent_api::server::ApiServer;
//! use polkagent_api::{InMemoryRunManager, InMemoryAgentStore};
//! use polkagent_config::Config;
//! use polkagent_event::EventBus;
//!
//! # async fn example(store: Arc<dyn polkagent_store_trait::EffectStore>) {
//! let agents = Arc::new(InMemoryAgentStore::new());
//! let run_manager = Arc::new(InMemoryRunManager::new());
//! let event_bus = EventBus::with_default_capacity();
//! let server = ApiServer::new(Config::default(), agents, run_manager, store, event_bus);
//! server.serve("127.0.0.1:4840").await.expect("server failed");
//! # }
//! ```

use std::{future::Future, sync::Arc};

use axum::{http::HeaderValue, middleware, Router};
use thiserror::Error;
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    trace::{DefaultMakeSpan, DefaultOnFailure, DefaultOnRequest, DefaultOnResponse, TraceLayer},
};
use tracing::{info, Level};

use polkagent_config::Config;
use polkagent_event::EventBus;
use polkagent_store_trait::EffectStore;
use polkagent_telemetry::{LogFormat, TelemetryConfig, TelemetryGuard};

use crate::auth::{auth_middleware, AuthState};
use crate::rate_limit::{rate_limit_middleware, RateLimitState};
use crate::read_only::read_only_middleware;
use crate::run::RunManagerTrait;
use crate::state::AgentStore;

use crate::{routes, state::AppState};

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors produced by [`ApiServer::serve`].
#[derive(Debug, Error)]
pub enum ServerError {
    /// The bind address is not a valid socket address.
    #[error("invalid bind address '{addr}': {source}")]
    InvalidAddr {
        addr: String,
        #[source]
        source: std::net::AddrParseError,
    },

    /// The TCP listener could not be bound.
    #[error("failed to bind listener on '{addr}': {source}")]
    BindFailed {
        addr: String,
        #[source]
        source: std::io::Error,
    },

    /// The server returned an I/O error while running.
    #[error("server error: {0}")]
    Serve(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// CORS helpers
// ---------------------------------------------------------------------------

/// Build a [`CorsLayer`] from the configured origin list.
///
/// - **Empty list** → `CorsLayer::permissive()` (development mode: allow all).
/// - **Non-empty list** → only the listed origins are allowed; each entry is
///   treated as an exact `HeaderValue` match.
fn build_cors_layer(origins: &[String]) -> CorsLayer {
    if origins.is_empty() {
        // No origins configured — fall back to permissive (dev mode).
        return CorsLayer::permissive();
    }

    let header_values: Vec<HeaderValue> = origins
        .iter()
        .filter_map(|o| HeaderValue::from_str(o).ok())
        .collect();

    if header_values.is_empty() {
        // All origins were invalid — fall back to permissive rather than
        // silently blocking every request.
        return CorsLayer::permissive();
    }

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(header_values))
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any)
}

// ---------------------------------------------------------------------------
// ApiServer
// ---------------------------------------------------------------------------

/// The HTTP API server.
///
/// Holds everything needed to construct the Axum app and start serving. Call
/// [`ApiServer::serve`] to bind and run.
pub struct ApiServer {
    state: AppState,
}

impl ApiServer {
    /// Construct a new `ApiServer` with injectable storage backends.
    ///
    /// # Arguments
    ///
    /// - `config`: the active platform configuration.
    /// - `agents`: the agent spec store (in-memory or durable).
    /// - `run_manager`: manages run lifecycle operations.
    /// - `effect_store`: the durable effect/outbox store (used for readiness checks).
    /// - `event_bus`: the in-process event bus for real-time WebSocket streaming.
    pub fn new(
        config: Config,
        agents: Arc<dyn AgentStore>,
        run_manager: Arc<dyn RunManagerTrait>,
        effect_store: Arc<dyn EffectStore>,
        event_bus: EventBus,
    ) -> Self {
        let state = AppState::new(config, agents, run_manager, effect_store, event_bus);
        Self { state }
    }

    /// Construct an `ApiServer` directly from a pre-built [`AppState`].
    ///
    /// Useful in tests that need to inject optional stores (e.g. an
    /// `EventStore` or `ArtifactStore`) via the builder methods on `AppState`
    /// before creating the server.
    #[must_use]
    pub fn from_state(state: crate::state::AppState) -> Self {
        Self { state }
    }

    /// Build the Axum router (routes + middleware) without binding.
    ///
    /// Exposed so that tests can construct the router and pass it to an
    /// `axum_test::TestServer` without opening a real TCP socket.
    pub fn into_router(self) -> Router {
        // ---------------------------------------------------------------
        // CORS
        //
        // When `config.api.cors_origins` is empty we default to permissive
        // mode (development / testing).  In production, operators must list
        // at least one origin so that the wildcard is replaced with an
        // explicit allow-list.
        // ---------------------------------------------------------------
        let cors = build_cors_layer(&self.state.config.api.cors_origins);

        // Request tracing middleware: records HTTP method, path, status, and
        // latency for every request. Spans are emitted at INFO level.
        // `DefaultMakeSpan` includes `http.method`, `http.target`, and
        // `otel.kind`=server on the span. `DefaultOnResponse` logs status and
        // duration at INFO level. `DefaultOnFailure` logs errors at ERROR level.
        let trace = TraceLayer::new_for_http()
            .make_span_with(
                DefaultMakeSpan::new()
                    .level(Level::INFO)
                    .include_headers(false),
            )
            .on_request(DefaultOnRequest::new().level(Level::DEBUG))
            .on_response(
                DefaultOnResponse::new()
                    .level(Level::INFO)
                    .include_headers(false),
            )
            .on_failure(DefaultOnFailure::new().level(Level::ERROR));

        // Per-client rate limiting middleware, configured via
        // `config.server.rate_limit`. When `enabled` is false the middleware
        // is a no-op pass-through (no performance overhead).
        let rate_limit_state = Arc::new(RateLimitState::from_config(
            &self.state.config.server.rate_limit,
        ));

        // Auth middleware — validates Bearer / X-Api-Key headers when
        // `config.auth.enabled = true`.  No-op when disabled.
        let auth_state = Arc::new(AuthState::from_config(&self.state.config.auth));

        // Read-only guard: when `config.api.read_only` is true, any request
        // that is not GET/HEAD/OPTIONS is rejected with 405 Method Not Allowed
        // before it reaches any route handler or the rate limiter.
        let read_only = self.state.config.api.read_only;

        routes::register(self.state)
            .layer(middleware::from_fn_with_state(auth_state, auth_middleware))
            .layer(middleware::from_fn_with_state(
                read_only,
                read_only_middleware,
            ))
            .layer(middleware::from_fn_with_state(
                rate_limit_state,
                rate_limit_middleware,
            ))
            .layer(trace)
            .layer(cors)
    }

    /// Initialise the telemetry subsystem from the server's `Config`.
    ///
    /// Reads `config.observability.{service_name,otlp_endpoint}` and
    /// `config.log.level`, then calls [`polkagent_telemetry::init_telemetry`].
    /// Returns a [`TelemetryGuard`] that must be held for the lifetime of the
    /// server process. Gracefully returns `Ok(guard_with_no_provider)` on any
    /// non-fatal error (e.g. the subscriber was already set in tests).
    pub fn init_telemetry(config: &Config) -> TelemetryGuard {
        let obs = &config.observability;

        // Log level: config value, or fall back to "info".
        let log_level = {
            let lvl = &config.log.level;
            if lvl.is_empty() {
                "info".to_owned()
            } else {
                lvl.clone()
            }
        };

        // Mirror the config's log format selection.
        let log_format = match config.log.format {
            polkagent_config::schema::LogFormat::Json => LogFormat::Json,
            polkagent_config::schema::LogFormat::Pretty => LogFormat::Pretty,
        };

        // Respect NO_COLOR for the API server's tracing output as well.
        let ansi = std::env::var_os("NO_COLOR").is_none();

        let telemetry_cfg = TelemetryConfig {
            log_level,
            log_format,
            otlp_endpoint: obs.otlp_endpoint.clone(),
            service_name: obs.service_name.clone(),
            ansi,
        };

        // Attempt to initialise; swallow the error if a subscriber is already
        // active (common in integration tests that re-use the same process).
        polkagent_telemetry::init_telemetry(telemetry_cfg)
            .unwrap_or_else(|_| TelemetryGuard::no_op())
    }

    /// Bind to `bind_addr` and serve requests until the process is killed.
    ///
    /// This is a long-running async function. Drive it on the Tokio runtime:
    ///
    /// ```no_run
    /// # #[tokio::main]
    /// # async fn main() {
    /// # use std::sync::Arc;
    /// # let store: Arc<dyn polkagent_store_trait::EffectStore> = unimplemented!();
    /// # let agents = Arc::new(polkagent_api::InMemoryAgentStore::new());
    /// # let run_mgr = Arc::new(polkagent_api::InMemoryRunManager::new());
    /// # let event_bus = polkagent_event::EventBus::with_default_capacity();
    /// # let server = polkagent_api::server::ApiServer::new(
    /// #     polkagent_config::Config::default(),
    /// #     agents,
    /// #     run_mgr,
    /// #     store,
    /// #     event_bus,
    /// # );
    /// server.serve("127.0.0.1:4840").await.unwrap();
    /// # }
    /// ```
    pub async fn serve(self, bind_addr: &str) -> Result<(), ServerError> {
        self.serve_with_shutdown(bind_addr, std::future::pending())
            .await
    }

    /// Bind to `bind_addr` and drain active HTTP connections after `shutdown`
    /// resolves.
    ///
    /// The listener stops accepting new connections when shutdown begins, then
    /// Axum waits for in-flight work to finish before this future returns.
    pub async fn serve_with_shutdown<F>(
        self,
        bind_addr: &str,
        shutdown: F,
    ) -> Result<(), ServerError>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let addr: std::net::SocketAddr =
            bind_addr
                .parse()
                .map_err(|source| ServerError::InvalidAddr {
                    addr: bind_addr.to_owned(),
                    source,
                })?;

        // Initialise telemetry from config before building the router.
        // The guard is kept alive for the duration of the server.
        let _telemetry_guard = Self::init_telemetry(&self.state.config);

        let router = self.into_router();

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|source| ServerError::BindFailed {
                addr: bind_addr.to_owned(),
                source,
            })?;

        info!(
            bind_addr = %addr,
            "Polkagent API server listening"
        );

        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown)
            .await?;
        Ok(())
    }
}
