//! Durable approval and execution-checkpoint storage contracts.
//!
//! These types deliberately contain no terminal, HTTP, or ACP surface types.
//! They freeze the store-neutral APR-00 boundary used by the first `SQLite`
//! coordinator and by future adapters.

use std::time::Duration;

use async_trait::async_trait;
use polkagent_core::{
    AgentId, ApprovalId, ConversationId, DataClassification, EffectId, PrincipalId, RunId, StepId,
    Timestamp, TurnId, WorkerId,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::StoreRetryClass;

/// The only execution-checkpoint schema understood by this release.
pub const EXECUTION_CHECKPOINT_SCHEMA_VERSION: u32 = 1;

/// The only approval-subject schema understood by this release.
pub const APPROVAL_SUBJECT_SCHEMA_VERSION: u32 = 1;

/// Maximum number of approval rows returned by one store query.
pub const MAX_APPROVAL_PAGE_SIZE: u32 = 100;

/// Durable approval lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    /// Awaiting one authorized decision.
    Pending,
    /// Approved for exactly one execution claim.
    Approved,
    /// Rejected without effect I/O.
    Denied,
    /// The run deadline elapsed before a human decision.
    Expired,
    /// The parent run or session was cancelled.
    Cancelled,
}

/// One-shot decision accepted by the coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    /// Authorize the exact subject once.
    AllowOnce,
    /// Reject the exact subject once and continue with a typed tool error.
    RejectOnce,
    /// Expire the request at its durable deadline.
    Expire,
    /// Cancel the request with its parent run/session.
    Cancel,
}

impl ApprovalDecision {
    /// Return the terminal approval state produced by this decision.
    #[must_use]
    pub const fn status(self) -> ApprovalStatus {
        match self {
            Self::AllowOnce => ApprovalStatus::Approved,
            Self::RejectOnce => ApprovalStatus::Denied,
            Self::Expire => ApprovalStatus::Expired,
            Self::Cancel => ApprovalStatus::Cancelled,
        }
    }
}

/// Kind of principal making a durable approval decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPrincipalType {
    /// An authenticated human operator.
    Human,
    /// An authenticated service acting on cancellation or expiry authority.
    Service,
    /// A quorum decision. The quorum proof remains in the policy/audit layer.
    Quorum,
}

/// Durable effect state understood by approval checkpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointEffectStatus {
    /// Waiting for an approval decision.
    AwaitingApproval,
    /// Approved and ready for the exact continuation to claim.
    Approved,
    /// Rejected without effect I/O.
    Denied,
    /// Expired without effect I/O.
    Expired,
    /// Cancelled without effect I/O.
    Cancelled,
}

/// One effect and its durable state inside an executor checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointEffect {
    /// Stable effect identity.
    pub effect_id: EffectId,
    /// Approval-relevant lifecycle state.
    pub status: CheckpointEffectStatus,
}

/// Versioned canonical subject authorized by one approval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalSubject {
    /// Serialized subject schema version.
    pub schema_version: u32,
    /// Exact owning conversation.
    pub conversation_id: ConversationId,
    /// Exact executor turn.
    pub turn_id: TurnId,
    /// Exact normalized tool-call step.
    pub step_id: StepId,
    /// Exact run.
    pub run_id: RunId,
    /// Exact agent whose tool allowlist produced the call.
    pub agent_id: AgentId,
    /// Exact effect intent.
    pub effect_id: EffectId,
    /// Stable provider/tool-call identifier.
    pub tool_call_id: String,
    /// Canonical registered tool name.
    pub tool_name: String,
    /// Validated canonical arguments. These are sensitive checkpoint data and
    /// must never be copied into logs or approval event projections.
    pub validated_arguments: serde_json::Value,
    /// Required grant action.
    pub required_action: String,
    /// Required grant resource.
    pub required_resource: String,
    /// Immutable lexical working-directory provenance, when applicable.
    pub working_directory: Option<String>,
    /// Canonical security/tenant/custody scope used by policy evaluation.
    pub security_scope: serde_json::Value,
    /// Digest of the exact registered tool specification.
    pub tool_spec_digest: String,
    /// Digest of the policy snapshot that explicitly escalated this subject.
    pub policy_snapshot_digest: String,
    /// Digest of the complete versioned canonical subject.
    pub subject_digest: String,
}

