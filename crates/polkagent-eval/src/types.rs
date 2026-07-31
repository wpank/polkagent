//! Core evaluation types for the Polkagent eval framework.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// EvalCategory
// ---------------------------------------------------------------------------

/// The category of an evaluation case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalCategory {
    /// Evaluation of the agent's ability to explain chain concepts.
    ChainExplanation,
    /// Evaluation of the agent's safety judgment.
    SafetyJudgment,
    /// Evaluation of tool use correctness.
    ToolUse,
    /// Evaluation of governance-related reasoning.
    Governance,
    /// Evaluation of treasury-related operations.
    Treasury,
    /// General evaluation cases not fitting another category.
    General,
}

// ---------------------------------------------------------------------------
// EvalInput
// ---------------------------------------------------------------------------

/// The input provided to the agent for a single evaluation case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalInput {
    /// The user prompt presented to the agent.
    pub prompt: String,
    /// Additional context key/value pairs provided alongside the prompt.
    pub context: HashMap<String, serde_json::Value>,
    /// Names of tools that should be available to the agent.
    pub tools_available: Vec<String>,
}

impl EvalInput {
    /// Create a minimal `EvalInput` with only a prompt.
    #[must_use]
    pub fn from_prompt(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            context: HashMap::new(),
            tools_available: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// ExpectedToolCall
// ---------------------------------------------------------------------------

/// Describes a tool call that the agent is expected to make.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedToolCall {
    /// The name of the tool that should be called.
    pub tool_name: String,
    /// Key/value pairs that must appear in the tool's arguments.
    pub args_contain: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// ExpectedOutcome
// ---------------------------------------------------------------------------

/// The expected high-level outcome of an evaluation case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedOutcome {
    /// The agent should complete the task successfully.
    Success,
    /// The agent should refuse the task (e.g., for safety reasons).
    Refusal,
    /// The agent should produce or surface an error.
    Error,
}

// ---------------------------------------------------------------------------
// Expected
// ---------------------------------------------------------------------------

/// Specification of what a correct agent response looks like.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Expected {
    /// Strings that must be present in the model's output text.
    pub must_contain: Vec<String>,
    /// Strings that must be absent from the model's output text.
    pub must_not_contain: Vec<String>,
    /// Tool calls that the model must make.
    pub expected_tool_calls: Vec<ExpectedToolCall>,
    /// The expected high-level outcome, if any.
    pub expected_outcome: Option<ExpectedOutcome>,
    /// Name of a custom scorer function to apply, if any.
    pub custom_scorer: Option<String>,
}

impl Default for Expected {
    fn default() -> Self {
        Self {
            must_contain: Vec::new(),
            must_not_contain: Vec::new(),
            expected_tool_calls: Vec::new(),
            expected_outcome: None,
            custom_scorer: None,
        }
    }
}

// ---------------------------------------------------------------------------
// EvalCase
// ---------------------------------------------------------------------------

/// A single evaluation case within a suite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalCase {
    /// Unique identifier for this case within the suite.
    pub id: String,
    /// Human-readable name for this case.
    pub name: String,
    /// The category this case belongs to.
    pub category: EvalCategory,
    /// The input to present to the agent.
    pub input: EvalInput,
    /// Specification of the expected correct response.
    pub expected: Expected,
    /// Freeform tags for filtering and grouping.
    pub tags: Vec<String>,
    /// Maximum number of seconds to allow this case to run before timing out.
    pub timeout_secs: u64,
}

// ---------------------------------------------------------------------------
// EvalSuite
// ---------------------------------------------------------------------------

/// A named, versioned collection of evaluation cases.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalSuite {
    /// Short machine-readable name for this suite.
    pub name: String,
    /// Human-readable description of what this suite evaluates.
    pub description: String,
    /// Semantic version string for this suite (e.g., `"1.0.0"`).
    pub version: String,
    /// The evaluation cases that make up this suite.
    pub cases: Vec<EvalCase>,
}

impl EvalSuite {
    /// Return the number of cases in the suite.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cases.len()
    }

    /// Return `true` if the suite contains no cases.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cases.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_input_from_prompt() {
        let input = EvalInput::from_prompt("hello");
        assert_eq!(input.prompt, "hello");
        assert!(input.context.is_empty());
        assert!(input.tools_available.is_empty());
    }

    #[test]
    fn expected_default() {
        let expected = Expected::default();
        assert!(expected.must_contain.is_empty());
        assert!(expected.must_not_contain.is_empty());
        assert!(expected.expected_tool_calls.is_empty());
        assert!(expected.expected_outcome.is_none());
        assert!(expected.custom_scorer.is_none());
    }

    #[test]
    fn eval_category_serde() {
        let cat = EvalCategory::SafetyJudgment;
        let json = serde_json::to_string(&cat).expect("serialize");
        assert_eq!(json, r#""safety_judgment""#);
        let back: EvalCategory = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cat, back);
    }

    #[test]
    fn eval_suite_len_and_is_empty() {
        let mut suite = EvalSuite {
            name: "test".into(),
            description: "desc".into(),
            version: "1.0.0".into(),
            cases: Vec::new(),
        };
        assert!(suite.is_empty());
        assert_eq!(suite.len(), 0);
        suite.cases.push(EvalCase {
            id: "c1".into(),
            name: "case one".into(),
            category: EvalCategory::General,
            input: EvalInput::from_prompt("hi"),
            expected: Expected::default(),
            tags: Vec::new(),
            timeout_secs: 30,
        });
        assert!(!suite.is_empty());
        assert_eq!(suite.len(), 1);
    }

    #[test]
    fn expected_outcome_serde() {
        let outcomes = [
            (ExpectedOutcome::Success, r#""success""#),
            (ExpectedOutcome::Refusal, r#""refusal""#),
            (ExpectedOutcome::Error, r#""error""#),
        ];
        for (outcome, expected_json) in &outcomes {
            let json = serde_json::to_string(outcome).expect("serialize");
            assert_eq!(&json, expected_json);
            let back: ExpectedOutcome = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(outcome, &back);
        }
    }
}
