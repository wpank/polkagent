//! Intent lifecycle: status enum, validated transitions, and state machine.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::PaymentError;

// ---------------------------------------------------------------------------
// IntentStatus
// ---------------------------------------------------------------------------

/// Fine-grained lifecycle states for a payment intent.
///
/// The allowed transitions form a directed graph:
///
/// ```text
/// Drafting -> Proposed -> Verified -> AwaitingApproval -> Ready -> Signing -> Submitted -> Finalized -> Receipted
///                      \-> Refused                                                     \-> Dropped
/// ```
///
/// Any state can also transition to `Refused` when the intent is explicitly
/// rejected (except terminal states `Receipted` and `Dropped`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentStatus {
    /// The intent is being assembled; fields may be incomplete.
    Drafting,
    /// All required fields have been set and the intent is proposed for
    /// verification.
    Proposed,
    /// The intent has passed pre-flight verification checks.
    Verified,
    /// The intent was refused (by verification, approval, or user).
    Refused,
    /// The intent is awaiting explicit human/policy approval.
    AwaitingApproval,
    /// Approval granted; the intent is ready for signing.
    Ready,
    /// The transaction is being signed.
    Signing,
    /// The signed transaction has been submitted to the network.
    Submitted,
    /// The transaction has reached finality on-chain.
    Finalized,
    /// A receipt has been generated and the lifecycle is complete.
    Receipted,
}

impl IntentStatus {
    /// Returns `true` if this status is terminal (no further transitions
    /// are allowed).
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Receipted | Self::Refused)
    }
}

impl std::fmt::Display for IntentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Drafting => "drafting",
            Self::Proposed => "proposed",
            Self::Verified => "verified",
            Self::Refused => "refused",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Ready => "ready",
            Self::Signing => "signing",
            Self::Submitted => "submitted",
            Self::Finalized => "finalized",
            Self::Receipted => "receipted",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// IntentTransition — validated state transitions
// ---------------------------------------------------------------------------