/// Typed executor state required to recover after process replacement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionCheckpoint {
    /// Serialized checkpoint schema version.
    pub schema_version: u32,
    /// Exact run being resumed.
    pub run_id: RunId,
    /// Exact executor turn being resumed.
    pub turn_id: TurnId,
    /// Exact interaction conversation.
    pub conversation_id: ConversationId,
    /// Exact agent configuration owner.
    pub agent_id: AgentId,
    /// Canonical model identifier.
    pub model_id: String,
    /// Canonical executor/provider identifier.
    pub executor_id: String,
    /// Typed inference messages, including the complete assistant tool-call
    /// group. The schema is executor-owned but versioned by this envelope.
    pub messages: serde_json::Value,
    /// Complete parallel tool-call group needed for deterministic reduction.
    pub tool_calls: serde_json::Value,
    /// Next model-turn sequence.
    pub next_model_turn: u32,
    /// Next normalized step sequence.
    pub next_step_sequence: u32,
    /// Next effect sequence.
    pub next_effect_sequence: u64,
    /// Exact accumulated usage snapshot.
    pub accumulated_usage: serde_json::Value,
    /// Exact decimal cost representation, when known.
    pub accumulated_cost: Option<String>,
    /// Durable run deadline.
    pub deadline_at: Option<Timestamp>,
    /// Retry classification for the pending effect boundary.
    pub retry_class: StoreRetryClass,
    /// Current approval-relevant effect states.
    pub effects: Vec<CheckpointEffect>,
    /// Monotonic expected-state version, starting at one.
    pub version: u64,
    /// Storage/output classification of the checkpoint payload.
    pub classification: DataClassification,
    /// Retention expiry, when configured.
    pub retention_expires_at: Option<Timestamp>,
    /// Integrity digest of the canonical checkpoint contents.
    pub integrity_digest: String,
}

/// Runtime state of a stored checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointStatus {
    /// Paused pending an approval decision; not resumable.
    PausedForApproval,
    /// Ready for a recovery worker to lease.
    Resumable,
    /// Leased by exactly one recovery worker.
    Leased,
    /// Retained terminal tombstone.
    Terminal,
}

/// Stored checkpoint plus its lease metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredCheckpoint {
    /// Versioned checkpoint payload.
    pub checkpoint: ExecutionCheckpoint,
    /// Recovery lifecycle state.
    pub status: CheckpointStatus,
    /// Worker currently holding the recovery lease.
    pub lease_owner: Option<WorkerId>,
    /// Durable lease expiry.
    pub lease_expires_at: Option<Timestamp>,
    /// Initial persistence time.
    pub created_at: Timestamp,
    /// Last successful CAS time.
    pub updated_at: Timestamp,
}

/// Checkpoint leased for recovery.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeasedCheckpoint {
    /// Stored checkpoint after the lease CAS.
    pub stored: StoredCheckpoint,
}

/// State requested after a checkpoint progress commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointCommitStatus {
    /// More executor work remains.
    Resumable,
    /// The run reached a terminal state; retain only a tombstone.
    Terminal,
}

/// Replacement payload for `commit_progress`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckpointProgress {
    /// Worker that must own the current lease.
    pub worker_id: WorkerId,
    /// Complete next checkpoint. Its version must equal `expected_version + 1`.
    pub checkpoint: ExecutionCheckpoint,
    /// State to persist after the commit.
    pub status: CheckpointCommitStatus,
}

/// Approval request metadata safe to expose after projection redaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequestMetadata {
    /// Short, bounded operation title.
    pub title: String,
    /// Bounded safe description that excludes raw arguments and secrets.
    pub description: String,
    /// Policy reason for escalation.
    pub reason: String,
    /// Tenant scope.
    pub tenant_id: String,
    /// Workspace scope.
    pub workspace_id: String,
    /// Principal authorized to read and resolve this smallest one-principal
    /// approval slice.
    pub authorized_principal_id: PrincipalId,
}

/// Atomic request to persist an effect, approval, checkpoint, run CAS, and event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PauseForApproval {
    /// Stable caller-supplied approval identity.
    pub approval_id: ApprovalId,
    /// Exact run-state revision expected before the pause transaction.
    pub expected_run_state_version: u64,
    /// Canonical authorization subject.
    pub subject: ApprovalSubject,
    /// Full effect payload used by the effect executor after approval.
    pub effect_payload: serde_json::Value,
    /// Effect idempotency key.
    pub effect_idempotency_key: String,
    /// Effect retry classification.
    pub retry_class: StoreRetryClass,
    /// Versioned executor checkpoint.
    pub checkpoint: ExecutionCheckpoint,
    /// Safe request metadata.
    pub metadata: ApprovalRequestMetadata,
    /// Durable decision deadline.
    pub deadline_at: Timestamp,
}

