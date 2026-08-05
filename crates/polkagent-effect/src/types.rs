//! Domain types for the effect pipeline.
//!
//! This module defines the full set of domain types used by `polkagent-effect`:
//! - [`EffectIntent`] — the durable pre-I/O record of what should happen.
//! - [`EffectAttempt`] — one claim-and-execute lifecycle for an intent.
//! - [`EffectOutcome`] — the immutable post-I/O result.
//! - [`EffectIntentState`] — state machine for an intent's progression.
//! - [`AttemptState`] — state machine for an attempt's progression.
//! - [`OutcomeResult`] — the five terminal outcomes an attempt can produce.
//! - [`EffectKind`] — discriminant for the type of external I/O.
//! - [`ApprovalType`] — who or what granted/denied approval.
//! - [`ApprovalRecord`] — the durable record of an approval decision.
//!
//! ## Key invariants (from PRD-03 §5.5)
//!
//! - **EFF-INV-1:** An `EffectIntent` is committed BEFORE any external I/O.
//! - **EFF-INV-2:** Each `EffectAttempt` has a time-limited lease. Expiry
//!   triggers retry or unknown-outcome handling per retry class.
//! - **EFF-INV-3:** An `EffectOutcome` is immutable once recorded.
//! - **EFF-INV-4:** The idempotency key is stable across retries of the same
//!   intent.
//! - **EFF-INV-5:** `NoAutoRetry` intents are never automatically retried.

use std::time::Duration;

use chrono::{DateTime, Utc};
use polkagent_card::ActionCard;
use serde::{Deserialize, Serialize};

use polkagent_core::{
    EffectAttemptId, EffectId, EffectOutcomeId, RetryClass, RunId, StepId, TurnId, WorkerId,
};

use crate::idempotency::IdempotencyKey;

// ---------------------------------------------------------------------------
// EffectKind
// ---------------------------------------------------------------------------

/// Discriminant tag identifying the type of external I/O that an effect
/// performs. Used as input to [`IdempotencyKey`] generation and lease-duration
/// defaults.
///
/// The variants here name the categories described in PRD-03 §5.2. A real
/// implementation would carry the full variant data (route, input, etc.);
/// for the effect-pipeline crate those payloads are stored in the `payload`
/// JSON blob in the store and only the discriminant is needed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    /// Call a model API (inference).
    ModelCall,
    /// Invoke a tool (file I/O, shell, external API, etc.).
    ToolCall,
    /// Request a signature from an isolated signer.
    SignatureRequest,
    /// Broadcast a signed transaction to a chain.
    Broadcast,
    /// Observe transaction finality on a chain.
    FinalityWatch,
    /// Deliver a message or result through a transport.
    Delivery,
    /// Read chain state or metadata (idempotent query).
    ChainRead,
    /// Run a simulation or dry-run (idempotent query).
    Simulation,
    /// Harness-specific operation (start/resume session, etc.).
    HarnessOperation,
}

impl EffectKind {
    /// Returns the string discriminant used in idempotency key generation.
    #[must_use]
    pub fn discriminant_str(self) -> &'static str {
        match self {
            Self::ModelCall => "model_call",
            Self::ToolCall => "tool_call",
            Self::SignatureRequest => "signature_request",
            Self::Broadcast => "broadcast",
            Self::FinalityWatch => "finality_watch",
            Self::Delivery => "delivery",
            Self::ChainRead => "chain_read",
            Self::Simulation => "simulation",
            Self::HarnessOperation => "harness_operation",
        }
    }

    /// Default lease duration for a claim on this effect kind.
    ///
    /// - Short-lived queries (reads, simulations) get 30 s — they are fast
    ///   and idempotent so quick expiry enables prompt recovery.
    /// - Chain submissions and signature requests get 120 s — the external
    ///   round-trip may be slow and the retry class is `NoAutoRetry`.
    /// - All others default to 60 s.
    #[must_use]
    pub fn default_lease_duration(self) -> Duration {
        match self {
            Self::ChainRead | Self::Simulation => Duration::from_secs(30),
            Self::SignatureRequest | Self::Broadcast | Self::FinalityWatch => {
                Duration::from_secs(120)
            }
            _ => Duration::from_secs(60),
        }
    }

    /// The [`RetryClass`] that should be assigned to new intents of this kind
    /// when no explicit class is specified.
    #[must_use]
    pub fn default_retry_class(self) -> RetryClass {
        match self {
            Self::ChainRead | Self::Simulation | Self::ModelCall => RetryClass::Idempotent,
            Self::ToolCall | Self::HarnessOperation | Self::Delivery => {
                RetryClass::CheckBeforeRetry
            }
            Self::SignatureRequest | Self::Broadcast | Self::FinalityWatch => {
                RetryClass::NoAutoRetry
            }
        }
    }
}

