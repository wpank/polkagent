//! Turn, step, and token-usage domain types.
//!
//! A [`Run`](crate::run::Run) is decomposed into one or more [`Turn`]s. Each
//! turn is a single request/response cycle through the reducer: context
//! assembly → model inference → output parsing → effect dispatch (or
//! terminal output). Each turn is further decomposed into ordered [`Step`]s,
//! each of which represents one discrete action.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{ArtifactId, EffectId, RunId, StepId, TurnId};

// ---------------------------------------------------------------------------
// TokenUsage
// ---------------------------------------------------------------------------

/// Token and cost accounting for one inference call.
///
/// The `total_tokens` field is the authoritative sum used for budget
/// enforcement. The optional `cost_usd` field is a billing estimate derived
/// from the provider's published pricing at the time of the call.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct TokenUsage {
    /// Tokens in the prompt/context sent to the model.
    pub input_tokens: u32,
    /// Tokens in the model's response.
    pub output_tokens: u32,
    /// Sum of input and output tokens.
    pub total_tokens: u32,
    /// Prompt-cache read tokens (provider-specific; zero if not applicable).
    pub cache_read_tokens: u32,
    /// Prompt-cache write tokens (provider-specific; zero if not applicable).
    pub cache_write_tokens: u32,
    /// Estimated USD cost at the time of the call. `None` if pricing data is
    /// unavailable or if the call was made to a free/local model.
    pub cost_usd: Option<f64>,
}

impl TokenUsage {
    /// Accumulate another `TokenUsage` into this one (for run-level totals).
    pub fn accumulate(&mut self, other: &Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(other.cache_write_tokens);
        self.cost_usd = match (self.cost_usd, other.cost_usd) {
            (Some(a), Some(b)) => Some(a + b),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        };
    }
}

// ---------------------------------------------------------------------------
// MessageRole
// ---------------------------------------------------------------------------

/// The sender role for a turn, mirroring the model API convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    /// A message originating from the human user or an external trigger.
    User,
    /// A message produced by the AI model.
    Assistant,
    /// An internal system message (context injection, tool result, etc.).
    System,
}

// ---------------------------------------------------------------------------
// StepKind
// ---------------------------------------------------------------------------

/// The type of work performed by a [`Step`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    /// Pure context assembly: deterministic, no I/O.
    ContextAssembly,
    /// Model inference: calls the provider API (an effect).
    ModelInference,
    /// Tool invocation: calls a registered tool (an effect).
    ToolInvocation,
    /// Policy evaluation: deterministic gate check against the policy engine.
    PolicyEvaluation,
    /// Approval check: determines whether human or quorum approval is needed.
    ApprovalCheck,
    /// Output parsing: pure transformation of raw model output.
    OutputParsing,
    /// One phase of a chain action saga (decode, simulate, sign, broadcast…).
    ChainAction,
    /// Delivery of a result to an external transport.
    Delivery,
}

// ---------------------------------------------------------------------------
// Step
// ---------------------------------------------------------------------------

/// A discrete, ordered unit of work within a [`Turn`].
///
/// Steps are immutable records created during execution. Each step may
/// produce artifacts and reference up to one effect intent (for steps that
/// require external I/O).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// Stable identifier for this step.
    pub id: StepId,
    /// The run this step belongs to.
    pub run_id: RunId,
    /// The turn within which this step was executed.
    pub turn_id: TurnId,
    /// Monotonically increasing position within the turn (0-indexed).
    pub step_number: u32,
    /// What kind of work this step performs.
    pub kind: StepKind,
    /// If this step dispatches an effect, the ID of the resulting intent.
    pub effect_intent_id: Option<EffectId>,
    /// Artifacts produced by this step (context packs, model responses, etc.).
    pub artifacts_produced: Vec<ArtifactId>,
    /// When this step began execution.
    pub started_at: DateTime<Utc>,
    /// When this step completed (absent while still in progress).
    pub completed_at: Option<DateTime<Utc>>,
}

impl Step {
    /// Returns `true` if this step has produced an outcome (success or
    /// failure) and will not be modified further.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.completed_at.is_some()
    }
}

// ---------------------------------------------------------------------------
// Turn
// ---------------------------------------------------------------------------

/// A single request/response cycle within a [`Run`](crate::run::Run).
///
/// Turns are strictly sequential: turn N+1 cannot begin until turn N
/// completes. Each turn records the role of the initiating message, the
/// steps executed, and aggregate token usage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    /// Stable identifier for this turn.
    pub id: TurnId,
    /// The run this turn belongs to.
    pub run_id: RunId,
    /// Monotonically increasing position within the run (0-indexed).
    pub sequence: u32,
    /// Role of the initiating message (usually [`MessageRole::User`] for the
    /// first turn; [`MessageRole::Assistant`] for effect-result turns).
    pub role: MessageRole,
    /// Steps executed within this turn, in order.
    pub steps: Vec<Step>,
    /// Aggregate token usage across all inference steps in this turn.
    pub token_usage: TokenUsage,
    /// When this turn began.
    pub started_at: DateTime<Utc>,
    /// When this turn completed (absent while still in progress).
    pub completed_at: Option<DateTime<Utc>>,
}

impl Turn {
    /// Create a new turn with no steps and zero token usage.
    #[must_use]
    pub fn new(id: TurnId, run_id: RunId, sequence: u32, role: MessageRole) -> Self {
        Self {
            id,
            run_id,
            sequence,
            role,
            steps: Vec::new(),
            token_usage: TokenUsage::default(),
            started_at: Utc::now(),
            completed_at: None,
        }
    }

