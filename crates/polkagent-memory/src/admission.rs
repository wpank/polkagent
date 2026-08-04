//! Memory admission control.
//!
//! Before a new [`MemoryEntry`] is written to the store an admission policy
//! is consulted. Policies may:
//!
//! - **Accept** the entry as-is.
//! - **Reject** the entry with a human-readable reason.
//! - **Accept with merge** — indicate that the entry should be merged with an
//!   existing entry (identified by [`MemoryId`]) rather than stored as a
//!   separate record.
//!
//! Multiple policies can be composed with [`CompositeAdmission`], which
//! requires all constituent policies to accept an entry.
//!
//! # Default thresholds
//!
//! | Checker | Field | Default |
//! |---------|-------|---------|
//! | [`NoveltyChecker`] | minimum content-word overlap | 0.7 (reject if similarity ≥ 0.7) |
//! | [`RelevanceChecker`] | minimum `relevance_score` | 0.3 |
//! | [`ConfidenceChecker`] | minimum `confidence` | 0.5 |

use crate::types::{MemoryEntry, MemoryId};

// ---------------------------------------------------------------------------
// AdmissionDecision
// ---------------------------------------------------------------------------

/// The outcome returned by an [`AdmissionPolicy`] evaluation.
#[derive(Debug, Clone, PartialEq)]
pub enum AdmissionDecision {
    /// The entry is accepted and should be stored as a new record.
    Accept,
    /// The entry is rejected. The string contains the reason.
    Reject(String),
    /// The entry is accepted but should be merged with an existing entry
    /// rather than stored as a separate record.
    AcceptWithMerge(MemoryId),
}

// ---------------------------------------------------------------------------
// AdmissionPolicy trait
// ---------------------------------------------------------------------------

/// A policy that decides whether a new memory entry should be admitted to the
/// store.
pub trait AdmissionPolicy: Send + Sync {
    /// Evaluate the incoming `entry` against the `existing` entries in the
    /// store (typically scoped to the same agent).
    fn evaluate(&self, entry: &MemoryEntry, existing: &[MemoryEntry]) -> AdmissionDecision;
}

// ---------------------------------------------------------------------------
// NoveltyChecker
// ---------------------------------------------------------------------------

/// Rejects entries whose content is too similar to an existing entry.
///
/// Similarity is computed as the Jaccard coefficient on the word sets of the
/// two content strings (case-insensitive, whitespace-tokenised). If the
/// coefficient is ≥ `threshold` the new entry is considered a duplicate and
/// rejected.
///
/// Default threshold: **0.7** — meaning an entry whose word set overlaps by
/// ≥ 70 % with any existing entry will be rejected.
pub struct NoveltyChecker {
    /// Minimum similarity (inclusive) at which an entry is rejected.
    pub threshold: f64,
}

impl Default for NoveltyChecker {
    fn default() -> Self {
        Self { threshold: 0.7 }
    }
}

impl NoveltyChecker {
    /// Create a new checker with a custom similarity threshold.
    #[must_use]
    pub fn with_threshold(threshold: f64) -> Self {
        Self { threshold }
    }

    fn jaccard(a: &str, b: &str) -> f64 {
        use std::collections::HashSet;
        let words_a: HashSet<&str> = a.split_whitespace().collect();
        let words_b: HashSet<&str> = b.split_whitespace().collect();
        if words_a.is_empty() && words_b.is_empty() {
            return 1.0;
        }
        let intersection = words_a.intersection(&words_b).count();
        let union = words_a.union(&words_b).count();
        #[allow(clippy::cast_precision_loss)]
        if union == 0 {
            0.0
        } else {
            intersection as f64 / union as f64
        }
    }
}

