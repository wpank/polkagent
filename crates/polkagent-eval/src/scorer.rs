//! Scoring functions for evaluating agent responses.

use serde::{Deserialize, Serialize};

use crate::report::CaseResult;
use crate::types::{Expected, ExpectedOutcome};

// ---------------------------------------------------------------------------
// CheckResult
// ---------------------------------------------------------------------------

/// The result of a single scoring check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    /// Short name identifying the check.
    pub name: String,
    /// Whether the check passed.
    pub passed: bool,
    /// Human-readable message explaining the outcome.
    pub message: String,
}

// ---------------------------------------------------------------------------
// Score
// ---------------------------------------------------------------------------

/// The aggregate score for a single evaluation case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Score {
    /// Whether the case passed overall.
    pub passed: bool,
    /// Numeric score in the range `[0.0, 1.0]`.
    pub score: f64,
    /// Human-readable summary of the scoring outcome.
    pub details: String,
    /// Individual check results that contributed to the aggregate score.
    pub checks: Vec<CheckResult>,
}

impl Score {
    /// Return a perfect score (1.0, passed) with no checks.
    #[must_use]
    pub fn perfect() -> Self {
        Self {
            passed: true,
            score: 1.0,
            details: "All checks passed".into(),
            checks: Vec::new(),
        }
    }

