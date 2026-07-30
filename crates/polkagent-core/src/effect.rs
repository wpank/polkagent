//! Effect pipeline domain types.
//!
//! The effect pipeline is Polkagent's core safety mechanism. Every external
//! action — model calls, tool invocations, chain transactions — goes through
//! three durable records:
//!
//! 1. [`EffectIntent`] — the *command*, written before any I/O occurs.
//! 2. [`EffectAttempt`] — one claim/lease/execution lifecycle.
//! 3. [`EffectOutcome`] — the immutable result.
//!
//! # Invariants
//!
//! - **EFF-INV-1:** An `EffectIntent` is committed to the database *before*
//!   any external I/O occurs.
//! - **EFF-INV-2:** Each `EffectAttempt` has a time-limited lease; expired
//!   leases are handled by the lease reaper according to the intent's
//!   [`RetryClass`].
//! - **EFF-INV-3:** An `EffectOutcome` is immutable once written.
//! - **EFF-INV-4:** The idempotency key is stable across retries.
//! - **EFF-INV-5:** `NoAutoRetry` effects are never automatically retried.
//! - **EFF-INV-6:** `Unknown` outcomes are preserved as-is and never silently
//!   collapsed to success or failure.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{ArtifactId, EffectAttemptId, EffectId, EffectOutcomeId, RunId, StepId, TurnId};

// ---------------------------------------------------------------------------
// EffectKind
// ---------------------------------------------------------------------------

/// The category of external work that an [`EffectIntent`] requests.
///
/// This enum mirrors the PRD's `EffectKind` vocabulary and is used for
/// policy evaluation, scheduling, idempotency-key generation, and audit.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    /// Submit a signed transaction to a chain.
    ChainSubmit,
    /// Read state from a chain (query, simulation).
    ChainQuery,
    /// Call a model API (inference).
    ModelInference,
    /// Invoke a registered tool.
    ToolInvocation,
    /// Write a file to the local or remote filesystem.
    FileWrite,
    /// Read a file from the local or remote filesystem.
    FileRead,
    /// Make an outbound HTTP request.
    HttpRequest,
    /// Send a notification or result through a transport.
    Notification,
    /// Request a cryptographic signature from an isolated signer.
    SignatureRequest,
    /// Broadcast a pre-signed transaction payload.
    Broadcast,
    /// Observe finality for a submitted transaction.
    FinalityWatch,
    /// Deliver a message through a chat or webhook transport.
    Delivery,
    /// A harness-specific operation (start session, resume, ping, etc.).
    HarnessOperation,
}

impl std::fmt::Display for EffectKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::ChainSubmit => "chain_submit",
            Self::ChainQuery => "chain_query",
            Self::ModelInference => "model_inference",
            Self::ToolInvocation => "tool_invocation",
            Self::FileWrite => "file_write",
            Self::FileRead => "file_read",
            Self::HttpRequest => "http_request",
            Self::Notification => "notification",
            Self::SignatureRequest => "signature_request",
            Self::Broadcast => "broadcast",
            Self::FinalityWatch => "finality_watch",
            Self::Delivery => "delivery",
            Self::HarnessOperation => "harness_operation",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// RetryClass
// ---------------------------------------------------------------------------

/// Determines how the lease reaper handles a timed-out or failed attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RetryClass {
    /// Safe to retry automatically: read-only operations and idempotent
    /// writes. The same idempotency key is used on each retry.
    #[default]
    Idempotent,
    /// Retry with caution: the worker must check for partial completion
    /// before re-executing. Used for operations that may have partially
    /// succeeded but are not provably idempotent.
    CheckBeforeRetry,
    /// Never automatically retry: signing, broadcasting, fund transfers.
    /// A new intent must be explicitly created by the reducer after
    /// investigating the unknown outcome.
    NoAutoRetry,
}

impl RetryClass {
    /// Returns `true` if the lease reaper may automatically re-queue the
    /// intent after a lease expiry without explicit operator action.
    #[must_use]
    pub fn allows_auto_retry(self) -> bool {
        matches!(self, Self::Idempotent | Self::CheckBeforeRetry)
    }
}

// ---------------------------------------------------------------------------
// EffectIntentState
// ---------------------------------------------------------------------------