/// Validate whether a transition from `from` to `to` is permitted.
///
/// Returns `Ok(())` if the transition is valid, or a [`PaymentError`] if not.
pub fn validate_transition(from: IntentStatus, to: IntentStatus) -> Result<(), PaymentError> {
    let valid = match from {
        IntentStatus::Drafting => to == IntentStatus::Proposed || to == IntentStatus::Refused,
        IntentStatus::Proposed => to == IntentStatus::Verified || to == IntentStatus::Refused,
        IntentStatus::Verified => {
            to == IntentStatus::AwaitingApproval || to == IntentStatus::Refused
        }
        IntentStatus::AwaitingApproval => to == IntentStatus::Ready || to == IntentStatus::Refused,
        IntentStatus::Ready => to == IntentStatus::Signing || to == IntentStatus::Refused,
        IntentStatus::Signing => to == IntentStatus::Submitted || to == IntentStatus::Refused,
        IntentStatus::Submitted => to == IntentStatus::Finalized || to == IntentStatus::Refused,
        IntentStatus::Finalized => to == IntentStatus::Receipted,
        // Terminal states
        IntentStatus::Refused | IntentStatus::Receipted => false,
    };

    if valid {
        Ok(())
    } else {
        Err(PaymentError::InvalidStatusTransition {
            from: from.to_string(),
            to: to.to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// IntentStateMachine
// ---------------------------------------------------------------------------

/// A state machine that tracks the current lifecycle status of a payment
/// intent and records a timestamped history of all transitions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentStateMachine {
    current_status: IntentStatus,
    history: Vec<(IntentStatus, DateTime<Utc>)>,
}

impl IntentStateMachine {
    /// Create a new state machine starting in [`IntentStatus::Drafting`].
    #[must_use]
    pub fn new() -> Self {
        let now = Utc::now();
        Self {
            current_status: IntentStatus::Drafting,
            history: vec![(IntentStatus::Drafting, now)],
        }
    }

    /// Create a state machine starting in the given status. Useful for
    /// restoring from persistence.
    #[must_use]
    pub fn with_status(status: IntentStatus, history: Vec<(IntentStatus, DateTime<Utc>)>) -> Self {
        Self {
            current_status: status,
            history,
        }
    }

    /// The current lifecycle status.
    #[must_use]
    pub fn current_status(&self) -> IntentStatus {
        self.current_status
    }

    /// The full transition history as `(status, timestamp)` pairs.
    #[must_use]
    pub fn history(&self) -> &[(IntentStatus, DateTime<Utc>)] {
        &self.history
    }

    /// Attempt to transition to `new_status`.
    ///
    /// Returns `Ok(())` if the transition is valid, recording it in the
    /// history with the current UTC timestamp. Returns an error if the
    /// transition is not allowed.
    pub fn transition(&mut self, new_status: IntentStatus) -> Result<(), PaymentError> {
        validate_transition(self.current_status, new_status)?;
        let now = Utc::now();
        self.current_status = new_status;
        self.history.push((new_status, now));
        Ok(())
    }

    /// Returns `true` if the machine is in a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.current_status.is_terminal()
    }
}

impl Default for IntentStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- IntentStatus ---

    #[test]
    fn display_all_variants() {
        assert_eq!(IntentStatus::Drafting.to_string(), "drafting");
        assert_eq!(IntentStatus::Proposed.to_string(), "proposed");
        assert_eq!(IntentStatus::Verified.to_string(), "verified");
        assert_eq!(IntentStatus::Refused.to_string(), "refused");
        assert_eq!(
            IntentStatus::AwaitingApproval.to_string(),
            "awaiting_approval"
        );
        assert_eq!(IntentStatus::Ready.to_string(), "ready");
        assert_eq!(IntentStatus::Signing.to_string(), "signing");
        assert_eq!(IntentStatus::Submitted.to_string(), "submitted");
        assert_eq!(IntentStatus::Finalized.to_string(), "finalized");
        assert_eq!(IntentStatus::Receipted.to_string(), "receipted");
    }

    #[test]
    fn terminal_states() {
        assert!(IntentStatus::Receipted.is_terminal());
        assert!(IntentStatus::Refused.is_terminal());
        assert!(!IntentStatus::Drafting.is_terminal());
        assert!(!IntentStatus::Submitted.is_terminal());
    }

    #[test]
    fn serde_round_trip() {
        for status in [
            IntentStatus::Drafting,
            IntentStatus::Proposed,
            IntentStatus::Verified,
            IntentStatus::Refused,
            IntentStatus::AwaitingApproval,
            IntentStatus::Ready,
            IntentStatus::Signing,
            IntentStatus::Submitted,
            IntentStatus::Finalized,
            IntentStatus::Receipted,
        ] {
            let json = serde_json::to_string(&status).expect("serialize");
            let back: IntentStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(status, back);
        }
    }

    // --- validate_transition ---

    #[test]
    fn valid_happy_path() {
        // The full happy path: Drafting -> Proposed -> Verified ->
        // AwaitingApproval -> Ready -> Signing -> Submitted -> Finalized ->
        // Receipted.
        let transitions = [
            (IntentStatus::Drafting, IntentStatus::Proposed),
            (IntentStatus::Proposed, IntentStatus::Verified),
            (IntentStatus::Verified, IntentStatus::AwaitingApproval),
            (IntentStatus::AwaitingApproval, IntentStatus::Ready),
            (IntentStatus::Ready, IntentStatus::Signing),
            (IntentStatus::Signing, IntentStatus::Submitted),
            (IntentStatus::Submitted, IntentStatus::Finalized),
            (IntentStatus::Finalized, IntentStatus::Receipted),
        ];
        for (from, to) in transitions {
            assert!(
                validate_transition(from, to).is_ok(),
                "transition {from} -> {to} should be valid"
            );
        }
    }

    #[test]
    fn refuse_from_non_terminal() {
        // Most non-terminal states can transition to Refused.
        for from in [
            IntentStatus::Drafting,
            IntentStatus::Proposed,
            IntentStatus::Verified,
            IntentStatus::AwaitingApproval,
            IntentStatus::Ready,
            IntentStatus::Signing,
            IntentStatus::Submitted,
        ] {
            assert!(
                validate_transition(from, IntentStatus::Refused).is_ok(),
                "transition {from} -> Refused should be valid"
            );
        }
    }

    #[test]
    fn cannot_transition_from_terminal() {
        for to in [
            IntentStatus::Drafting,
            IntentStatus::Proposed,
            IntentStatus::Verified,
            IntentStatus::Ready,
        ] {
            assert!(validate_transition(IntentStatus::Refused, to).is_err());
            assert!(validate_transition(IntentStatus::Receipted, to).is_err());
        }
    }

    #[test]
    fn invalid_skip_transition() {
        // Cannot skip from Drafting directly to Verified.
        assert!(validate_transition(IntentStatus::Drafting, IntentStatus::Verified).is_err());
    }

    #[test]
    fn invalid_backward_transition() {
        // Cannot go backward from Verified to Drafting.
        assert!(validate_transition(IntentStatus::Verified, IntentStatus::Drafting).is_err());
    }

    #[test]
    fn finalized_cannot_refuse() {
        // Finalized can only go to Receipted.
        assert!(validate_transition(IntentStatus::Finalized, IntentStatus::Refused).is_err());
    }

    // --- IntentStateMachine ---

    #[test]
    fn new_starts_in_drafting() {
        let sm = IntentStateMachine::new();
        assert_eq!(sm.current_status(), IntentStatus::Drafting);
        assert_eq!(sm.history().len(), 1);
        assert_eq!(sm.history()[0].0, IntentStatus::Drafting);
    }

    #[test]
    fn transition_records_history() {
        let mut sm = IntentStateMachine::new();
        sm.transition(IntentStatus::Proposed)
            .expect("valid transition");
        assert_eq!(sm.current_status(), IntentStatus::Proposed);
        assert_eq!(sm.history().len(), 2);
        assert_eq!(sm.history()[1].0, IntentStatus::Proposed);
    }

    #[test]
    fn full_happy_path_machine() {
        let mut sm = IntentStateMachine::new();
        sm.transition(IntentStatus::Proposed).expect("valid");
        sm.transition(IntentStatus::Verified).expect("valid");
        sm.transition(IntentStatus::AwaitingApproval)
            .expect("valid");
        sm.transition(IntentStatus::Ready).expect("valid");
        sm.transition(IntentStatus::Signing).expect("valid");
        sm.transition(IntentStatus::Submitted).expect("valid");
        sm.transition(IntentStatus::Finalized).expect("valid");
        sm.transition(IntentStatus::Receipted).expect("valid");

        assert_eq!(sm.current_status(), IntentStatus::Receipted);
        assert!(sm.is_terminal());
        assert_eq!(sm.history().len(), 9); // initial + 8 transitions
    }

    #[test]
    fn invalid_transition_rejected() {
        let mut sm = IntentStateMachine::new();
        let err = sm.transition(IntentStatus::Verified);
        assert!(err.is_err());
        // State should not have changed.
        assert_eq!(sm.current_status(), IntentStatus::Drafting);
        assert_eq!(sm.history().len(), 1);
    }

    #[test]
    fn refuse_mid_flow() {
        let mut sm = IntentStateMachine::new();
        sm.transition(IntentStatus::Proposed).expect("valid");
        sm.transition(IntentStatus::Refused).expect("valid");
        assert!(sm.is_terminal());
        assert_eq!(sm.history().len(), 3);
    }

    #[test]
    fn cannot_transition_after_terminal() {
        let mut sm = IntentStateMachine::new();
        sm.transition(IntentStatus::Proposed).expect("valid");
        sm.transition(IntentStatus::Refused).expect("valid");
        let err = sm.transition(IntentStatus::Verified);
        assert!(err.is_err());
    }

    #[test]
    fn with_status_restores() {
        let history = vec![
            (IntentStatus::Drafting, Utc::now()),
            (IntentStatus::Proposed, Utc::now()),
        ];
        let sm = IntentStateMachine::with_status(IntentStatus::Proposed, history);
        assert_eq!(sm.current_status(), IntentStatus::Proposed);
        assert_eq!(sm.history().len(), 2);
    }

    #[test]
    fn default_is_drafting() {
        let sm = IntentStateMachine::default();
        assert_eq!(sm.current_status(), IntentStatus::Drafting);
    }

    #[test]
    fn history_timestamps_increase() {
        let mut sm = IntentStateMachine::new();
        sm.transition(IntentStatus::Proposed).expect("valid");
        sm.transition(IntentStatus::Verified).expect("valid");
        let history = sm.history();
        for window in history.windows(2) {
            assert!(window[1].1 >= window[0].1);
        }
    }

    #[test]
    fn state_machine_serde_round_trip() {
        let mut sm = IntentStateMachine::new();
        sm.transition(IntentStatus::Proposed).expect("valid");
        let json = serde_json::to_string(&sm).expect("serialize");
        let back: IntentStateMachine = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.current_status(), IntentStatus::Proposed);
        assert_eq!(back.history().len(), 2);
    }
}
