//! **EXPERIMENTAL** — Evolutionary skill selection (PRD-09 §6.5).
//!
//! Allows multiple skill variants to compete for task routing. A Thompson
//! sampling strategy preferentially routes tasks to higher-scoring variants
//! while still exploring alternatives.
//!
//! # Safety invariant
//!
//! This module **never** modifies Cedar grants or safety gates. Variant
//! selection is bounded by the agent's existing grant — no variant can
//! request more authority than the configured skill.
//!
//! # Status
//!
//! **Research prototype — not production-ready.** Gated behind the
//! `evolutionary` feature flag.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::manifest::SkillManifest;

// ---------------------------------------------------------------------------
// SkillVariant
// ---------------------------------------------------------------------------

/// A named variant of a skill, backed by a manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillVariant {
    /// Unique label within the variant set (e.g. `"v1-baseline"`).
    pub label: String,
    /// The underlying skill manifest for this variant.
    pub manifest: SkillManifest,
}

impl SkillVariant {
    /// Create a new skill variant.
    #[must_use]
    pub fn new(label: impl Into<String>, manifest: SkillManifest) -> Self {
        Self {
            label: label.into(),
            manifest,
        }
    }
}

// ---------------------------------------------------------------------------
// EvolutionarySelector
// ---------------------------------------------------------------------------

/// Manages a pool of skill variants and selects among them using
/// Thompson sampling.
///
/// The selector maintains a Beta-Bernoulli posterior for each variant.
/// When asked to [`select`](Self::select), it draws from each posterior
/// and returns the variant whose draw is highest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionarySelector {
    variants: HashMap<String, SkillVariant>,
    successes: HashMap<String, u64>,
    failures: HashMap<String, u64>,
    success_threshold: f64,
    pinned: Option<String>,
    #[serde(skip, default = "default_seed")]
    seed: u64,
}

fn default_seed() -> u64 {
    42
}

/// Convert a counter to `f64` via exactly representable 32-bit halves.
///
/// Thompson sampling is inherently floating-point; this avoids an unchecked
/// integer cast while preserving the intended rounded value for large counts.
fn count_to_f64(value: u64) -> f64 {
    let high = u32::try_from(value >> 32).unwrap_or(u32::MAX);
    let low = u32::try_from(value & u64::from(u32::MAX)).unwrap_or(u32::MAX);
    f64::from(high) * 4_294_967_296.0 + f64::from(low)
}

impl EvolutionarySelector {
    /// Create a new selector with the given success threshold.
    ///
    /// A case score ≥ `success_threshold` counts as a success; below
    /// counts as a failure.
    #[must_use]
    pub fn new(success_threshold: f64) -> Self {
        Self {
            variants: HashMap::new(),
            successes: HashMap::new(),
            failures: HashMap::new(),
            success_threshold,
            pinned: None,
            seed: 42,
        }
    }

    /// Register a skill variant.
    pub fn add_variant(&mut self, variant: SkillVariant) {
        let label = variant.label.clone();
        self.variants.insert(label.clone(), variant);
        self.successes.entry(label.clone()).or_insert(0);
        self.failures.entry(label).or_insert(0);
    }

    /// Pin a specific variant, disabling Thompson sampling.
    ///
    /// When pinned, [`select`](Self::select) always returns the pinned
    /// variant regardless of scores.
    pub fn pin(&mut self, label: &str) -> bool {
        if self.variants.contains_key(label) {
            self.pinned = Some(label.to_owned());
            true
        } else {
            false
        }
    }

    /// Remove the pin, re-enabling Thompson sampling.
    pub fn unpin(&mut self) {
        self.pinned = None;
    }

    /// Set the PRNG seed for reproducible selection.
    pub fn set_seed(&mut self, seed: u64) {
        self.seed = seed;
    }

    /// Record an observed score for a variant.
    pub fn record(&mut self, label: &str, score: f64) {
        if !self.variants.contains_key(label) {
            return;
        }
        if score >= self.success_threshold {
            *self.successes.entry(label.to_owned()).or_insert(0) += 1;
        } else {
            *self.failures.entry(label.to_owned()).or_insert(0) += 1;
        }
    }

    /// Select the best variant using Thompson sampling (or the pinned
    /// variant if one is set).
    ///
    /// Returns `None` only if there are no variants registered.
    #[must_use]
    pub fn select(&mut self) -> Option<&SkillVariant> {
        if self.variants.is_empty() {
            return None;
        }

        // If pinned, bypass Thompson sampling.
        if let Some(ref label) = self.pinned {
            return self.variants.get(label);
        }

        // Thompson sampling: draw from each arm's posterior, pick best.
        let mut rng = SimpleRng::new(self.seed);
        self.seed = rng.next_u64();

        let mut best_label: Option<&str> = None;
        let mut best_sample = f64::NEG_INFINITY;

        let mut keys: Vec<&String> = self.variants.keys().collect();
        keys.sort();

        for key in keys {
            let alpha = count_to_f64(*self.successes.get(key.as_str()).unwrap_or(&0)) + 1.0;
            let beta = count_to_f64(*self.failures.get(key.as_str()).unwrap_or(&0)) + 1.0;
            let sample = rng.beta_sample(alpha, beta);
            if sample > best_sample {
                best_sample = sample;
                best_label = Some(key.as_str());
            }
        }

        best_label.and_then(|label| self.variants.get(label))
    }