/// The state machine state of an [`EffectIntent`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectIntentState {
    /// Committed to the outbox. No worker has claimed it.
    Pending,
    /// A worker holds a time-limited lease. No other worker may claim it.
    Claimed {
        /// The ID of the worker holding the lease.
        worker_id: String,
        /// When the lease expires. After this time the reaper may reclaim.
        lease_expires: DateTime<Utc>,
    },
    /// The worker is performing the external I/O.
    Executing {
        /// The ID of the worker executing this intent.
        worker_id: String,
        /// When execution began.
        started_at: DateTime<Utc>,
    },
    /// An outcome has been recorded. Terminal.
    Resolved {
        /// The ID of the recorded outcome.
        outcome_id: EffectOutcomeId,
    },
    /// The attempt failed with a retriable error. A new attempt will be
    /// created after backoff.
    Retrying {
        /// When the next attempt may begin.
        next_attempt_after: DateTime<Utc>,
        /// Description of the last error.
        last_error: String,
    },
    /// Cancelled because the owning run was cancelled or a newer intent
    /// supersedes this one. Terminal.
    Superseded {
        /// Human-readable reason for supersession.
        reason: String,
    },
}

impl EffectIntentState {
    /// Returns `true` if no further state transitions are permitted.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Resolved { .. } | Self::Superseded { .. })
    }
}

// ---------------------------------------------------------------------------
// IdempotencyKey
// ---------------------------------------------------------------------------

/// A stable, content-addressed key used for deduplication of effect
/// executions.
///
/// The key is derived from `(run_id, intent_sequence, effect_kind_tag,
/// canonical_input_digest)` using SHA-256 so that it is deterministic across
/// process restarts and can be passed to external APIs that support idempotent
/// operations.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Create an idempotency key from a pre-computed hex digest string.
    #[must_use]
    pub fn from_hex(hex: impl Into<String>) -> Self {
        Self(hex.into())
    }

    /// Return the key as a hex string (suitable for HTTP headers and
    /// external API fields).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// EffectIntent
// ---------------------------------------------------------------------------

/// The durable pre-I/O command recorded before any external action occurs.
///
/// An `EffectIntent` is created by the reducer and committed to the outbox
/// atomically with the run state transition. Workers claim intents from the
/// outbox using a time-limited lease.
///
/// **INV:** An `EffectIntent` is durable before the corresponding I/O begins.
/// If the process crashes after creating the intent but before executing it,
/// the intent remains in the outbox for a worker to claim on restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectIntent {
    /// Stable, globally unique identifier.
    pub id: EffectId,
    /// The run that produced this intent.
    pub run_id: RunId,
    /// The turn within which this intent was produced.
    pub turn_id: TurnId,
    /// The step that requested this effect.
    pub step_id: StepId,
    /// What kind of external action is requested.
    pub kind: EffectKind,
    /// Stable deduplication key. Passed to external systems that support
    /// idempotent operations.
    pub idempotency_key: IdempotencyKey,
    /// Monotonically increasing position within the run's outbox.
    pub sequence: u64,
    /// Current state of this intent in the outbox processing pipeline.
    pub state: EffectIntentState,
    /// How the lease reaper should handle a timed-out attempt.
    pub retry_class: RetryClass,
    /// Maximum number of execution attempts (1 for `NoAutoRetry`).
    pub max_attempts: u32,
    /// How many attempts have been created so far.
    pub attempt_count: u32,
    /// When this intent was created.
    pub created_at: DateTime<Utc>,
    /// When this intent was resolved (reached a terminal state).
    pub resolved_at: Option<DateTime<Utc>>,
    /// Wall-clock deadline for this effect. If exceeded without resolution
    /// the intent is marked timed-out.
    pub deadline: Option<DateTime<Utc>>,
    /// Opaque JSON payload describing the effect's inputs. The schema
    /// depends on the `kind`.
    pub payload_json: Option<String>,
}

// ---------------------------------------------------------------------------
// AttemptState
// ---------------------------------------------------------------------------

