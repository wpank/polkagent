//! Run state machine: enforces valid `RunState` transitions via a transition
//! table.
//!
//! # State machine
//!
//! The authoritative table (derived from PRD-03 §3.3):
//!
//! ```text
//! Created     → Queued          (Start / Enqueue)
//! Queued      → Running         (WorkerClaimed)
//! Running     → AwaitingApproval (RequestApproval)
//! Running     → WaitingEffect   (DispatchEffects)
//! Running     → Completing      (CompleteStep - final turn done)
//! Running     → Failed          (Fail)
//! Running     → Cancelled       (Cancel)
//! Running     → TimedOut        (Timeout)
//! AwaitingApproval → Running    (GrantApproval)
//! AwaitingApproval → Cancelled  (DenyApproval / Cancel)
//! AwaitingApproval → TimedOut   (Timeout)
//! WaitingEffect    → Running    (EffectsResolved)
//! WaitingEffect    → Failed     (Fail)
//! WaitingEffect    → Cancelled  (Cancel)
//! WaitingEffect    → TimedOut   (Timeout)
//! Completing  → Completed       (Complete)
//! Completing  → Failed          (Fail)
//!
//! Terminal states (no further transitions): Completed, Failed, Cancelled, TimedOut
//! ```

use serde::{Deserialize, Serialize};

use polkagent_core::RunState;

use crate::error::TransitionError;

// ---------------------------------------------------------------------------
// RunTransition
// ---------------------------------------------------------------------------

/// All events that can drive a `RunState` transition.
///
/// Each variant corresponds to one logical event in the execution model
/// (PRD-03 §3.3). The state machine validates that an event is legal in the
/// current state before applying it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunTransition {
    /// Enqueue the run for execution (Created → Queued).
    Start,
    /// A worker has claimed the run (Queued → Running).
    WorkerClaimed,
    /// A turn completed; request effects or continue (no state change on its
    /// own; used internally, but can be used to signal step completion).
    CompleteStep,
    /// Dispatch one or more effect intents (Running → WaitingEffect).
    DispatchEffects,
    /// All required effects resolved; resume execution (WaitingEffect → Running).
    EffectsResolved,
    /// Approval is required before the next action (Running → AwaitingApproval).
    RequestApproval,
    /// Approval was granted; resume execution (AwaitingApproval → Running).
    GrantApproval,
    /// Approval was denied; cancel the run (AwaitingApproval → Cancelled).
    DenyApproval(String),
    /// The run completed all turns successfully (Completing → Completed).
    Complete,
    /// The run encountered an unrecoverable error (Running|WaitingEffect|Completing → Failed).
    Fail(String),
    /// The run was cancelled (Running|WaitingEffect|AwaitingApproval → Cancelled).
    Cancel(String),
    /// The run exceeded its deadline (any non-terminal → TimedOut).
    Timeout,
}

// ---------------------------------------------------------------------------
// RunStateMachine
// ---------------------------------------------------------------------------

/// Enforces valid `RunState` transitions via an exhaustive transition table.
///
/// All state changes in the execution model MUST go through
/// [`RunStateMachine::transition`]. This ensures the system can never reach
/// an invalid state combination.
///
/// # Usage
///
/// ```
/// use polkagent_core::RunState;
/// use polkagent_run::state_machine::{RunStateMachine, RunTransition};
///
/// let machine = RunStateMachine::new();
/// let next = machine.transition(&RunState::Created, RunTransition::Start).unwrap();
/// assert_eq!(next, RunState::Queued);
/// ```
#[derive(Debug, Default, Clone)]
pub struct RunStateMachine;

