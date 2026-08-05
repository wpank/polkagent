//! Budget enforcement for runs and turns.
//!
//! [`BudgetEnforcer`] checks resource limits before and after each executor
//! call. It tracks accumulated usage across the run lifetime and per-turn,
//! rejecting operations that would exceed configured limits.
//!
//! # Usage
//!
//! ```text
//! let budget = Budget { max_cost_usd: Some(1.0), .. };
//! let mut enforcer = BudgetEnforcer::new(budget);
//!
//! // Before each turn:
//! enforcer.pre_check()?;
//! enforcer.begin_turn();
//!
//! // After each inference call:
//! enforcer.record(usage_record)?;
//!
//! // End of turn:
//! enforcer.end_turn()?;
//! ```

use polkagent_core::usage::{Budget, BudgetLimits, Cost, UsageRecord, UsageSummary};

/// Convert a token count for diagnostic display only.
///
/// Budget decisions are made with the original integer values before this
/// conversion, so values above `f64`'s exact integer range cannot affect
/// enforcement.
#[allow(
    clippy::cast_precision_loss,
    reason = "BudgetViolation exposes diagnostic limit/actual values as f64"
)]
fn token_count_for_diagnostic(value: u64) -> f64 {
    value as f64
}

// ---------------------------------------------------------------------------
// BudgetViolation
// ---------------------------------------------------------------------------

/// Describes which budget limit was exceeded and by how much.
#[derive(Debug, Clone, PartialEq)]
pub struct BudgetViolation {
    /// Which resource limit was exceeded.
    pub resource: String,
    /// The configured limit value.
    pub limit: f64,
    /// The current or projected value that exceeded the limit.
    pub actual: f64,
    /// Human-readable description.
    pub message: String,
}

impl std::fmt::Display for BudgetViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for BudgetViolation {}

// ---------------------------------------------------------------------------
// BudgetEnforcer
// ---------------------------------------------------------------------------

/// Enforces budget limits before and after executor calls.
///
/// The enforcer maintains two usage summaries:
/// - **Run-level**: accumulated across all turns in the run.
/// - **Turn-level**: reset at the start of each turn.
///
/// Limits from `Budget.per_run` are checked against the run summary;
/// limits from `Budget.per_turn` are checked against the turn summary.
/// The top-level `Budget.max_cost_usd` and `Budget.max_tokens` apply
/// at the run level.
#[derive(Debug)]
pub struct BudgetEnforcer {
    /// The budget configuration.
    budget: Budget,
    /// Accumulated usage across the entire run.
    run_summary: UsageSummary,
    /// Usage within the current turn (reset on `begin_turn`).
    turn_summary: UsageSummary,
}

impl BudgetEnforcer {
    /// Create a new enforcer with the given budget.
    #[must_use]
    pub fn new(budget: Budget) -> Self {
        Self {
            budget,
            run_summary: UsageSummary::default(),
            turn_summary: UsageSummary::default(),
        }
    }

    /// Create an enforcer with no limits.
    #[must_use]
    pub fn unlimited() -> Self {
        Self::new(Budget::UNLIMITED)
    }

    /// Return a reference to the configured budget.
    #[must_use]
    pub fn budget(&self) -> &Budget {
        &self.budget
    }

    /// Return the run-level usage summary.
    #[must_use]
    pub fn run_summary(&self) -> &UsageSummary {
        &self.run_summary
    }

    /// Return the current turn's usage summary.
    #[must_use]
    pub fn turn_summary(&self) -> &UsageSummary {
        &self.turn_summary
    }