/// The state machine state of an [`EffectAttempt`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptState {
    /// Attempt record created; lease acquired.
    Created,
    /// Worker has the lease and is about to begin I/O.
    Leased,
    /// External I/O is in progress.
    InProgress,
    /// Outcome has been recorded. Terminal.
    Completed {
        /// The ID of the outcome record.
        outcome_id: EffectOutcomeId,
    },
    /// The attempt was cancelled (run cancelled or lease superseded).
    Cancelled {
        /// Human-readable reason.
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// EffectAttempt
// ---------------------------------------------------------------------------

/// One execution lifecycle (claim → execute → outcome) for an
/// [`EffectIntent`].
///
/// Multiple `EffectAttempt` records may exist for a single intent if retries
/// occur. Each attempt is immutable after it produces an outcome.
///
/// **INV:** At most one `EffectAttempt` for a given intent is in `Leased` or
/// `InProgress` state at any time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectAttempt {
    /// Stable, globally unique identifier.
    pub id: EffectAttemptId,
    /// The intent this attempt is executing.
    pub intent_id: EffectId,
    /// The run this attempt belongs to.
    pub run_id: RunId,
    /// Which attempt this is (1-based).
    pub attempt_number: u32,
    /// Idempotency key inherited from the intent.
    pub idempotency_key: IdempotencyKey,
    /// Worker that claimed this attempt.
    pub worker_id: String,
    /// When the lease expires. The reaper reclaims after this time.
    pub lease_expires: DateTime<Utc>,
    /// How the reaper should treat a timed-out lease.
    pub retry_class: RetryClass,
    /// Current state of this attempt.
    pub state: AttemptState,
    /// When this attempt record was created.
    pub created_at: DateTime<Utc>,
    /// When the worker acquired the lease.
    pub claimed_at: DateTime<Utc>,
    /// When external I/O began.
    pub started_at: Option<DateTime<Utc>>,
    /// When this attempt reached a terminal state.
    pub completed_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// OutcomeStatus
// ---------------------------------------------------------------------------

/// The observed result status of one [`EffectAttempt`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeStatus {
    /// Effect completed successfully.
    Success,
    /// Effect failed with a known, categorized error.
    Failure,
    /// Effect timed out before completion.
    Timeout,
    /// Effect was cancelled before it could complete.
    Cancelled,
    /// The outcome cannot be determined.
    ///
    /// **INV:** `Unknown` is never silently converted to `Success` or
    /// `Failure` without fresh, independent evidence. The system preserves
    /// this state and surfaces it to operators.
    Unknown,
}

// ---------------------------------------------------------------------------
// EffectOutcome
// ---------------------------------------------------------------------------

/// The immutable record of what actually happened during one
/// [`EffectAttempt`].
///
/// `EffectOutcome` is created exactly once per attempt and never modified.
/// Subsequent observations (e.g., fresh finality checks) create new
/// `EffectAttempt` and `EffectOutcome` records rather than mutating existing
/// ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectOutcome {
    /// Stable, globally unique identifier.
    pub id: EffectOutcomeId,
    /// The attempt this outcome describes.
    pub attempt_id: EffectAttemptId,
    /// The intent this outcome resolves.
    pub intent_id: EffectId,
    /// The run this outcome belongs to.
    pub run_id: RunId,
    /// The observed result status.
    pub status: OutcomeStatus,
    /// Human-readable error message (present for `Failure`, `Timeout`,
    /// `Cancelled`, and `Unknown` statuses).
    pub error_message: Option<String>,
    /// Typed result data as opaque JSON (present for `Success`).
    pub result_json: Option<String>,
    /// External identifiers produced by this effect (transaction hashes,
    /// block numbers, provider request IDs, etc.).
    pub external_refs: Vec<ExternalRef>,
    /// Artifacts produced by this effect execution.
    pub artifacts: Vec<ArtifactId>,
    /// When the outcome was observed.
    pub observed_at: DateTime<Utc>,
    /// Whether retrying is likely to succeed (`Failure` status only).
    pub retriable: bool,
}

// ---------------------------------------------------------------------------
// ExternalRef
// ---------------------------------------------------------------------------

