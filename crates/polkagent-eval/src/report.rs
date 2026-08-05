//! Report generation for evaluation runs.

use std::collections::HashMap;
use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::scorer::Score;
use crate::types::EvalCategory;
use crate::usize_to_f64;

// ---------------------------------------------------------------------------
// ToolCallRecord
// ---------------------------------------------------------------------------

/// A record of a tool call made by the model during an evaluation case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    /// The name of the tool that was called.
    pub tool_name: String,
    /// The JSON-serialized arguments passed to the tool.
    pub arguments_json: String,
}

// ---------------------------------------------------------------------------
// CaseResult
// ---------------------------------------------------------------------------

/// The result of running a single evaluation case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseResult {
    /// The ID of the evaluation case.
    pub case_id: String,
    /// The human-readable name of the evaluation case.
    pub case_name: String,
    /// The score assigned to this result.
    pub score: Score,
    /// The full text output produced by the model.
    pub model_output: String,
    /// All tool calls made by the model during this case.
    pub tool_calls_made: Vec<ToolCallRecord>,
    /// How long the case took to complete, in milliseconds.
    pub duration_ms: u64,
    /// Error message if the case failed to run at all.
    pub error: Option<String>,
    /// The category this case belongs to (copied from the case definition).
    pub category: EvalCategory,
}

// ---------------------------------------------------------------------------
// EvalReport
// ---------------------------------------------------------------------------

/// The full report produced after running an evaluation suite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalReport {
    /// Name of the suite that was evaluated.
    pub suite_name: String,
    /// When this report was generated.
    pub timestamp: DateTime<Utc>,
    /// Total number of cases in the suite.
    pub total_cases: usize,
    /// Number of cases that passed.
    pub passed: usize,
    /// Number of cases that failed.
    pub failed: usize,
    /// Number of cases that were skipped (e.g., due to timeout or error).
    pub skipped: usize,
    /// Mean score across all cases in `[0.0, 1.0]`.
    pub mean_score: f64,
    /// Mean score broken down by category.
    pub category_scores: HashMap<EvalCategory, f64>,
    /// Individual case results.
    pub results: Vec<CaseResult>,
}

impl EvalReport {
    /// Build an `EvalReport` from a suite name and a list of case results.
    #[must_use]
    pub fn from_results(suite_name: impl Into<String>, results: Vec<CaseResult>) -> Self {
        let total_cases = results.len();
        let passed = results.iter().filter(|r| r.score.passed).count();
        let skipped = results.iter().filter(|r| r.error.is_some()).count();
        let failed = total_cases.saturating_sub(passed).saturating_sub(skipped);

        let mean_score = if total_cases == 0 {
            0.0
        } else {
            results.iter().map(|r| r.score.score).sum::<f64>() / usize_to_f64(total_cases)
        };

        // Aggregate per-category scores.
        let mut category_totals: HashMap<EvalCategory, (f64, usize)> = HashMap::new();
        for r in &results {
            let entry = category_totals.entry(r.category).or_insert((0.0, 0));
            entry.0 += r.score.score;
            entry.1 += 1;
        }
        let category_scores: HashMap<EvalCategory, f64> = category_totals
            .into_iter()
            .map(|(cat, (sum, count))| (cat, sum / usize_to_f64(count)))
            .collect();

        Self {
            suite_name: suite_name.into(),
            timestamp: Utc::now(),
            total_cases,
            passed,
            failed,
            skipped,
            mean_score,
            category_scores,
            results,
        }
    }
}

// ---------------------------------------------------------------------------
// Report rendering
// ---------------------------------------------------------------------------

/// Serialize an `EvalReport` to a `serde_json::Value`.
#[must_use]
pub fn report_to_json(report: &EvalReport) -> serde_json::Value {
    serde_json::to_value(report).unwrap_or_else(
        |e| serde_json::json!({ "error": format!("Failed to serialize report: {e}") }),
    )
}

/// Render an `EvalReport` as a Markdown document.
#[must_use]
pub fn report_to_markdown(report: &EvalReport) -> String {
    let mut md = String::new();

    let _ = writeln!(md, "# Eval Report: {}\n", report.suite_name);
    let _ = writeln!(
        md,
        "**Timestamp:** {}\n",
        report.timestamp.format("%Y-%m-%d %H:%M:%S UTC")
    );
    md.push_str("## Summary\n\n");
    md.push_str("| Metric | Value |\n");
    md.push_str("|--------|-------|\n");
    let _ = writeln!(md, "| Total cases | {} |", report.total_cases);
    let _ = writeln!(md, "| Passed | {} |", report.passed);
    let _ = writeln!(md, "| Failed | {} |", report.failed);
    let _ = writeln!(md, "| Skipped | {} |", report.skipped);
    let _ = writeln!(md, "| Mean score | {:.3} |", report.mean_score);
    md.push('\n');

    if !report.category_scores.is_empty() {
        md.push_str("## Category Scores\n\n");
        md.push_str("| Category | Score |\n");
        md.push_str("|----------|-------|\n");
        let mut categories: Vec<_> = report.category_scores.iter().collect();
        categories.sort_by_key(|(cat, _)| format!("{cat:?}"));
        for (cat, score) in &categories {
            let _ = writeln!(md, "| {cat:?} | {score:.3} |");
        }
        md.push('\n');
    }

    md.push_str("## Case Results\n\n");
    for result in &report.results {
        let status = if result.error.is_some() {
            "SKIP"
        } else if result.score.passed {
            "PASS"
        } else {
            "FAIL"
        };
        let _ = writeln!(
            md,
            "### [{status}] {} ({})\n",
            result.case_name, result.case_id
        );
        let _ = writeln!(md, "- **Score:** {:.3}", result.score.score);
        let _ = writeln!(md, "- **Duration:** {}ms", result.duration_ms);
        let _ = writeln!(md, "- **Details:** {}", result.score.details);
        if let Some(err) = &result.error {
            let _ = writeln!(md, "- **Error:** {err}");
        }
        md.push('\n');
    }

    md
}

