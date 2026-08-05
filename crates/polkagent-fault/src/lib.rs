//! Fault injection framework for testing crash safety and resilience.
//!
//! This crate provides the infrastructure to inject controlled faults — panics,
//! timeouts, errors, data corruption, and artificial delays — at named injection
//! points within the Polkagent component stack.
//!
//! # Architecture
//!
//! A [`FaultInjector`] holds a registry of named [`FaultPoint`]s, each
//! combining a [`Fault`] variant with a [`FaultSchedule`] that controls when
//! the fault fires. Wrapper types (`FaultExecutor`, `FaultSigner`,
//! `FaultTransport`, `FaultStore`) implement the corresponding port traits and
//! consult the injector at named injection points.
//!
//! A typical test looks like:
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use polkagent_fault::{FaultInjector, Fault, FaultSchedule};
//! use polkagent_fault::executor::FaultExecutor;
//! use polkagent_executor_fake::FakeExecutor;
//!
//! let injector = Arc::new(FaultInjector::new());
//! injector.add_fault(
//!     "before_execute",
//!     Fault::Error { message: "injected failure".into() },
//!     FaultSchedule::Once,
//! );
//! // Wrap any executor and the first call will fail; subsequent calls pass through.
//! ```
//!
//! # Modules
//!
//! - [`types`] — [`Fault`], [`Corruption`], [`FaultSchedule`], [`FaultPoint`].
//! - [`injector`] — [`FaultInjector`]: the central registry.
//! - [`executor`] — [`executor::FaultExecutor`]: wraps any
//!   [`polkagent_executor_trait::ModelExecutor`].
//! - [`signer`] — [`signer::FaultSigner`]: wraps any
//!   [`polkagent_signer_trait::Signer`].
//! - [`transport`] — [`transport::FaultTransport`]: wraps any
//!   [`polkagent_transport_trait::Transport`].
//! - [`store`] — [`store::FaultStore`]: wraps any
//!   [`polkagent_store_trait::EffectStore`].

pub mod executor;
pub mod injector;
pub mod signer;
pub mod store;
pub mod transport;
pub mod types;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use injector::FaultInjector;
pub use types::{Corruption, Fault, FaultPoint, FaultSchedule};
