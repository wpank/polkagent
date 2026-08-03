//! Multi-agent group coordination for the Polkagent platform.
//!
//! This crate implements the group coordination layer described in PRD-09 §4-6.
//! It provides:
//!
//! - **[`types`]** — Core vocabulary: [`types::GroupId`], [`types::Group`],
//!   [`types::GroupMember`], [`types::MemberRole`], [`types::QuorumPolicy`],
//!   [`types::GroupBudget`], [`types::GrantSpec`], [`types::EffectiveGrant`],
//!   and [`types::RunResult`].
//! - **[`coordinator`]** — [`coordinator::GroupCoordinator`]: create/manage
//!   groups, add/remove members, resolve effective grants, and track budgets.
//! - **[`quorum`]** — [`quorum::check_quorum`]: evaluate quorum policies
//!   against a set of votes.
//! - **[`propagation`]** — [`propagation::propagate_cancellation`] and
//!   [`propagation::aggregate_evidence`]: cancellation fan-out and run-result
//!   aggregation.
//! - **[`execution`]** — [`execution::ExecutionMode`], executors
//!   ([`execution::SequentialExecutor`], [`execution::ParallelExecutor`],
//!   [`execution::PipelineExecutor`], [`execution::ConsensusExecutor`]),
//!   [`execution::ExecutionPlan`], [`execution::GroupTask`],
//!   [`execution::ExecutionResult`], and [`execution::ExecutionEvidence`].
//! - **[`store`]** — [`store::GroupStore`] async storage trait.
//! - **[`memory_store`]** — [`memory_store::MemoryGroupStore`] in-memory
//!   implementation for tests.
//! - **[`error`]** — [`error::GroupError`] error enum.
//!
//! # Module layout
//!
//! | Module | Responsibility |
//! |---|---|
//! | [`types`] | Core domain types for groups and members |
//! | [`coordinator`] | Mutable group state and business logic |
//! | [`quorum`] | Quorum policy evaluation |
//! | [`propagation`] | Cancellation propagation and evidence aggregation |
//! | [`execution`] | Execution modes and task orchestration (PRD-09 §5-6) |
//! | [`store`] | Async persistence trait |
//! | [`memory_store`] | In-memory store for tests |
//! | [`error`] | Error types |

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

pub mod coordinator;
pub mod error;
pub mod execution;
pub mod memory_store;
pub mod propagation;
pub mod quorum;
pub mod store;
pub mod types;

// ---------------------------------------------------------------------------
// Flat re-exports — the public API surface
// ---------------------------------------------------------------------------

pub use coordinator::GroupCoordinator;
pub use error::{GroupError, GroupResult};
pub use execution::{
    ConsensusExecutor, ExecutionEvidence, ExecutionMode, ExecutionPlan, ExecutionResult, Executor,
    GroupExecutor, GroupTask, ParallelExecutor, PipelineExecutor, SequentialExecutor, TaskId,
    TaskResult,
};
pub use memory_store::MemoryGroupStore;
pub use propagation::{GroupEvidence, aggregate_evidence, propagate_cancellation};
pub use quorum::{
    Decision, QuorumResult, Vote, VoteDecision, check_quorum, count_approvals, count_denials,
};
pub use store::GroupStore;
pub use types::{
    EffectiveGrant, GrantSpec, Group, GroupBudget, GroupId, GroupMember, MemberRole, QuorumPolicy,
    RunResult,
};
