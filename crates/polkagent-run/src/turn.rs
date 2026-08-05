//! Turn management helpers for the run lifecycle.
//!
//! A [`polkagent_core::Run`] is decomposed into sequential [`Turn`]s. Each turn represents
//! one request/response cycle through the reducer: context assembly →
//! model inference → output parsing → effect dispatch.
//!
//! This module provides:
//! - [`TurnOutput`] — the result of completing a turn.
//! - [`TurnManager`] — creates and completes turns within a run, updating
//!   token usage and persisting via the store.

use chrono::Utc;
use polkagent_core::{
    turn::{MessageRole, TokenUsage, Turn},
    RunId, TurnId,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

// ---------------------------------------------------------------------------
// TurnInput
// ---------------------------------------------------------------------------

/// The input provided to a turn.
///
/// For the first turn this is the user's initial message. For subsequent turns
/// it is the result of resolving the previous turn's effects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnInput {
    /// Role of the message initiating this turn.
    pub role: MessageRole,
    /// The text or tool-result content.
    pub content: String,
}

impl TurnInput {
    /// Create a new user-initiated turn input.
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
        }
    }

    /// Create a new assistant-continuation turn input.
    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// TurnOutput
// ---------------------------------------------------------------------------

/// The result produced by completing a turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnOutput {
    /// Token usage accumulated during this turn.
    pub token_usage: TokenUsage,
    /// Whether this turn is the final turn (the run will complete after this).
    pub is_final: bool,
    /// Human-readable summary of the turn's output, if available.
    pub summary: Option<String>,
}

impl TurnOutput {
    /// Create a non-final turn output with zero token usage.
    #[must_use]
    pub fn continuing() -> Self {
        Self {
            token_usage: TokenUsage::default(),
            is_final: false,
            summary: None,
        }
    }

    /// Create a final turn output that will end the run.
    #[must_use]
    pub fn terminal(summary: impl Into<String>) -> Self {
        Self {
            token_usage: TokenUsage::default(),
            is_final: true,
            summary: Some(summary.into()),
        }
    }

    /// Attach token usage to this output.
    #[must_use]
    pub fn with_usage(mut self, usage: TokenUsage) -> Self {
        self.token_usage = usage;
        self
    }
}

// ---------------------------------------------------------------------------
// TurnManager
// ---------------------------------------------------------------------------

/// Creates and completes turns within a run.
///
/// Callers use this to:
/// 1. Start a new turn with [`TurnManager::create_turn`].
/// 2. Process the turn (model inference, effects, etc.).
/// 3. Complete it with [`TurnManager::complete_turn`].
///
/// The `TurnManager` is stateless; all state is passed in or derived.
#[derive(Debug, Default, Clone)]
pub struct TurnManager;

impl TurnManager {
    /// Create a new `TurnManager`.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Create a new `Turn` for the given run.
    ///
    /// The turn is created in memory only; callers are responsible for
    /// persisting it via the store.
    #[must_use]
    #[instrument(skip(self))]
    pub fn create_turn(&self, run_id: RunId, sequence: u32, input: &TurnInput) -> Turn {
        debug!(
            %run_id,
            sequence,
            role = ?input.role,
            "Creating turn"
        );
        Turn::new(TurnId::new(), run_id, sequence, input.role)
    }

    /// Mark a turn as complete and record its output.
    ///
    /// Sets `completed_at` and updates token usage. Returns the updated turn.
    #[must_use]
    #[instrument(skip(self, turn, output))]
    pub fn complete_turn(&self, mut turn: Turn, output: &TurnOutput) -> Turn {
        debug!(
            turn_id = %turn.id,
            is_final = output.is_final,
            tokens = output.token_usage.total_tokens,
            "Completing turn"
        );
        turn.completed_at = Some(Utc::now());
        turn.token_usage = output.token_usage;
        turn
    }

    /// Returns `true` if the turn has been completed (i.e. has a `completed_at`).
    #[must_use]
    pub fn is_complete(turn: &Turn) -> bool {
        turn.is_complete()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::RunId;

    fn manager() -> TurnManager {
        TurnManager::new()
    }

    #[test]
    fn create_turn_has_correct_run_id_and_sequence() {
        let m = manager();
        let run_id = RunId::new();
        let input = TurnInput::user("Hello");
        let turn = m.create_turn(run_id, 0, &input);
        assert_eq!(turn.run_id, run_id);
        assert_eq!(turn.sequence, 0);
        assert!(!turn.is_complete());
    }

    #[test]
    fn create_turn_respects_role() {
        let m = manager();
        let run_id = RunId::new();
        let input = TurnInput::assistant("result");
        let turn = m.create_turn(run_id, 1, &input);
        assert_eq!(turn.role, MessageRole::Assistant);
    }

    #[test]
    fn complete_turn_sets_completed_at() {
        let m = manager();
        let run_id = RunId::new();
        let input = TurnInput::user("hi");
        let turn = m.create_turn(run_id, 0, &input);
        assert!(!turn.is_complete());

        let output = TurnOutput::continuing();
        let completed = m.complete_turn(turn, &output);
        assert!(completed.is_complete());
    }

    #[test]
    fn complete_turn_updates_token_usage() {
        let m = manager();
        let run_id = RunId::new();
        let input = TurnInput::user("hi");
        let turn = m.create_turn(run_id, 0, &input);

        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            ..Default::default()
        };
        let output = TurnOutput::continuing().with_usage(usage);
        let completed = m.complete_turn(turn, &output);
        assert_eq!(completed.token_usage.total_tokens, 150);
    }

    #[test]
    fn turn_output_terminal_has_is_final_true() {
        let output = TurnOutput::terminal("done");
        assert!(output.is_final);
        assert_eq!(output.summary.as_deref(), Some("done"));
    }

    #[test]
    fn turn_output_continuing_has_is_final_false() {
        let output = TurnOutput::continuing();
        assert!(!output.is_final);
    }

    #[test]
    fn turn_input_user_has_user_role() {
        let input = TurnInput::user("hello");
        assert_eq!(input.role, MessageRole::User);
        assert_eq!(input.content, "hello");
    }

    #[test]
    fn turn_input_assistant_has_assistant_role() {
        let input = TurnInput::assistant("result");
        assert_eq!(input.role, MessageRole::Assistant);
    }

    #[test]
    fn is_complete_delegates_to_turn() {
        let m = manager();
        let run_id = RunId::new();
        let input = TurnInput::user("hi");
        let turn = m.create_turn(run_id, 0, &input);
        assert!(!TurnManager::is_complete(&turn));

        let output = TurnOutput::continuing();
        let completed = m.complete_turn(turn, &output);
        assert!(TurnManager::is_complete(&completed));
    }
}
