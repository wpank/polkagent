//! Budget enforcement: limits, state tracking, and spend decisions.

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::error::PaymentError;
use crate::types::{Amount, AssetId, CostRecord, UsageSummary};

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "validated USD estimates are intentionally quantized to six decimal places"
)]
fn micro_usd_from_estimate(estimated_usd: f64) -> Result<u128, PaymentError> {
    let scaled = estimated_usd * 1_000_000.0;
    if !scaled.is_finite() || scaled.is_sign_negative() {
        return Err(PaymentError::validation(
            "estimated_usd",
            "cost must be a finite non-negative number",
        ));
    }
    if scaled >= u128::MAX as f64 {
        return Err(PaymentError::ArithmeticOverflow {
            context: "converting estimated USD to micro-dollars".into(),
        });
    }
    Ok(scaled as u128)
}

#[allow(
    clippy::cast_precision_loss,
    reason = "usage summaries expose an approximate f64 USD value by contract"
)]
fn micro_usd_as_f64(value: u128) -> f64 {
    value as f64 / 1_000_000.0
}

// ---------------------------------------------------------------------------
// BudgetConfig
// ---------------------------------------------------------------------------

/// Per-agent budget limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfig {
    /// Maximum spend allowed in a single run.
    pub max_per_run: Option<Amount>,
    /// Maximum spend allowed per calendar day (UTC).
    pub max_per_day: Option<Amount>,
    /// Maximum spend allowed per calendar month (UTC).
    pub max_per_month: Option<Amount>,
    /// Percentage at which a warning is issued (0..100).
    pub warn_at_percent: u8,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_per_run: None,
            max_per_day: None,
            max_per_month: None,
            warn_at_percent: 80,
        }
    }
}

// ---------------------------------------------------------------------------
// BudgetState
// ---------------------------------------------------------------------------

/// Tracks accumulated spend for an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetState {
    /// Total spent since the start of the current calendar day (UTC).
    pub spent_today: Amount,
    /// Total spent since the start of the current calendar month (UTC).
    pub spent_this_month: Amount,
    /// When the counters were last reset.
    pub last_reset: DateTime<Utc>,
}