impl RunStateMachine {
    /// Create a new state machine.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Apply a transition event from the given current state.
    ///
    /// Returns the next `RunState` on success, or a `TransitionError` if the
    /// transition is not allowed in the current state.
    ///
    /// # Errors
    ///
    /// Returns `TransitionError` when:
    /// - The current state is terminal (`Completed`, `Failed`, `Cancelled`,
    ///   `TimedOut`).
    /// - The event is not defined for the current state in the transition
    ///   table.
    pub fn transition(
        &self,
        current: &RunState,
        event: RunTransition,
    ) -> Result<RunState, TransitionError> {
        // Terminal states block all transitions.
        if current.is_terminal() {
            return Err(TransitionError::new(
                current.clone(),
                event.clone(),
                format!("run is in terminal state `{current}`; no further transitions are allowed"),
            ));
        }

        match (current, &event) {
            // -----------------------------------------------------------------
            // Created
            // -----------------------------------------------------------------
            (RunState::Created, RunTransition::Start) => Ok(RunState::Queued),

            // -----------------------------------------------------------------
            // Queued
            // -----------------------------------------------------------------
            (RunState::Queued, RunTransition::WorkerClaimed) => Ok(RunState::Running),

            // Cancel from Queued is allowed (operator/user pre-start cancel).
            (RunState::Queued, RunTransition::Cancel(reason)) => Ok(RunState::Cancelled {
                reason: reason.clone(),
            }),

            // Timeout from Queued (waited too long in queue).
            (RunState::Queued, RunTransition::Timeout) => Ok(RunState::TimedOut),

            // -----------------------------------------------------------------
            // Running
            // -----------------------------------------------------------------
            (RunState::Running, RunTransition::RequestApproval) => {
                Ok(RunState::AwaitingApproval {
                    request_id: String::new(), // caller sets the request_id after
                })
            }
            (RunState::Running, RunTransition::DispatchEffects) => Ok(RunState::WaitingEffect {
                pending_intent_ids: Vec::new(), // caller populates intent IDs
            }),
            (RunState::Running, RunTransition::CompleteStep) => {
                // CompleteStep inside Running: remains Running until the
                // executor decides to move to Completing.
                Ok(RunState::Running)
            }
            (RunState::Running, RunTransition::Complete) => Ok(RunState::Completing),
            (RunState::Running, RunTransition::Fail(reason)) => Ok(RunState::Failed {
                reason: reason.clone(),
            }),
            (RunState::Running, RunTransition::Cancel(reason)) => Ok(RunState::Cancelled {
                reason: reason.clone(),
            }),
            (RunState::Running, RunTransition::Timeout) => Ok(RunState::TimedOut),

            // -----------------------------------------------------------------
            // AwaitingApproval
            // -----------------------------------------------------------------
            (RunState::AwaitingApproval { .. }, RunTransition::GrantApproval) => {
                Ok(RunState::Running)
            }
            (RunState::AwaitingApproval { .. }, RunTransition::DenyApproval(reason)) => {
                Ok(RunState::Cancelled {
                    reason: reason.clone(),
                })
            }
            (RunState::AwaitingApproval { .. }, RunTransition::Cancel(reason)) => {
                Ok(RunState::Cancelled {
                    reason: reason.clone(),
                })
            }
            (RunState::AwaitingApproval { .. }, RunTransition::Timeout) => Ok(RunState::TimedOut),
            (RunState::AwaitingApproval { .. }, RunTransition::Fail(reason)) => {
                Ok(RunState::Failed {
                    reason: reason.clone(),
                })
            }

            // -----------------------------------------------------------------
            // WaitingEffect
            // -----------------------------------------------------------------
            (RunState::WaitingEffect { .. }, RunTransition::EffectsResolved) => {
                Ok(RunState::Running)
            }
            (RunState::WaitingEffect { .. }, RunTransition::Fail(reason)) => Ok(RunState::Failed {
                reason: reason.clone(),
            }),
            (RunState::WaitingEffect { .. }, RunTransition::Cancel(reason)) => {
                Ok(RunState::Cancelled {
                    reason: reason.clone(),
                })
            }
            (RunState::WaitingEffect { .. }, RunTransition::Timeout) => Ok(RunState::TimedOut),

            // -----------------------------------------------------------------
            // Completing
            // -----------------------------------------------------------------
            (RunState::Completing, RunTransition::Complete) => Ok(RunState::Completed),
            (RunState::Completing, RunTransition::Fail(reason)) => Ok(RunState::Failed {
                reason: reason.clone(),
            }),
            (RunState::Completing, RunTransition::Cancel(reason)) => Ok(RunState::Cancelled {
                reason: reason.clone(),
            }),
            (RunState::Completing, RunTransition::Timeout) => Ok(RunState::TimedOut),

            // -----------------------------------------------------------------
            // Any state not covered above is invalid.
            // -----------------------------------------------------------------
            _ => Err(TransitionError::new(
                current.clone(),
                event.clone(),
                format!("transition `{event:?}` is not defined for state `{current}`"),
            )),
        }
    }

