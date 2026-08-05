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
//! | [`config`] | [`AutonomyLevel`] and [`DataClassification`] |
//! | [`agent`] | [`AgentState`], [`DegradationStage`], [`AgentSpec`] |
//! | [`run`] | [`RunState`], [`Run`] |
//! | [`turn`] | [`Turn`], [`Step`], [`TokenUsage`] |
//! | [`effect`] | [`EffectIntent`], [`EffectAttempt`], [`EffectOutcome`], [`EffectKind`] |
//! | [`event`] | [`RunEvent`], [`EventKind`], [`Durability`] |
//! | [`artifact`] | [`Artifact`], [`ArtifactKind`], [`BlobRef`] |
//! | [`error`] | [`PolkagentError`] |
//! | [`types`] | Low-level primitives: [`Timestamp`], [`DurabilityClass`] |

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
pub mod usage;

// ---------------------------------------------------------------------------
// Convenience re-exports
//
// The most frequently used types are re-exported at the crate root so that
// downstream code can write `polkagent_core::RunId` instead of
// `polkagent_core::ids::RunId`.
// ---------------------------------------------------------------------------

pub use agent::{
    AgentSpec, AgentState, DegradationStage, MemoryConfig, ModelPreference, ResourceLimits,
};
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
    EventId, GrantId, PrincipalId, RunId, StepId, TurnId, UsageRecordId, WorkerId,
};
pub use run::{Run, RunState};
pub use turn::{MessageRole, Step, StepKind, TokenUsage, Turn};
pub use types::{now, DurabilityClass, Timestamp};
pub use usage::{Budget, BudgetLimits, BudgetScope, Cost, UsageRecord, UsageSource, UsageSummary};
