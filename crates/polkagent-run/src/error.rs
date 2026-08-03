//! Error types for the polkagent-run crate.

use thiserror::Error;

use crate::state_machine::RunTransition;
use polkagent_core::RunId;
use polkagent_core::RunState;

// ---------------------------------------------------------------------------
// RunError
// ---------------------------------------------------------------------------

/// All errors that the run management layer can produce.
#[derive(Debug, Error)]
pub enum RunError {
    /// The requested state transition is not valid in the current state.
    #[error("invalid transition: {0}")]
    Transition(#[from] TransitionError),

    /// The run was not found in the store.
    #[error("run not found: {0}")]
    NotFound(RunId),

    /// The turn was not found in the store.
    #[error("turn not found")]
    TurnNotFound,

    /// A store operation failed.
    #[error("store error: {0}")]
    Store(String),

    /// An event recording operation failed.
    #[error("event error: {0}")]
    Event(String),

    /// The run has already reached a terminal state.
    #[error("run {0} is already in a terminal state ({1})")]
    AlreadyTerminal(RunId, String),

    /// The run deadline has been exceeded.
    #[error("run {0} exceeded its deadline")]
    DeadlineExceeded(RunId),

    /// The harness does not meet the task's requirements.
    #[error("harness capability mismatch: {0}")]
    HarnessValidation(String),

    /// Serialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// TransitionError
// ---------------------------------------------------------------------------

/// A state transition was rejected because it is not valid for the current
/// state, as defined by the transition table in `RunStateMachine`.
#[derive(Debug, Error, Clone)]
#[error(
    "invalid transition from state `{from_state}` via event `{attempted_transition:?}`: {reason}"
)]
pub struct TransitionError {
    /// The state the run was in when the transition was attempted.
    pub from_state: RunState,
    /// The transition that was attempted.
    pub attempted_transition: RunTransition,
    /// Human-readable explanation of why the transition is invalid.
    pub reason: String,
}

impl TransitionError {
    /// Construct a new `TransitionError`.
    pub(crate) fn new(
        from_state: RunState,
        attempted_transition: RunTransition,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            from_state,
            attempted_transition,
            reason: reason.into(),
        }
    }
}