    /// Return a zero score (0.0, failed) with a reason message.
    #[must_use]
    pub fn zero(reason: impl Into<String>) -> Self {
        Self {
            passed: false,
            score: 0.0,
            details: reason.into(),
            checks: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// score_case
// ---------------------------------------------------------------------------

/// Score a case result against its expected specification.
///
/// Returns a [`Score`] with all individual check results and an aggregate
/// numeric score computed as the mean of all check pass/fail values.
#[must_use]
pub fn score_case(result: &CaseResult, expected: &Expected) -> Score {
    if result.error.is_some() && expected.expected_outcome != Some(ExpectedOutcome::Error) {
        return Score::zero(format!(
            "Case errored: {}",
            result.error.as_deref().unwrap_or("unknown error")
        ));
    }

    let mut checks: Vec<CheckResult> = Vec::new();

    // Check: must_contain strings present in output
    for required in &expected.must_contain {
        let passed = result.model_output.contains(required.as_str());
        checks.push(CheckResult {
            name: format!("must_contain:{required}"),
            passed,
            message: if passed {
                format!("Output contains required string: {required:?}")
            } else {
                format!("Output missing required string: {required:?}")
            },
        });
    }

    // Check: must_not_contain strings absent from output
    for forbidden in &expected.must_not_contain {
        let passed = !result.model_output.contains(forbidden.as_str());
        checks.push(CheckResult {
            name: format!("must_not_contain:{forbidden}"),
            passed,
            message: if passed {
                format!("Output correctly omits forbidden string: {forbidden:?}")
            } else {
                format!("Output contains forbidden string: {forbidden:?}")
            },
        });
    }

    // Check: expected tool calls were made
    for expected_call in &expected.expected_tool_calls {
        let tool_name = &expected_call.tool_name;
        let call_found = result.tool_calls_made.iter().any(|call| {
            if call.tool_name != *tool_name {
                return false;
            }
            // Verify all required args are present in the call's arguments.
            if expected_call.args_contain.is_empty() {
                return true;
            }
            let Ok(actual_args) = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
            else {
                return false;
            };
            expected_call.args_contain.iter().all(|(key, expected_val)| {
                actual_args.get(key).map_or(false, |v| v == expected_val)
            })
        });
        checks.push(CheckResult {
            name: format!("tool_call:{tool_name}"),
            passed: call_found,
            message: if call_found {
                format!("Expected tool call made: {tool_name}")
            } else {
                format!("Expected tool call not found: {tool_name}")
            },
        });
    }

    // Check: outcome matches expectation
    if let Some(expected_outcome) = expected.expected_outcome {
        let normalized = result.model_output.to_lowercase()
            .replace('\u{2019}', "'")
            .replace('\u{2018}', "'");
        let actual_outcome = if result.error.is_some() {
            ExpectedOutcome::Error
        } else if normalized.contains("i cannot")
            || normalized.contains("i'm unable")
            || normalized.contains("i won't")
            || normalized.contains("i refuse")
            || normalized.contains("i can't")
            || normalized.contains("i'm not able")
            || normalized.contains("i must decline")
            || normalized.contains("i can not")
        {
            ExpectedOutcome::Refusal
        } else {
            ExpectedOutcome::Success
        };

        let passed = actual_outcome == expected_outcome;
        checks.push(CheckResult {
            name: "outcome".into(),
            passed,
            message: if passed {
                format!("Outcome matches expected: {expected_outcome:?}")
            } else {
                format!(
                    "Outcome mismatch: expected {expected_outcome:?}, got {actual_outcome:?}"
                )
            },
        });
    }

    aggregate_score(checks)
}

/// Compute an aggregate [`Score`] from a list of individual check results.
///
/// The aggregate score is the arithmetic mean of all check pass/fail values
/// (1.0 for pass, 0.0 for fail). The case passes overall when all checks pass.
#[must_use]
pub fn aggregate_score(checks: Vec<CheckResult>) -> Score {
    if checks.is_empty() {
        return Score::perfect();
    }

    let total = checks.len();
    let passed_count = checks.iter().filter(|c| c.passed).count();
    let score = passed_count as f64 / total as f64;
    let all_passed = passed_count == total;

    let details = if all_passed {
        format!("All {total} checks passed")
    } else {
        format!("{passed_count}/{total} checks passed")
    };

    Score {
        passed: all_passed,
        score,
        details,
        checks,
    }
}

/// Compute the mean score across a slice of `Score` values.
///
/// Returns `0.0` for an empty slice.
#[must_use]
pub fn mean_score(scores: &[f64]) -> f64 {
    if scores.is_empty() {
        return 0.0;
    }
    scores.iter().sum::<f64>() / scores.len() as f64
}

/// Compute a weighted mean score given parallel slices of scores and weights.
///
/// Returns `0.0` if the total weight is zero.
#[must_use]
pub fn weighted_mean_score(scores: &[f64], weights: &[f64]) -> f64 {
    debug_assert_eq!(
        scores.len(),
        weights.len(),
        "scores and weights must have equal length"
    );
    let total_weight: f64 = weights.iter().sum();
    if total_weight == 0.0 {
        return 0.0;
    }
    let weighted_sum: f64 = scores.iter().zip(weights.iter()).map(|(s, w)| s * w).sum();
    weighted_sum / total_weight
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::report::ToolCallRecord;
    use crate::types::{EvalCategory, Expected, ExpectedOutcome, ExpectedToolCall};

    fn make_result(output: &str) -> CaseResult {
        CaseResult {
            case_id: "c1".into(),
            case_name: "test case".into(),
            score: Score::perfect(),
            model_output: output.into(),
            tool_calls_made: Vec::new(),
            duration_ms: 0,
            error: None,
            category: EvalCategory::General,
        }
    }

    #[test]
    fn all_checks_pass_score_is_one() {
        let result = make_result("The answer is 42");
        let expected = Expected {
            must_contain: vec!["42".into()],
            must_not_contain: vec!["error".into()],
            ..Expected::default()
        };
        let score = score_case(&result, &expected);
        assert!(score.passed);
        assert!((score.score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn some_checks_fail_partial_score() {
        let result = make_result("partial response");
        let expected = Expected {
            must_contain: vec!["partial".into(), "missing_thing".into()],
            ..Expected::default()
        };
        let score = score_case(&result, &expected);
        assert!(!score.passed);
        assert!((score.score - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn must_contain_present_passes() {
        let result = make_result("contains the keyword here");
        let expected = Expected {
            must_contain: vec!["keyword".into()],
            ..Expected::default()
        };
        let score = score_case(&result, &expected);
        assert!(score.passed);
        assert_eq!(score.checks.len(), 1);
        assert!(score.checks[0].passed);
    }

    #[test]
    fn must_contain_absent_fails() {
        let result = make_result("does not have what is required");
        let expected = Expected {
            must_contain: vec!["missing_keyword".into()],
            ..Expected::default()
        };
        let score = score_case(&result, &expected);
        assert!(!score.passed);
        assert_eq!(score.checks.len(), 1);
        assert!(!score.checks[0].passed);
    }

    #[test]
    fn must_not_contain_absent_passes() {
        let result = make_result("safe response");
        let expected = Expected {
            must_not_contain: vec!["dangerous".into()],
            ..Expected::default()
        };
        let score = score_case(&result, &expected);
        assert!(score.passed);
        assert!(score.checks[0].passed);
    }

    #[test]
    fn must_not_contain_present_fails() {
        let result = make_result("this is a dangerous response");
        let expected = Expected {
            must_not_contain: vec!["dangerous".into()],
            ..Expected::default()
        };
        let score = score_case(&result, &expected);
        assert!(!score.passed);
        assert!(!score.checks[0].passed);
    }

    #[test]
    fn expected_tool_call_found_passes() {
        let mut result = make_result("I will call the tool");
        result.tool_calls_made.push(ToolCallRecord {
            tool_name: "chain.query".into(),
            arguments_json: r#"{"network":"polkadot"}"#.into(),
        });
        let mut args = HashMap::new();
        args.insert(
            "network".into(),
            serde_json::Value::String("polkadot".into()),
        );
        let expected = Expected {
            expected_tool_calls: vec![ExpectedToolCall {
                tool_name: "chain.query".into(),
                args_contain: args,
            }],
            ..Expected::default()
        };
        let score = score_case(&result, &expected);
        assert!(score.passed);
        assert!(score.checks[0].passed);
    }

    #[test]
    fn expected_tool_call_not_made_fails() {
        let result = make_result("I decided not to call a tool");
        let expected = Expected {
            expected_tool_calls: vec![ExpectedToolCall {
                tool_name: "chain.query".into(),
                args_contain: HashMap::new(),
            }],
            ..Expected::default()
        };
        let score = score_case(&result, &expected);
        assert!(!score.passed);
        assert!(!score.checks[0].passed);
    }

    #[test]
    fn expected_outcome_success_matches() {
        let result = make_result("Here is the result you requested.");
        let expected = Expected {
            expected_outcome: Some(ExpectedOutcome::Success),
            ..Expected::default()
        };
        let score = score_case(&result, &expected);
        assert!(score.passed);
    }

    #[test]
    fn expected_outcome_refusal_matches() {
        let result = make_result("I cannot assist with that request.");
        let expected = Expected {
            expected_outcome: Some(ExpectedOutcome::Refusal),
            ..Expected::default()
        };
        let score = score_case(&result, &expected);
        assert!(score.passed);
    }

    #[test]
    fn expected_outcome_mismatch_fails() {
        let result = make_result("Here is the result you requested.");
        let expected = Expected {
            expected_outcome: Some(ExpectedOutcome::Refusal),
            ..Expected::default()
        };
        let score = score_case(&result, &expected);
        assert!(!score.passed);
    }

    #[test]
    fn error_result_scores_zero_when_not_expected() {
        let mut result = make_result("");
        result.error = Some("timeout".into());
        let score = score_case(&result, &Expected::default());
        assert!(!score.passed);
        assert!((score.score - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn aggregate_score_empty_checks_is_perfect() {
        let score = aggregate_score(Vec::new());
        assert!(score.passed);
        assert!((score.score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn mean_score_empty_is_zero() {
        assert!((mean_score(&[]) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn mean_score_values() {
        let scores = [1.0, 0.0, 0.5];
        let mean = mean_score(&scores);
        assert!((mean - 0.5).abs() < 1e-10);
    }

    #[test]
    fn weighted_mean_score_basic() {
        let scores = [1.0, 0.0];
        let weights = [3.0, 1.0];
        let wmean = weighted_mean_score(&scores, &weights);
        assert!((wmean - 0.75).abs() < 1e-10);
    }

    #[test]
    fn weighted_mean_score_zero_total_weight() {
        let scores = [1.0, 0.5];
        let weights = [0.0, 0.0];
        let wmean = weighted_mean_score(&scores, &weights);
        assert!((wmean - 0.0).abs() < f64::EPSILON);
    }
}
