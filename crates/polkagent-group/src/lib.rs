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
//! - **[`execution_plan_codec`]** — the validated canonical v1 durable plan
//!   boundary and its domain-separated digest.
//! - **[`store`]** — [`store::GroupStore`] async storage trait.
//! - **[`service`]** — durable, surface-neutral validated group CRUD.
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
//! | [`execution_plan_codec`] | Canonical durable execution-plan v1 codec |
//! | [`store`] | Async persistence trait |
//! | [`service`] | Validated durable application operations |
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
pub mod execution_plan_codec;
pub mod memory_store;
pub mod propagation;
pub mod quorum;
pub mod service;
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
pub use execution_plan_codec::{
    compute_execution_plan_digest_v1, decode_execution_plan_v1, encode_execution_plan_v1,
    CanonicalExecutionPlanV1, DurableDependencyEdgeV1, DurableExecutionPlanV1, DurableGrantSpecV1,
    DurableGroupTaskV1, ExecutionPlanCodecError, EXECUTION_PLAN_V1_CONTRACT,
    EXECUTION_PLAN_V1_DIGEST_PREFIX, EXECUTION_PLAN_V1_MAX_CANONICAL_BYTES,
    EXECUTION_PLAN_V1_MAX_DEPENDENCY_EDGES, EXECUTION_PLAN_V1_MAX_TASKS,
    EXECUTION_PLAN_V1_MAX_TASK_INPUT_BYTES, EXECUTION_PLAN_V1_MAX_TASK_INPUT_DEPTH,
    EXECUTION_PLAN_V1_SCHEMA_VERSION,
};
pub use memory_store::MemoryGroupStore;
pub use propagation::{aggregate_evidence, propagate_cancellation, GroupEvidence};
pub use quorum::{
    check_quorum, count_approvals, count_denials, Decision, QuorumResult, Vote, VoteDecision,
};
pub use service::{GroupPolicyUpdate, GroupService};
pub use store::GroupStore;
pub use types::{
    EffectiveGrant, GrantSpec, Group, GroupBudget, GroupId, GroupMember, MemberRole, QuorumPolicy,
    RunResult,
};
