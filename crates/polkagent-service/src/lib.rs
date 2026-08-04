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
//! | [`negotiate`] | Capability negotiation: model/provider selection based on requirements |
//! | [`router`] | [`DefaultModelRouter`]: health-aware model routing with pluggable policies |
//! | [`plugins`] | [`ServicePluginManager`]: plugin lifecycle management and capability validation |
//! | [`scheduled`] | [`ScheduledTaskManager`]: scheduled/recurring agent runs |
//! | [`metadata_watcher`] | [`MetadataDriftWatcher`]: periodic metadata drift detection |
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
pub mod explain;
pub mod harness;
pub mod lifecycle;
pub mod metadata_watcher;
pub mod negotiate;
pub mod plugins;
pub mod provider;
pub mod router;
pub mod scheduled;
pub mod webhook;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use app::{AppService, AppServiceBuilder};
pub use error::ServiceError;
pub use harness::{HarnessInfo, HarnessRegistry, HarnessResolution};
pub use lifecycle::{StartupContext, shutdown, startup};
pub use negotiate::{
    Capability, CapabilityRequirement, MissingCapability, NegotiatedCapabilities, ProbeCache,
    ProviderRestrictions,
};
pub use provider::{ProviderInfo, ProviderRegistry, ProviderStatus};
pub use plugins::{PluginInfo, ServicePluginManager};
pub use router::{
    DefaultModelRouter, ModelRouter, ProviderHealthInfo, RouteDecision, RouteError, RouteReason,
    RouteRequest, RoutingPolicy, SelectedRoute,
};
pub use metadata_watcher::MetadataDriftWatcher;
pub use scheduled::ScheduledTaskManager;
pub use webhook::WebhookDispatcher;
