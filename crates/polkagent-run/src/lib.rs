//! Run/Turn/Step lifecycle manager for the Polkagent platform.
//!
//! This crate owns the [`RunManager`], which coordinates creating, starting,
//! cancelling, and querying runs. All state transitions are validated by
//! [`RunStateMachine`] and recorded durably via an [`EventRecorder`].
//!
//! # Module layout
//!
//! | Module | Responsibility |
//! |---|---|
//! | [`dag`] | [`ExecutionDag`], [`DagExecutor`]: DAG-based workflow composition |
//! | [`manager`] | [`RunManager`]: create, enqueue, start, fail, cancel, complete runs |
//! | [`orchestrator`] | [`RunOrchestrator`]: connects RunManager to ModelExecutor for turn loop |
//! | [`state_machine`] | [`RunStateMachine`]: exhaustive transition table for [`RunState`] |
//! | [`turn`] | [`TurnManager`], [`TurnInput`], [`TurnOutput`]: turn helpers |
//! | [`timeout`] | [`TimeoutEnforcer`], [`TimeoutConfig`]: deadline enforcement |
//! | [`error`] | [`RunError`], [`TransitionError`]: error types |

pub mod dag;
pub mod error;
pub mod manager;
pub mod orchestrator;
pub mod state_machine;
pub mod timeout;
pub mod turn;

// ---------------------------------------------------------------------------
// Flat re-exports — the public API surface
// ---------------------------------------------------------------------------

pub use dag::{DagError, DagExecutor, ExecutionDag, NodeId, NodeState};
pub use error::{RunError, TransitionError};
pub use manager::RunManager;
pub use orchestrator::{RunOrchestrator, RunOrchestratorConfig, RunOutcome};
pub use state_machine::{RunStateMachine, RunTransition};
pub use timeout::{TimeoutConfig, TimeoutEnforcer};
pub use turn::{TurnInput, TurnManager, TurnOutput};