/// Fully persisted approval record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredApproval {
    /// Stable approval identity.
    pub id: ApprovalId,
    /// Canonical subject.
    pub subject: ApprovalSubject,
    /// Current lifecycle status.
    pub status: ApprovalStatus,
    /// Safe request metadata.
    pub metadata: ApprovalRequestMetadata,
    /// Durable decision deadline.
    pub deadline_at: Timestamp,
    /// Persist time.
    pub requested_at: Timestamp,
    /// Authorized decision principal, after resolution.
    pub decided_by: Option<PrincipalId>,
    /// Kind of decision principal.
    pub principal_type: Option<ApprovalPrincipalType>,
    /// Originating surface identifier.
    pub decision_surface: Option<String>,
    /// Optional bounded rationale.
    pub rationale: Option<String>,
    /// Optional bounded one-shot conditions.
    pub conditions: Vec<String>,
    /// Terminal decision, after resolution.
    pub decision: Option<ApprovalDecision>,
    /// Exact run-state revision from which the terminal decision committed.
    pub decision_run_state_version: Option<u64>,
    /// Decision time.
    pub decided_at: Option<Timestamp>,
    /// Stable durable request-event identifier.
    pub requested_event_id: String,
    /// Stable durable decision-event identifier, after resolution.
    pub decision_event_id: Option<String>,
}

/// Scoped approval query. Possession of an approval UUID is insufficient.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalScope {
    /// Tenant scope.
    pub tenant_id: String,
    /// Workspace scope.
    pub workspace_id: String,
    /// Optional exact conversation restriction.
    pub conversation_id: Option<ConversationId>,
    /// Authenticated principal requesting the operation.
    pub principal_id: PrincipalId,
}

/// Bounded offset page requested from an approval query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalPage {
    /// Maximum number of records, from one through 100.
    pub limit: u32,
    /// Number of matching records to skip.
    pub offset: u32,
}

/// Atomic pending-to-terminal approval decision request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveApproval {
    /// Stable approval identity.
    pub approval_id: ApprovalId,
    /// Exact run-state revision expected before the decision transaction.
    pub expected_run_state_version: u64,
    /// Exact effect identity.
    pub effect_id: EffectId,
    /// Exact run identity.
    pub run_id: RunId,
    /// Exact turn identity.
    pub turn_id: TurnId,
    /// Exact conversation identity.
    pub conversation_id: ConversationId,
    /// Exact canonical subject digest.
    pub subject_digest: String,
    /// Tenant/workspace authorization scope.
    pub scope: ApprovalScope,
    /// Authenticated decision principal.
    pub principal_id: PrincipalId,
    /// Kind of authenticated principal.
    pub principal_type: ApprovalPrincipalType,
    /// Stable originating surface name.
    pub surface: String,
    /// One-shot decision.
    pub decision: ApprovalDecision,
    /// Optional bounded rationale.
    pub rationale: Option<String>,
    /// Optional bounded one-shot conditions.
    pub conditions: Vec<String>,
}

/// Whether a resolve call won the CAS or repeated the exact winning decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolveDisposition {
    /// This call committed the pending-to-terminal transition.
    Applied,
    /// The exact same decision was already committed.
    AlreadyApplied,
}

/// Result of a durable approval resolution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolveApprovalResult {
    /// Durable approval after resolution.
    pub approval: StoredApproval,
    /// CAS/idempotency disposition.
    pub disposition: ResolveDisposition,
}

/// Request for the exact continuation to lease an approved effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimApprovedEffect {
    /// Stable approval identity.
    pub approval_id: ApprovalId,
    /// Exact effect identity.
    pub effect_id: EffectId,
    /// Exact run identity.
    pub run_id: RunId,
    /// Exact canonical subject digest.
    pub subject_digest: String,
    /// Checkpoint version the continuation reconstructed.
    pub expected_checkpoint_version: u64,
    /// Worker taking the execution lease.
    pub worker_id: WorkerId,
    /// Requested lease duration.
    #[serde(with = "duration_millis")]
    pub lease_duration: Duration,
}

/// Approved effect after the exact continuation won its lease CAS.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimedEffect {
    /// Stable approval identity.
    pub approval_id: ApprovalId,
    /// Exact effect identity.
    pub effect_id: EffectId,
    /// Exact run identity.
    pub run_id: RunId,
    /// Worker holding the execution lease.
    pub worker_id: WorkerId,
    /// Durable lease expiry.
    pub lease_expires_at: Timestamp,
    /// Exact effect payload persisted before approval.
    pub effect_payload: serde_json::Value,
    /// Retry classification.
    pub retry_class: StoreRetryClass,
    /// Checkpoint version bound to this claim.
    pub checkpoint_version: u64,
}

