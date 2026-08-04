//! Run event domain types.
//!
//! Events are the ordered, durable observations produced during execution.
//! They serve two purposes:
//!
//! 1. **Durable lifecycle recording** — every state transition is described
//!    by at least one event written atomically with the state change.
//! 2. **Real-time UI streaming** — ephemeral events (streaming tokens,
//!    progress updates) are published live but may be lost on crash.
//!
//! # Invariants
//!
//! - **EVENT-ORD-1:** Within a run, events have a strictly monotonic
//!   `sequence` number. No gaps, no reordering.
//! - **EVENT-ORD-2:** `Durable` events are committed atomically with the
//!   state change they describe.
//! - **EVENT-ORD-3:** `Ephemeral` events are delivered best-effort. Clients
//!   must tolerate gaps.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{
    ArtifactId, EffectAttemptId, EffectId, EffectOutcomeId, EventId, RunId, StepId, TurnId,
};

// ---------------------------------------------------------------------------
// Durability
// ---------------------------------------------------------------------------

/// Persistence requirement for a [`RunEvent`].
///
/// The storage layer uses this to decide whether to write the event
/// synchronously in the same transaction as the state change (`Durable`),
/// or to deliver it best-effort through the live event stream only
/// (`Ephemeral`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Durability {
    /// Must survive crash; required for correctness and audit.
    ///
    /// Written to the event store in the same database transaction as the
    /// state change it describes.
    Durable,
    /// Best-effort delivery. May be lost on crash or if the client is not
    /// connected. Used for streaming tokens, progress, and diagnostics.
    ///
    /// **Note:** This maps to `BestEffort` in some PRD sections; the runtime
    /// implementation uses this variant name.
    Ephemeral,
    /// Written to the event store only if the configured retention policy
    /// permits. Falls back to `Ephemeral` behaviour when retention is
    /// disabled.
    Diagnostic,
}

// ---------------------------------------------------------------------------
// EventKind
// ---------------------------------------------------------------------------

