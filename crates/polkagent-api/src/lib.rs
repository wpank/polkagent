//! `polkagent-api` — REST API server for the Polkagent platform.
//!
//! This crate implements the HTTP API surface described in PRD-14 using Axum 0.8.
//! All endpoints live under `/api/v1alpha1`. Health probes are available at
//! `/health/{live,ready,startup}` without the versioned prefix.
//!
//! # Storage abstraction
//!
//! The API layer is decoupled from any specific storage backend via trait
//! objects:
//!
//! - [`state::AgentStore`] — agent spec persistence.
//! - [`run::RunManagerTrait`] — run lifecycle management.
//! - [`polkagent_store_trait::EffectStore`] — effect pipeline persistence.
//!
//! In-memory implementations ([`state::InMemoryAgentStore`] and
//! [`run::InMemoryRunManager`]) are provided for tests and local development.
//! Production deployments inject durable SQLite-backed implementations.
//!
//! # Quick start
//!
//! ```no_run
//! use std::sync::Arc;
//! use polkagent_api::{ApiServer, InMemoryRunManager};
//! use polkagent_api::state::InMemoryAgentStore;
//! use polkagent_config::Config;
//! use polkagent_event::EventBus;
//!
//! # async fn example(store: Arc<dyn polkagent_store_trait::EffectStore>) {
//! let agents = Arc::new(InMemoryAgentStore::new());
//! let run_manager = Arc::new(InMemoryRunManager::new());
//! let event_bus = EventBus::with_default_capacity();
//! let server = ApiServer::new(Config::default(), agents, run_manager, store, event_bus);
//! server.serve("127.0.0.1:4840").await.expect("server error");
//! # }
//! ```
//!
//! # Crate layout
//!
//! | Module | Purpose |
//! |---|---|
//! | [`server`] | `ApiServer` — top-level binding and startup |
//! | [`state`] | `AppState` shared across all handlers |
//! | [`routes`] | Route registration for each resource group |
//! | [`dto`] | Request/response data transfer objects |
//! | [`error`] | `ApiError` with `IntoResponse` impl |

mod access_policy;
pub mod auth;
pub mod dto;
pub mod durable;
pub mod error;
pub mod rate_limit;
pub mod read_only;
pub mod routes;
pub mod run;
pub mod server;
pub mod state;
pub mod validate;

// Convenient re-exports for consumers that construct the server.
pub use durable::{
    app_state_from_runtime, RuntimeAgentStore, RuntimeMemoryStore, RuntimeRunManager,
    RuntimeSkillRegistry, RuntimeToolRegistryStore, UnavailableRuntimeRoute,
    RUNTIME_UNAVAILABLE_ROUTES,
};
pub use polkagent_store_sqlite::SqliteApiArtifactStore as RuntimeArtifactStore;
pub use run::{InMemoryRunManager, RunManagerTrait};
pub use server::ApiServer;
pub use state::{AgentStore, AgentStoreError, AppState, InMemoryAgentStore};
