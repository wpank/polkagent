//! In-process event bus, recorder, projections, and query helpers for
//! the Polkagent platform.
//!
//! # Overview
//!
//! This crate implements the event system described in PRD-10 §8–§9. It
//! provides:
//!
//! - **[`RunEvent`]** — the fully-typed, timestamped event record.
//! - **[`EventBus`]** — a `tokio::sync::broadcast`-based in-process fan-out
//!   channel (PRD-10 §9.1).
//! - **[`EventRecorder`]** — appends events to an
//!   [`polkagent_store_trait::event::EventStore`], enforces
//!   ordering invariants, then broadcasts on the bus.
//! - **[`Projection`] / [`ProjectionEngine`]** — trait-based projection
//!   framework for deriving read models from the event stream.
//! - **[`RunStatusProjection`]** — maintains current [`RunState`] per run.
//! - **[`events_for_run`] / [`latest_state`]** — query helpers.
//!
//! # Module layout
//!
//! | Module | Responsibility |
//! |---|---|
//! | [`bus`] | `tokio::sync::broadcast`-based in-process event bus |
//! | [`recorder`] | Append events to an `EventStore`; enforce ordering invariants |
//! | [`projection`] | [`Projection`] trait, [`ProjectionEngine`], [`RunStatusProjection`] |
//! | [`query`] | Convenience query helpers over an `EventStore` |
//! | [`types`] | [`RunEvent`], [`EventPayload`], [`EventType`] |
//! | [`error`] | [`EventError`] |

pub mod bus;
pub mod error;
pub mod projection;
pub mod query;
pub mod recorder;
pub mod types;

// ---------------------------------------------------------------------------
// Flat re-exports — the public API surface
// ---------------------------------------------------------------------------

pub use bus::{EventBus, EventReceiver};
pub use error::EventError;
pub use projection::{Projection, ProjectionEngine, RunStatusProjection};
pub use query::{decode_run_event, events_for_run, latest_state};
pub use recorder::EventRecorder;
pub use types::{EventPayload, EventType, RunEvent, TERMINAL_EVENT_TYPES};

// Re-export core types used in public signatures so callers need fewer imports.
pub use polkagent_core::{
    ArtifactId, ConversationId, DurabilityClass, EventId, RunId, RunState, Timestamp,
};