/// A reference to an external resource produced or consumed by an effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalRef {
    /// A label identifying what this reference represents (e.g.
    /// `"tx_hash"`, `"block_number"`, `"provider_request_id"`).
    pub key: String,
    /// The reference value (free-form string; may be a hex hash, URL, etc.).
    pub value: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{RunId, StepId, TurnId};

    fn make_intent() -> EffectIntent {
        EffectIntent {
            id: EffectId::new(),
            run_id: RunId::new(),
            turn_id: TurnId::new(),
            step_id: StepId::new(),
            kind: EffectKind::ModelInference,
            idempotency_key: IdempotencyKey::from_hex("deadbeef"),
            sequence: 1,
            state: EffectIntentState::Pending,
            retry_class: RetryClass::Idempotent,
            max_attempts: 3,
            attempt_count: 0,
            created_at: Utc::now(),
            resolved_at: None,
            deadline: None,
            payload_json: None,
        }
    }

    // --- EffectKind ---

    #[test]
    fn effect_kind_serde_round_trip() {
        let kind = EffectKind::ChainSubmit;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, r#""chain_submit""#);
        let back: EffectKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, kind);
    }

    #[test]
    fn all_effect_kinds_serialize_without_panic() {
        let kinds = [
            EffectKind::ChainSubmit,
            EffectKind::ChainQuery,
            EffectKind::ModelInference,
            EffectKind::ToolInvocation,
            EffectKind::FileWrite,
            EffectKind::FileRead,
            EffectKind::HttpRequest,
            EffectKind::Notification,
            EffectKind::SignatureRequest,
            EffectKind::Broadcast,
            EffectKind::FinalityWatch,
            EffectKind::Delivery,
            EffectKind::HarnessOperation,
        ];
        for kind in &kinds {
            let json = serde_json::to_string(kind).unwrap();
            let back: EffectKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, &back);
        }
    }

    // --- RetryClass ---

    #[test]
    fn retry_class_serde_round_trip() {
        let cls = RetryClass::NoAutoRetry;
        let json = serde_json::to_string(&cls).unwrap();
        assert_eq!(json, r#""no_auto_retry""#);
        let back: RetryClass = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cls);
    }

    // --- EffectIntentState ---

    #[test]
    fn resolved_and_superseded_are_terminal() {
        assert!(EffectIntentState::Resolved {
            outcome_id: EffectOutcomeId::new()
        }
        .is_terminal());
        assert!(EffectIntentState::Superseded {
            reason: "run cancelled".into()
        }
        .is_terminal());
    }

    #[test]
    fn pending_claimed_executing_retrying_are_not_terminal() {
        let non_terminal = [
            EffectIntentState::Pending,
            EffectIntentState::Claimed {
                worker_id: "w-1".into(),
                lease_expires: Utc::now(),
            },
            EffectIntentState::Executing {
                worker_id: "w-1".into(),
                started_at: Utc::now(),
            },
            EffectIntentState::Retrying {
                next_attempt_after: Utc::now(),
                last_error: "timeout".into(),
            },
        ];
        for state in &non_terminal {
            assert!(
                !state.is_terminal(),
                "{state:?} should not be terminal"
            );
        }
    }

    #[test]
    fn effect_intent_state_serde_round_trip() {
        let state = EffectIntentState::Claimed {
            worker_id: "worker-42".into(),
            lease_expires: Utc::now(),
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: EffectIntentState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);
    }

    // --- IdempotencyKey ---

    #[test]
    fn idempotency_key_as_str() {
        let key = IdempotencyKey::from_hex("aabbcc");
        assert_eq!(key.as_str(), "aabbcc");
    }

    #[test]
    fn idempotency_key_serde_round_trip() {
        let key = IdempotencyKey::from_hex("0102030405");
        let json = serde_json::to_string(&key).unwrap();
        let back: IdempotencyKey = serde_json::from_str(&json).unwrap();
        assert_eq!(key, back);
    }

    // --- EffectIntent ---

    #[test]
    fn effect_intent_serde_round_trip() {
        let intent = make_intent();
        let json = serde_json::to_string(&intent).unwrap();
        let back: EffectIntent = serde_json::from_str(&json).unwrap();
        assert_eq!(intent.id, back.id);
        assert_eq!(intent.kind, back.kind);
        assert_eq!(intent.sequence, back.sequence);
    }

    // --- EffectOutcome ---

    #[test]
    fn outcome_status_unknown_is_preserved() {
        let status = OutcomeStatus::Unknown;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, r#""unknown""#);
        let back: OutcomeStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, OutcomeStatus::Unknown);
        // Critical: Unknown must not equal Success or Failure.
        assert_ne!(back, OutcomeStatus::Success);
        assert_ne!(back, OutcomeStatus::Failure);
    }

    #[test]
    fn effect_outcome_serde_round_trip() {
        let outcome = EffectOutcome {
            id: EffectOutcomeId::new(),
            attempt_id: EffectAttemptId::new(),
            intent_id: EffectId::new(),
            run_id: RunId::new(),
            status: OutcomeStatus::Success,
            error_message: None,
            result_json: Some(r#"{"tx_hash":"0xdeadbeef"}"#.into()),
            external_refs: vec![ExternalRef {
                key: "tx_hash".into(),
                value: "0xdeadbeef".into(),
            }],
            artifacts: vec![],
            observed_at: Utc::now(),
            retriable: false,
        };
        let json = serde_json::to_string(&outcome).unwrap();
        let back: EffectOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(outcome.id, back.id);
        assert_eq!(outcome.status, back.status);
        assert_eq!(outcome.retriable, back.retriable);
    }
}
