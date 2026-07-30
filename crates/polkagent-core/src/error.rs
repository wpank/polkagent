//! Error types for the Polkagent platform.
//!
//! [`PolkagentError`] is the top-level error enum for the core crate. Each
//! variant covers a distinct failure category. Higher-level crates
//! (`polkagent-kernel`, `polkagent-app`) define their own error types that
//! wrap or extend this taxonomy.
//!
//! # Design
//!
//! - All variants carry enough context for operators to diagnose the problem
//!   without inspecting raw logs.
//! - `Internal` errors indicate programming defects (invariant violations,
//!   unexpected `None`, etc.) — they should be treated as bugs.
//! - `InvalidTransition` errors are expected during normal operation
//!   (concurrent requests, race conditions) and should be retried or
//!   surfaced to the caller gracefully.

use thiserror::Error;

/// The top-level error type for `polkagent-core`.
#[derive(Debug, Error)]
pub enum PolkagentError {
    // --- Identity / ID errors ---
    /// A UUID or ID string could not be parsed.
    #[error("invalid ID format: {0}")]
    InvalidId(#[from] uuid::Error),

    // --- Serialization errors ---
    /// A JSON serialization or deserialization failure.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    // --- State machine errors ---
    /// A requested state transition is not permitted from the current state.
    #[error("invalid state transition from '{from}' via '{event}'")]
    InvalidTransition {
        /// The state the entity was in when the transition was attempted.
        from: String,
        /// The event or input that triggered the attempted transition.
        event: String,
    },

    /// A state transition was attempted on a terminal state.
    #[error("entity is in a terminal state and cannot transition further")]
    TerminalState,

    // --- Validation errors ---
    /// A domain value failed validation (missing required field, out-of-range
    /// value, etc.).
    #[error("validation failed for field '{field}': {message}")]
    Validation {
        /// Which field or constraint failed.
        field: String,
        /// Human-readable description of the failure.
        message: String,
    },

    // --- Classification / access control errors ---
    /// An operation was rejected because the data classification of an item
    /// does not permit it (e.g. `SecretForbidden` item in context assembly).
    #[error("classification violation ({classification}): {reason}")]
    ClassificationViolation {
        /// Which classification tier caused the rejection.
        classification: String,
        /// What boundary was violated (context assembly, telemetry, export).
        reason: String,
    },

    // --- Artifact integrity errors ---
    /// An artifact's BLAKE3 digest did not match the stored value.
    ///
    /// This indicates tampering or storage corruption and must be treated as
    /// a security event.
    #[error("artifact integrity check failed for artifact '{artifact_id}'")]
    IntegrityCheckFailed {
        /// The artifact whose digest did not verify.
        artifact_id: String,
    },

    // --- Effect pipeline errors ---
    /// An effect was dispatched but no matching worker is registered.
    #[error("no worker available for effect kind '{kind}'")]
    NoWorkerForEffect {
        /// The `EffectKind` discriminant string.
        kind: String,
    },

    /// An idempotency conflict was detected: an effect with the same
    /// idempotency key already exists.
    #[error("idempotency conflict for key '{key}'")]
    IdempotencyConflict {
        /// The conflicting idempotency key.
        key: String,
    },

    /// An effect's lease has expired and no outcome was recorded.
    #[error("effect lease expired for intent '{intent_id}'")]
    LeaseExpired {
        /// The intent whose lease expired.
        intent_id: String,
    },

    // --- Authorization errors ---
    /// An operation was denied by the policy engine.
    #[error("policy denied: {reason}")]
    PolicyDenied {
        /// Why the policy engine denied the request.
        reason: String,
    },

    /// An approval is required before the operation can proceed.
    #[error("approval required for effect '{effect_kind}'")]
    ApprovalRequired {
        /// The effect kind that requires approval.
        effect_kind: String,
    },

    /// An approval reference was tampered with or bound to a different
    /// payload than the one being submitted.
    #[error("approval digest mismatch: approval is bound to a different payload")]
    ApprovalDigestMismatch,

    // --- Budget / resource errors ---
    /// The run's token or cost budget has been exhausted.
    #[error("budget exhausted: {resource} limit reached ({limit})")]
    BudgetExhausted {
        /// Which resource was exhausted (e.g. `"input_tokens"`).
        resource: String,
        /// The limit that was exceeded.
        limit: String,
    },

    // --- Unknown outcome errors ---
    /// An outcome is genuinely indeterminate and cannot be safely assumed to
    /// be success or failure.
    ///
    /// **INV:** This variant is never collapsed to another variant without
    /// independent evidence.
    #[error("effect outcome is unknown: {context}")]
    UnknownOutcome {
        /// What is known about the effect's state.
        context: String,
    },

    // --- Store errors ---
    /// A sequence number conflict in the event store (duplicate sequence
    /// within a run).
    #[error("event sequence conflict: sequence {sequence} already exists for run '{run_id}'")]
    SequenceConflict {
        /// The run ID.
        run_id: String,
        /// The conflicting sequence number.
        sequence: u64,
    },

    // --- Internal / unexpected errors ---
    /// An internal invariant was violated. Indicates a programming defect.
    #[error("internal error: {message}")]
    Internal {
        /// Description of the violated invariant or unexpected condition.
        message: String,
    },

    /// A required feature is not yet implemented.
    #[error("not implemented: {feature}")]
    NotImplemented {
        /// Which feature is missing.
        feature: String,
    },
}

impl PolkagentError {
    /// Construct an `Internal` error from any displayable value.
    pub fn internal(message: impl std::fmt::Display) -> Self {
        Self::Internal {
            message: message.to_string(),
        }
    }