/// The type of observation that a [`RunEvent`] records.
///
/// Lifecycle events (`RunCreated`, `RunCompleted`, …) are always `Durable`.
/// Streaming and diagnostic events may be `Ephemeral` or `Diagnostic`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    // --- Lifecycle: Run (always Durable) ---
    /// The run record was created.
    RunCreated,
    /// The run transitioned from `Created` to `Queued`.
    RunQueued,
    /// The run transitioned from `Queued` to `Running`.
    RunStarted,
    /// The run entered the `AwaitingApproval` state.
    ApprovalRequested {
        /// ID of the approval request artifact/record.
        request_id: String,
    },
    /// The run left `AwaitingApproval` because approval was granted.
    ApprovalGranted {
        /// ID of the approval record.
        approval_id: String,
    },
    /// The run left `AwaitingApproval` because approval was denied.
    ApprovalDenied {
        /// Human-readable denial reason.
        reason: String,
    },
    /// The run reached the `Completing` state.
    RunCompleting,
    /// The run reached the `Completed` terminal state.
    RunCompleted {
        /// The ID of the terminal output artifact.
        output_artifact_id: Option<ArtifactId>,
        /// Total input tokens consumed across all turns.
        #[serde(default)]
        input_tokens: u64,
        /// Total output tokens consumed across all turns.
        #[serde(default)]
        output_tokens: u64,
    },
    /// The run reached the `Failed` terminal state.
    RunFailed {
        /// Human-readable failure reason.
        reason: String,
    },
    /// The run was cancelled.
    RunCancelled {
        /// Human-readable cancellation reason.
        reason: String,
    },
    /// The run timed out.
    RunTimedOut,
    /// A `Failed` run was re-queued for retry.
    RunRetryQueued,

    // --- Lifecycle: Turn (always Durable) ---
    /// A new turn began within the run.
    TurnStarted {
        /// 0-based position of this turn within the run.
        turn_number: u32,
        /// The turn's stable ID.
        turn_id: TurnId,
    },
    /// A turn completed.
    TurnCompleted {
        /// 0-based position.
        turn_number: u32,
        /// The turn's stable ID.
        turn_id: TurnId,
    },

    // --- Lifecycle: Step (always Durable) ---
    /// A step began within a turn.
    StepStarted {
        /// The step's stable ID.
        step_id: StepId,
    },
    /// A step completed.
    StepCompleted {
        /// The step's stable ID.
        step_id: StepId,
    },

    // --- Lifecycle: Effects (always Durable) ---
    /// An [`EffectIntent`](crate::effect::EffectIntent) was committed to the
    /// outbox.
    EffectIntentCreated {
        /// The intent's stable ID.
        intent_id: EffectId,
    },
    /// An [`EffectAttempt`](crate::effect::EffectAttempt) was started by a
    /// worker.
    EffectAttemptStarted {
        /// The attempt's stable ID.
        attempt_id: EffectAttemptId,
    },
    /// An [`EffectOutcome`](crate::effect::EffectOutcome) was recorded.
    EffectOutcomeRecorded {
        /// The outcome's stable ID.
        outcome_id: EffectOutcomeId,
    },
    /// All pending effects for the current turn resolved; the run resumed.
    EffectsResolved,

    // --- Lifecycle: Artifacts (always Durable) ---
    /// An artifact was created.
    ArtifactCreated {
        /// The artifact's stable ID.
        artifact_id: ArtifactId,
    },

    // --- Streaming / progress (Ephemeral) ---
    /// A fragment of streaming text from a model inference.
    StreamingToken {
        /// The text fragment.
        text: String,
    },
    /// A progress update from a long-running step.
    ProgressUpdate {
        /// Human-readable progress message.
        message: String,
        /// Completion percentage in [0.0, 100.0], if known.
        percentage: Option<f32>,
    },

    // --- Tool streaming (Ephemeral) ---
    /// A tool call began.
    ToolCallStarted {
        /// The tool's name.
        tool_name: String,
    },
    /// A tool call completed.
    ToolCallCompleted {
        /// The tool's name.
        tool_name: String,
    },

    // --- Delivery (Durable) ---
    /// Delivery of the run result to a transport began.
    DeliveryStarted,
    /// Delivery completed successfully.
    DeliveryCompleted,

    // --- Diagnostic (Diagnostic durability) ---
    /// A structured diagnostic log entry produced during execution.
    DiagnosticLog {
        /// Log severity level.
        level: LogLevel,
        /// Log message.
        message: String,
    },

    // --- Budget (Durable) ---
    /// A budget resource was consumed.
    BudgetConsumed {
        /// Which resource was consumed.
        resource: String,
        /// Amount consumed (units depend on the resource kind).
        amount_str: String,
    },
    /// A budget is running low.
    BudgetWarning {
        /// Which resource is low.
        resource: String,
        /// Remaining amount (units depend on the resource kind).
        remaining_str: String,
    },

    // --- Metadata (Durable) ---
    /// Metadata drift was detected on a chain.
    MetadataDriftDetected {
        /// The chain where drift was detected.
        chain_id: String,
        /// The pinned (trusted) metadata hash.
        pinned_hash: String,
        /// The current on-chain metadata hash.
        current_hash: String,
    },
}

/// Severity level for diagnostic log events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    /// Very fine-grained tracing; disabled in production by default.
    Trace,
    /// Debugging information; useful during development.
    Debug,
    /// Informational messages about normal operation.
    Info,
    /// Warning conditions that do not prevent operation but deserve attention.
    Warn,
    /// Error conditions that indicate a failure has occurred.
    Error,
}

// ---------------------------------------------------------------------------
// EventCorrelation
// ---------------------------------------------------------------------------