    /// Pre-flight check: can a new turn start without immediately violating
    /// run-level limits?
    ///
    /// Call this *before* beginning a turn. Returns `Ok(())` if the run
    /// still has budget remaining. Returns `Err(BudgetViolation)` if the
    /// accumulated usage already exceeds a run-level limit.
    pub fn pre_check(&self) -> Result<(), BudgetViolation> {
        // Check top-level max_cost_usd.
        if let Some(limit) = self.budget.max_cost_usd {
            if self.run_summary.total_cost_usd > limit {
                return Err(BudgetViolation {
                    resource: "cost_usd".into(),
                    limit,
                    actual: self.run_summary.total_cost_usd,
                    message: format!(
                        "run cost ${:.6} exceeds budget ${limit:.6}",
                        self.run_summary.total_cost_usd
                    ),
                });
            }
        }

        // Check top-level max_tokens.
        if let Some(limit) = self.budget.max_tokens {
            if self.run_summary.total_tokens > limit {
                return Err(BudgetViolation {
                    resource: "total_tokens".into(),
                    limit: token_count_for_diagnostic(limit),
                    actual: token_count_for_diagnostic(self.run_summary.total_tokens),
                    message: format!(
                        "run tokens {} exceed budget {limit}",
                        self.run_summary.total_tokens
                    ),
                });
            }
        }

        // Check per-run limits.
        if let Some(ref limits) = self.budget.per_run {
            check_limits(&self.run_summary, limits, "run")?;
        }

        Ok(())
    }

    /// Signal the start of a new turn. Resets the turn-level summary.
    pub fn begin_turn(&mut self) {
        self.turn_summary = UsageSummary::default();
    }

    /// Record a usage observation and check limits.
    ///
    /// Updates both run-level and turn-level summaries. Returns
    /// `Err(BudgetViolation)` if any limit is now exceeded.
    pub fn record(&mut self, usage: &UsageRecord) -> Result<(), BudgetViolation> {
        self.run_summary.record(usage);
        self.turn_summary.record(usage);

        // Check top-level limits.
        if let Some(limit) = self.budget.max_cost_usd {
            if self.run_summary.total_cost_usd > limit {
                return Err(BudgetViolation {
                    resource: "cost_usd".into(),
                    limit,
                    actual: self.run_summary.total_cost_usd,
                    message: format!(
                        "run cost ${:.6} exceeds budget ${limit:.6}",
                        self.run_summary.total_cost_usd
                    ),
                });
            }
        }
        if let Some(limit) = self.budget.max_tokens {
            if self.run_summary.total_tokens > limit {
                return Err(BudgetViolation {
                    resource: "total_tokens".into(),
                    limit: token_count_for_diagnostic(limit),
                    actual: token_count_for_diagnostic(self.run_summary.total_tokens),
                    message: format!(
                        "run tokens {} exceed budget {limit}",
                        self.run_summary.total_tokens
                    ),
                });
            }
        }

        // Check per-run limits.
        if let Some(ref limits) = self.budget.per_run {
            check_limits(&self.run_summary, limits, "run")?;
        }

        // Check per-turn limits.
        if let Some(ref limits) = self.budget.per_turn {
            check_limits(&self.turn_summary, limits, "turn")?;
        }

        Ok(())
    }

    /// End-of-turn check. Validates per-turn limits one final time.
    ///
    /// Call after the last `record()` in a turn. Returns `Ok(())` if the
    /// turn stayed within its budget.
    pub fn end_turn(&self) -> Result<(), BudgetViolation> {
        if let Some(ref limits) = self.budget.per_turn {
            check_limits(&self.turn_summary, limits, "turn")?;
        }
        Ok(())
    }