/// Store-neutral approval/checkpoint error taxonomy.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum ApprovalStoreError {
    /// A required record does not exist.
    #[error("not found: {resource_type} id={id}")]
    NotFound {
        /// Stable resource type.
        resource_type: String,
        /// Stable resource identity.
        id: String,
    },
    /// A uniqueness, race, or different-decision conflict occurred.
    #[error("conflict: {resource_type} id={id}: {message}")]
    Conflict {
        /// Stable resource type.
        resource_type: String,
        /// Stable resource identity.
        id: String,
        /// Safe conflict explanation.
        message: String,
    },
    /// A state-machine or expected-version precondition failed.
    #[error("invalid transition for {resource_type} id={id}: expected {expected}, found {actual}")]
    InvalidTransition {
        /// Stable resource type.
        resource_type: String,
        /// Stable resource identity.
        id: String,
        /// Required prior state/version.
        expected: String,
        /// Observed state/version.
        actual: String,
    },
    /// Conversation, tenant, workspace, or lineage authorization failed.
    #[error("approval scope or lineage does not match")]
    ScopeMismatch,
    /// The supplied canonical subject digest does not match the stored subject.
    #[error("approval subject digest does not match")]
    DigestMismatch,
    /// The approval deadline elapsed before this decision.
    #[error("approval deadline elapsed")]
    DeadlineElapsed,
    /// The checkpoint schema is newer or otherwise unsupported.
    #[error("unsupported checkpoint schema version {version}")]
    UnsupportedCheckpointVersion {
        /// Unsupported stored or supplied version.
        version: u32,
    },
    /// A stored payload failed integrity or decoding validation.
    #[error("approval store integrity error: {message}")]
    Integrity {
        /// Safe integrity description.
        message: String,
    },
    /// Backend operation failed without a safe caller-visible classification.
    #[error("approval store backend error: {message}")]
    Backend {
        /// Safe backend description.
        message: String,
    },
}

/// Atomic approval/effect/run/checkpoint transaction boundary.
#[async_trait]
pub trait ApprovalCoordinatorStore: Send + Sync + 'static {
    /// Atomically pause a running run for one exact approval.
    async fn pause_for_approval(
        &self,
        request: PauseForApproval,
    ) -> Result<StoredApproval, ApprovalStoreError>;

    /// Atomically resolve a pending approval with idempotent identical retry.
    async fn resolve_approval(
        &self,
        request: ResolveApproval,
    ) -> Result<ResolveApprovalResult, ApprovalStoreError>;

    /// Load one approval by exact identity and authorization scope.
    async fn get_approval(
        &self,
        approval_id: ApprovalId,
        scope: ApprovalScope,
    ) -> Result<StoredApproval, ApprovalStoreError>;

    /// List pending approvals within an exact authorization scope.
    async fn list_pending(
        &self,
        scope: ApprovalScope,
        page: ApprovalPage,
    ) -> Result<Vec<StoredApproval>, ApprovalStoreError>;

    /// Lease one approved effect for its exact continuation only.
    async fn claim_approved_effect(
        &self,
        request: ClaimApprovedEffect,
    ) -> Result<ClaimedEffect, ApprovalStoreError>;
}

/// Durable versioned execution-checkpoint boundary.
#[async_trait]
pub trait ExecutionCheckpointStore: Send + Sync + 'static {
    /// Load one checkpoint by run identity.
    async fn get_checkpoint(&self, run_id: RunId) -> Result<StoredCheckpoint, ApprovalStoreError>;

    /// Lease resumable checkpoints using one atomic expected-state update.
    async fn lease_resumable(
        &self,
        worker_id: WorkerId,
        lease_duration: Duration,
        page: ApprovalPage,
    ) -> Result<Vec<LeasedCheckpoint>, ApprovalStoreError>;

    /// Commit progress only from the expected version and current lease owner.
    async fn commit_progress(
        &self,
        expected_version: u64,
        next: CheckpointProgress,
    ) -> Result<u64, ApprovalStoreError>;
}

mod duration_millis {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let millis = u64::try_from(duration.as_millis()).map_err(serde::ser::Error::custom)?;
        serializer.serialize_u64(millis)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        u64::deserialize(deserializer).map(Duration::from_millis)
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "serialized contract fixtures intentionally stop at the exact drift"
)]
mod tests {
    use std::{fmt::Debug, str::FromStr};

    use chrono::TimeZone as _;

    use super::*;

