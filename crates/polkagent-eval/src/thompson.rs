//! **EXPERIMENTAL** — Thompson sampling selector for skill variant routing
//! (PRD-09 §6.5).
//!
//! Implements a Beta-Bernoulli Thompson sampling strategy that routes
//! incoming tasks to the variant most likely to be the best performer.
//! Better-performing variants accumulate more successes and are therefore
//! sampled more frequently over time.
//!
//! This module only *reads* evaluation scores — it never modifies Cedar
//! grants or safety gates.
//!
//! # Status
//!
//! **Research prototype — not production-ready.**

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// SimpleRng — minimal PRNG to avoid a `rand` dependency
// ---------------------------------------------------------------------------

/// Minimal xorshift64-based PRNG.
///
/// Provides just enough randomness for Thompson sampling without pulling
/// in the full `rand` crate.
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

    /// Uniform f64 in `[0, 1)`.
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / ((1u64 << 53) as f64)
    }

    /// Approximate sample from Beta(alpha, beta) using the Jöhnk algorithm
    /// for small parameters and the ratio-of-uniforms otherwise.
    ///
    /// For the prototype we use a simpler approach: the inverse-CDF is
    /// expensive, so we use a Gamma-ratio representation:
    /// Beta(a,b) = Gamma(a,1) / (Gamma(a,1) + Gamma(b,1)).
    fn beta_sample(&mut self, alpha: f64, beta: f64) -> f64 {
        let x = self.gamma_sample(alpha);
        let y = self.gamma_sample(beta);
        if x + y == 0.0 {
            return 0.5;
        }
        x / (x + y)
    }

    /// Sample from Gamma(shape, 1) using Marsaglia & Tsang's method for shape ≥ 1,
    /// and a shape-boosting trick for shape < 1.
    fn gamma_sample(&mut self, shape: f64) -> f64 {
        if shape < 1.0 {
            // Gamma(a) = Gamma(a+1) * U^(1/a)
            let boosted = self.gamma_sample(shape + 1.0);
            let u = self.next_f64().max(1e-30);
            return boosted * u.powf(1.0 / shape);
        }

        // Marsaglia & Tsang for shape >= 1
        let d = shape - 1.0 / 3.0;
        let c = 1.0 / (9.0 * d).sqrt();

        loop {
            let x = self.standard_normal();
            let v_base = 1.0 + c * x;
            if v_base <= 0.0 {
                continue;
            }
            let v = v_base * v_base * v_base;
            let u = self.next_f64().max(1e-30);

            // Accept/reject
            if u < 1.0 - 0.0331 * (x * x) * (x * x) {
                return d * v;
            }
            if u.ln() < 0.5 * x * x + d * (1.0 - v + v.ln()) {
                return d * v;
            }
        }
    }

    /// Approximate standard normal via Box-Muller.
    fn standard_normal(&mut self) -> f64 {
        let u1 = self.next_f64().max(1e-30);
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

// ---------------------------------------------------------------------------
// BetaArm
// ---------------------------------------------------------------------------

/// Per-variant Beta distribution state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BetaArm {
    /// Number of observed successes (score ≥ threshold).
    pub successes: u64,
    /// Number of observed failures (score < threshold).
    pub failures: u64,
}

impl BetaArm {
    fn new() -> Self {
        Self {
            successes: 0,
            failures: 0,
        }
    }

    fn alpha(&self) -> f64 {
        self.successes as f64 + 1.0 // +1 for uniform prior
    }

    fn beta_param(&self) -> f64 {
        self.failures as f64 + 1.0
    }

    fn mean(&self) -> f64 {
        self.alpha() / (self.alpha() + self.beta_param())
    }
}

// ---------------------------------------------------------------------------
// ThompsonSelector
// ---------------------------------------------------------------------------

/// Thompson sampling selector for routing tasks to skill variants.
///
/// Each variant (arm) maintains a Beta distribution parameterised by
/// observed successes and failures. When asked to select a variant,
/// we draw a sample from each arm's posterior and pick the arm with
/// the highest draw.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThompsonSelector {
    arms: HashMap<String, BetaArm>,
    success_threshold: f64,
    #[serde(skip, default = "default_seed")]
    seed: u64,
}

