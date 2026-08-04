//! Run domain types.
//!
//! A [`Run`] is the fundamental unit of durable execution in Polkagent. Every
//! piece of work — from a simple model call to a multi-step chain action —
//! is a run. Runs are owned by a single agent and belong to a conversation.
//!
//! The [`RunState`] enum encodes the full lifecycle state machine. Terminal
//! states (`Completed`, `Failed`, `Cancelled`, `Unknown`) are immutable once
//! entered.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, ConversationId, EffectId, RunId};
use crate::turn::TokenUsage;

// ---------------------------------------------------------------------------
// RunState
// ---------------------------------------------------------------------------

/// The lifecycle state of a [`Run`].
///
/// # State machine summary
///
/// ```text
/// Created → Queued → Running ⇌ WaitingApproval
///                          ⇌ WaitingEffect
///                          ↓
///                    Completed | Failed | Cancelled | Unknown
/// ```
///
/// Terminal states: `Completed`, `Failed`, `Cancelled`, `Unknown`.
/// Once a run enters a terminal state it cannot transition further, except
/// that a `Failed` run may be re-queued if the retry policy permits.
///
/// All state transitions are validated by the kernel's state machine and
/// committed atomically with the corresponding [`RunEvent`](crate::event::RunEvent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RunState {
    /// Run record exists with a unique ID. No execution has begun.
    Created,
    /// Validated and accepted for execution. Waiting for a scheduling slot.
    Queued,
    /// Actively executing turns. The turn actor owns this run.
    Running,
    /// Execution paused: a policy gate requires human, quorum, or external
    /// approval before proceeding.
    AwaitingApproval {
        /// ID of the pending approval request.
        request_id: String,
    },
    /// Execution paused: one or more effects are being performed by workers.
    /// The run resumes when all required outcomes are received.
    WaitingEffect {
        /// IDs of all effect intents that must resolve before the run
        /// continues.
        pending_intent_ids: Vec<EffectId>,
    },
    /// Transitioning from the last turn to terminal state. Artifacts are
    /// being finalized.
    Completing,
    /// Terminal: all turns finished successfully.
    Completed,
    /// Terminal: execution encountered an unrecoverable error.
    Failed {
        /// Human-readable description of the failure cause.
        reason: String,
    },
    /// Terminal: execution was cancelled (by user, operator, policy,
    /// timeout, or budget exhaustion).
    Cancelled {
        /// Human-readable description of the cancellation cause.
        reason: String,
    },
    /// Terminal (pending manual resolution): the outcome is genuinely
    /// indeterminate (e.g. a chain broadcast succeeded but finality
    /// observation timed out).
    ///
    /// **Invariant:** `Unknown` is never silently converted to `Completed`
    /// or `Failed` without fresh, independent evidence.
    TimedOut,
}

impl RunState {
    /// Returns `true` if no further state transitions are permitted.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed { .. } | Self::Cancelled { .. } | Self::TimedOut
        )
    }

    /// Returns `true` if the run is currently executing (turn actor is
    /// active).
    #[must_use]
    pub fn is_executing(&self) -> bool {
        matches!(self, Self::Running | Self::Completing)
    }

    /// Returns `true` if the run is waiting for an external event before it
    /// can continue.
    #[must_use]
    pub fn is_waiting(&self) -> bool {
        matches!(
            self,
            Self::AwaitingApproval { .. } | Self::WaitingEffect { .. }
        )
    }
}

impl std::fmt::Display for RunState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Queued => write!(f, "queued"),
            Self::Running => write!(f, "running"),
            Self::AwaitingApproval { request_id } => write!(f, "awaiting_approval:{request_id}"),
            Self::WaitingEffect { pending_intent_ids } => {
                write!(f, "waiting_effect:")?;
                for (i, id) in pending_intent_ids.iter().enumerate() {
                    if i > 0 {
                        write!(f, ",")?;
                    }
                    write!(f, "{id}")?;
                }
                Ok(())
            }
            Self::Completing => write!(f, "completing"),
            Self::Completed => write!(f, "completed"),
            Self::Failed { reason } => write!(f, "failed:{reason}"),
            Self::Cancelled { reason } => write!(f, "cancelled:{reason}"),
            Self::TimedOut => write!(f, "timed_out"),
        }
    }
}

// ---------------------------------------------------------------------------
// Run
// ---------------------------------------------------------------------------

/// The fundamental unit of durable execution.
///
/// A `Run` represents one complete unit of agent work, from receiving the
/// initial input to producing terminal output (or failing). The run record is
/// the authoritative state holder; all other records (turns, steps, effects,
/// events) reference it.
///
/// # Invariants
///
/// - `state` transitions are atomic with their associated `RunEvent`.
/// - `token_usage` is monotonically non-decreasing.
/// - Once `state` is terminal, the run is immutable (except for operator
///   manual resolution of `TimedOut` state).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// Stable, globally unique identifier.
    pub id: RunId,
    /// The agent that owns this run.
    pub agent_id: AgentId,
    /// The conversation this run belongs to (if initiated via chat transport).
    pub conversation_id: Option<ConversationId>,
    /// Current lifecycle state.
    pub state: RunState,
    /// Monotonically increasing event counter. Incremented on every state
    /// transition.
    pub event_sequence: u64,
    /// Number of turns that have been started.
    pub turns_count: u32,
    /// Aggregate token usage across all inference steps in this run.
    pub token_usage: TokenUsage,
    /// When this run was created.
    pub created_at: DateTime<Utc>,
    /// When this run was last updated (state transition or metadata change).
    pub updated_at: DateTime<Utc>,
    /// When execution actually began (transition to `Running`).
    pub started_at: Option<DateTime<Utc>>,
    /// When this run reached a terminal state.
    pub completed_at: Option<DateTime<Utc>>,
    /// Wall-clock deadline. If `None`, no timeout is enforced.
    pub deadline: Option<DateTime<Utc>>,
}