/// Correlation identifiers linking an event to the specific run component
/// that produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EventCorrelation {
    /// Always present: the run this event belongs to.
    pub run_id: RunId,
    /// The turn, if this event occurred within a turn.
    pub turn_id: Option<TurnId>,
    /// The step, if this event occurred within a step.
    pub step_id: Option<StepId>,
    /// The effect intent, if this event describes an effect.
    pub effect_intent_id: Option<EffectId>,
    /// The effect attempt, if this event describes one attempt.
    pub effect_attempt_id: Option<EffectAttemptId>,
}

// ---------------------------------------------------------------------------
// RunEvent
// ---------------------------------------------------------------------------

/// An ordered, durable observation produced during run execution.
///
/// All events for a run are stored in the event store with a strictly
/// monotonic `sequence` number. Projections, audit tools, and recovery
/// replay all read from this store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunEvent {
    /// Stable, globally unique identifier for this event.
    pub id: EventId,
    /// The run this event belongs to.
    pub run_id: RunId,
    /// Monotonically increasing sequence number within this run.
    ///
    /// **INV:** No two events for the same run share a `sequence` value.
    /// The event store enforces this with a `UNIQUE(run_id, sequence)` index.
    pub sequence: u64,
    /// The type of observation.
    pub kind: EventKind,
    /// Persistence requirement.
    pub durability: Durability,
    /// Correlation to the specific run component that produced this event.
    pub correlation: EventCorrelation,
    /// A parent event ID, if this event was directly caused by another event
    /// (e.g. `EffectIntentCreated` caused by `TurnStarted`).
    pub causation_id: Option<EventId>,
    /// When this event was produced.
    pub timestamp: DateTime<Utc>,
}

impl RunEvent {
    /// Create a minimal durable lifecycle event.
    #[must_use]
    pub fn new_durable(
        id: EventId,
        run_id: RunId,
        sequence: u64,
        kind: EventKind,
        correlation: EventCorrelation,
    ) -> Self {
        Self {
            id,
            run_id,
            sequence,
            kind,
            durability: Durability::Durable,
            correlation,
            causation_id: None,
            timestamp: Utc::now(),
        }
    }

