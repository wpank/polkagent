//! `polkagent-api` — REST API server for the Polkagent platform.
//!
//! This crate implements the HTTP API surface described in PRD-14 using Axum 0.8.
//! All endpoints live under `/api/v1alpha1`. Health probes are available at
//! `/health/{live,ready,startup}` without the versioned prefix.
//!
//! # Quick start
//!
//! ```no_run
//! use std::sync::Arc;
//! use polkagent_api::{ApiServer, RunManager};
//! use polkagent_config::Config;
//!
//! # async fn example(store: Arc<dyn polkagent_store_trait::EffectStore>) {
//! let server = ApiServer::new(Config::default(), RunManager::new(), store);
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

pub mod dto;
pub mod error;
pub mod routes;
pub mod run;
pub mod server;
pub mod state;

// Convenient re-exports for consumers that construct the server.
pub use run::RunManager;
pub use server::ApiServer;
pub use state::AppState;
