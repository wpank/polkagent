use chrono::{DateTime, Utc};
use polkagent_payment::{CostEstimator, PricingEntry};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunCost {
    pub run_id: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub estimated: bool,
    pub computed_at: DateTime<Utc>,
}

pub struct RunCostCalculator {
    estimator: CostEstimator,
}

impl RunCostCalculator {
    #[must_use]
    pub fn new() -> Self {
        Self {
            estimator: CostEstimator::new(),
        }
    }

    #[must_use]
    pub fn with_pricing(entries: Vec<PricingEntry>) -> Self {
        Self {
            estimator: CostEstimator::with_entries(entries),
        }
    }

    #[must_use]
    pub fn compute(
        &self,
        run_id: &str,
        provider: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> RunCost {
        let cost_usd = self
            .estimator
            .estimate(provider, model, input_tokens, output_tokens);

        RunCost {
            run_id: run_id.to_owned(),
            provider: provider.to_owned(),
            model: model.to_owned(),
            input_tokens,
            output_tokens,
            cost_usd,
            estimated: true,
            computed_at: Utc::now(),
        }
    }

    pub fn compute_aggregate(&self, costs: &[polkagent_payment::CostRecord]) -> AggregateCost {
        let mut total_input_tokens: u64 = 0;
        let mut total_output_tokens: u64 = 0;
        let mut total_cost_usd: f64 = 0.0;
        let mut run_count: u64 = 0;

        let mut seen_runs = std::collections::HashSet::new();

        for record in costs {
            total_input_tokens = total_input_tokens.saturating_add(record.input_tokens);
            total_output_tokens = total_output_tokens.saturating_add(record.output_tokens);
            total_cost_usd += record.estimated_usd;
            if seen_runs.insert(record.run_id.clone()) {
                run_count += 1;
            }
        }

        AggregateCost {
            run_count,
            total_input_tokens,
            total_output_tokens,
            total_cost_usd,
        }
    }
}

impl Default for RunCostCalculator {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggregateCost {
    pub run_count: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usd: f64,
}

impl Default for AggregateCost {
    fn default() -> Self {
        Self {
            run_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost_usd: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_run_cost_anthropic() {
        let calc = RunCostCalculator::new();
        let cost = calc.compute("run-1", "anthropic", "claude-sonnet-4", 1000, 500);

        assert_eq!(cost.run_id, "run-1");
        assert_eq!(cost.provider, "anthropic");
        assert_eq!(cost.model, "claude-sonnet-4");
        assert_eq!(cost.input_tokens, 1000);
        assert_eq!(cost.output_tokens, 500);
        // $3/M input + $15/M output = $0.003 + $0.0075 = $0.0105
        assert!((cost.cost_usd - 0.0105).abs() < 0.0001);
        assert!(cost.estimated);
    }

    #[test]
    fn compute_run_cost_zero_tokens() {
        let calc = RunCostCalculator::new();
        let cost = calc.compute("run-2", "anthropic", "claude-sonnet-4", 0, 0);

        assert_eq!(cost.cost_usd, 0.0);
    }

    #[test]
    fn compute_run_cost_unknown_model() {
        let calc = RunCostCalculator::new();
        let cost = calc.compute("run-3", "unknown", "mystery-model", 1000, 500);

        assert_eq!(cost.cost_usd, 0.0);
    }

    #[test]
    fn aggregate_costs() {
        let calc = RunCostCalculator::new();
        let records = vec![
            polkagent_payment::CostRecord {
                run_id: "run-1".into(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4".into(),
                input_tokens: 1000,
                output_tokens: 500,
                estimated_usd: 0.01,
                recorded_at: Utc::now(),
            },
            polkagent_payment::CostRecord {
                run_id: "run-2".into(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4".into(),
                input_tokens: 2000,
                output_tokens: 1000,
                estimated_usd: 0.02,
                recorded_at: Utc::now(),
            },
            polkagent_payment::CostRecord {
                run_id: "run-1".into(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4".into(),
                input_tokens: 500,
                output_tokens: 200,
                estimated_usd: 0.005,
                recorded_at: Utc::now(),
            },
        ];

        let agg = calc.compute_aggregate(&records);
        assert_eq!(agg.run_count, 2);
        assert_eq!(agg.total_input_tokens, 3500);
        assert_eq!(agg.total_output_tokens, 1700);
        assert!((agg.total_cost_usd - 0.035).abs() < 1e-9);
    }

    #[test]
    fn aggregate_empty() {
        let calc = RunCostCalculator::new();
        let agg = calc.compute_aggregate(&[]);
        assert_eq!(agg.run_count, 0);
        assert_eq!(agg.total_input_tokens, 0);
        assert_eq!(agg.total_cost_usd, 0.0);
    }

    #[test]
    fn custom_pricing() {
        let calc = RunCostCalculator::with_pricing(vec![PricingEntry {
            provider: "custom".into(),
            model_pattern: "my-model".into(),
            input_per_million: 1.0,
            output_per_million: 2.0,
        }]);

        let cost = calc.compute("run-x", "custom", "my-model", 1_000_000, 1_000_000);
        assert!((cost.cost_usd - 3.0).abs() < 0.001);
    }

    #[test]
    fn run_cost_serde_round_trip() {
        let cost = RunCost {
            run_id: "run-1".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4".into(),
            input_tokens: 1000,
            output_tokens: 500,
            cost_usd: 0.0105,
            estimated: true,
            computed_at: Utc::now(),
        };
        let json = serde_json::to_string(&cost).expect("serialize");
        let back: RunCost = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cost.run_id, back.run_id);
        assert_eq!(cost.provider, back.provider);
        assert!((cost.cost_usd - back.cost_usd).abs() < f64::EPSILON);
    }

    #[test]
    fn aggregate_cost_default() {
        let agg = AggregateCost::default();
        assert_eq!(agg.run_count, 0);
        assert_eq!(agg.total_input_tokens, 0);
        assert_eq!(agg.total_output_tokens, 0);
        assert_eq!(agg.total_cost_usd, 0.0);
    }
}
