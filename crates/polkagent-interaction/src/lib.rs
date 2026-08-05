//! Surface-neutral contracts for durable, multi-turn Polkagent interactions.
//!
//! This crate deliberately contains no terminal, TUI, HTTP, ACP, database, or
//! execution adapter. It defines the stable vocabulary those adapters share:
//!
//! - interaction targets, configuration, prompt requests, and turn handles;
//! - structured text, tool, plan, approval, usage, lifecycle, and terminal
//!   events;
//! - replay-aware event-stream and interaction-service traits;
//! - one typed parser and catalog for the MVP slash commands.
//!
//! Persistence, run orchestration, event projection, and adapter wiring are
//! later FND-02 slices. Keeping those concerns out of this crate prevents a
//! surface protocol from becoming the domain contract.

#![forbid(unsafe_code)]
#![warn(
    missing_docs,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used
)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic))]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod command;
pub mod error;
pub mod event;
pub mod executor;
pub mod hub;
pub mod ids;
pub mod model;
pub mod persistence;
pub mod service;

pub use command::{
    AgentTargetView, CancelTarget, CommandAvailability, CommandCategory, CommandContext,
    CommandExecutor, CommandInvocation, CommandMutability, CommandName, CommandOutput,
    CommandParseError, CommandRegistry, CommandRequest, CommandSpec, InteractionCommand,
    ParsedLine, RunDetailView, RunSummaryView,
};
pub use error::{InteractionError, InteractionErrorCode};
pub use event::{
    ApprovalStatus, ApprovalView, InteractionEvent, InteractionEventEnvelope, PlanEntryStatus,
    PlanEntryView, RunRole, ToolCallKind, ToolCallStatus, ToolCallView, ToolLocation, TurnResult,
    UsageView,
};
pub use executor::{InteractionCommandRuntime, ServiceCommandExecutor};
pub use hub::InteractionEventHub;
pub use ids::{InteractionEventId, InteractionTurnId, PlanEntryId, ToolCallId};
pub use model::{
    ApprovalDecision, ClientCapabilities, ClientCapability, ClientContext, ConfigOption,
    ConfigOptionValue, ConfigUpdate, CreateInteractionRequest, InteractionConfig,
    InteractionContent, InteractionOverrides, InteractionState, InteractionSummary,
    InteractionTarget, ListInteractionsRequest, OverrideValue, PromptRequest, SubscriptionRequest,
    TurnHandle, TurnState, TurnSummary,
};
pub use persistence::{
    InteractionRunLink, InteractionStore, NewAssistantMessage, NewInteraction, NewInteractionEvent,
    NewInteractionTurn, StoredInteractionTurn,
};
pub use service::{
    BoxInteractionEventStream, InteractionEventStream, InteractionService, StartedTurn, StreamError,
};