// ---------------------------------------------------------------------------
// EffectPriority
// ---------------------------------------------------------------------------

/// Scheduling priority for effect workers. Higher priority intents are claimed
/// before lower priority ones when multiple intents are pending.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum EffectPriority {
    Low = 0,
    #[default]
    Normal = 1,
    High = 2,
    Critical = 3,
}

// ---------------------------------------------------------------------------
// EffectIntent
// ---------------------------------------------------------------------------

/// The durable, pre-I/O record of what should happen.
///
/// An intent is created by the reducer and committed to the outbox atomically
/// with the run state transition. The intent exists BEFORE any external action
/// occurs (EFF-INV-1). It is the "command" side of the effect pipeline.
///
/// The full kind-specific payload (model parameters, tool input, etc.) is
/// stored in the `payload` field as an opaque JSON value. The pipeline layer
/// only needs the discriminant and metadata to manage the claim lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectIntent {
    /// Stable identifier for this intent.
    pub id: EffectId,
    /// The run this intent belongs to.
    pub run_id: RunId,
    /// The turn this intent was produced in.
    pub turn_id: TurnId,
    /// The step within the turn that requested this effect.
    pub step_id: StepId,

    /// Discriminant describing what type of external I/O to perform.
    pub kind: EffectKind,

    /// Stable idempotency key for deduplication (EFF-INV-4).
    pub idempotency_key: IdempotencyKey,

    /// Ordering within the run's outbox. Lower sequence numbers are processed
    /// first within the same priority tier.
    pub sequence: u64,

    /// Current state in the intent state machine.
    pub state: EffectIntentState,

    /// Deadline after which the effect is considered timed out.
    pub deadline: Option<DateTime<Utc>>,

    /// Upper bound on the number of attempts before the intent is abandoned.
    pub max_attempts: u32,

    /// Number of attempts that have been made so far.
    pub attempt_count: u32,

    /// Retry class controlling auto-retry behaviour.
    pub retry_class: RetryClass,

    /// Scheduling priority.
    pub priority: EffectPriority,

    /// Opaque JSON payload containing kind-specific parameters.
    pub payload: serde_json::Value,

    /// When this intent was first persisted (UTC).
    pub created_at: DateTime<Utc>,

    /// When this intent was last resolved, if ever.
    pub resolved_at: Option<DateTime<Utc>>,

    /// Action card generated for this intent, if applicable.
    ///
    /// Present for signable effects (e.g., `SignatureRequest`, `Broadcast`)
    /// and absent for read-only effects (e.g., `ChainRead`, `Simulation`).
    /// The card is generated before the intent is persisted so it is
    /// available for display to the user during the approval flow.
    ///
    /// Defaults to `None` for backward compatibility with serialized intents
    /// that predate this field.
    #[serde(default)]
    pub action_card: Option<ActionCard>,
}

impl EffectIntent {
    /// Attach an [`ActionCard`] to this intent.
    ///
    /// The card captures all canonical fields needed to display a safety
    /// review to the user before the effect is approved and executed.
    /// Calling this after construction (but before persisting) is the
    /// intended pattern in the orchestrator turn loop.
    pub fn attach_card(&mut self, card: ActionCard) {
        self.action_card = Some(card);
    }