    /// Return the number of registered variants.
    #[must_use]
    pub fn variant_count(&self) -> usize {
        self.variants.len()
    }

    /// Return the posterior mean score for each variant.
    #[must_use]
    pub fn means(&self) -> HashMap<String, f64> {
        self.variants
            .keys()
            .map(|label| {
                let successes = count_to_f64(*self.successes.get(label).unwrap_or(&0)) + 1.0;
                let failures = count_to_f64(*self.failures.get(label).unwrap_or(&0)) + 1.0;
                (label.clone(), successes / (successes + failures))
            })
            .collect()
    }

    /// Check whether a variant is registered.
    #[must_use]
    pub fn has_variant(&self, label: &str) -> bool {
        self.variants.contains_key(label)
    }

    /// Return a reference to a variant by label.
    #[must_use]
    pub fn get_variant(&self, label: &str) -> Option<&SkillVariant> {
        self.variants.get(label)
    }
}

// ---------------------------------------------------------------------------
// SimpleRng — duplicated from polkagent-eval to avoid cross-feature dep
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn next_f64(&mut self) -> f64 {
        count_to_f64(self.next_u64() >> 11) / 9_007_199_254_740_992.0
    }

    fn beta_sample(&mut self, alpha: f64, beta: f64) -> f64 {
        let alpha_sample = self.gamma_sample(alpha);
        let beta_sample = self.gamma_sample(beta);
        if alpha_sample + beta_sample == 0.0 {
            return 0.5;
        }
        alpha_sample / (alpha_sample + beta_sample)
    }

    fn gamma_sample(&mut self, shape: f64) -> f64 {
        if shape < 1.0 {
            let boosted = self.gamma_sample(shape + 1.0);
            let uniform_sample = self.next_f64().max(1e-30);
            return boosted * uniform_sample.powf(1.0 / shape);
        }

        let adjusted_shape = shape - 1.0 / 3.0;
        let scale = 1.0 / (9.0 * adjusted_shape).sqrt();

        loop {
            let normal_sample = self.standard_normal();
            let cube_base = 1.0 + scale * normal_sample;
            if cube_base <= 0.0 {
                continue;
            }
            let cube = cube_base * cube_base * cube_base;
            let uniform_sample = self.next_f64().max(1e-30);

            if uniform_sample
                < 1.0 - 0.0331 * (normal_sample * normal_sample) * (normal_sample * normal_sample)
            {
                return adjusted_shape * cube;
            }
            if uniform_sample.ln()
                < 0.5 * normal_sample * normal_sample + adjusted_shape * (1.0 - cube + cube.ln())
            {
                return adjusted_shape * cube;
            }
        }
    }

    fn standard_normal(&mut self) -> f64 {
        let radial_sample = self.next_f64().max(1e-30);
        let angular_sample = self.next_f64();
        (-2.0 * radial_sample.ln()).sqrt() * (2.0 * std::f64::consts::PI * angular_sample).cos()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(
        clippy::expect_used,
        reason = "selector tests unwrap variants only after deterministic fixture registration"
    )]

    use super::*;
    use crate::manifest::{CapabilitiesSection, PromptsSection, SkillManifest, SkillSection};

    fn test_manifest(name: &str, version: &str) -> SkillManifest {
        SkillManifest {
            skill: SkillSection {
                name: name.into(),
                version: version.into(),
                description: String::new(),
                authors: Vec::new(),
                license: String::new(),
            },
            capabilities: CapabilitiesSection::default(),
            prompts: PromptsSection::default(),
            config: HashMap::new(),
            dependencies: HashMap::new(),
        }
    }

    #[test]
    fn add_and_count_variants() {
        let mut sel = EvolutionarySelector::new(0.5);
        assert_eq!(sel.variant_count(), 0);

        sel.add_variant(SkillVariant::new("v1", test_manifest("skill-a", "0.1.0")));
        sel.add_variant(SkillVariant::new("v2", test_manifest("skill-a", "0.2.0")));
        sel.add_variant(SkillVariant::new("v3", test_manifest("skill-a", "0.3.0")));

        assert_eq!(sel.variant_count(), 3);
        assert!(sel.has_variant("v1"));
        assert!(sel.has_variant("v2"));
        assert!(sel.has_variant("v3"));
        assert!(!sel.has_variant("v4"));
    }

    #[test]
    fn select_returns_some_with_variants() {
        let mut sel = EvolutionarySelector::new(0.5);
        sel.add_variant(SkillVariant::new("v1", test_manifest("s", "0.1.0")));
        assert!(sel.select().is_some());
    }

    #[test]
    fn select_returns_none_when_empty() {
        let mut sel = EvolutionarySelector::new(0.5);
        assert!(sel.select().is_none());
    }

    #[test]
    fn pinned_variant_always_selected() {
        let mut sel = EvolutionarySelector::new(0.5);
        sel.add_variant(SkillVariant::new("v1", test_manifest("s", "0.1.0")));
        sel.add_variant(SkillVariant::new("v2", test_manifest("s", "0.2.0")));

        // Give v1 many failures to make it unlikely to be selected naturally
        for _ in 0..100 {
            sel.record("v1", 0.0);
        }
        for _ in 0..100 {
            sel.record("v2", 1.0);
        }

        // Pin v1
        assert!(sel.pin("v1"));

        for _ in 0..20 {
            let selected = sel.select().expect("non-empty");
            assert_eq!(selected.label, "v1", "pinned variant must be returned");
        }

        // Unpin restores Thompson sampling
        sel.unpin();
        let mut saw_v2 = false;
        for _ in 0..50 {
            if sel.select().expect("non-empty").label == "v2" {
                saw_v2 = true;
                break;
            }
        }
        assert!(saw_v2, "after unpin, v2 (the better variant) should appear");
    }

    #[test]
    fn pin_unknown_variant_returns_false() {
        let mut sel = EvolutionarySelector::new(0.5);
        sel.add_variant(SkillVariant::new("v1", test_manifest("s", "0.1.0")));
        assert!(!sel.pin("nonexistent"));
    }

    #[test]
    fn dominant_variant_favored() {
        let mut sel = EvolutionarySelector::new(0.5);
        sel.add_variant(SkillVariant::new("strong", test_manifest("s", "0.1.0")));
        sel.add_variant(SkillVariant::new("weak", test_manifest("s", "0.2.0")));
        sel.add_variant(SkillVariant::new("medium", test_manifest("s", "0.3.0")));
        sel.set_seed(777);

        for _ in 0..50 {
            sel.record("strong", 1.0);
        }
        for _ in 0..50 {
            sel.record("weak", 0.0);
        }
        for _ in 0..25 {
            sel.record("medium", 1.0);
            sel.record("medium", 0.0);
        }

        let mut counts: HashMap<String, usize> = HashMap::new();
        for _ in 0..100 {
            let v = sel.select().expect("non-empty");
            *counts.entry(v.label.clone()).or_insert(0) += 1;
        }

        let strong_count = counts.get("strong").copied().unwrap_or(0);
        let weak_count = counts.get("weak").copied().unwrap_or(0);

        assert!(
            strong_count > weak_count,
            "strong ({strong_count}) should be selected more than weak ({weak_count})"
        );
    }

    #[test]
    fn means_reflect_observations() {
        let mut sel = EvolutionarySelector::new(0.5);
        sel.add_variant(SkillVariant::new("good", test_manifest("s", "0.1.0")));
        sel.add_variant(SkillVariant::new("bad", test_manifest("s", "0.2.0")));

        for _ in 0..20 {
            sel.record("good", 1.0);
        }
        for _ in 0..20 {
            sel.record("bad", 0.0);
        }

        let means = sel.means();
        assert!(means["good"] > 0.8);
        assert!(means["bad"] < 0.2);
    }

    #[test]
    fn three_variant_ranking_integration() {
        let mut sel = EvolutionarySelector::new(0.5);
        sel.add_variant(SkillVariant::new("alpha", test_manifest("s", "1.0.0")));
        sel.add_variant(SkillVariant::new("beta", test_manifest("s", "1.1.0")));
        sel.add_variant(SkillVariant::new("gamma", test_manifest("s", "1.2.0")));

        // alpha: excellent (0.9 mean)
        for _ in 0..18 {
            sel.record("alpha", 0.9);
        }
        for _ in 0..2 {
            sel.record("alpha", 0.1);
        }

        // beta: poor (0.2 mean)
        for _ in 0..4 {
            sel.record("beta", 0.9);
        }
        for _ in 0..16 {
            sel.record("beta", 0.1);
        }

        // gamma: middling (0.5 mean)
        for _ in 0..10 {
            sel.record("gamma", 0.9);
        }
        for _ in 0..10 {
            sel.record("gamma", 0.1);
        }

        let means = sel.means();
        assert!(means["alpha"] > means["gamma"]);
        assert!(means["gamma"] > means["beta"]);
    }

    #[test]
    fn does_not_modify_grants_invariant() {
        // This test documents the safety invariant: EvolutionarySelector
        // has no fields, methods, or types related to Cedar grants or
        // safety gates. Selection only routes tasks — it never escalates
        // privileges.
        let mut sel = EvolutionarySelector::new(0.5);
        sel.add_variant(SkillVariant::new("v1", test_manifest("s", "0.1.0")));

        // Verify the manifest's grants are untouched after selection
        let manifest_before = sel.get_variant("v1").expect("exists").manifest.clone();
        sel.record("v1", 1.0);
        let _ = sel.select();
        let manifest_after = sel.get_variant("v1").expect("exists").manifest.clone();

        assert_eq!(
            manifest_before.capabilities.required_grants,
            manifest_after.capabilities.required_grants,
        );
    }
}