fn default_seed() -> u64 {
    42
}

impl ThompsonSelector {
    /// Create a new selector with the given variant IDs and a success
    /// threshold.
    ///
    /// A case score ≥ `success_threshold` counts as a success; below
    /// counts as a failure.
    #[must_use]
    pub fn new(variants: &[&str], success_threshold: f64) -> Self {
        let arms = variants
            .iter()
            .map(|&v| (v.to_owned(), BetaArm::new()))
            .collect();
        Self {
            arms,
            success_threshold,
            seed: 42,
        }
    }

    /// Set the PRNG seed for reproducible selection.
    pub fn set_seed(&mut self, seed: u64) {
        self.seed = seed;
    }

    /// Record an observed score for the given variant.
    pub fn record(&mut self, variant: &str, score: f64) {
        if let Some(arm) = self.arms.get_mut(variant) {
            if score >= self.success_threshold {
                arm.successes += 1;
            } else {
                arm.failures += 1;
            }
        }
    }

    /// Record a batch of scores from an eval report for a variant.
    pub fn record_scores(&mut self, variant: &str, scores: &[f64]) {
        for &s in scores {
            self.record(variant, s);
        }
    }

    /// Select the variant with the highest Thompson sample.
    ///
    /// Returns `None` only if there are no arms registered.
    #[must_use]
    pub fn select(&mut self) -> Option<String> {
        if self.arms.is_empty() {
            return None;
        }

        let mut rng = SimpleRng::new(self.seed);
        // Advance seed for next call
        self.seed = rng.next_u64();

        let mut best_variant = None;
        let mut best_sample = f64::NEG_INFINITY;

        // Deterministic iteration order
        let mut keys: Vec<&String> = self.arms.keys().collect();
        keys.sort();

        for key in keys {
            let arm = &self.arms[key];
            let sample = rng.beta_sample(arm.alpha(), arm.beta_param());
            if sample > best_sample {
                best_sample = sample;
                best_variant = Some(key.clone());
            }
        }

        best_variant
    }

    /// Return the posterior mean for each variant.
    #[must_use]
    pub fn means(&self) -> HashMap<String, f64> {
        self.arms.iter().map(|(k, arm)| (k.clone(), arm.mean())).collect()
    }

    /// Return a reference to the arm state for a variant.
    #[must_use]
    pub fn arm(&self, variant: &str) -> Option<&BetaArm> {
        self.arms.get(variant)
    }

