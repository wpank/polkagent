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

use std::sync::Arc;

use axum::Router;
use thiserror::Error;
use tower_http::{
    cors::CorsLayer,
    trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer},
};
use tracing::{info, Level};

use polkagent_config::Config;
use polkagent_event::EventBus;
use polkagent_store_trait::EffectStore;

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

    /// Build the Axum router (routes + middleware) without binding.
    ///
    /// Exposed so that tests can construct the router and pass it to an
    /// `axum_test::TestServer` without opening a real TCP socket.
    #[must_use]
    pub fn into_router(self) -> Router {
        let cors = CorsLayer::permissive();

        let trace = TraceLayer::new_for_http()
            .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
            .on_response(DefaultOnResponse::new().level(Level::INFO));

        routes::register(self.state)
            .layer(trace)
            .layer(cors)
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
        let addr: std::net::SocketAddr = bind_addr
            .parse()
            .map_err(|source| ServerError::InvalidAddr {
                addr: bind_addr.to_owned(),
                source,
            })?;

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

        axum::serve(listener, router).await?;
        Ok(())
    }
}
