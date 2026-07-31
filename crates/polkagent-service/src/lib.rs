//! Application service layer for the Polkagent platform.
//!
//! This crate is the top-level composition root that wires all Polkagent
//! components (stores, executor, event bus, run manager, effect pipeline,
//! provider registry) into a usable application service.
//!
//! # Modules
//!
//! | Module | Responsibility |
//! |--------|---------------|
//! | [`app`] | [`AppService`] and [`AppServiceBuilder`]: the central service facade |
//! | [`provider`] | [`ProviderRegistry`]: manages configured model providers |
//! | [`lifecycle`] | [`startup`] / [`shutdown`]: full application initialization and teardown |
//! | [`error`] | [`ServiceError`]: unified error type |
//!
//! # Quick start
//!
//! ```rust,no_run
//! use polkagent_service::{AppService, ServiceError};
//! use polkagent_config::Config;
//!
//! # fn example() -> Result<(), ServiceError> {
//! // In production, use lifecycle::startup() instead.
//! // This shows the builder API for fine-grained control.
//! let config = Config::default();
//! // ... create stores, executor, event bus ...
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(
    missing_docs,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used
)]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod app;
pub mod error;
pub mod lifecycle;
pub mod provider;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use app::{AppService, AppServiceBuilder};
pub use error::ServiceError;
pub use lifecycle::{StartupContext, shutdown, startup};
pub use provider::{ProviderInfo, ProviderRegistry, ProviderStatus};
