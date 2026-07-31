//! Cost estimation from LLM token usage.
//!
//! [`CostEstimator`] holds a pricing table for known models and computes
//! estimated USD costs from input/output token counts.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// PricingEntry
// ---------------------------------------------------------------------------

/// Pricing data for a single model at a specific provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PricingEntry {
    /// The LLM provider name (e.g. "anthropic", "openai", "local").
    pub provider: String,
    /// A pattern that matches model identifiers. Currently matched via
    /// simple string equality or prefix matching.
    pub model_pattern: String,
    /// Cost per million input (prompt) tokens, in USD.
    pub input_per_million: f64,
    /// Cost per million output (completion) tokens, in USD.
    pub output_per_million: f64,
}

// ---------------------------------------------------------------------------
// CostEstimator
// ---------------------------------------------------------------------------

/// Estimates USD cost from token usage based on a configurable pricing table.
pub struct CostEstimator {
    entries: Vec<PricingEntry>,
}

impl CostEstimator {
    /// Create a new estimator with the default pricing table for well-known
    /// models.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: default_pricing_table(),
        }
    }

    /// Create an estimator with a custom pricing table.
    #[must_use]
    pub fn with_entries(entries: Vec<PricingEntry>) -> Self {
        Self { entries }
    }

    /// Add a pricing entry to the table.
    pub fn add_entry(&mut self, entry: PricingEntry) {
        self.entries.push(entry);
    }

    /// Estimate the USD cost for a given provider/model and token counts.
    ///
    /// Returns the estimated cost in dollars. Returns `0.0` if no matching
    /// pricing entry is found (with a tracing warning).
    #[must_use]
    pub fn estimate(
        &self,
        provider: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> f64 {
        if let Some(entry) = self.find_entry(provider, model) {
            let input_cost = (input_tokens as f64) * entry.input_per_million / 1_000_000.0;
            let output_cost = (output_tokens as f64) * entry.output_per_million / 1_000_000.0;

            input_cost + output_cost
        } else {
            tracing::warn!(
                provider = provider,
                model = model,
                "no pricing data found — returning $0 estimate"
            );
            0.0
        }
    }

    /// Find the best-matching pricing entry for a provider/model pair.
    fn find_entry(&self, provider: &str, model: &str) -> Option<&PricingEntry> {
        // Exact match first.
        let exact = self
            .entries
            .iter()
            .find(|e| e.provider == provider && e.model_pattern == model);
        if exact.is_some() {
            return exact;
        }

        // Prefix match (e.g. "claude-sonnet-4" matches "claude-sonnet-4-20250514").
        self.entries
            .iter()
            .find(|e| e.provider == provider && model.starts_with(&e.model_pattern))
    }
}

