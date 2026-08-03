//! Promotion candidates: track corpus versions eligible for promotion.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// PromotionCandidate
// ---------------------------------------------------------------------------

/// Represents a corpus version that has improved enough to be promoted.
///
/// A candidate is created when a new corpus version achieves a higher
/// aggregate eval score than the previously-promoted version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotionCandidate {
    /// The identifier of the corpus this candidate belongs to.
    pub corpus_id: String,
    /// The version of the corpus being proposed for promotion.
    pub corpus_version: String,
    /// Aggregate score of the previously-promoted corpus version (`[0.0, 1.0]`).
    pub old_score: f64,
    /// Aggregate score of this candidate corpus version (`[0.0, 1.0]`).
    pub new_score: f64,
    /// Absolute improvement over the old score (`new_score − old_score`).
    pub improvement: f64,
    /// UTC timestamp when this candidate was created.
    pub created_at: DateTime<Utc>,
}

impl PromotionCandidate {
    /// Create a new `PromotionCandidate`.
    ///
    /// `improvement` is computed automatically as `new_score − old_score`.
    #[must_use]
    pub fn new(
        corpus_id: impl Into<String>,
        corpus_version: impl Into<String>,
        old_score: f64,
        new_score: f64,
    ) -> Self {
        Self {
            corpus_id: corpus_id.into(),
            corpus_version: corpus_version.into(),
            old_score,
            new_score,
            improvement: new_score - old_score,
            created_at: Utc::now(),
        }
    }

    /// Return `true` if the candidate actually improves over the old score.
    #[must_use]
    pub fn is_improvement(&self) -> bool {
        self.improvement > 0.0
    }

    /// Return `true` if the improvement meets or exceeds the given threshold.
    #[must_use]
    pub fn meets_threshold(&self, threshold: f64) -> bool {
        self.improvement >= threshold
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promotion_candidate_computes_improvement() {
        let c = PromotionCandidate::new("corpus-a", "2.0.0", 0.7, 0.85);
        assert!((c.improvement - 0.15).abs() < 1e-10);
    }

    #[test]
    fn promotion_candidate_is_improvement_true() {
        let c = PromotionCandidate::new("c", "v2", 0.5, 0.6);
        assert!(c.is_improvement());
    }

    #[test]
    fn promotion_candidate_is_improvement_false_when_equal() {
        let c = PromotionCandidate::new("c", "v2", 0.5, 0.5);
        assert!(!c.is_improvement());
    }

    #[test]
    fn promotion_candidate_is_improvement_false_when_regression() {
        let c = PromotionCandidate::new("c", "v2", 0.8, 0.6);
        assert!(!c.is_improvement());
    }

    #[test]
    fn promotion_candidate_meets_threshold_true() {
        // Use values that have exact binary representations to avoid fp issues.
        let c = PromotionCandidate::new("c", "v2", 0.25, 0.75); // improvement = 0.5
        assert!(c.meets_threshold(0.25));
        assert!(c.meets_threshold(0.5));
    }

    #[test]
    fn promotion_candidate_meets_threshold_false() {
        let c = PromotionCandidate::new("c", "v2", 0.5, 0.5625); // improvement = 0.0625
        assert!(!c.meets_threshold(0.125));
    }

    #[test]
    fn promotion_candidate_meets_threshold_exact() {
        // improvement = 0.25 exactly, threshold = 0.25 → should pass (>=)
        let c = PromotionCandidate::new("c", "v2", 0.5, 0.75);
        assert!(c.meets_threshold(0.25));
    }

    #[test]
    fn promotion_candidate_fields_set_correctly() {
        let c = PromotionCandidate::new("my-corpus", "3.1.0", 0.4, 0.9);
        assert_eq!(c.corpus_id, "my-corpus");
        assert_eq!(c.corpus_version, "3.1.0");
        assert!((c.old_score - 0.4).abs() < f64::EPSILON);
        assert!((c.new_score - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn promotion_candidate_created_at_is_recent() {
        let before = Utc::now();
        let c = PromotionCandidate::new("c", "v", 0.0, 1.0);
        let after = Utc::now();
        assert!(c.created_at >= before);
        assert!(c.created_at <= after);
    }
}
