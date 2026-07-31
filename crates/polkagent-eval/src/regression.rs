//! Regression detection between evaluation report baselines and current runs.

use serde::{Deserialize, Serialize};

use crate::report::EvalReport;

// ---------------------------------------------------------------------------
// Regression / Improvement
// ---------------------------------------------------------------------------

/// A performance regression detected between a baseline and a current report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Regression {
    /// The ID of the case that regressed.
    pub case_id: String,
    /// The score in the baseline report.
    pub baseline_score: f64,
    /// The score in the current report.
    pub current_score: f64,
    /// The signed score delta (`current_score - baseline_score`), always negative.
    pub delta: f64,
}

/// A performance improvement detected between a baseline and a current report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Improvement {
    /// The ID of the case that improved.
    pub case_id: String,
    /// The score in the baseline report.
    pub baseline_score: f64,
    /// The score in the current report.
    pub current_score: f64,
    /// The signed score delta (`current_score - baseline_score`), always positive.
    pub delta: f64,
}

// ---------------------------------------------------------------------------
// RegressionResult
// ---------------------------------------------------------------------------

/// The result of comparing a baseline report against a current report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegressionResult {
    /// Cases whose score decreased compared to the baseline.
    pub regressions: Vec<Regression>,
    /// Cases whose score increased compared to the baseline.
    pub improvements: Vec<Improvement>,
    /// Number of cases whose score was unchanged.
    pub unchanged: usize,
    /// Number of cases present in the current report but not the baseline.
    pub new_cases: usize,
    /// Number of cases present in the baseline but missing from the current report.
    pub removed_cases: usize,
}

impl RegressionResult {
    /// Returns `true` if there are no regressions.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.regressions.is_empty()
    }

    /// Returns the total number of cases that changed (regressed or improved).
    #[must_use]
    pub fn changed_count(&self) -> usize {
        self.regressions.len() + self.improvements.len()
    }
}

// ---------------------------------------------------------------------------
// RegressionDetector
// ---------------------------------------------------------------------------

/// Detects regressions and improvements between two eval reports.
#[derive(Debug, Default)]
pub struct RegressionDetector {
    /// Minimum absolute score delta required to classify a change as a
    /// regression or improvement (defaults to `0.0001`).
    pub min_delta: f64,
}

impl RegressionDetector {
    /// Create a new `RegressionDetector` with a custom minimum delta threshold.
    #[must_use]
    pub fn with_min_delta(min_delta: f64) -> Self {
        Self { min_delta }
    }

    /// Compare a baseline report against a current report and return the result.
    #[must_use]
    pub fn compare_reports(
        &self,
        baseline: &EvalReport,
        current: &EvalReport,
    ) -> RegressionResult {
        compare_reports_with_threshold(baseline, current, self.min_delta)
    }
}

/// Compare two reports with a given minimum delta threshold.
///
/// This is the free function variant of [`RegressionDetector::compare_reports`].
#[must_use]
pub fn compare_reports(baseline: &EvalReport, current: &EvalReport) -> RegressionResult {
    compare_reports_with_threshold(baseline, current, 1e-9)
}