impl Default for CostEstimator {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Default pricing table
// ---------------------------------------------------------------------------

fn default_pricing_table() -> Vec<PricingEntry> {
    vec![
        // Anthropic
        PricingEntry {
            provider: "anthropic".into(),
            model_pattern: "claude-sonnet-4".into(),
            input_per_million: 3.0,
            output_per_million: 15.0,
        },
        PricingEntry {
            provider: "anthropic".into(),
            model_pattern: "claude-haiku-4-5".into(),
            input_per_million: 0.80,
            output_per_million: 4.0,
        },
        // OpenAI
        PricingEntry {
            provider: "openai".into(),
            model_pattern: "gpt-4o-mini".into(),
            input_per_million: 0.15,
            output_per_million: 0.60,
        },
        PricingEntry {
            provider: "openai".into(),
            model_pattern: "gpt-4o".into(),
            input_per_million: 2.50,
            output_per_million: 10.0,
        },
        // Local / self-hosted (free)
        PricingEntry {
            provider: "local".into(),
            model_pattern: "local".into(),
            input_per_million: 0.0,
            output_per_million: 0.0,
        },
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_claude_sonnet() {
        let estimator = CostEstimator::new();
        // 1M input + 1M output
        let cost = estimator.estimate("anthropic", "claude-sonnet-4", 1_000_000, 1_000_000);
        // $3 input + $15 output = $18
        assert!((cost - 18.0).abs() < 0.001);
    }

    #[test]
    fn estimate_claude_haiku() {
        let estimator = CostEstimator::new();
        let cost = estimator.estimate("anthropic", "claude-haiku-4-5", 1_000_000, 1_000_000);
        // $0.80 input + $4.00 output = $4.80
        assert!((cost - 4.80).abs() < 0.001);
    }

    #[test]
    fn estimate_gpt4o() {
        let estimator = CostEstimator::new();
        let cost = estimator.estimate("openai", "gpt-4o", 1_000_000, 1_000_000);
        // $2.50 input + $10 output = $12.50
        assert!((cost - 12.50).abs() < 0.001);
    }

    #[test]
    fn estimate_gpt4o_mini() {
        let estimator = CostEstimator::new();
        let cost = estimator.estimate("openai", "gpt-4o-mini", 1_000_000, 1_000_000);
        // $0.15 input + $0.60 output = $0.75
        assert!((cost - 0.75).abs() < 0.001);
    }

    #[test]
    fn estimate_local_is_free() {
        let estimator = CostEstimator::new();
        let cost = estimator.estimate("local", "local", 5_000_000, 2_000_000);
        assert!((cost - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn estimate_unknown_model_returns_zero() {
        let estimator = CostEstimator::new();
        let cost = estimator.estimate("unknown-provider", "unknown-model", 1000, 500);
        assert!((cost - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn estimate_zero_tokens() {
        let estimator = CostEstimator::new();
        let cost = estimator.estimate("anthropic", "claude-sonnet-4", 0, 0);
        assert!((cost - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn estimate_typical_call() {
        let estimator = CostEstimator::new();
        // Typical call: 1000 input, 500 output with claude-sonnet-4
        let cost = estimator.estimate("anthropic", "claude-sonnet-4", 1000, 500);
        // $3 * 1000/1M + $15 * 500/1M = $0.003 + $0.0075 = $0.0105
        assert!((cost - 0.0105).abs() < 0.0001);
    }

    #[test]
    fn prefix_matching() {
        let estimator = CostEstimator::new();
        // A versioned model name should match via prefix.
        let cost = estimator.estimate(
            "anthropic",
            "claude-sonnet-4-20250514",
            1_000_000,
            1_000_000,
        );
        assert!((cost - 18.0).abs() < 0.001);
    }

    #[test]
    fn gpt4o_mini_matched_before_gpt4o() {
        let estimator = CostEstimator::new();
        // "gpt-4o-mini" should match the mini entry, not the gpt-4o entry.
        let cost = estimator.estimate("openai", "gpt-4o-mini", 1_000_000, 1_000_000);
        // Should be $0.75, not $12.50.
        assert!((cost - 0.75).abs() < 0.001);
    }

    #[test]
    fn custom_pricing_table() {
        let estimator = CostEstimator::with_entries(vec![PricingEntry {
            provider: "custom".into(),
            model_pattern: "my-model".into(),
            input_per_million: 1.0,
            output_per_million: 2.0,
        }]);

        let cost = estimator.estimate("custom", "my-model", 1_000_000, 1_000_000);
        assert!((cost - 3.0).abs() < 0.001);
    }

    #[test]
    fn add_entry() {
        let mut estimator = CostEstimator::new();
        estimator.add_entry(PricingEntry {
            provider: "custom".into(),
            model_pattern: "special".into(),
            input_per_million: 5.0,
            output_per_million: 10.0,
        });

        let cost = estimator.estimate("custom", "special", 1_000_000, 1_000_000);
        assert!((cost - 15.0).abs() < 0.001);
    }

    #[test]
    fn pricing_entry_serde_round_trip() {
        let entry = PricingEntry {
            provider: "anthropic".into(),
            model_pattern: "claude-sonnet-4".into(),
            input_per_million: 3.0,
            output_per_million: 15.0,
        };
        let json = serde_json::to_string(&entry).expect("serialize");
        let back: PricingEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(entry, back);
    }
}