    fn id<T>(suffix: u128) -> T
    where
        T: FromStr,
        T::Err: Debug,
    {
        format!("{suffix:032x}")
            .parse()
            .expect("static identifier fixture")
    }

    fn fixture() -> PauseForApproval {
        let deadline = chrono::Utc
            .with_ymd_and_hms(2026, 8, 6, 12, 0, 0)
            .single()
            .expect("static deadline");
        let effect_id = id(5);
        PauseForApproval {
            approval_id: id(1),
            expected_run_state_version: 0,
            subject: ApprovalSubject {
                schema_version: APPROVAL_SUBJECT_SCHEMA_VERSION,
                conversation_id: id(2),
                turn_id: id(3),
                step_id: id(7),
                run_id: id(4),
                agent_id: id(6),
                effect_id,
                tool_call_id: "call-1".into(),
                tool_name: "filesystem.write".into(),
                validated_arguments: serde_json::json!({"path":"notes.txt"}),
                required_action: "write".into(),
                required_resource: "workspace/notes.txt".into(),
                working_directory: Some("/workspace".into()),
                security_scope: serde_json::json!({"tenant":"tenant-a"}),
                tool_spec_digest: "blake3:tool".into(),
                policy_snapshot_digest: "blake3:policy".into(),
                subject_digest: "blake3:subject".into(),
            },
            effect_payload: serde_json::json!({"kind":"tool_call"}),
            effect_idempotency_key: "effect-key".into(),
            retry_class: StoreRetryClass::NoAutoRetry,
            checkpoint: ExecutionCheckpoint {
                schema_version: EXECUTION_CHECKPOINT_SCHEMA_VERSION,
                run_id: id(4),
                turn_id: id(3),
                conversation_id: id(2),
                agent_id: id(6),
                model_id: "model-a".into(),
                executor_id: "provider-a".into(),
                messages: serde_json::json!([]),
                tool_calls: serde_json::json!([{"id":"call-1"}]),
                next_model_turn: 2,
                next_step_sequence: 3,
                next_effect_sequence: u64::from(u32::MAX) + 17,
                accumulated_usage: serde_json::json!({"input_tokens":10}),
                accumulated_cost: Some("0.001".into()),
                deadline_at: Some(deadline),
                retry_class: StoreRetryClass::NoAutoRetry,
                effects: vec![CheckpointEffect {
                    effect_id,
                    status: CheckpointEffectStatus::AwaitingApproval,
                }],
                version: 1,
                classification: DataClassification::Private,
                retention_expires_at: None,
                integrity_digest: "blake3:checkpoint".into(),
            },
            metadata: ApprovalRequestMetadata {
                title: "Write notes.txt".into(),
                description: "Write one workspace file".into(),
                reason: "write grant required".into(),
                tenant_id: "tenant-a".into(),
                workspace_id: "workspace-a".into(),
                authorized_principal_id: id(8),
            },
            deadline_at: deadline,
        }
    }

    #[test]
    fn pause_contract_has_frozen_serialized_shape() {
        let value = serde_json::to_value(fixture()).expect("serialize fixture");
        assert_eq!(value["subject"]["schema_version"], 1);
        assert_eq!(value["checkpoint"]["schema_version"], 1);
        assert_eq!(value["checkpoint"]["version"], 1);
        assert_eq!(value["expected_run_state_version"], 0);
        assert_eq!(
            value["checkpoint"]["next_effect_sequence"],
            u64::from(u32::MAX) + 17
        );
        assert_eq!(value["retry_class"], "no_auto_retry");
        assert_eq!(value["checkpoint"]["classification"], "private");
        assert_eq!(
            value["subject"]["effect_id"],
            value["checkpoint"]["effects"][0]["effect_id"]
        );
        assert_eq!(
            value["checkpoint"]["effects"][0]["status"],
            "awaiting_approval"
        );
        let back: PauseForApproval = serde_json::from_value(value).expect("deserialize fixture");
        assert_eq!(back, fixture());
    }

    #[test]
    fn claim_duration_serializes_as_milliseconds() {
        let request = ClaimApprovedEffect {
            approval_id: id(1),
            effect_id: id(2),
            run_id: id(3),
            subject_digest: "digest".into(),
            expected_checkpoint_version: 7,
            worker_id: id(4),
            lease_duration: Duration::from_millis(1_500),
        };
        let value = serde_json::to_value(&request).expect("serialize claim");
        assert_eq!(value["lease_duration"], 1_500);
        let back: ClaimApprovedEffect = serde_json::from_value(value).expect("deserialize claim");
        assert_eq!(back, request);
    }
}