    /// Returns `true` if this intent is in a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.state,
            EffectIntentState::Resolved { .. } | EffectIntentState::Superseded { .. }
        )
    }

    /// Returns `true` if this intent can be claimed by a worker.
    #[must_use]
    pub fn is_claimable(&self) -> bool {
        self.state == EffectIntentState::Pending
    }

    /// Returns the lease owner and expiry if this intent is currently claimed.
    #[must_use]
    pub fn current_lease(&self) -> Option<(WorkerId, DateTime<Utc>)> {
        match &self.state {
            EffectIntentState::Claimed {
                worker_id,
                lease_expires,
            } => Some((*worker_id, *lease_expires)),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// EffectIntentState
// ---------------------------------------------------------------------------

/// State machine for an effect intent's progression through the pipeline.
///
/// ```text
///         ┌─────────┐
///         │ Pending │
///         └────┬────┘
///              │ claim (worker lease)
///         ┌────▼─────┐
///         │ Claimed  │
///         └────┬─────┘
///              │ begin I/O
///         ┌────▼──────┐
///         │ Executing │
///         └──┬──┬──┬──┘
///      ┌─────┘  │  └────────┐
///      │        │           │
/// ┌────▼───┐ ┌──▼─────┐ ┌──▼──────────┐
/// │Resolved│ │Retrying│ │  Superseded  │
/// └────────┘ └──┬─────┘ └─────────────┘
///               │
///          (returns to Pending)
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tag", rename_all = "snake_case")]
pub enum EffectIntentState {
    /// Intent committed to outbox. No worker has claimed it.
    Pending,

    /// A worker holds a time-limited lease. No other worker may claim it.
    Claimed {
        worker_id: WorkerId,
        lease_expires: DateTime<Utc>,
    },

    /// The worker is performing the external I/O.
    Executing {
        worker_id: WorkerId,
        started_at: DateTime<Utc>,
    },

    /// An outcome has been recorded. Terminal.
    Resolved { outcome_id: EffectOutcomeId },

    /// The attempt failed with a retriable error. A new attempt will be
    /// created after backoff. The intent returns to `Pending`.
    Retrying {
        next_attempt_after: DateTime<Utc>,
        last_error: String,
    },

    /// The intent was cancelled (run cancelled or a newer intent replaces it).
    /// Terminal.
    Superseded { reason: SupersessionReason },
}

/// Why an intent was superseded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupersessionReason {
    /// The parent run was cancelled.
    RunCancelled,
    /// A newer, equivalent intent was created.
    Replaced,
    /// The intent exceeded `max_attempts`.
    MaxAttemptsExceeded,
    /// The intent's deadline has passed.
    DeadlineExceeded,
}

// ---------------------------------------------------------------------------
// EffectAttempt
// ---------------------------------------------------------------------------

/// One attempt to execute an `EffectIntent`.
///
/// Multiple attempts may exist for a single intent (retries). Each attempt
/// is immutable after it produces an outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectAttempt {
    /// Stable identifier for this attempt.
    pub id: EffectAttemptId,

    /// The intent this attempt belongs to.
    pub intent_id: EffectId,

    /// The run this attempt belongs to.
    pub run_id: RunId,

    /// 1-based counter: which attempt this is for the intent.
    pub attempt_number: u32,

    /// Idempotency key inherited from the intent (EFF-INV-4).
    pub idempotency_key: IdempotencyKey,

    /// Worker that claimed this attempt.
    pub worker_id: WorkerId,

    /// When the current lease expires. If exceeded without outcome, the
    /// attempt is timed out.
    pub lease_expires: DateTime<Utc>,

    /// Retry class controlling backoff and retry eligibility.
    pub retry_class: RetryClass,

    /// Current state.
    pub state: AttemptState,

    /// When the attempt record was created.
    pub created_at: DateTime<Utc>,

    /// When the worker claimed the lease.
    pub claimed_at: DateTime<Utc>,

    /// When external I/O actually began (after lease acquired).
    pub started_at: Option<DateTime<Utc>>,

    /// When the attempt completed (success, failure, or cancellation).
    pub completed_at: Option<DateTime<Utc>>,
}

impl EffectAttempt {
    /// Returns `true` if the lease has expired relative to `now`.
    #[must_use]
    pub fn is_lease_expired(&self, now: DateTime<Utc>) -> bool {
        now > self.lease_expires
    }
}

/// State machine for one execution attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tag", rename_all = "snake_case")]
pub enum AttemptState {
    /// Attempt created; lease has been issued but I/O not yet started.
    Leased,
    /// External I/O is in progress.
    InProgress,
    /// Attempt completed; see the associated `EffectOutcome`.
    Completed { outcome_id: EffectOutcomeId },
    /// Attempt was cancelled before or during execution.
    Cancelled { reason: CancellationReason },
}

/// Why an attempt was cancelled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationReason {
    RunCancelled,
    LeaseExpired,
    WorkerShutdown,
    ExplicitCancel,
}

// ---------------------------------------------------------------------------
// EffectOutcome
// ---------------------------------------------------------------------------

/// The immutable result observed for one attempt.
///
/// Created exactly once per attempt and never modified (EFF-INV-3). The
/// `digest` field allows integrity verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectOutcome {
    /// Stable identifier for this outcome.
    pub id: EffectOutcomeId,

    /// The attempt that produced this outcome.
    pub attempt_id: EffectAttemptId,

    /// The intent this outcome belongs to.
    pub intent_id: EffectId,

    /// The run this outcome belongs to.
    pub run_id: RunId,

    /// The terminal result.
    pub result: OutcomeResult,

    /// When this outcome was observed (UTC).
    pub observed_at: DateTime<Utc>,

    /// BLAKE3 digest of `serde_json::to_vec(self.result)` for integrity
    /// verification. Computed by the pipeline when recording the outcome.
    pub digest: [u8; 32],
}

