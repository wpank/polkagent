//! Event types for the `polkagent-event` crate.
//!
//! This module re-exports and extends the core event types from
//! `polkagent-core` with the full PRD-10 §8.1 event type catalog and
//! payload definitions.
//!
//! # Core types (from `polkagent-core`)
//!
//! - [`RunEvent`] — the ordered event record with sequence numbers.
//! - [`EventKind`] — the typed event payload.
//! - [`Durability`] — persistence class.
//!
//! # Extensions
//!
//! - [`EventType`] — a flat enumeration of all canonical event type names
//!   (used for filtering, projection dispatch, and store queries).
//! - [`EventPayload`] — an alternative flat payload type aligned with the
//!   PRD-10 §8.1 catalog (used in the store trait wire format).
//! - [`TERMINAL_EVENT_TYPES`] — the four terminal event type names.

// Re-export core event types as the primary types in this crate.
pub use polkagent_core::event::{
    Durability, EventCorrelation, EventKind, LogLevel, RunEvent,
};
// Re-export core RunState for projection use.
pub use polkagent_core::run::RunState;

use polkagent_core::DurabilityClass;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Terminal event type names
// ---------------------------------------------------------------------------

/// The canonical `EventKind` variant names for the four terminal run events.
///
/// PRD-10 REQ-EVT-004: at most one terminal event per run.
pub const TERMINAL_EVENT_TYPES: &[&str] = &[
    "run_completed",
    "run_failed",
    "run_cancelled",
    "run_timed_out",
];

// ---------------------------------------------------------------------------
// EventType  (flat catalog for filtering / dispatch)
// ---------------------------------------------------------------------------

/// A flat enumeration of every canonical event type in PRD-10 §8.1.
///
/// Used for:
/// - [`crate::bus`] subscription filters.
/// - [`crate::projection`] event dispatch.
/// - [`polkagent_store_trait::event::EventFilter`] queries.
/// - Store queries by event type name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    // ── 8.1.1  Lifecycle ─────────────────────────────────────────────────
    RunCreated,
    RunQueued,
    RunStarted,
    RunCompleting,
    RunCompleted,
    RunFailed,
    RunCancelled,
    RunTimedOut,
    RunResumed,
    RunRetryQueued,
    TurnStarted,
    TurnCompleted,

    // ── 8.1.2  Streaming ─────────────────────────────────────────────────
    TextDelta,
    ToolCallStarted,
    ToolCallCompleted,
    ThinkingStart,
    ThinkingDelta,
    ThinkingEnd,
    ProgressUpdate,

    // ── 8.1.3  Effects ───────────────────────────────────────────────────
    EffectIntentCreated,
    EffectAttemptStarted,
    EffectOutcomeRecorded,
    EffectRetryScheduled,
    EffectCancelled,
    EffectLeaseExpired,

    // ── 8.1.4  Approval ──────────────────────────────────────────────────
    ApprovalRequested,
    ApprovalGranted,
    ApprovalDenied,
    ApprovalExpired,
    MandateApplied,

    // ── 8.1.5  Error ─────────────────────────────────────────────────────
    ErrorOccurred,
    PanicRecovered,
    IntegrityViolation,
    PolicyDenial,
    ClassificationEscalation,

    // ── 8.1.6  System ────────────────────────────────────────────────────
    SystemStarted,
    SystemShutdown,
    BackupCompleted,
    RetentionEnforced,
    SchemasMigrated,
    ConfigurationChanged,

    // ── Metadata ────────────────────────────────────────────────────────
    MetadataDriftDetected,
}

