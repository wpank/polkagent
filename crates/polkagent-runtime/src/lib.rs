//! Shared production runtime composition for Polkagent executable surfaces.
//!
//! This crate is the concrete composition root above `polkagent-service`.
//! It opens durable stores, resolves execution adapters, builds one
//! [`polkagent_service::AppService`], performs startup recovery, and
//! rehydrates persisted agents. CLI, TUI, ACP, and server surfaces can then
//! share the returned [`PolkagentRuntime`] instead of assembling partial
//! services independently.

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
#![cfg_attr(
    test,
    allow(
        clippy::expect_used,
        clippy::unwrap_used,
        reason = "tests fail immediately at fixture boundaries"
    )
)]

mod adapters;
mod error;
mod factory;
mod interaction;
mod options;
mod readiness;

pub use error::RuntimeError;
pub use factory::{PolkagentRuntime, RuntimeFactory};
pub use interaction::DurableInteractionService;
pub use options::{AdapterPolicy, RuntimeOptions};
pub use readiness::{
    ComponentReadiness, ComponentState, ConfigSource, ReadinessWarning, RuntimeReadiness,
    WarningCode,
};