/// Produce a one-line summary string for an `EvalReport`.
#[must_use]
pub fn report_summary(report: &EvalReport) -> String {
    format!(
        "{}: {}/{} passed ({:.1}%) — mean score {:.3}",
        report.suite_name,
        report.passed,
        report.total_cases,
        if report.total_cases == 0 {
            0.0
        } else {
            usize_to_f64(report.passed) / usize_to_f64(report.total_cases) * 100.0
        },
        report.mean_score,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// These assertion-oriented unit tests intentionally fail fast on fixture errors.
#[allow(clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::scorer::Score;
    use crate::types::EvalCategory;

    fn passing_result(id: &str, category: EvalCategory) -> CaseResult {
        CaseResult {
            case_id: id.into(),
            case_name: format!("Case {id}"),
            score: Score {
                passed: true,
                score: 1.0,
                details: "ok".into(),
                checks: Vec::new(),
            },
            model_output: "output".into(),
            tool_calls_made: Vec::new(),
            duration_ms: 100,
            error: None,
            category,
        }
    }

    fn failing_result(id: &str, category: EvalCategory) -> CaseResult {
        CaseResult {
            case_id: id.into(),
            case_name: format!("Case {id}"),
            score: Score {
                passed: false,
                score: 0.0,
                details: "failed".into(),
                checks: Vec::new(),
            },
            model_output: "output".into(),
            tool_calls_made: Vec::new(),
            duration_ms: 100,
            error: None,
            category,
        }
    }

    #[test]
    fn report_from_results_counts() {
        let results = vec![
            passing_result("c1", EvalCategory::General),
            failing_result("c2", EvalCategory::General),
        ];
        let report = EvalReport::from_results("test-suite", results);
        assert_eq!(report.total_cases, 2);
        assert_eq!(report.passed, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(report.skipped, 0);
    }

    #[test]
    fn report_mean_score() {
        let results = vec![
            passing_result("c1", EvalCategory::General),
            failing_result("c2", EvalCategory::General),
        ];
        let report = EvalReport::from_results("test-suite", results);
        assert!((report.mean_score - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn report_empty_suite() {
        let report = EvalReport::from_results("empty", Vec::new());
        assert_eq!(report.total_cases, 0);
        assert_eq!(report.passed, 0);
        assert_eq!(report.failed, 0);
        assert!((report.mean_score - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn category_scores_aggregated() {
        let results = vec![
            passing_result("c1", EvalCategory::SafetyJudgment),
            failing_result("c2", EvalCategory::SafetyJudgment),
            passing_result("c3", EvalCategory::General),
        ];
        let report = EvalReport::from_results("test", results);
        let safety_score = report
            .category_scores
            .get(&EvalCategory::SafetyJudgment)
            .copied()
            .unwrap_or(0.0);
        assert!((safety_score - 0.5).abs() < f64::EPSILON);
        let general_score = report
            .category_scores
            .get(&EvalCategory::General)
            .copied()
            .unwrap_or(0.0);
        assert!((general_score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn report_to_json_contains_suite_name() {
        let report = EvalReport::from_results("my-suite", Vec::new());
        let json = report_to_json(&report);
        assert_eq!(json["suite_name"], "my-suite");
    }

    #[test]
    fn report_to_json_has_results_array() {
        let results = vec![passing_result("c1", EvalCategory::General)];
        let report = EvalReport::from_results("suite", results);
        let json = report_to_json(&report);
        assert!(json["results"].is_array());
        assert_eq!(json["results"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn report_to_markdown_contains_suite_name() {
        let report = EvalReport::from_results("safety-eval", Vec::new());
        let md = report_to_markdown(&report);
        assert!(md.contains("safety-eval"));
    }

    #[test]
    fn report_to_markdown_contains_summary_table() {
        let report = EvalReport::from_results("test", Vec::new());
        let md = report_to_markdown(&report);
        assert!(md.contains("Total cases"));
        assert!(md.contains("Passed"));
        assert!(md.contains("Mean score"));
    }

    #[test]
    fn report_to_markdown_contains_case_results() {
        let results = vec![
            passing_result("c1", EvalCategory::General),
            failing_result("c2", EvalCategory::SafetyJudgment),
        ];
        let report = EvalReport::from_results("suite", results);
        let md = report_to_markdown(&report);
        assert!(md.contains("PASS"));
        assert!(md.contains("FAIL"));
        assert!(md.contains("Case c1"));
        assert!(md.contains("Case c2"));
    }

    #[test]
    fn report_summary_format() {
        let results = vec![
            passing_result("c1", EvalCategory::General),
            failing_result("c2", EvalCategory::General),
        ];
        let report = EvalReport::from_results("my-suite", results);
        let summary = report_summary(&report);
        assert!(summary.contains("my-suite"));
        assert!(summary.contains("1/2"));
        assert!(summary.contains("50.0%"));
    }

    #[test]
    fn report_summary_empty() {
        let report = EvalReport::from_results("empty-suite", Vec::new());
        let summary = report_summary(&report);
        assert!(summary.contains("empty-suite"));
        assert!(summary.contains("0/0"));
        assert!(summary.contains("0.0%"));
    }

    #[test]
    fn report_skipped_count() {
        let mut errored = failing_result("c1", EvalCategory::General);
        errored.error = Some("timeout".into());
        let results = vec![passing_result("c2", EvalCategory::General), errored];
        let report = EvalReport::from_results("suite", results);
        assert_eq!(report.skipped, 1);
    }
}