impl AdmissionPolicy for NoveltyChecker {
    fn evaluate(&self, entry: &MemoryEntry, existing: &[MemoryEntry]) -> AdmissionDecision {
        let content_lower = entry.content.to_lowercase();

        for existing_entry in existing {
            // Only compare entries of the same memory type and agent.
            if existing_entry.agent_id != entry.agent_id
                || existing_entry.memory_type != entry.memory_type
            {
                continue;
            }

            let existing_lower = existing_entry.content.to_lowercase();
            let similarity = Self::jaccard(&content_lower, &existing_lower);

            if similarity >= self.threshold {
                return AdmissionDecision::Reject(format!(
                    "entry is too similar to existing memory {} (similarity={:.3}, threshold={:.3})",
                    existing_entry.id, similarity, self.threshold
                ));
            }
        }

        AdmissionDecision::Accept
    }
}

// ---------------------------------------------------------------------------
// RelevanceChecker
// ---------------------------------------------------------------------------

/// Rejects entries whose `relevance_score` is below the minimum threshold.
///
/// Default threshold: **0.3**.
pub struct RelevanceChecker {
    /// Minimum relevance score (inclusive) required for acceptance.
    pub min_relevance: f64,
}

impl Default for RelevanceChecker {
    fn default() -> Self {
        Self { min_relevance: 0.3 }
    }
}

impl RelevanceChecker {
    /// Create a checker with a custom minimum relevance score.
    #[must_use]
    pub fn with_min(min_relevance: f64) -> Self {
        Self { min_relevance }
    }
}

impl AdmissionPolicy for RelevanceChecker {
    fn evaluate(&self, entry: &MemoryEntry, _existing: &[MemoryEntry]) -> AdmissionDecision {
        if entry.relevance_score < self.min_relevance {
            AdmissionDecision::Reject(format!(
                "relevance score {:.3} is below minimum {:.3}",
                entry.relevance_score, self.min_relevance
            ))
        } else {
            AdmissionDecision::Accept
        }
    }
}

// ---------------------------------------------------------------------------
// ConfidenceChecker
// ---------------------------------------------------------------------------

/// Rejects entries whose `confidence` is below the minimum threshold.
///
/// Default threshold: **0.5**.
pub struct ConfidenceChecker {
    /// Minimum confidence (inclusive) required for acceptance.
    pub min_confidence: f64,
}

impl Default for ConfidenceChecker {
    fn default() -> Self {
        Self {
            min_confidence: 0.5,
        }
    }
}

impl ConfidenceChecker {
    /// Create a checker with a custom minimum confidence.
    #[must_use]
    pub fn with_min(min_confidence: f64) -> Self {
        Self { min_confidence }
    }
}

impl AdmissionPolicy for ConfidenceChecker {
    fn evaluate(&self, entry: &MemoryEntry, _existing: &[MemoryEntry]) -> AdmissionDecision {
        if entry.confidence < self.min_confidence {
            AdmissionDecision::Reject(format!(
                "confidence {:.3} is below minimum {:.3}",
                entry.confidence, self.min_confidence
            ))
        } else {
            AdmissionDecision::Accept
        }
    }
}

// ---------------------------------------------------------------------------
// CompositeAdmission
// ---------------------------------------------------------------------------

/// Chains multiple [`AdmissionPolicy`] implementations.
///
/// All policies must return [`AdmissionDecision::Accept`] for the composite to
/// accept an entry. The first non-accept decision wins and is returned
/// immediately (short-circuit evaluation).
pub struct CompositeAdmission {
    policies: Vec<Box<dyn AdmissionPolicy>>,
}

impl Default for CompositeAdmission {
    fn default() -> Self {
        Self::new()
    }
}

impl CompositeAdmission {
    /// Create an empty composite (accepts everything).
    #[must_use]
    pub fn new() -> Self {
        Self {
            policies: Vec::new(),
        }
    }

    /// Add a policy to the chain.
    pub fn add<P: AdmissionPolicy + 'static>(&mut self, policy: P) {
        self.policies.push(Box::new(policy));
    }

    /// Builder-style alternative to [`add`](Self::add).
    #[must_use]
    pub fn with<P: AdmissionPolicy + 'static>(mut self, policy: P) -> Self {
        self.add(policy);
        self
    }
}

