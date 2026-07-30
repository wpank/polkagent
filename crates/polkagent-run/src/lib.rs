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
//! | [`manager`] | [`RunManager`]: create, enqueue, start, fail, cancel, complete runs |
//! | [`state_machine`] | [`RunStateMachine`]: exhaustive transition table for [`RunState`] |
//! | [`turn`] | [`TurnManager`], [`TurnInput`], [`TurnOutput`]: turn helpers |
//! | [`timeout`] | [`TimeoutEnforcer`], [`TimeoutConfig`]: deadline enforcement |
//! | [`error`] | [`RunError`], [`TransitionError`]: error types |

pub mod error;
pub mod manager;
pub mod state_machine;
pub mod timeout;
pub mod turn;

// ---------------------------------------------------------------------------
// Flat re-exports — the public API surface
// ---------------------------------------------------------------------------

pub use error::{RunError, TransitionError};
pub use manager::RunManager;
pub use state_machine::{RunStateMachine, RunTransition};
pub use timeout::{TimeoutConfig, TimeoutEnforcer};
pub use turn::{TurnInput, TurnManager, TurnOutput};
