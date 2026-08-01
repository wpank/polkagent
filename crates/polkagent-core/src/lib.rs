//! `polkagent-core` — Foundational domain types for the Polkagent platform.
//!
//! This crate defines the core vocabulary types shared across every layer of
//! the Polkagent stack: IDs, enums, structs, and error types. It has no
//! dependency on I/O, async, or database code and can be compiled in any
//! context.
//!
//! # Module overview
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`ids`] | Typed UUID newtypes for every domain entity |
//! | [`config`] | [`AutonomyLevel`](config::AutonomyLevel) and [`DataClassification`](config::DataClassification) |
//! | [`agent`] | [`AgentState`](agent::AgentState), [`DegradationStage`](agent::DegradationStage), [`AgentSpec`](agent::AgentSpec) |
//! | [`run`] | [`RunState`](run::RunState), [`Run`](run::Run) |
//! | [`turn`] | [`Turn`](turn::Turn), [`Step`](turn::Step), [`TokenUsage`](turn::TokenUsage) |
//! | [`effect`] | [`EffectIntent`](effect::EffectIntent), [`EffectAttempt`](effect::EffectAttempt), [`EffectOutcome`](effect::EffectOutcome), [`EffectKind`](effect::EffectKind) |
//! | [`event`] | [`RunEvent`](event::RunEvent), [`EventKind`](event::EventKind), [`Durability`](event::Durability) |
//! | [`artifact`] | [`Artifact`](artifact::Artifact), [`ArtifactKind`](artifact::ArtifactKind), [`BlobRef`](artifact::BlobRef) |
//! | [`error`] | [`PolkagentError`](error::PolkagentError) |
//! | [`types`] | Low-level primitives: [`Timestamp`](types::Timestamp), [`DurabilityClass`](types::DurabilityClass) |

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

pub mod agent;
pub mod artifact;
pub mod config;
pub mod effect;
pub mod error;
pub mod event;
pub mod ids;
pub mod run;
pub mod turn;
pub mod types;

// ---------------------------------------------------------------------------
// Convenience re-exports
//
// The most frequently used types are re-exported at the crate root so that
// downstream code can write `polkagent_core::RunId` instead of
// `polkagent_core::ids::RunId`.
// ---------------------------------------------------------------------------

pub use agent::{AgentSpec, AgentState, DegradationStage, MemoryConfig, ModelPreference, ResourceLimits};
pub use artifact::{Artifact, ArtifactKind, BlobRef};
pub use config::{AutonomyLevel, DataClassification};
pub use effect::{
    AttemptState, EffectAttempt, EffectIntent, EffectIntentState, EffectKind, EffectOutcome,
    ExternalRef, IdempotencyKey, OutcomeStatus, RetryClass,
};
pub use error::PolkagentError;
pub use event::{Durability, EventCorrelation, EventKind, LogLevel, RunEvent};
pub use ids::{
    AgentId, ApprovalId, ArtifactId, ConversationId, EffectAttemptId, EffectId, EffectOutcomeId,
    EventId, GrantId, PrincipalId, RunId, StepId, TurnId, WorkerId,
};
pub use run::{Run, RunState};
pub use turn::{MessageRole, Step, StepKind, TokenUsage, Turn};
pub use types::{DurabilityClass, Timestamp, now};