    /// Returns `true` if `state` is a terminal state (no further transitions
    /// are possible).
    #[must_use]
    pub fn is_terminal(state: &RunState) -> bool {
        state.is_terminal()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::RunState;

    fn sm() -> RunStateMachine {
        RunStateMachine::new()
    }

    // --- Created transitions ---

    #[test]
    fn created_to_queued_via_start() {
        let next = sm()
            .transition(&RunState::Created, RunTransition::Start)
            .unwrap();
        assert_eq!(next, RunState::Queued);
    }

    #[test]
    fn created_invalid_worker_claimed() {
        let err = sm()
            .transition(&RunState::Created, RunTransition::WorkerClaimed)
            .unwrap_err();
        assert_eq!(err.from_state, RunState::Created);
    }

    // --- Queued transitions ---

    #[test]
    fn queued_to_running_via_worker_claimed() {
        let next = sm()
            .transition(&RunState::Queued, RunTransition::WorkerClaimed)
            .unwrap();
        assert_eq!(next, RunState::Running);
    }

    #[test]
    fn queued_cancel() {
        let next = sm()
            .transition(&RunState::Queued, RunTransition::Cancel("test".into()))
            .unwrap();
        assert!(matches!(next, RunState::Cancelled { .. }));
    }

    #[test]
    fn queued_timeout() {
        let next = sm()
            .transition(&RunState::Queued, RunTransition::Timeout)
            .unwrap();
        assert_eq!(next, RunState::TimedOut);
    }

    #[test]
    fn queued_invalid_complete() {
        let err = sm()
            .transition(&RunState::Queued, RunTransition::Complete)
            .unwrap_err();
        assert_eq!(err.from_state, RunState::Queued);
    }

    // --- Running transitions ---

    #[test]
    fn running_to_awaiting_approval() {
        let next = sm()
            .transition(&RunState::Running, RunTransition::RequestApproval)
            .unwrap();
        assert!(matches!(next, RunState::AwaitingApproval { .. }));
    }

    #[test]
    fn running_to_waiting_effect() {
        let next = sm()
            .transition(&RunState::Running, RunTransition::DispatchEffects)
            .unwrap();
        assert!(matches!(next, RunState::WaitingEffect { .. }));
    }

    #[test]
    fn running_complete_step_stays_running() {
        let next = sm()
            .transition(&RunState::Running, RunTransition::CompleteStep)
            .unwrap();
        assert_eq!(next, RunState::Running);
    }

    #[test]
    fn running_to_completing_via_complete() {
        let next = sm()
            .transition(&RunState::Running, RunTransition::Complete)
            .unwrap();
        assert_eq!(next, RunState::Completing);
    }

    #[test]
    fn running_to_failed_via_fail() {
        let next = sm()
            .transition(&RunState::Running, RunTransition::Fail("error".into()))
            .unwrap();
        assert!(matches!(next, RunState::Failed { .. }));
    }

    #[test]
    fn running_to_cancelled_via_cancel() {
        let next = sm()
            .transition(
                &RunState::Running,
                RunTransition::Cancel("user cancel".into()),
            )
            .unwrap();
        assert!(matches!(next, RunState::Cancelled { .. }));
    }

    #[test]
    fn running_to_timed_out() {
        let next = sm()
            .transition(&RunState::Running, RunTransition::Timeout)
            .unwrap();
        assert_eq!(next, RunState::TimedOut);
    }

    // --- AwaitingApproval transitions ---

    #[test]
    fn awaiting_approval_to_running_via_grant() {
        let current = RunState::AwaitingApproval {
            request_id: "req-1".into(),
        };
        let next = sm()
            .transition(&current, RunTransition::GrantApproval)
            .unwrap();
        assert_eq!(next, RunState::Running);
    }

    #[test]
    fn awaiting_approval_to_cancelled_via_deny() {
        let current = RunState::AwaitingApproval {
            request_id: "req-1".into(),
        };
        let next = sm()
            .transition(&current, RunTransition::DenyApproval("denied".into()))
            .unwrap();
        assert!(matches!(next, RunState::Cancelled { .. }));
    }

    #[test]
    fn awaiting_approval_to_cancelled_via_cancel() {
        let current = RunState::AwaitingApproval {
            request_id: "req-1".into(),
        };
        let next = sm()
            .transition(&current, RunTransition::Cancel("user".into()))
            .unwrap();
        assert!(matches!(next, RunState::Cancelled { .. }));
    }

    #[test]
    fn awaiting_approval_to_timed_out() {
        let current = RunState::AwaitingApproval {
            request_id: "req-1".into(),
        };
        let next = sm().transition(&current, RunTransition::Timeout).unwrap();
        assert_eq!(next, RunState::TimedOut);
    }

    #[test]
    fn awaiting_approval_to_failed_via_fail() {
        let current = RunState::AwaitingApproval {
            request_id: "req-1".into(),
        };
        let next = sm()
            .transition(&current, RunTransition::Fail("fatal".into()))
            .unwrap();
        assert!(matches!(next, RunState::Failed { .. }));
    }

    // --- WaitingEffect transitions ---

    #[test]
    fn waiting_effect_to_running_via_effects_resolved() {
        let current = RunState::WaitingEffect {
            pending_intent_ids: vec![],
        };
        let next = sm()
            .transition(&current, RunTransition::EffectsResolved)
            .unwrap();
        assert_eq!(next, RunState::Running);
    }

    #[test]
    fn waiting_effect_to_failed() {
        let current = RunState::WaitingEffect {
            pending_intent_ids: vec![],
        };
        let next = sm()
            .transition(&current, RunTransition::Fail("effect failed".into()))
            .unwrap();
        assert!(matches!(next, RunState::Failed { .. }));
    }

    #[test]
    fn waiting_effect_to_cancelled() {
        let current = RunState::WaitingEffect {
            pending_intent_ids: vec![],
        };
        let next = sm()
            .transition(&current, RunTransition::Cancel("cancel".into()))
            .unwrap();
        assert!(matches!(next, RunState::Cancelled { .. }));
    }

    #[test]
    fn waiting_effect_to_timed_out() {
        let current = RunState::WaitingEffect {
            pending_intent_ids: vec![],
        };
        let next = sm().transition(&current, RunTransition::Timeout).unwrap();
        assert_eq!(next, RunState::TimedOut);
    }

    // --- Completing transitions ---

    #[test]
    fn completing_to_completed() {
        let next = sm()
            .transition(&RunState::Completing, RunTransition::Complete)
            .unwrap();
        assert_eq!(next, RunState::Completed);
    }

    #[test]
    fn completing_to_failed() {
        let next = sm()
            .transition(
                &RunState::Completing,
                RunTransition::Fail("late error".into()),
            )
            .unwrap();
        assert!(matches!(next, RunState::Failed { .. }));
    }

    #[test]
    fn completing_to_cancelled() {
        let next = sm()
            .transition(
                &RunState::Completing,
                RunTransition::Cancel("cancel".into()),
            )
            .unwrap();
        assert!(matches!(next, RunState::Cancelled { .. }));
    }

    #[test]
    fn completing_to_timed_out() {
        let next = sm()
            .transition(&RunState::Completing, RunTransition::Timeout)
            .unwrap();
        assert_eq!(next, RunState::TimedOut);
    }

    // --- Terminal states block all transitions ---

    #[test]
    fn completed_is_terminal_blocks_all() {
        let transitions = vec![
            RunTransition::Start,
            RunTransition::WorkerClaimed,
            RunTransition::Complete,
            RunTransition::Fail("x".into()),
            RunTransition::Cancel("x".into()),
            RunTransition::Timeout,
        ];
        for event in transitions {
            let err = sm()
                .transition(&RunState::Completed, event.clone())
                .unwrap_err();
            assert_eq!(err.from_state, RunState::Completed, "event: {event:?}");
        }
    }

    #[test]
    fn failed_is_terminal() {
        let state = RunState::Failed { reason: "x".into() };
        let err = sm()
            .transition(&state, RunTransition::Complete)
            .unwrap_err();
        assert!(err.from_state.is_terminal());
    }

    #[test]
    fn cancelled_is_terminal() {
        let state = RunState::Cancelled { reason: "x".into() };
        let err = sm().transition(&state, RunTransition::Start).unwrap_err();
        assert!(err.from_state.is_terminal());
    }

    #[test]
    fn timed_out_is_terminal() {
        let err = sm()
            .transition(&RunState::TimedOut, RunTransition::Complete)
            .unwrap_err();
        assert!(err.from_state.is_terminal());
    }

    // --- is_terminal helper ---

    #[test]
    fn is_terminal_covers_all_terminal_states() {
        assert!(RunStateMachine::is_terminal(&RunState::Completed));
        assert!(RunStateMachine::is_terminal(&RunState::Failed {
            reason: "x".into()
        }));
        assert!(RunStateMachine::is_terminal(&RunState::Cancelled {
            reason: "x".into()
        }));
        assert!(RunStateMachine::is_terminal(&RunState::TimedOut));
    }

    #[test]
    fn is_terminal_false_for_non_terminal_states() {
        assert!(!RunStateMachine::is_terminal(&RunState::Created));
        assert!(!RunStateMachine::is_terminal(&RunState::Queued));
        assert!(!RunStateMachine::is_terminal(&RunState::Running));
        assert!(!RunStateMachine::is_terminal(&RunState::AwaitingApproval {
            request_id: "r".into()
        }));
        assert!(!RunStateMachine::is_terminal(&RunState::WaitingEffect {
            pending_intent_ids: vec![]
        }));
        assert!(!RunStateMachine::is_terminal(&RunState::Completing));
    }

    // --- TransitionError carries context ---

    #[test]
    fn transition_error_carries_from_state_and_event() {
        let err = sm()
            .transition(&RunState::Queued, RunTransition::Complete)
            .unwrap_err();
        assert_eq!(err.from_state, RunState::Queued);
        assert_eq!(err.attempted_transition, RunTransition::Complete);
        assert!(!err.reason.is_empty());
    }

    // --- Fail reason is preserved ---

    #[test]
    fn fail_reason_is_preserved_in_failed_state() {
        let next = sm()
            .transition(
                &RunState::Running,
                RunTransition::Fail("provider timeout".into()),
            )
            .unwrap();
        match next {
            RunState::Failed { reason } => assert_eq!(reason, "provider timeout"),
            _ => panic!("expected Failed"),
        }
    }

    // --- Cancel reason is preserved ---

    #[test]
    fn cancel_reason_is_preserved_in_cancelled_state() {
        let next = sm()
            .transition(
                &RunState::Running,
                RunTransition::Cancel("user request".into()),
            )
            .unwrap();
        match next {
            RunState::Cancelled { reason } => assert_eq!(reason, "user request"),
            _ => panic!("expected Cancelled"),
        }
    }

    // --- DenyApproval reason is preserved ---

    #[test]
    fn deny_approval_reason_propagated_to_cancelled() {
        let current = RunState::AwaitingApproval {
            request_id: "req-42".into(),
        };
        let next = sm()
            .transition(
                &current,
                RunTransition::DenyApproval("budget exceeded".into()),
            )
            .unwrap();
        match next {
            RunState::Cancelled { reason } => assert_eq!(reason, "budget exceeded"),
            _ => panic!("expected Cancelled"),
        }
    }
}