impl Run {
    /// Create a new run in the [`RunState::Created`] state.
    #[must_use]
    pub fn new(id: RunId, agent_id: AgentId) -> Self {
        let now = Utc::now();
        Self {
            id,
            agent_id,
            conversation_id: None,
            state: RunState::Created,
            event_sequence: 0,
            turns_count: 0,
            token_usage: TokenUsage::default(),
            created_at: now,
            updated_at: now,
            started_at: None,
            completed_at: None,
            deadline: None,
        }
    }

    /// Returns `true` if this run is in a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_run() -> Run {
        Run::new(RunId::new(), AgentId::new())
    }

    // --- RunState ---

    #[test]
    fn terminal_states_are_correctly_identified() {
        let terminal = [
            RunState::Completed,
            RunState::Failed {
                reason: "boom".into(),
            },
            RunState::Cancelled {
                reason: "user request".into(),
            },
            RunState::TimedOut,
        ];
        for state in &terminal {
            assert!(state.is_terminal(), "{state:?} should be terminal");
        }
    }

    #[test]
    fn non_terminal_states_are_correctly_identified() {
        let non_terminal = [
            RunState::Created,
            RunState::Queued,
            RunState::Running,
            RunState::AwaitingApproval {
                request_id: "req-1".into(),
            },
            RunState::WaitingEffect {
                pending_intent_ids: vec![],
            },
            RunState::Completing,
        ];
        for state in &non_terminal {
            assert!(!state.is_terminal(), "{state:?} should not be terminal");
        }
    }

    #[test]
    fn is_executing_only_for_running_and_completing() {
        assert!(RunState::Running.is_executing());
        assert!(RunState::Completing.is_executing());
        assert!(!RunState::Created.is_executing());
        assert!(!RunState::Queued.is_executing());
        assert!(!RunState::Completed.is_executing());
    }

    #[test]
    fn is_waiting_only_for_approval_and_effect() {
        assert!(RunState::AwaitingApproval {
            request_id: "r".into()
        }
        .is_waiting());
        assert!(RunState::WaitingEffect {
            pending_intent_ids: vec![]
        }
        .is_waiting());
        assert!(!RunState::Running.is_waiting());
        assert!(!RunState::Completed.is_waiting());
    }

    #[test]
    fn run_state_serde_round_trip_simple() {
        let state = RunState::Queued;
        let json = serde_json::to_string(&state).unwrap();
        let back: RunState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);
    }

    #[test]
    fn run_state_serde_round_trip_failed() {
        let state = RunState::Failed {
            reason: "provider timeout".into(),
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: RunState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);
    }

    #[test]
    fn run_state_serde_round_trip_waiting_effect() {
        let effect_id = EffectId::new();
        let state = RunState::WaitingEffect {
            pending_intent_ids: vec![effect_id],
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: RunState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);
    }

    #[test]
    fn run_state_json_contains_state_tag() {
        let json = serde_json::to_string(&RunState::Running).unwrap();
        assert!(json.contains(r#""state":"running""#));
    }

    // --- Run ---

    #[test]
    fn run_new_starts_in_created_state() {
        let run = make_run();
        assert_eq!(run.state, RunState::Created);
        assert_eq!(run.turns_count, 0);
        assert_eq!(run.event_sequence, 0);
        assert!(run.started_at.is_none());
        assert!(run.completed_at.is_none());
    }

    #[test]
    fn run_is_terminal_delegates_to_state() {
        let mut run = make_run();
        assert!(!run.is_terminal());
        run.state = RunState::Completed;
        assert!(run.is_terminal());
    }

    #[test]
    fn run_serde_round_trip() {
        let run = make_run();
        let json = serde_json::to_string(&run).unwrap();
        let back: Run = serde_json::from_str(&json).unwrap();
        assert_eq!(run.id, back.id);
        assert_eq!(run.agent_id, back.agent_id);
        assert_eq!(run.state, back.state);
        assert_eq!(run.turns_count, back.turns_count);
    }

    /// Validate that the state tag serialization stays stable.
    #[test]
    fn run_state_serialization_is_stable() {
        let cases: &[(&str, RunState)] = &[
            ("created", RunState::Created),
            ("queued", RunState::Queued),
            ("running", RunState::Running),
            ("completing", RunState::Completing),
            ("completed", RunState::Completed),
            ("timed_out", RunState::TimedOut),
        ];
        for (expected_tag, state) in cases {
            let json = serde_json::to_string(state).unwrap();
            let value: serde_json::Value = serde_json::from_str(&json).unwrap();
            let tag = value["state"].as_str().expect("state tag present");
            assert_eq!(tag, *expected_tag, "tag mismatch for {state:?}");
        }
    }

    /// Terminal states cannot proceed further (kernel-level enforcement,
    /// documented here for clarity).
    #[test]
    fn terminal_states_block_further_transitions() {
        let terminal_states = [
            RunState::Completed,
            RunState::Failed {
                reason: String::new(),
            },
            RunState::Cancelled {
                reason: String::new(),
            },
            RunState::TimedOut,
        ];
        for state in &terminal_states {
            // The kernel's apply_run_event() would return
            // Err(RunTransitionError::TerminalState) for any event against
            // these states. We verify the is_terminal predicate here.
            assert!(
                state.is_terminal(),
                "{state:?} must be terminal to block further transitions"
            );
        }
    }
}