fn compare_reports_with_threshold(
    baseline: &EvalReport,
    current: &EvalReport,
    min_delta: f64,
) -> RegressionResult {
    use std::collections::HashMap;

    let baseline_map: HashMap<&str, f64> = baseline
        .results
        .iter()
        .map(|r| (r.case_id.as_str(), r.score.score))
        .collect();

    let current_map: HashMap<&str, f64> = current
        .results
        .iter()
        .map(|r| (r.case_id.as_str(), r.score.score))
        .collect();

    let mut regressions = Vec::new();
    let mut improvements = Vec::new();
    let mut unchanged = 0usize;
    let mut new_cases = 0usize;
    let mut removed_cases = 0usize;

    // Iterate over current report cases.
    for result in &current.results {
        let case_id = result.case_id.as_str();
        let current_score = result.score.score;

        if let Some(&baseline_score) = baseline_map.get(case_id) {
            let delta = current_score - baseline_score;
            if delta < -min_delta {
                regressions.push(Regression {
                    case_id: result.case_id.clone(),
                    baseline_score,
                    current_score,
                    delta,
                });
            } else if delta > min_delta {
                improvements.push(Improvement {
                    case_id: result.case_id.clone(),
                    baseline_score,
                    current_score,
                    delta,
                });
            } else {
                unchanged += 1;
            }
        } else {
            new_cases += 1;
        }
    }

    // Count cases in baseline that are absent from current.
    for case_id in baseline_map.keys() {
        if !current_map.contains_key(case_id) {
            removed_cases += 1;
        }
    }

    RegressionResult {
        regressions,
        improvements,
        unchanged,
        new_cases,
        removed_cases,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{CaseResult, EvalReport};
    use crate::scorer::Score;
    use crate::types::EvalCategory;

    fn make_report_with_scores(suite_name: &str, cases: &[(&str, f64)]) -> EvalReport {
        let results: Vec<CaseResult> = cases
            .iter()
            .map(|(id, score)| CaseResult {
                case_id: (*id).into(),
                case_name: format!("Case {id}"),
                score: Score {
                    passed: *score >= 1.0,
                    score: *score,
                    details: String::new(),
                    checks: Vec::new(),
                },
                model_output: String::new(),
                tool_calls_made: Vec::new(),
                duration_ms: 0,
                error: None,
                category: EvalCategory::General,
            })
            .collect();
        EvalReport::from_results(suite_name, results)
    }

    #[test]
    fn regression_detected_when_score_decreases() {
        let baseline = make_report_with_scores("s", &[("c1", 1.0), ("c2", 0.8)]);
        let current = make_report_with_scores("s", &[("c1", 1.0), ("c2", 0.4)]);
        let result = compare_reports(&baseline, &current);
        assert_eq!(result.regressions.len(), 1);
        assert_eq!(result.regressions[0].case_id, "c2");
        assert!((result.regressions[0].delta - (-0.4)).abs() < 1e-9);
    }

    #[test]
    fn improvement_detected_when_score_increases() {
        let baseline = make_report_with_scores("s", &[("c1", 0.5)]);
        let current = make_report_with_scores("s", &[("c1", 0.9)]);
        let result = compare_reports(&baseline, &current);
        assert_eq!(result.improvements.len(), 1);
        assert_eq!(result.improvements[0].case_id, "c1");
        assert!((result.improvements[0].delta - 0.4).abs() < 1e-9);
    }

    #[test]
    fn unchanged_counted_when_score_same() {
        let baseline = make_report_with_scores("s", &[("c1", 0.7)]);
        let current = make_report_with_scores("s", &[("c1", 0.7)]);
        let result = compare_reports(&baseline, &current);
        assert_eq!(result.unchanged, 1);
        assert!(result.regressions.is_empty());
        assert!(result.improvements.is_empty());
    }

    #[test]
    fn new_cases_counted() {
        let baseline = make_report_with_scores("s", &[("c1", 1.0)]);
        let current = make_report_with_scores("s", &[("c1", 1.0), ("c2", 0.9)]);
        let result = compare_reports(&baseline, &current);
        assert_eq!(result.new_cases, 1);
    }

    #[test]
    fn removed_cases_counted() {
        let baseline = make_report_with_scores("s", &[("c1", 1.0), ("c2", 0.9)]);
        let current = make_report_with_scores("s", &[("c1", 1.0)]);
        let result = compare_reports(&baseline, &current);
        assert_eq!(result.removed_cases, 1);
    }

    #[test]
    fn is_clean_when_no_regressions() {
        let baseline = make_report_with_scores("s", &[("c1", 0.5)]);
        let current = make_report_with_scores("s", &[("c1", 1.0)]);
        let result = compare_reports(&baseline, &current);
        assert!(result.is_clean());
    }

    #[test]
    fn is_not_clean_when_regressions_exist() {
        let baseline = make_report_with_scores("s", &[("c1", 1.0)]);
        let current = make_report_with_scores("s", &[("c1", 0.5)]);
        let result = compare_reports(&baseline, &current);
        assert!(!result.is_clean());
    }

    #[test]
    fn changed_count_includes_regressions_and_improvements() {
        let baseline = make_report_with_scores("s", &[("c1", 1.0), ("c2", 0.5), ("c3", 0.7)]);
        let current = make_report_with_scores("s", &[("c1", 0.5), ("c2", 1.0), ("c3", 0.7)]);
        let result = compare_reports(&baseline, &current);
        assert_eq!(result.changed_count(), 2);
        assert_eq!(result.unchanged, 1);
    }

    #[test]
    fn multiple_regressions_all_captured() {
        let baseline = make_report_with_scores("s", &[("c1", 1.0), ("c2", 0.9), ("c3", 0.8)]);
        let current = make_report_with_scores("s", &[("c1", 0.0), ("c2", 0.0), ("c3", 0.8)]);
        let result = compare_reports(&baseline, &current);
        assert_eq!(result.regressions.len(), 2);
        assert_eq!(result.unchanged, 1);
    }

    #[test]
    fn empty_baseline_and_current() {
        let baseline = make_report_with_scores("s", &[]);
        let current = make_report_with_scores("s", &[]);
        let result = compare_reports(&baseline, &current);
        assert!(result.is_clean());
        assert_eq!(result.unchanged, 0);
        assert_eq!(result.new_cases, 0);
        assert_eq!(result.removed_cases, 0);
    }

    #[test]
    fn detector_with_min_delta_ignores_small_changes() {
        let baseline = make_report_with_scores("s", &[("c1", 0.9)]);
        let current = make_report_with_scores("s", &[("c1", 0.899)]);
        // delta = -0.001, which is less than default min_delta=1e-9 but small
        let detector = RegressionDetector::with_min_delta(0.01);
        let result = detector.compare_reports(&baseline, &current);
        // 0.899 - 0.9 = -0.001, which is < -0.01 threshold? No, -0.001 > -0.01.
        // So the change should be classified as unchanged.
        assert!(result.is_clean());
        assert_eq!(result.unchanged, 1);
    }

    #[test]
    fn detector_with_min_delta_catches_large_regression() {
        let baseline = make_report_with_scores("s", &[("c1", 1.0)]);
        let current = make_report_with_scores("s", &[("c1", 0.5)]);
        let detector = RegressionDetector::with_min_delta(0.1);
        let result = detector.compare_reports(&baseline, &current);
        assert!(!result.is_clean());
        assert_eq!(result.regressions.len(), 1);
    }

    #[test]
    fn regression_delta_is_negative() {
        let baseline = make_report_with_scores("s", &[("c1", 0.8)]);
        let current = make_report_with_scores("s", &[("c1", 0.3)]);
        let result = compare_reports(&baseline, &current);
        assert_eq!(result.regressions.len(), 1);
        assert!(result.regressions[0].delta < 0.0);
    }

    #[test]
    fn improvement_delta_is_positive() {
        let baseline = make_report_with_scores("s", &[("c1", 0.3)]);
        let current = make_report_with_scores("s", &[("c1", 0.8)]);
        let result = compare_reports(&baseline, &current);
        assert_eq!(result.improvements.len(), 1);
        assert!(result.improvements[0].delta > 0.0);
    }
}