    /// Returns `true` if this turn has completed.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.completed_at.is_some()
    }

    /// Return the total number of steps in this turn.
    #[must_use]
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{RunId, TurnId};

    fn make_turn() -> Turn {
        Turn::new(TurnId::new(), RunId::new(), 0, MessageRole::User)
    }

    // --- TokenUsage ---

    #[test]
    fn token_usage_default_is_zero() {
        let usage = TokenUsage::default();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.total_tokens, 0);
        assert!(usage.cost_usd.is_none());
    }

    #[test]
    fn token_usage_accumulate_sums_fields() {
        let mut a = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            cache_read_tokens: 10,
            cache_write_tokens: 5,
            cost_usd: Some(0.01),
        };
        let b = TokenUsage {
            input_tokens: 200,
            output_tokens: 80,
            total_tokens: 280,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: Some(0.02),
        };
        a.accumulate(&b);
        assert_eq!(a.input_tokens, 300);
        assert_eq!(a.output_tokens, 130);
        assert_eq!(a.total_tokens, 430);
        assert_eq!(a.cache_read_tokens, 10);
        assert!((a.cost_usd.unwrap() - 0.03).abs() < f64::EPSILON);
    }

    #[test]
    fn token_usage_accumulate_none_plus_some() {
        let mut a = TokenUsage::default();
        let b = TokenUsage {
            cost_usd: Some(0.05),
            ..Default::default()
        };
        a.accumulate(&b);
        assert_eq!(a.cost_usd, Some(0.05));
    }

    #[test]
    fn token_usage_accumulate_none_plus_none() {
        let mut a = TokenUsage::default();
        a.accumulate(&TokenUsage::default());
        assert!(a.cost_usd.is_none());
    }

    #[test]
    fn token_usage_saturates_on_overflow() {
        let mut a = TokenUsage {
            input_tokens: u32::MAX,
            ..Default::default()
        };
        a.accumulate(&TokenUsage {
            input_tokens: 1,
            ..Default::default()
        });
        assert_eq!(a.input_tokens, u32::MAX);
    }

    #[test]
    fn token_usage_serde_round_trip() {
        let usage = TokenUsage {
            input_tokens: 42,
            output_tokens: 17,
            total_tokens: 59,
            cache_read_tokens: 3,
            cache_write_tokens: 0,
            cost_usd: Some(0.0012),
        };
        let json = serde_json::to_string(&usage).unwrap();
        let back: TokenUsage = serde_json::from_str(&json).unwrap();
        assert_eq!(usage.input_tokens, back.input_tokens);
        assert_eq!(usage.output_tokens, back.output_tokens);
    }

    // --- Step ---

    #[test]
    fn step_is_complete_when_completed_at_is_set() {
        let step = Step {
            id: StepId::new(),
            run_id: RunId::new(),
            turn_id: TurnId::new(),
            step_number: 0,
            kind: StepKind::ContextAssembly,
            effect_intent_id: None,
            artifacts_produced: Vec::new(),
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
        };
        assert!(step.is_complete());
    }

    #[test]
    fn step_not_complete_when_no_completed_at() {
        let step = Step {
            id: StepId::new(),
            run_id: RunId::new(),
            turn_id: TurnId::new(),
            step_number: 0,
            kind: StepKind::ModelInference,
            effect_intent_id: None,
            artifacts_produced: Vec::new(),
            started_at: Utc::now(),
            completed_at: None,
        };
        assert!(!step.is_complete());
    }

    #[test]
    fn step_serde_round_trip() {
        let step = Step {
            id: StepId::new(),
            run_id: RunId::new(),
            turn_id: TurnId::new(),
            step_number: 2,
            kind: StepKind::ToolInvocation,
            effect_intent_id: Some(crate::ids::EffectId::new()),
            artifacts_produced: vec![ArtifactId::new()],
            started_at: Utc::now(),
            completed_at: None,
        };
        let json = serde_json::to_string(&step).unwrap();
        let back: Step = serde_json::from_str(&json).unwrap();
        assert_eq!(step.id, back.id);
        assert_eq!(step.step_number, back.step_number);
        assert_eq!(step.kind, back.kind);
    }

    // --- Turn ---

    #[test]
    fn turn_new_has_empty_steps_and_zero_usage() {
        let turn = make_turn();
        assert!(turn.steps.is_empty());
        assert_eq!(turn.token_usage.total_tokens, 0);
        assert!(!turn.is_complete());
    }

    #[test]
    fn turn_step_count() {
        let mut turn = make_turn();
        turn.steps.push(Step {
            id: StepId::new(),
            run_id: turn.run_id,
            turn_id: turn.id,
            step_number: 0,
            kind: StepKind::ContextAssembly,
            effect_intent_id: None,
            artifacts_produced: vec![],
            started_at: Utc::now(),
            completed_at: None,
        });
        assert_eq!(turn.step_count(), 1);
    }

    #[test]
    fn turn_is_complete_when_completed_at_set() {
        let mut turn = make_turn();
        turn.completed_at = Some(Utc::now());
        assert!(turn.is_complete());
    }

    #[test]
    fn turn_serde_round_trip() {
        let turn = make_turn();
        let json = serde_json::to_string(&turn).unwrap();
        let back: Turn = serde_json::from_str(&json).unwrap();
        assert_eq!(turn.id, back.id);
        assert_eq!(turn.sequence, back.sequence);
        assert_eq!(turn.role, back.role);
    }

    // --- MessageRole ---

    #[test]
    fn message_role_serde_round_trip() {
        let role = MessageRole::Assistant;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, r#""assistant""#);
        let back: MessageRole = serde_json::from_str(&json).unwrap();
        assert_eq!(back, role);
    }
}