    /// Create a minimal ephemeral streaming event.
    #[must_use]
    pub fn new_ephemeral(
        id: EventId,
        run_id: RunId,
        sequence: u64,
        kind: EventKind,
    ) -> Self {
        Self {
            id,
            run_id: run_id.clone(),
            sequence,
            kind,
            durability: Durability::Ephemeral,
            correlation: EventCorrelation {
                run_id,
                ..Default::default()
            },
            causation_id: None,
            timestamp: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{EventId, RunId};

    fn make_correlation(run_id: RunId) -> EventCorrelation {
        EventCorrelation {
            run_id,
            ..Default::default()
        }
    }

    // --- Durability ---

    #[test]
    fn durability_serde_round_trip() {
        let cases = [
            (Durability::Durable, r#""durable""#),
            (Durability::Ephemeral, r#""ephemeral""#),
            (Durability::Diagnostic, r#""diagnostic""#),
        ];
        for (variant, expected_json) in &cases {
            let json = serde_json::to_string(variant).unwrap();
            assert_eq!(&json, expected_json, "serialization mismatch");
            let back: Durability = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, &back);
        }
    }

    // --- EventKind ---

    #[test]
    fn event_kind_run_created_serde() {
        let kind = EventKind::RunCreated;
        let json = serde_json::to_string(&kind).unwrap();
        let back: EventKind = serde_json::from_str(&json).unwrap();
        assert_eq!(kind, back);
    }

    #[test]
    fn event_kind_run_failed_with_reason_serde() {
        let kind = EventKind::RunFailed {
            reason: "unrecoverable error".into(),
        };
        let json = serde_json::to_string(&kind).unwrap();
        let back: EventKind = serde_json::from_str(&json).unwrap();
        assert_eq!(kind, back);
    }

    #[test]
    fn event_kind_turn_started_serde() {
        let turn_id = TurnId::new();
        let kind = EventKind::TurnStarted {
            turn_number: 3,
            turn_id,
        };
        let json = serde_json::to_string(&kind).unwrap();
        let back: EventKind = serde_json::from_str(&json).unwrap();
        assert_eq!(kind, back);
    }

    #[test]
    fn event_kind_effect_intent_created_serde() {
        let intent_id = EffectId::new();
        let kind = EventKind::EffectIntentCreated { intent_id };
        let json = serde_json::to_string(&kind).unwrap();
        let back: EventKind = serde_json::from_str(&json).unwrap();
        assert_eq!(kind, back);
    }

    #[test]
    fn event_kind_streaming_token_serde() {
        let kind = EventKind::StreamingToken {
            text: "Hello, ".into(),
        };
        let json = serde_json::to_string(&kind).unwrap();
        let back: EventKind = serde_json::from_str(&json).unwrap();
        assert_eq!(kind, back);
    }

    #[test]
    fn event_kind_approval_denied_serde() {
        let kind = EventKind::ApprovalDenied {
            reason: "insufficient evidence".into(),
        };
        let json = serde_json::to_string(&kind).unwrap();
        let back: EventKind = serde_json::from_str(&json).unwrap();
        assert_eq!(kind, back);
    }

    // --- RunEvent ---

    #[test]
    fn run_event_new_durable_has_correct_durability() {
        let run_id = RunId::new();
        let evt = RunEvent::new_durable(
            EventId::new(),
            run_id.clone(),
            1,
            EventKind::RunCreated,
            make_correlation(run_id),
        );
        assert_eq!(evt.durability, Durability::Durable);
        assert_eq!(evt.sequence, 1);
    }

    #[test]
    fn run_event_new_ephemeral_has_correct_durability() {
        let run_id = RunId::new();
        let evt = RunEvent::new_ephemeral(
            EventId::new(),
            run_id,
            5,
            EventKind::StreamingToken {
                text: "hi".into(),
            },
        );
        assert_eq!(evt.durability, Durability::Ephemeral);
        assert_eq!(evt.sequence, 5);
    }

    #[test]
    fn run_event_serde_round_trip() {
        let run_id = RunId::new();
        let evt = RunEvent::new_durable(
            EventId::new(),
            run_id.clone(),
            42,
            EventKind::RunCompleted {
                output_artifact_id: Some(ArtifactId::new()),
                input_tokens: 0,
                output_tokens: 0,
            },
            make_correlation(run_id),
        );
        let json = serde_json::to_string(&evt).unwrap();
        let back: RunEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(evt.id, back.id);
        assert_eq!(evt.sequence, back.sequence);
        assert_eq!(evt.durability, back.durability);
        assert_eq!(evt.kind, back.kind);
    }

    #[test]
    fn monotonic_sequence_invariant_can_be_checked() {
        // Demonstrate that sequence numbers are numeric and comparable.
        let run_id = RunId::new();
        let correlation = make_correlation(run_id.clone());
        let evt1 = RunEvent::new_durable(
            EventId::new(),
            run_id.clone(),
            1,
            EventKind::RunCreated,
            correlation.clone(),
        );
        let evt2 = RunEvent::new_durable(
            EventId::new(),
            run_id,
            2,
            EventKind::RunQueued,
            correlation,
        );
        assert!(evt1.sequence < evt2.sequence);
    }

    // --- EventCorrelation ---

    #[test]
    fn event_correlation_default_has_only_run_id() {
        let run_id = RunId::new();
        let correlation = EventCorrelation {
            run_id: run_id.clone(),
            ..Default::default()
        };
        assert_eq!(correlation.run_id, run_id);
        assert!(correlation.turn_id.is_none());
        assert!(correlation.step_id.is_none());
        assert!(correlation.effect_intent_id.is_none());
        assert!(correlation.effect_attempt_id.is_none());
    }
}
