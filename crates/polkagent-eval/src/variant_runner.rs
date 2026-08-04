//! **EXPERIMENTAL** — Multi-variant evaluation runner (PRD-09 §6.5).
//!
//! Runs the same [`EvalSuite`] against multiple "skill variants" so that
//! their scores can be compared side-by-side. This module only *reads* from
//! the evaluation framework — it never modifies Cedar grants or safety gates.
//!
//! # Status
//!
//! **Research prototype — not production-ready.**

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::report::EvalReport;
use crate::scorer::mean_score;

// ---------------------------------------------------------------------------
// VariantId
// ---------------------------------------------------------------------------

/// Opaque identifier for a skill variant within an evaluation run.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VariantId(pub String);

impl VariantId {
    /// Create a new variant identifier.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for VariantId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ---------------------------------------------------------------------------
// VariantReport
// ---------------------------------------------------------------------------

/// The evaluation result for a single variant run against a task corpus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantReport {
    /// Which variant produced this report.
    pub variant_id: VariantId,
    /// The underlying eval report.
    pub eval_report: EvalReport,
}

// ---------------------------------------------------------------------------
// VariantComparison
// ---------------------------------------------------------------------------

/// Side-by-side comparison of multiple variants evaluated on the same corpus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantComparison {
    /// Per-variant results, keyed by variant id.
    pub reports: HashMap<String, VariantReport>,
    /// Variant id with the highest mean score.
    pub best_variant: VariantId,
    /// Sorted ranking: (variant_id, mean_score) from best to worst.
    pub ranking: Vec<(VariantId, f64)>,
}

impl VariantComparison {
    /// Build a comparison from a set of variant reports.
    ///
    /// # Panics
    ///
    /// Panics if `reports` is empty.
    #[must_use]
    pub fn from_reports(reports: Vec<VariantReport>) -> Self {
        assert!(!reports.is_empty(), "at least one variant report required");

        let mut ranking: Vec<(VariantId, f64)> = reports
            .iter()
            .map(|vr| {
                let scores: Vec<f64> = vr.eval_report.results.iter().map(|r| r.score.score).collect();
                (vr.variant_id.clone(), mean_score(&scores))
            })
            .collect();

        // Sort descending by score, then by name for determinism.
        ranking.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0 .0.cmp(&b.0 .0))
        });

        let best_variant = ranking[0].0.clone();

        let map: HashMap<String, VariantReport> = reports
            .into_iter()
            .map(|vr| (vr.variant_id.0.clone(), vr))
            .collect();

        Self {
            reports: map,
            best_variant,
            ranking,
        }
    }

    /// Return the mean score for a given variant, or `None` if not found.
    #[must_use]
    pub fn score_for(&self, variant_id: &str) -> Option<f64> {
        self.ranking
            .iter()
            .find(|(vid, _)| vid.0 == variant_id)
            .map(|(_, score)| *score)
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

    fn make_case_result(id: &str, score_val: f64) -> CaseResult {
        CaseResult {
            case_id: id.into(),
            case_name: format!("Case {id}"),
            score: Score {
                passed: score_val >= 1.0 - f64::EPSILON,
                score: score_val,
                details: "test".into(),
                checks: Vec::new(),
            },
            model_output: String::new(),
            tool_calls_made: Vec::new(),
            duration_ms: 10,
            error: None,
            category: EvalCategory::General,
        }
    }

    fn make_variant_report(variant: &str, scores: &[f64]) -> VariantReport {
        let results: Vec<CaseResult> = scores
            .iter()
            .enumerate()
            .map(|(i, &s)| make_case_result(&format!("c{i}"), s))
            .collect();
        VariantReport {
            variant_id: VariantId::new(variant),
            eval_report: EvalReport::from_results("test-suite", results),
        }
    }

    #[test]
    fn comparison_ranks_correctly() {
        let reports = vec![
            make_variant_report("low", &[0.0, 0.0, 0.5]),
            make_variant_report("high", &[1.0, 1.0, 1.0]),
            make_variant_report("mid", &[0.5, 0.5, 0.5]),
        ];
        let cmp = VariantComparison::from_reports(reports);

        assert_eq!(cmp.best_variant.0, "high");
        assert_eq!(cmp.ranking.len(), 3);
        assert_eq!(cmp.ranking[0].0 .0, "high");
        assert_eq!(cmp.ranking[1].0 .0, "mid");
        assert_eq!(cmp.ranking[2].0 .0, "low");
    }

    #[test]
    fn score_for_returns_correct_value() {
        let reports = vec![
            make_variant_report("a", &[1.0, 0.0]),
            make_variant_report("b", &[0.5, 0.5]),
        ];
        let cmp = VariantComparison::from_reports(reports);

        let a_score = cmp.score_for("a").expect("variant a exists");
        assert!((a_score - 0.5).abs() < f64::EPSILON);

        let b_score = cmp.score_for("b").expect("variant b exists");
        assert!((b_score - 0.5).abs() < f64::EPSILON);

        assert!(cmp.score_for("nonexistent").is_none());
    }

    #[test]
    fn three_variants_score_comparison() {
        let reports = vec![
            make_variant_report("variant-alpha", &[0.8, 0.9, 1.0]),
            make_variant_report("variant-beta", &[0.3, 0.4, 0.5]),
            make_variant_report("variant-gamma", &[0.6, 0.7, 0.6]),
        ];
        let cmp = VariantComparison::from_reports(reports);

        assert_eq!(cmp.best_variant.0, "variant-alpha");

        let alpha = cmp.score_for("variant-alpha").expect("exists");
        let beta = cmp.score_for("variant-beta").expect("exists");
        let gamma = cmp.score_for("variant-gamma").expect("exists");

        assert!(alpha > gamma, "alpha ({alpha}) should beat gamma ({gamma})");
        assert!(gamma > beta, "gamma ({gamma}) should beat beta ({beta})");
    }

    #[test]
    fn variant_id_display() {
        let id = VariantId::new("my-variant");
        assert_eq!(id.to_string(), "my-variant");
    }

    #[test]
    fn deterministic_tiebreak() {
        let reports = vec![
            make_variant_report("z-variant", &[0.5]),
            make_variant_report("a-variant", &[0.5]),
        ];
        let cmp = VariantComparison::from_reports(reports);
        // Equal scores => alphabetical tiebreak
        assert_eq!(cmp.ranking[0].0 .0, "a-variant");
        assert_eq!(cmp.ranking[1].0 .0, "z-variant");
    }
}