impl EventType {
    /// Return the canonical snake_case string representation.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RunCreated => "run_created",
            Self::RunQueued => "run_queued",
            Self::RunStarted => "run_started",
            Self::RunCompleting => "run_completing",
            Self::RunCompleted => "run_completed",
            Self::RunFailed => "run_failed",
            Self::RunCancelled => "run_cancelled",
            Self::RunTimedOut => "run_timed_out",
            Self::RunResumed => "run_resumed",
            Self::RunRetryQueued => "run_retry_queued",
            Self::TurnStarted => "turn_started",
            Self::TurnCompleted => "turn_completed",
            Self::TextDelta => "text_delta",
            Self::ToolCallStarted => "tool_call_started",
            Self::ToolCallCompleted => "tool_call_completed",
            Self::ThinkingStart => "thinking_start",
            Self::ThinkingDelta => "thinking_delta",
            Self::ThinkingEnd => "thinking_end",
            Self::ProgressUpdate => "progress_update",
            Self::EffectIntentCreated => "effect_intent_created",
            Self::EffectAttemptStarted => "effect_attempt_started",
            Self::EffectOutcomeRecorded => "effect_outcome_recorded",
            Self::EffectRetryScheduled => "effect_retry_scheduled",
            Self::EffectCancelled => "effect_cancelled",
            Self::EffectLeaseExpired => "effect_lease_expired",
            Self::ApprovalRequested => "approval_requested",
            Self::ApprovalGranted => "approval_granted",
            Self::ApprovalDenied => "approval_denied",
            Self::ApprovalExpired => "approval_expired",
            Self::MandateApplied => "mandate_applied",
            Self::ErrorOccurred => "error_occurred",
            Self::PanicRecovered => "panic_recovered",
            Self::IntegrityViolation => "integrity_violation",
            Self::PolicyDenial => "policy_denial",
            Self::ClassificationEscalation => "classification_escalation",
            Self::SystemStarted => "system_started",
            Self::SystemShutdown => "system_shutdown",
            Self::BackupCompleted => "backup_completed",
            Self::RetentionEnforced => "retention_enforced",
            Self::SchemasMigrated => "schemas_migrated",
            Self::ConfigurationChanged => "configuration_changed",
            Self::MetadataDriftDetected => "metadata_drift_detected",
        }
    }

    /// Returns `true` if this type is a terminal run lifecycle event.
    ///
    /// PRD-10 REQ-EVT-004: at most one terminal event per run.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::RunCompleted | Self::RunFailed | Self::RunCancelled | Self::RunTimedOut
        )
    }

    /// Returns the default [`DurabilityClass`] for this event type.
    #[must_use]
    pub fn default_durability(self) -> DurabilityClass {
        match self {
            // Ephemeral (never persisted)
            Self::TextDelta
            | Self::ThinkingStart
            | Self::ThinkingDelta
            | Self::ThinkingEnd
            | Self::ProgressUpdate => DurabilityClass::Ephemeral,

            // Diagnostic (persisted with bounded retention)
            Self::ToolCallStarted | Self::ToolCallCompleted => DurabilityClass::Diagnostic,

            // Everything else is durable
            _ => DurabilityClass::Durable,
        }
    }

    /// Derive the [`EventType`] from an [`EventKind`] discriminant, returning
    /// `None` for variants not yet catalogued.
    #[must_use]
    pub fn from_kind(kind: &EventKind) -> Option<Self> {
        let t = match kind {
            EventKind::RunCreated => Self::RunCreated,
            EventKind::RunQueued => Self::RunQueued,
            EventKind::RunStarted => Self::RunStarted,
            EventKind::RunCompleting => Self::RunCompleting,
            EventKind::RunCompleted { .. } => Self::RunCompleted,
            EventKind::RunFailed { .. } => Self::RunFailed,
            EventKind::RunCancelled { .. } => Self::RunCancelled,
            EventKind::RunTimedOut => Self::RunTimedOut,
            EventKind::RunRetryQueued => Self::RunRetryQueued,
            EventKind::TurnStarted { .. } => Self::TurnStarted,
            EventKind::TurnCompleted { .. } => Self::TurnCompleted,
            EventKind::EffectIntentCreated { .. } => Self::EffectIntentCreated,
            EventKind::EffectAttemptStarted { .. } => Self::EffectAttemptStarted,
            EventKind::EffectOutcomeRecorded { .. } => Self::EffectOutcomeRecorded,
            EventKind::ApprovalRequested { .. } => Self::ApprovalRequested,
            EventKind::ApprovalGranted { .. } => Self::ApprovalGranted,
            EventKind::ApprovalDenied { .. } => Self::ApprovalDenied,
            EventKind::StreamingToken { .. } => Self::TextDelta,
            EventKind::ProgressUpdate { .. } => Self::ProgressUpdate,
            EventKind::ToolCallStarted { .. } => Self::ToolCallStarted,
            EventKind::ToolCallCompleted { .. } => Self::ToolCallCompleted,
            EventKind::MetadataDriftDetected { .. } => Self::MetadataDriftDetected,
            _ => return None,
        };
        Some(t)
    }
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Wire-format event payload for the store trait (a simplified alternative to
/// `EventKind` for use with `polkagent-store-trait`).
///
/// This type is intentionally separate from `EventKind` so that the store
/// trait remains free of the richer domain types.
pub type EventPayload = serde_json::Value;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_event_types_constant_includes_all_four() {
        assert!(TERMINAL_EVENT_TYPES.contains(&"run_completed"));
        assert!(TERMINAL_EVENT_TYPES.contains(&"run_failed"));
        assert!(TERMINAL_EVENT_TYPES.contains(&"run_cancelled"));
        assert!(TERMINAL_EVENT_TYPES.contains(&"run_timed_out"));
        assert_eq!(TERMINAL_EVENT_TYPES.len(), 4);
    }

    #[test]
    fn terminal_event_types_are_terminal() {
        let terminal = [
            EventType::RunCompleted,
            EventType::RunFailed,
            EventType::RunCancelled,
            EventType::RunTimedOut,
        ];
        for t in terminal {
            assert!(t.is_terminal(), "{t} should be terminal");
            assert!(
                TERMINAL_EVENT_TYPES.contains(&t.as_str()),
                "{t} should be in TERMINAL_EVENT_TYPES"
            );
        }
    }

    #[test]
    fn non_terminal_types_are_not_terminal() {
        assert!(!EventType::RunStarted.is_terminal());
        assert!(!EventType::TurnStarted.is_terminal());
        assert!(!EventType::EffectIntentCreated.is_terminal());
    }

    #[test]
    fn event_type_display_is_snake_case() {
        assert_eq!(EventType::RunCreated.to_string(), "run_created");
        assert_eq!(
            EventType::EffectOutcomeRecorded.to_string(),
            "effect_outcome_recorded"
        );
    }

    #[test]
    fn event_type_default_durability_ephemeral_for_streaming() {
        assert_eq!(
            EventType::TextDelta.default_durability(),
            DurabilityClass::Ephemeral
        );
        assert_eq!(
            EventType::ProgressUpdate.default_durability(),
            DurabilityClass::Ephemeral
        );
    }

    #[test]
    fn event_type_default_durability_durable_for_lifecycle() {
        assert_eq!(
            EventType::RunCreated.default_durability(),
            DurabilityClass::Durable
        );
        assert_eq!(
            EventType::RunCompleted.default_durability(),
            DurabilityClass::Durable
        );
    }

    #[test]
    fn from_kind_maps_lifecycle_events() {
        let cases = [
            (EventKind::RunCreated, EventType::RunCreated),
            (EventKind::RunQueued, EventType::RunQueued),
            (EventKind::RunStarted, EventType::RunStarted),
            (EventKind::RunTimedOut, EventType::RunTimedOut),
        ];
        for (kind, expected) in cases {
            assert_eq!(
                EventType::from_kind(&kind),
                Some(expected),
                "from_kind({kind:?}) should be {expected}"
            );
        }
    }

    #[test]
    fn event_type_serde_round_trip() {
        let t = EventType::EffectOutcomeRecorded;
        let json = serde_json::to_string(&t).unwrap();
        let back: EventType = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }
}