impl AdmissionPolicy for CompositeAdmission {
    fn evaluate(&self, entry: &MemoryEntry, existing: &[MemoryEntry]) -> AdmissionDecision {
        for policy in &self.policies {
            let decision = policy.evaluate(entry, existing);
            if decision != AdmissionDecision::Accept {
                return decision;
            }
        }
        AdmissionDecision::Accept
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classification::Classification;
    use crate::types::{MemoryId, MemoryType};
    use chrono::Utc;
    use polkagent_core::ids::AgentId;

    fn base_entry(agent_id: AgentId, content: &str) -> MemoryEntry {
        let now = Utc::now();
        MemoryEntry {
            id: MemoryId::new(),
            agent_id,
            episode_id: None,
            memory_type: MemoryType::Semantic,
            content: content.to_string(),
            embedding: None,
            metadata: serde_json::json!({}),
            provenance: None,
            created_at: now,
            accessed_at: now,
            access_count: 0,
            relevance_score: 1.0,
            confidence: 1.0,
            classification: Classification::default(),
        }
    }

    // -- NoveltyChecker --

    #[test]
    fn novelty_novel_entry_accepted() {
        let agent = AgentId::new();
        let checker = NoveltyChecker::default();
        let new_entry = base_entry(
            agent,
            "Rust is a systems language with zero-cost abstractions",
        );
        let existing = vec![base_entry(agent, "Python is a dynamic scripting language")];

        assert_eq!(
            checker.evaluate(&new_entry, &existing),
            AdmissionDecision::Accept
        );
    }

    #[test]
    fn novelty_duplicate_entry_rejected() {
        let agent = AgentId::new();
        let checker = NoveltyChecker::default();
        let content = "Rust is fast and memory safe";
        let new_entry = base_entry(agent, content);
        let existing = vec![base_entry(agent, content)];

        match checker.evaluate(&new_entry, &existing) {
            AdmissionDecision::Reject(_) => (),
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn novelty_near_duplicate_rejected() {
        let agent = AgentId::new();
        // threshold 0.7 — words "Rust is fast" vs "Rust is fast and safe" share 3/5 = 0.6
        // Let's use nearly identical sentences.
        let checker = NoveltyChecker::default();
        let new_entry = base_entry(agent, "Rust is fast memory safe language");
        let existing = vec![base_entry(
            agent,
            "Rust is fast memory safe language systems",
        )];
        // Jaccard = 5/6 ≈ 0.83 → rejected
        match checker.evaluate(&new_entry, &existing) {
            AdmissionDecision::Reject(_) => (),
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn novelty_different_memory_types_not_compared() {
        let agent = AgentId::new();
        let checker = NoveltyChecker::with_threshold(0.1); // very low threshold
        let new_entry = base_entry(agent, "identical content");
        let mut existing = base_entry(agent, "identical content");
        existing.memory_type = MemoryType::Procedural; // different type

        // Should accept because we only compare same memory_type.
        assert_eq!(
            checker.evaluate(&new_entry, &[existing]),
            AdmissionDecision::Accept
        );
    }

    // -- RelevanceChecker --

    #[test]
    fn relevance_high_relevance_accepted() {
        let agent = AgentId::new();
        let checker = RelevanceChecker::default();
        let entry = base_entry(agent, "high relevance content");
        assert_eq!(checker.evaluate(&entry, &[]), AdmissionDecision::Accept);
    }

    #[test]
    fn relevance_low_relevance_rejected() {
        let agent = AgentId::new();
        let checker = RelevanceChecker::default();
        let mut entry = base_entry(agent, "low relevance content");
        entry.relevance_score = 0.1; // below 0.3 threshold

        match checker.evaluate(&entry, &[]) {
            AdmissionDecision::Reject(_) => (),
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn relevance_exactly_at_threshold_accepted() {
        let agent = AgentId::new();
        let checker = RelevanceChecker::default();
        let mut entry = base_entry(agent, "threshold content");
        entry.relevance_score = 0.3;

        assert_eq!(checker.evaluate(&entry, &[]), AdmissionDecision::Accept);
    }

    // -- ConfidenceChecker --

    #[test]
    fn confidence_high_confidence_accepted() {
        let agent = AgentId::new();
        let checker = ConfidenceChecker::default();
        let entry = base_entry(agent, "well known fact");
        assert_eq!(checker.evaluate(&entry, &[]), AdmissionDecision::Accept);
    }

    #[test]
    fn confidence_low_confidence_rejected() {
        let agent = AgentId::new();
        let checker = ConfidenceChecker::default();
        let mut entry = base_entry(agent, "uncertain claim");
        entry.confidence = 0.2; // below 0.5 threshold

        match checker.evaluate(&entry, &[]) {
            AdmissionDecision::Reject(_) => (),
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn confidence_exactly_at_threshold_accepted() {
        let agent = AgentId::new();
        let checker = ConfidenceChecker::default();
        let mut entry = base_entry(agent, "borderline claim");
        entry.confidence = 0.5;

        assert_eq!(checker.evaluate(&entry, &[]), AdmissionDecision::Accept);
    }

    // -- CompositeAdmission --

    #[test]
    fn composite_all_pass_accepted() {
        let agent = AgentId::new();
        let composite = CompositeAdmission::new()
            .with(ConfidenceChecker::default())
            .with(RelevanceChecker::default());

        let entry = base_entry(agent, "well established fact");
        assert_eq!(composite.evaluate(&entry, &[]), AdmissionDecision::Accept);
    }

    #[test]
    fn composite_first_fail_rejected() {
        let agent = AgentId::new();
        let composite = CompositeAdmission::new()
            .with(ConfidenceChecker::default())
            .with(RelevanceChecker::default());

        let mut entry = base_entry(agent, "uncertain claim");
        entry.confidence = 0.1; // fails confidence check

        match composite.evaluate(&entry, &[]) {
            AdmissionDecision::Reject(_) => (),
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn composite_second_fail_rejected() {
        let agent = AgentId::new();
        let composite = CompositeAdmission::new()
            .with(ConfidenceChecker::default())
            .with(RelevanceChecker::default());

        let mut entry = base_entry(agent, "irrelevant content");
        entry.relevance_score = 0.1; // fails relevance check (confidence is fine at 1.0)

        match composite.evaluate(&entry, &[]) {
            AdmissionDecision::Reject(_) => (),
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn composite_empty_accepts_everything() {
        let agent = AgentId::new();
        let composite = CompositeAdmission::new();
        let mut entry = base_entry(agent, "anything");
        entry.confidence = 0.0;
        entry.relevance_score = 0.0;

        assert_eq!(composite.evaluate(&entry, &[]), AdmissionDecision::Accept);
    }

    #[test]
    fn composite_novelty_and_confidence_all_pass() {
        let agent = AgentId::new();
        let composite = CompositeAdmission::new()
            .with(NoveltyChecker::default())
            .with(ConfidenceChecker::default());

        let existing = vec![base_entry(
            agent,
            "python scripting language for automation",
        )];
        let new_entry = base_entry(agent, "Rust systems language zero cost abstractions safe");

        assert_eq!(
            composite.evaluate(&new_entry, &existing),
            AdmissionDecision::Accept
        );
    }

    #[test]
    fn composite_novelty_fails_overrides_confidence_pass() {
        let agent = AgentId::new();
        let composite = CompositeAdmission::new()
            .with(NoveltyChecker::default())
            .with(ConfidenceChecker::default());

        let content = "identical identical identical identical identical content";
        let existing = vec![base_entry(agent, content)];
        let new_entry = base_entry(agent, content);

        match composite.evaluate(&new_entry, &existing) {
            AdmissionDecision::Reject(_) => (),
            other => panic!("expected Reject, got {other:?}"),
        }
    }
}