    /// Check whether a projected cost would exceed the run budget.
    ///
    /// Returns `Err(BudgetViolation)` if adding `projected_cost` to the
    /// current run total would exceed `max_cost_usd`.
    pub fn check_projected_cost(&self, projected_cost: &Cost) -> Result<(), BudgetViolation> {
        if let Some(limit) = self.budget.max_cost_usd {
            let projected = self.run_summary.total_cost_usd + projected_cost.amount_usd;
            if projected > limit {
                return Err(BudgetViolation {
                    resource: "cost_usd".into(),
                    limit,
                    actual: projected,
                    message: format!(
                        "projected cost ${projected:.6} would exceed budget ${limit:.6}"
                    ),
                });
            }
        }
        if let Some(ref limits) = self.budget.per_run {
            if let Some(cost_limit) = limits.max_cost_usd {
                let projected = self.run_summary.total_cost_usd + projected_cost.amount_usd;
                if projected > cost_limit {
                    return Err(BudgetViolation {
                        resource: "run.cost_usd".into(),
                        limit: cost_limit,
                        actual: projected,
                        message: format!(
                            "projected cost ${projected:.6} would exceed per-run limit ${cost_limit:.6}"
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    /// Check whether projected tokens would exceed the run budget.
    pub fn check_projected_tokens(&self, projected_tokens: u64) -> Result<(), BudgetViolation> {
        if let Some(limit) = self.budget.max_tokens {
            let projected = self
                .run_summary
                .total_tokens
                .saturating_add(projected_tokens);
            if projected > limit {
                return Err(BudgetViolation {
                    resource: "total_tokens".into(),
                    limit: token_count_for_diagnostic(limit),
                    actual: token_count_for_diagnostic(projected),
                    message: format!("projected tokens {projected} would exceed budget {limit}"),
                });
            }
        }
        Ok(())
    }

    /// Return the remaining cost budget, if a limit is set.
    #[must_use]
    pub fn remaining_cost_usd(&self) -> Option<f64> {
        self.budget
            .max_cost_usd
            .map(|limit| (limit - self.run_summary.total_cost_usd).max(0.0))
    }

    /// Return the remaining token budget, if a limit is set.
    #[must_use]
    pub fn remaining_tokens(&self) -> Option<u64> {
        self.budget
            .max_tokens
            .map(|limit| limit.saturating_sub(self.run_summary.total_tokens))
    }
}

/// Check a usage summary against granular limits.
fn check_limits(
    summary: &UsageSummary,
    limits: &BudgetLimits,
    scope: &str,
) -> Result<(), BudgetViolation> {
    if let Some(limit) = limits.max_cost_usd {
        if summary.total_cost_usd > limit {
            return Err(BudgetViolation {
                resource: format!("{scope}.cost_usd"),
                limit,
                actual: summary.total_cost_usd,
                message: format!(
                    "{scope} cost ${:.6} exceeds limit ${limit:.6}",
                    summary.total_cost_usd
                ),
            });
        }
    }
    if let Some(limit) = limits.max_tokens {
        if summary.total_tokens > limit {
            return Err(BudgetViolation {
                resource: format!("{scope}.total_tokens"),
                limit: token_count_for_diagnostic(limit),
                actual: token_count_for_diagnostic(summary.total_tokens),
                message: format!(
                    "{scope} tokens {} exceed limit {limit}",
                    summary.total_tokens
                ),
            });
        }
    }
    if let Some(limit) = limits.max_input_tokens {
        if summary.total_input_tokens > limit {
            return Err(BudgetViolation {
                resource: format!("{scope}.input_tokens"),
                limit: token_count_for_diagnostic(limit),
                actual: token_count_for_diagnostic(summary.total_input_tokens),
                message: format!(
                    "{scope} input tokens {} exceed limit {limit}",
                    summary.total_input_tokens
                ),
            });
        }
    }
    if let Some(limit) = limits.max_output_tokens {
        if summary.total_output_tokens > limit {
            return Err(BudgetViolation {
                resource: format!("{scope}.output_tokens"),
                limit: token_count_for_diagnostic(limit),
                actual: token_count_for_diagnostic(summary.total_output_tokens),
                message: format!(
                    "{scope} output tokens {} exceed limit {limit}",
                    summary.total_output_tokens
                ),
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::ids::{RunId, StepId, UsageRecordId};
    use polkagent_core::turn::TokenUsage;
    use polkagent_core::types::now;
    use polkagent_core::usage::{Budget, BudgetLimits, Cost, UsageRecord, UsageSource};
    use std::time::Duration;

    fn make_record(input: u32, output: u32, cost_usd: f64) -> UsageRecord {
        UsageRecord {
            id: UsageRecordId::new(),
            source: UsageSource::Executor,
            run_id: RunId::new(),
            step_id: StepId::new(),
            tokens: TokenUsage {
                input_tokens: input,
                output_tokens: output,
                total_tokens: input + output,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: Some(cost_usd),
            },
            cost: Cost::estimated(cost_usd),
            duration: Duration::from_millis(100),
            timestamp: now(),
        }
    }

    // ---- Unlimited enforcer -------------------------------------------------

    #[test]
    fn unlimited_pre_check_always_passes() {
        let enforcer = BudgetEnforcer::unlimited();
        assert!(enforcer.pre_check().is_ok());
    }

    #[test]
    fn unlimited_record_always_passes() {
        let mut enforcer = BudgetEnforcer::unlimited();
        let rec = make_record(1_000_000, 500_000, 100.0);
        assert!(enforcer.record(&rec).is_ok());
    }

    #[test]
    fn unlimited_end_turn_always_passes() {
        let enforcer = BudgetEnforcer::unlimited();
        assert!(enforcer.end_turn().is_ok());
    }

    // ---- Cost budget --------------------------------------------------------

    #[test]
    fn cost_budget_allows_within_limit() {
        let budget = Budget {
            max_cost_usd: Some(1.0),
            ..Default::default()
        };
        let mut enforcer = BudgetEnforcer::new(budget);
        enforcer.begin_turn();
        let rec = make_record(100, 50, 0.5);
        assert!(enforcer.record(&rec).is_ok());
    }

    #[test]
    fn cost_budget_rejects_over_limit() {
        let budget = Budget {
            max_cost_usd: Some(1.0),
            ..Default::default()
        };
        let mut enforcer = BudgetEnforcer::new(budget);
        enforcer.begin_turn();
        let rec = make_record(100, 50, 1.5);
        let result = enforcer.record(&rec);
        assert!(result.is_err());
        let violation = result.unwrap_err();
        assert_eq!(violation.resource, "cost_usd");
        assert!((violation.limit - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn cost_budget_accumulates_across_records() {
        let budget = Budget {
            max_cost_usd: Some(1.0),
            ..Default::default()
        };
        let mut enforcer = BudgetEnforcer::new(budget);
        enforcer.begin_turn();
        assert!(enforcer.record(&make_record(100, 50, 0.4)).is_ok());
        assert!(enforcer.record(&make_record(100, 50, 0.4)).is_ok());
        // Third should push over $1.0.
        let result = enforcer.record(&make_record(100, 50, 0.4));
        assert!(result.is_err());
    }

    #[test]
    fn pre_check_fails_when_budget_already_exceeded() {
        let budget = Budget {
            max_cost_usd: Some(0.5),
            ..Default::default()
        };
        let mut enforcer = BudgetEnforcer::new(budget);
        enforcer.begin_turn();
        // Force over budget.
        let _ = enforcer.record(&make_record(100, 50, 0.6));
        // pre_check should now fail.
        assert!(enforcer.pre_check().is_err());
    }

    // ---- Token budget -------------------------------------------------------

    #[test]
    fn token_budget_allows_within_limit() {
        let budget = Budget {
            max_tokens: Some(1000),
            ..Default::default()
        };
        let mut enforcer = BudgetEnforcer::new(budget);
        enforcer.begin_turn();
        let rec = make_record(300, 200, 0.0);
        assert!(enforcer.record(&rec).is_ok());
    }

    #[test]
    fn token_budget_rejects_over_limit() {
        let budget = Budget {
            max_tokens: Some(1000),
            ..Default::default()
        };
        let mut enforcer = BudgetEnforcer::new(budget);
        enforcer.begin_turn();
        let rec = make_record(600, 500, 0.0);
        let result = enforcer.record(&rec);
        assert!(result.is_err());
        let violation = result.unwrap_err();
        assert_eq!(violation.resource, "total_tokens");
    }

    // ---- Per-run limits -----------------------------------------------------

    #[test]
    fn per_run_input_token_limit() {
        let budget = Budget {
            per_run: Some(BudgetLimits {
                max_input_tokens: Some(500),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut enforcer = BudgetEnforcer::new(budget);
        enforcer.begin_turn();
        assert!(enforcer.record(&make_record(400, 50, 0.0)).is_ok());
        let result = enforcer.record(&make_record(200, 50, 0.0));
        assert!(result.is_err());
        let v = result.unwrap_err();
        assert_eq!(v.resource, "run.input_tokens");
    }

    #[test]
    fn per_run_output_token_limit() {
        let budget = Budget {
            per_run: Some(BudgetLimits {
                max_output_tokens: Some(200),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut enforcer = BudgetEnforcer::new(budget);
        enforcer.begin_turn();
        assert!(enforcer.record(&make_record(100, 150, 0.0)).is_ok());
        let result = enforcer.record(&make_record(100, 100, 0.0));
        assert!(result.is_err());
        let v = result.unwrap_err();
        assert_eq!(v.resource, "run.output_tokens");
    }

    // ---- Per-turn limits ----------------------------------------------------

    #[test]
    fn per_turn_cost_limit() {
        let budget = Budget {
            per_turn: Some(BudgetLimits {
                max_cost_usd: Some(0.5),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut enforcer = BudgetEnforcer::new(budget);
        enforcer.begin_turn();
        assert!(enforcer.record(&make_record(100, 50, 0.3)).is_ok());
        let result = enforcer.record(&make_record(100, 50, 0.3));
        assert!(result.is_err());
        let v = result.unwrap_err();
        assert_eq!(v.resource, "turn.cost_usd");
    }

    #[test]
    fn per_turn_resets_on_new_turn() {
        let budget = Budget {
            per_turn: Some(BudgetLimits {
                max_cost_usd: Some(0.5),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut enforcer = BudgetEnforcer::new(budget);

        // Turn 1: spend up to the limit.
        enforcer.begin_turn();
        assert!(enforcer.record(&make_record(100, 50, 0.4)).is_ok());

        // Turn 2: counter resets.
        enforcer.begin_turn();
        assert!(enforcer.record(&make_record(100, 50, 0.4)).is_ok());
    }

    #[test]
    fn per_turn_token_limit() {
        let budget = Budget {
            per_turn: Some(BudgetLimits {
                max_tokens: Some(500),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut enforcer = BudgetEnforcer::new(budget);
        enforcer.begin_turn();
        let result = enforcer.record(&make_record(300, 300, 0.0));
        assert!(result.is_err());
        let v = result.unwrap_err();
        assert_eq!(v.resource, "turn.total_tokens");
    }

    // ---- end_turn -----------------------------------------------------------

    #[test]
    fn end_turn_passes_within_limits() {
        let budget = Budget {
            per_turn: Some(BudgetLimits {
                max_cost_usd: Some(1.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut enforcer = BudgetEnforcer::new(budget);
        enforcer.begin_turn();
        let _ = enforcer.record(&make_record(100, 50, 0.5));
        assert!(enforcer.end_turn().is_ok());
    }

    // ---- Projected cost check -----------------------------------------------

    #[test]
    fn projected_cost_within_budget() {
        let budget = Budget {
            max_cost_usd: Some(1.0),
            ..Default::default()
        };
        let enforcer = BudgetEnforcer::new(budget);
        assert!(enforcer.check_projected_cost(&Cost::estimated(0.5)).is_ok());
    }

    #[test]
    fn projected_cost_over_budget() {
        let budget = Budget {
            max_cost_usd: Some(1.0),
            ..Default::default()
        };
        let mut enforcer = BudgetEnforcer::new(budget);
        enforcer.begin_turn();
        let _ = enforcer.record(&make_record(100, 50, 0.8));
        let result = enforcer.check_projected_cost(&Cost::estimated(0.5));
        assert!(result.is_err());
    }

    #[test]
    fn projected_cost_checks_per_run_limit_too() {
        let budget = Budget {
            per_run: Some(BudgetLimits {
                max_cost_usd: Some(0.5),
                ..Default::default()
            }),
            ..Default::default()
        };
        let enforcer = BudgetEnforcer::new(budget);
        let result = enforcer.check_projected_cost(&Cost::estimated(0.6));
        assert!(result.is_err());
        let v = result.unwrap_err();
        assert_eq!(v.resource, "run.cost_usd");
    }

    // ---- Projected tokens check ---------------------------------------------

    #[test]
    fn projected_tokens_within_budget() {
        let budget = Budget {
            max_tokens: Some(10_000),
            ..Default::default()
        };
        let enforcer = BudgetEnforcer::new(budget);
        assert!(enforcer.check_projected_tokens(5_000).is_ok());
    }

    #[test]
    fn projected_tokens_over_budget() {
        let budget = Budget {
            max_tokens: Some(10_000),
            ..Default::default()
        };
        let mut enforcer = BudgetEnforcer::new(budget);
        enforcer.begin_turn();
        let _ = enforcer.record(&make_record(5_000, 3_000, 0.0));
        let result = enforcer.check_projected_tokens(5_000);
        assert!(result.is_err());
    }

    #[test]
    fn projected_tokens_overflow_is_treated_as_over_budget() {
        let budget = Budget {
            max_tokens: Some(u64::MAX - 1),
            ..Budget::UNLIMITED
        };
        let mut enforcer = BudgetEnforcer::new(budget);
        enforcer.run_summary.total_tokens = u64::MAX - 10;

        assert!(enforcer.check_projected_tokens(100).is_err());
    }

    // ---- Remaining budget ---------------------------------------------------

    #[test]
    fn remaining_cost_usd_unlimited() {
        let enforcer = BudgetEnforcer::unlimited();
        assert!(enforcer.remaining_cost_usd().is_none());
    }

    #[test]
    fn remaining_cost_usd_with_limit() {
        let budget = Budget {
            max_cost_usd: Some(1.0),
            ..Default::default()
        };
        let mut enforcer = BudgetEnforcer::new(budget);
        enforcer.begin_turn();
        let _ = enforcer.record(&make_record(100, 50, 0.3));
        let remaining = enforcer.remaining_cost_usd().unwrap();
        assert!((remaining - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn remaining_cost_usd_clamped_to_zero() {
        let budget = Budget {
            max_cost_usd: Some(0.1),
            ..Default::default()
        };
        let mut enforcer = BudgetEnforcer::new(budget);
        enforcer.begin_turn();
        let _ = enforcer.record(&make_record(100, 50, 0.5));
        let remaining = enforcer.remaining_cost_usd().unwrap();
        assert_eq!(remaining, 0.0);
    }

    #[test]
    fn remaining_tokens_unlimited() {
        let enforcer = BudgetEnforcer::unlimited();
        assert!(enforcer.remaining_tokens().is_none());
    }

    #[test]
    fn remaining_tokens_with_limit() {
        let budget = Budget {
            max_tokens: Some(1000),
            ..Default::default()
        };
        let mut enforcer = BudgetEnforcer::new(budget);
        enforcer.begin_turn();
        let _ = enforcer.record(&make_record(300, 200, 0.0));
        let remaining = enforcer.remaining_tokens().unwrap();
        assert_eq!(remaining, 500);
    }

    // ---- BudgetViolation Display --------------------------------------------

    #[test]
    fn budget_violation_display() {
        let v = BudgetViolation {
            resource: "cost_usd".into(),
            limit: 1.0,
            actual: 1.5,
            message: "run cost $1.500000 exceeds budget $1.000000".into(),
        };
        let display = format!("{v}");
        assert!(display.contains("exceeds budget"));
    }

    // ---- Accessor tests -----------------------------------------------------

    #[test]
    fn budget_accessor() {
        let budget = Budget {
            max_cost_usd: Some(5.0),
            ..Default::default()
        };
        let enforcer = BudgetEnforcer::new(budget.clone());
        assert_eq!(enforcer.budget(), &budget);
    }

    #[test]
    fn run_summary_updated_on_record() {
        let mut enforcer = BudgetEnforcer::unlimited();
        enforcer.begin_turn();
        let rec = make_record(100, 50, 0.01);
        let _ = enforcer.record(&rec);
        assert_eq!(enforcer.run_summary().total_input_tokens, 100);
        assert_eq!(enforcer.run_summary().total_output_tokens, 50);
    }

    #[test]
    fn turn_summary_reset_on_begin_turn() {
        let mut enforcer = BudgetEnforcer::unlimited();
        enforcer.begin_turn();
        let _ = enforcer.record(&make_record(100, 50, 0.01));
        assert_eq!(enforcer.turn_summary().total_input_tokens, 100);

        enforcer.begin_turn();
        assert_eq!(enforcer.turn_summary().total_input_tokens, 0);
        assert_eq!(enforcer.run_summary().total_input_tokens, 100);
    }
}