/// The five terminal outcomes an attempt can produce.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "variant", rename_all = "snake_case")]
pub enum OutcomeResult {
    /// Effect completed successfully.
    Success {
        /// Opaque JSON payload with kind-specific result data.
        data: serde_json::Value,
    },

    /// Effect failed with a known error.
    Failure {
        /// Error classification.
        error_class: ErrorClass,
        /// Human-readable description.
        message: String,
        /// Whether this failure is eligible for retry.
        retriable: bool,
    },

    /// Effect timed out (lease expired without a result, or external timeout).
    Timeout {
        /// How long the system waited.
        waited_secs: u64,
        /// Whether partial work may have occurred on the external system.
        partial_work_possible: bool,
    },

    /// Effect was cancelled before or during execution.
    Cancelled {
        /// Whether partial work may have occurred.
        partial_work_possible: bool,
        /// Cancellation source.
        reason: CancellationReason,
    },

    /// Outcome cannot be determined. Preserved as-is; never silently promoted
    /// (EFF-INV-6).
    Unknown {
        /// Context for investigation.
        context: String,
        /// Recommended recovery action.
        resolution_hint: ResolutionHint,
    },
}

/// Error classification for `Failure` outcomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    ClientError,
    ServerError,
    NetworkError,
    AuthorizationError,
    ResourceExhaustion,
    ChainError,
}

/// Recommended action for `Unknown` outcomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionHint {
    /// Check the chain for transaction status.
    CheckChain,
    /// Retry the operation.
    RetryOperation,
    /// Requires manual investigation.
    ManualInvestigation,
    /// The effect can be safely abandoned.
    SafeToAbandon,
}

// ---------------------------------------------------------------------------
// ApprovalType
// ---------------------------------------------------------------------------

/// Identifies the principal type that granted or denied an approval decision.
///
/// Used in [`ApprovalRecord`] to record how the approval was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalType {
    /// A human operator approved/denied via the API.
    Human,
    /// A quorum of principals reached a consensus decision.
    Quorum,
    /// An automated service principal made the decision.
    Service,
    /// A pre-configured mandate (policy rule) applied automatically.
    Mandate,
}

// ---------------------------------------------------------------------------
// ApprovalRecord
// ---------------------------------------------------------------------------

/// The durable record of an approval or denial decision for an effect intent.
///
/// Created when `POST /effects/:id/approve` or `POST /effects/:id/deny` is
/// called. Stored alongside the intent so the decision can be audited.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRecord {
    /// Stable identifier for this approval record.
    pub id: uuid::Uuid,
    /// The effect intent this record belongs to.
    pub effect_id: EffectId,
    /// Who or what made the decision.
    pub approval_type: ApprovalType,
    /// Identifier of the principal (user, service, quorum) that made the decision.
    pub principal_id: String,
    /// Optional free-text comment explaining the decision.
    pub comment: Option<String>,
    /// Optional conditions attached to an approval (e.g. `["max_value:100"]`).
    pub conditions: Vec<String>,
    /// Whether the intent was approved or denied.
    pub decision: ApprovalDecision,
    /// When this record was created (UTC).
    pub created_at: DateTime<Utc>,
}