impl BudgetState {
    /// Create a fresh zero state for the given asset.
    #[must_use]
    pub fn new(asset: AssetId, decimals: u8) -> Self {
        Self {
            spent_today: Amount::zero(asset.clone(), decimals),
            spent_this_month: Amount::zero(asset, decimals),
            last_reset: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// BudgetDecision
// ---------------------------------------------------------------------------

/// The result of a budget check.
#[derive(Debug, Clone, PartialEq)]
pub enum BudgetDecision {
    /// The spend is within all configured limits.
    Allow,
    /// The spend is allowed but approaching a limit.
    Warn {
        /// How much of the budget remains, as a percentage (0..100).
        remaining_percent: u8,
    },
    /// The spend was denied because it would exceed a limit.
    Deny {
        /// Human-readable reason.
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// BudgetChecker
// ---------------------------------------------------------------------------

/// Checks proposed spends against configured budget limits and tracks
/// accumulated usage per agent.
pub struct BudgetChecker {
    /// Per-agent configuration.
    configs: HashMap<String, BudgetConfig>,
    /// Per-agent accumulated state.
    states: HashMap<String, BudgetState>,
}

impl BudgetChecker {
    /// Create a new budget checker with no pre-configured agents.
    #[must_use]
    pub fn new() -> Self {
        Self {
            configs: HashMap::new(),
            states: HashMap::new(),
        }
    }

    /// Register a budget configuration for an agent.
    pub fn set_config(&mut self, agent_id: impl Into<String>, config: BudgetConfig) {
        self.configs.insert(agent_id.into(), config);
    }

    /// Check whether a proposed `amount` is permitted for `agent_id`.
    ///
    /// Returns [`BudgetDecision::Allow`] if no config is registered for the
    /// agent (open by default — the policy layer is expected to gate whether
    /// budget checking is required).
    pub fn check(&mut self, agent_id: &str, amount: &Amount) -> BudgetDecision {
        let now = Utc::now();

        let config = match self.configs.get(agent_id) {
            Some(c) => c.clone(),
            None => return BudgetDecision::Allow,
        };

        // Ensure state exists and is reset for the current period.
        let state = self
            .states
            .entry(agent_id.to_string())
            .or_insert_with(|| BudgetState::new(amount.asset.clone(), amount.decimals));
        maybe_reset_state(state, now, &amount.asset, amount.decimals);

        // --- Per-day check ---
        if let Some(ref max_day) = config.max_per_day {
            match state.spent_today.checked_add(amount) {
                Ok(projected) => {
                    if projected.value > max_day.value {
                        return BudgetDecision::Deny {
                            reason: format!(
                                "daily limit would be exceeded: {} + {} > {}",
                                state.spent_today.display_human(),
                                amount.display_human(),
                                max_day.display_human(),
                            ),
                        };
                    }
                    let decision = maybe_warn(&projected, max_day, config.warn_at_percent, "daily");
                    if let Some(d) = decision {
                        return d;
                    }
                }
                Err(_) => {
                    return BudgetDecision::Deny {
                        reason: "arithmetic overflow computing daily spend".into(),
                    };
                }
            }
        }

        // --- Per-month check ---
        if let Some(ref max_month) = config.max_per_month {
            match state.spent_this_month.checked_add(amount) {
                Ok(projected) => {
                    if projected.value > max_month.value {
                        return BudgetDecision::Deny {
                            reason: format!(
                                "monthly limit would be exceeded: {} + {} > {}",
                                state.spent_this_month.display_human(),
                                amount.display_human(),
                                max_month.display_human(),
                            ),
                        };
                    }
                    let decision =
                        maybe_warn(&projected, max_month, config.warn_at_percent, "monthly");
                    if let Some(d) = decision {
                        return d;
                    }
                }
                Err(_) => {
                    return BudgetDecision::Deny {
                        reason: "arithmetic overflow computing monthly spend".into(),
                    };
                }
            }
        }

        BudgetDecision::Allow
    }

    /// Record a completed spend, updating the agent's accumulated state.
    pub fn record_spend(
        &mut self,
        agent_id: &str,
        cost_record: &CostRecord,
    ) -> Result<(), PaymentError> {
        let state = self
            .states
            .entry(agent_id.to_string())
            .or_insert_with(|| BudgetState::new(AssetId::Native, 0));

        let now = Utc::now();
        maybe_reset_state(
            state,
            now,
            &state.spent_today.asset.clone(),
            state.spent_today.decimals,
        );

        // Convert USD cost to a comparable Amount. We use a u128 representation
        // of micro-dollars (6 decimal places) for internal tracking.
        let micro_usd = micro_usd_from_estimate(cost_record.estimated_usd)?;
        let cost_amount = Amount::new(micro_usd, AssetId::Native, 6);

        state.spent_today = state.spent_today.checked_add(&cost_amount).map_err(|_| {
            PaymentError::ArithmeticOverflow {
                context: "recording daily spend".into(),
            }
        })?;
        state.spent_this_month =
            state
                .spent_this_month
                .checked_add(&cost_amount)
                .map_err(|_| PaymentError::ArithmeticOverflow {
                    context: "recording monthly spend".into(),
                })?;

        Ok(())
    }

    /// Get a usage summary for the given agent over a time period.
    ///
    /// This returns the currently tracked state; for historical queries the
    /// caller should use the [`PaymentStore`](crate::store::PaymentStore).
    #[must_use]
    pub fn get_usage(
        &self,
        agent_id: &str,
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
    ) -> UsageSummary {
        let state = self.states.get(agent_id);

        let (tokens, usd) = match state {
            Some(s) => {
                // Convert back from micro-dollars to USD.
                let usd = micro_usd_as_f64(s.spent_this_month.value);
                (0u64, usd)
            }
            None => (0, 0.0),
        };

        UsageSummary {
            total_runs: 0,
            total_tokens: tokens,
            estimated_usd: usd,
            period_start,
            period_end,
        }
    }
}

impl Default for BudgetChecker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Reset daily/monthly counters if the calendar day or month has changed.
fn maybe_reset_state(state: &mut BudgetState, now: DateTime<Utc>, asset: &AssetId, decimals: u8) {
    let last = state.last_reset;

    if now.date_naive() != last.date_naive() {
        state.spent_today = Amount::zero(asset.clone(), decimals);
        if now.month() != last.month() || now.year() != last.year() {
            state.spent_this_month = Amount::zero(asset.clone(), decimals);
        }
        state.last_reset = now;
    }
}

/// Return a `Warn` decision if the projected spend is above the warning
/// threshold, or `None` if no warning is needed.
fn maybe_warn(
    projected: &Amount,
    limit: &Amount,
    warn_percent: u8,
    label: &str,
) -> Option<BudgetDecision> {
    if limit.value == 0 {
        return None;
    }
    // Calculate usage as percentage (avoiding floating point).
    let usage_pct = (projected.value.saturating_mul(100)) / limit.value;
    let threshold = u128::from(warn_percent);
    if usage_pct >= threshold {
        let remaining = 100u128.saturating_sub(usage_pct);
        warn!("{label} budget at {usage_pct}% — {remaining}% remaining",);
        Some(BudgetDecision::Warn {
            remaining_percent: remaining.min(100) as u8,
        })
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn usd_asset() -> AssetId {
        AssetId::Native
    }

    fn usd(micro: u128) -> Amount {
        Amount::new(micro, usd_asset(), 6)
    }

    #[test]
    fn allow_when_no_config() {
        let mut checker = BudgetChecker::new();
        let decision = checker.check("agent-1", &usd(1_000_000));
        assert_eq!(decision, BudgetDecision::Allow);
    }

    #[test]
    fn deny_over_daily_limit() {
        let mut checker = BudgetChecker::new();
        checker.set_config(
            "agent-1",
            BudgetConfig {
                max_per_day: Some(usd(10_000_000)), // $10
                warn_at_percent: 80,
                ..Default::default()
            },
        );

        // First $5 should be allowed.
        let decision = checker.check("agent-1", &usd(5_000_000));
        assert_eq!(decision, BudgetDecision::Allow);

        // Record the $5.
        checker.states.entry("agent-1".to_string()).and_modify(|s| {
            s.spent_today = usd(5_000_000);
            s.spent_this_month = usd(5_000_000);
        });

        // Next $6 should be denied (total $11 > $10).
        let decision = checker.check("agent-1", &usd(6_000_000));
        assert!(matches!(decision, BudgetDecision::Deny { .. }));
    }

    #[test]
    fn warn_near_limit() {
        let mut checker = BudgetChecker::new();
        checker.set_config(
            "agent-1",
            BudgetConfig {
                max_per_day: Some(usd(10_000_000)), // $10
                warn_at_percent: 80,
                ..Default::default()
            },
        );

        // Pre-set state to $7 spent.
        checker.states.insert(
            "agent-1".to_string(),
            BudgetState {
                spent_today: usd(7_000_000),
                spent_this_month: usd(7_000_000),
                last_reset: Utc::now(),
            },
        );

        // Next $2 puts us at $9 = 90% -> should warn.
        let decision = checker.check("agent-1", &usd(2_000_000));
        assert!(matches!(decision, BudgetDecision::Warn { .. }));
    }

    #[test]
    fn deny_over_monthly_limit() {
        let mut checker = BudgetChecker::new();
        checker.set_config(
            "agent-1",
            BudgetConfig {
                max_per_month: Some(usd(100_000_000)), // $100
                warn_at_percent: 90,
                ..Default::default()
            },
        );

        // Pre-set state to $95 spent.
        checker.states.insert(
            "agent-1".to_string(),
            BudgetState {
                spent_today: usd(5_000_000),
                spent_this_month: usd(95_000_000),
                last_reset: Utc::now(),
            },
        );

        // Next $6 should be denied (total $101 > $100).
        let decision = checker.check("agent-1", &usd(6_000_000));
        assert!(matches!(decision, BudgetDecision::Deny { .. }));
    }

    #[test]
    fn record_spend_updates_state() {
        let mut checker = BudgetChecker::new();
        checker.set_config("agent-1", BudgetConfig::default());

        let cost = CostRecord {
            run_id: "run-1".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4".into(),
            input_tokens: 1000,
            output_tokens: 500,
            estimated_usd: 0.0105,
            recorded_at: Utc::now(),
        };

        checker
            .record_spend("agent-1", &cost)
            .expect("should succeed");
        let state = checker.states.get("agent-1").expect("state should exist");
        // 0.0105 USD = 10_500 micro-dollars
        assert_eq!(state.spent_today.value, 10_500);
    }

    #[test]
    fn record_spend_rejects_invalid_usd_estimates() {
        for estimated_usd in [-1.0, f64::NAN, f64::INFINITY] {
            let mut checker = BudgetChecker::new();
            let cost = CostRecord {
                run_id: "run-invalid".into(),
                provider: "fixture".into(),
                model: "fixture/model".into(),
                input_tokens: 0,
                output_tokens: 0,
                estimated_usd,
                recorded_at: Utc::now(),
            };

            let error = checker
                .record_spend("agent-1", &cost)
                .expect_err("invalid estimated USD must fail closed");
            assert!(matches!(
                error,
                PaymentError::Validation { ref field, .. } if field == "estimated_usd"
            ));
        }
    }

    #[test]
    fn get_usage_returns_summary() {
        let mut checker = BudgetChecker::new();
        checker.set_config("agent-1", BudgetConfig::default());

        let now = Utc::now();
        let usage = checker.get_usage("agent-1", now, now);
        assert_eq!(usage.total_runs, 0);
        assert!((usage.estimated_usd - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn budget_config_default() {
        let config = BudgetConfig::default();
        assert!(config.max_per_run.is_none());
        assert!(config.max_per_day.is_none());
        assert!(config.max_per_month.is_none());
        assert_eq!(config.warn_at_percent, 80);
    }
}