    /// Construct a `Validation` error.
    pub fn validation(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Validation {
            field: field.into(),
            message: message.into(),
        }
    }

    /// Returns `true` if this error indicates an invariant violation that
    /// should never occur in a correct implementation.
    #[must_use]
    pub fn is_invariant_violation(&self) -> bool {
        matches!(
            self,
            Self::Internal { .. }
                | Self::IntegrityCheckFailed { .. }
                | Self::SequenceConflict { .. }
        )
    }

    /// Returns `true` if this error might resolve on retry (transient
    /// conditions).
    #[must_use]
    pub fn is_potentially_transient(&self) -> bool {
        matches!(
            self,
            Self::LeaseExpired { .. } | Self::NoWorkerForEffect { .. }
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_error_constructor() {
        let err = PolkagentError::internal("something went wrong");
        assert!(err.to_string().contains("something went wrong"));
        assert!(err.is_invariant_violation());
    }

    #[test]
    fn validation_error_constructor() {
        let err = PolkagentError::validation("name", "must not be empty");
        assert!(err.to_string().contains("name"));
        assert!(err.to_string().contains("must not be empty"));
        assert!(!err.is_invariant_violation());
    }

    #[test]
    fn terminal_state_error_display() {
        let err = PolkagentError::TerminalState;
        let msg = err.to_string();
        assert!(!msg.is_empty());
    }

    #[test]
    fn invalid_transition_error_display() {
        let err = PolkagentError::InvalidTransition {
            from: "active".into(),
            event: "archive".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("active"));
        assert!(msg.contains("archive"));
    }

    #[test]
    fn integrity_check_failed_is_invariant_violation() {
        let err = PolkagentError::IntegrityCheckFailed {
            artifact_id: "art-123".into(),
        };
        assert!(err.is_invariant_violation());
    }

    #[test]
    fn sequence_conflict_is_invariant_violation() {
        let err = PolkagentError::SequenceConflict {
            run_id: "run-1".into(),
            sequence: 42,
        };
        assert!(err.is_invariant_violation());
    }

    #[test]
    fn lease_expired_is_transient() {
        let err = PolkagentError::LeaseExpired {
            intent_id: "intent-1".into(),
        };
        assert!(err.is_potentially_transient());
    }

    #[test]
    fn policy_denied_is_not_transient() {
        let err = PolkagentError::PolicyDenied {
            reason: "insufficient grant".into(),
        };
        assert!(!err.is_potentially_transient());
        assert!(!err.is_invariant_violation());
    }

    #[test]
    fn unknown_outcome_error_display() {
        let err = PolkagentError::UnknownOutcome {
            context: "chain RPC did not respond".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("chain RPC"));
    }

    #[test]
    fn classification_violation_error_display() {
        let err = PolkagentError::ClassificationViolation {
            classification: "secret_forbidden".into(),
            reason: "item excluded from context assembly".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("secret_forbidden"));
    }

    #[test]
    fn from_uuid_error() {
        let uuid_err = uuid::Uuid::parse_str("not-a-uuid").unwrap_err();
        let err: PolkagentError = uuid_err.into();
        assert!(matches!(err, PolkagentError::InvalidId(_)));
    }

    #[test]
    fn from_serde_json_error() {
        let json_err = serde_json::from_str::<serde_json::Value>("{invalid}").unwrap_err();
        let err: PolkagentError = json_err.into();
        assert!(matches!(err, PolkagentError::Serialization(_)));
    }
}