/// The binary outcome of an approval decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approved,
    Denied,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_kind_discriminant_str_is_stable() {
        assert_eq!(EffectKind::ModelCall.discriminant_str(), "model_call");
        assert_eq!(
            EffectKind::SignatureRequest.discriminant_str(),
            "signature_request"
        );
        assert_eq!(EffectKind::Broadcast.discriminant_str(), "broadcast");
    }

    #[test]
    fn chain_read_lease_is_30s() {
        assert_eq!(
            EffectKind::ChainRead.default_lease_duration(),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn signature_request_lease_is_120s() {
        assert_eq!(
            EffectKind::SignatureRequest.default_lease_duration(),
            Duration::from_secs(120)
        );
    }

    #[test]
    fn broadcast_retry_class_is_no_auto_retry() {
        assert_eq!(
            EffectKind::Broadcast.default_retry_class(),
            RetryClass::NoAutoRetry
        );
    }

    #[test]
    fn model_call_retry_class_is_idempotent() {
        assert_eq!(
            EffectKind::ModelCall.default_retry_class(),
            RetryClass::Idempotent
        );
    }

    #[test]
    fn effect_priority_ordering() {
        assert!(EffectPriority::Critical > EffectPriority::High);
        assert!(EffectPriority::High > EffectPriority::Normal);
        assert!(EffectPriority::Normal > EffectPriority::Low);
    }

    #[test]
    fn effect_intent_state_pending_is_claimable() {
        // EffectIntentState::Pending == EffectIntentState::Pending
        let state = EffectIntentState::Pending;
        assert_eq!(state, EffectIntentState::Pending);
    }

    #[test]
    fn outcome_result_success_serde_round_trip() {
        let result = OutcomeResult::Success {
            data: serde_json::json!({"txHash": "0xabc"}),
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: OutcomeResult = serde_json::from_str(&json).unwrap();
        let OutcomeResult::Success { data } = back else {
            panic!("expected Success");
        };
        assert_eq!(data["txHash"], "0xabc");
    }

    #[test]
    fn outcome_result_unknown_serde_round_trip() {
        let result = OutcomeResult::Unknown {
            context: "finality not observed".to_string(),
            resolution_hint: ResolutionHint::CheckChain,
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: OutcomeResult = serde_json::from_str(&json).unwrap();
        let OutcomeResult::Unknown {
            context,
            resolution_hint,
        } = back
        else {
            panic!("expected Unknown");
        };
        assert_eq!(context, "finality not observed");
        assert_eq!(resolution_hint, ResolutionHint::CheckChain);
    }

    // ── Action card integration ───────────────────────────────────────────────

    /// Build a minimal `EffectIntent` for use in tests.
    fn make_intent(kind: EffectKind) -> EffectIntent {
        use polkagent_core::TurnId;
        let run_id = RunId::new();
        let params_hash = IdempotencyKey::hash_params(b"test-intent");
        EffectIntent {
            id: EffectId::new(),
            run_id,
            turn_id: TurnId::new(),
            step_id: StepId::new(),
            kind,
            idempotency_key: IdempotencyKey::generate(run_id, 1, 0, kind, params_hash),
            sequence: 0,
            state: EffectIntentState::Pending,
            deadline: None,
            max_attempts: 3,
            attempt_count: 0,
            retry_class: kind.default_retry_class(),
            priority: EffectPriority::Normal,
            payload: serde_json::json!({"test": true}),
            created_at: chrono::Utc::now(),
            resolved_at: None,
            action_card: None,
        }
    }

    #[test]
    fn attach_card_sets_action_card_field() {
        let mut intent = make_intent(EffectKind::SignatureRequest);
        assert!(intent.action_card.is_none());

        let card = polkagent_card::ActionCardBuilder::new("Sign request")
            .with_payload_hash("abc123")
            .build();
        intent.attach_card(card.clone());

        assert!(intent.action_card.is_some());
        let attached = intent.action_card.as_ref().unwrap();
        assert_eq!(attached.card_id, card.card_id);
    }

    #[test]
    fn action_card_field_survives_serde_round_trip_with_effect_intent() {
        let mut intent = make_intent(EffectKind::Broadcast);
        let card = polkagent_card::ActionCardBuilder::new("Broadcast tx")
            .add_canonical(
                "Pallet",
                "Balances",
                polkagent_card::SectionSource::Metadata,
            )
            .with_payload_hash("deadbeef")
            .build();
        let card_id = card.card_id.clone();
        intent.attach_card(card);

        let json = serde_json::to_string(&intent).expect("serialize");
        let back: EffectIntent = serde_json::from_str(&json).expect("deserialize");

        let attached = back
            .action_card
            .expect("card must survive serde round-trip");
        assert_eq!(attached.card_id, card_id);
        assert_eq!(attached.payload_hash, "deadbeef");
    }

    #[test]
    fn action_card_defaults_to_none_when_absent_in_json() {
        // Simulate a legacy intent serialized before the action_card field was added.
        // IdempotencyKey serializes as a 64-char lowercase hex string (BLAKE3 digest).
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "run_id": "00000000-0000-0000-0000-000000000002",
            "turn_id": "00000000-0000-0000-0000-000000000003",
            "step_id": "00000000-0000-0000-0000-000000000004",
            "kind": "chain_read",
            "idempotency_key": "0000000000000000000000000000000000000000000000000000000000000000",
            "sequence": 0,
            "state": {"tag": "pending"},
            "deadline": null,
            "max_attempts": 3,
            "attempt_count": 0,
            "retry_class": "idempotent",
            "priority": "normal",
            "payload": {},
            "created_at": "2024-01-01T00:00:00Z",
            "resolved_at": null
        }"#;
        let intent: EffectIntent = serde_json::from_str(json)
            .expect("legacy intent (without action_card) must deserialize");
        assert!(
            intent.action_card.is_none(),
            "missing action_card must default to None"
        );
    }
}