    /// Return how many variants are registered.
    #[must_use]
    pub fn variant_count(&self) -> usize {
        self.arms.len()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_selector_has_uniform_priors() {
        let sel = ThompsonSelector::new(&["a", "b", "c"], 0.5);
        assert_eq!(sel.variant_count(), 3);
        for v in &["a", "b", "c"] {
            let arm = sel.arm(v).expect("arm exists");
            assert_eq!(arm.successes, 0);
            assert_eq!(arm.failures, 0);
            assert!((arm.mean() - 0.5).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn record_updates_arm() {
        let mut sel = ThompsonSelector::new(&["a"], 0.5);
        sel.record("a", 0.8); // success
        sel.record("a", 0.3); // failure
        sel.record("a", 1.0); // success

        let arm = sel.arm("a").expect("arm exists");
        assert_eq!(arm.successes, 2);
        assert_eq!(arm.failures, 1);
    }

    #[test]
    fn record_ignores_unknown_variant() {
        let mut sel = ThompsonSelector::new(&["a"], 0.5);
        sel.record("unknown", 1.0); // should not panic
        assert_eq!(sel.variant_count(), 1);
    }

    #[test]
    fn select_returns_some_for_nonempty() {
        let mut sel = ThompsonSelector::new(&["a", "b"], 0.5);
        let selected = sel.select();
        assert!(selected.is_some());
    }

    #[test]
    fn select_returns_none_for_empty() {
        let mut sel = ThompsonSelector::new(&[], 0.5);
        assert!(sel.select().is_none());
    }

    #[test]
    fn dominant_variant_selected_more_often() {
        let mut sel = ThompsonSelector::new(&["strong", "weak", "medium"], 0.5);
        sel.set_seed(12345);

        // Give "strong" many successes
        for _ in 0..50 {
            sel.record("strong", 1.0);
        }
        // Give "weak" many failures
        for _ in 0..50 {
            sel.record("weak", 0.0);
        }
        // Give "medium" mixed results
        for _ in 0..25 {
            sel.record("medium", 1.0);
            sel.record("medium", 0.0);
        }

        // Sample 100 times and count
        let mut counts: HashMap<String, usize> = HashMap::new();
        for _ in 0..100 {
            let v = sel.select().expect("non-empty");
            *counts.entry(v).or_insert(0) += 1;
        }

        let strong_count = counts.get("strong").copied().unwrap_or(0);
        let weak_count = counts.get("weak").copied().unwrap_or(0);

        assert!(
            strong_count > weak_count,
            "strong ({strong_count}) should be selected more than weak ({weak_count})"
        );
    }

    #[test]
    fn three_variants_score_comparison_thompson() {
        let mut sel = ThompsonSelector::new(&["alpha", "beta", "gamma"], 0.5);

        // alpha: excellent
        sel.record_scores("alpha", &[0.8, 0.9, 1.0, 0.85, 0.95]);
        // beta: poor
        sel.record_scores("beta", &[0.1, 0.2, 0.3, 0.15, 0.25]);
        // gamma: mediocre
        sel.record_scores("gamma", &[0.4, 0.6, 0.5, 0.55, 0.45]);

        let means = sel.means();
        let alpha_mean = means["alpha"];
        let beta_mean = means["beta"];
        let gamma_mean = means["gamma"];

        assert!(
            alpha_mean > gamma_mean,
            "alpha mean ({alpha_mean}) > gamma mean ({gamma_mean})"
        );
        assert!(
            gamma_mean > beta_mean,
            "gamma mean ({gamma_mean}) > beta mean ({beta_mean})"
        );
    }

    #[test]
    fn record_scores_batch() {
        let mut sel = ThompsonSelector::new(&["v1"], 0.5);
        sel.record_scores("v1", &[1.0, 0.0, 0.8, 0.2]);

        let arm = sel.arm("v1").expect("exists");
        assert_eq!(arm.successes, 2); // 1.0, 0.8
        assert_eq!(arm.failures, 2); // 0.0, 0.2
    }

    #[test]
    fn means_reflect_observations() {
        let mut sel = ThompsonSelector::new(&["a", "b"], 0.5);
        // a: all successes → mean should be high
        for _ in 0..10 {
            sel.record("a", 1.0);
        }
        // b: all failures → mean should be low
        for _ in 0..10 {
            sel.record("b", 0.0);
        }

        let means = sel.means();
        assert!(means["a"] > 0.8, "a mean should be high: {}", means["a"]);
        assert!(means["b"] < 0.2, "b mean should be low: {}", means["b"]);
    }

    #[test]
    fn reproducible_with_same_seed() {
        let mut sel1 = ThompsonSelector::new(&["a", "b", "c"], 0.5);
        sel1.set_seed(999);
        sel1.record_scores("a", &[0.8, 0.9]);
        sel1.record_scores("b", &[0.2, 0.3]);
        sel1.record_scores("c", &[0.5, 0.5]);

        let mut sel2 = sel1.clone();
        sel2.set_seed(999);

        // Reset sel1 seed too
        sel1.set_seed(999);

        let picks1: Vec<String> = (0..20).map(|_| sel1.select().expect("non-empty")).collect();
        let picks2: Vec<String> = (0..20).map(|_| sel2.select().expect("non-empty")).collect();

        assert_eq!(picks1, picks2, "same seed should produce same sequence");
    }

    #[test]
    fn serde_round_trip() {
        let mut sel = ThompsonSelector::new(&["a", "b"], 0.5);
        sel.record("a", 1.0);
        sel.record("b", 0.0);

        let json = serde_json::to_string(&sel).expect("serialize");
        let back: ThompsonSelector = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.variant_count(), 2);
        assert_eq!(back.arm("a").expect("exists").successes, 1);
        assert_eq!(back.arm("b").expect("exists").failures, 1);
    }
}
