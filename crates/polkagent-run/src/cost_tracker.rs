//! Per-run cost accumulation and budget enforcement.
//!
//! [`CostTracker`] accumulates LLM usage costs across turns of a single run
//! and enforces per-run budget limits. It integrates with
//! `polkagent-payment`'s `BudgetConfig` and `CostEstimator` to:
//!
//! 1. **Pre-turn budget check** — Estimate the cost of the *upcoming* turn
//!    (using the requested `max_tokens`) and reject it if the run budget
//!    would be exceeded.
//! 2. **Post-turn cost recording** — Record the *actual* cost after the
//!    executor returns, accumulating into the per-run total.
//! 3. **Budget-exceeded termination** — If actual spending exceeds the
//!    per-run limit, signal `BudgetExceeded` to the orchestrator.
//!
//! # Feature gate
//!
//! Most of this module requires the **`payment`** Cargo feature. The
//! `BudgetExceededReason` type is always compiled so that the orchestrator
//! can reference it unconditionally.

use polkagent_core::ids::RunId;

// ---------------------------------------------------------------------------
// BudgetExceededReason
// ---------------------------------------------------------------------------

/// The reason a run was terminated due to budget exhaustion.
#[derive(Debug, Clone)]
pub struct BudgetExceededReason {
    /// Human-readable description of the exceeded limit.
    pub reason: String,
    /// Total estimated USD spent up to the point of termination.
    pub spent_usd: f64,
}

// ---------------------------------------------------------------------------
// CostTracker
// ---------------------------------------------------------------------------

/// Tracks per-run LLM usage costs and enforces budget limits.
///
/// Construct one per run before the turn loop, then call:
///
/// - [`check_budget`](CostTracker::check_budget) *before* each executor call.
/// - [`record_turn_cost`](CostTracker::record_turn_cost) *after* each executor
///   call returns.
///
/// If the budget is exceeded, both methods return `Some(BudgetExceededReason)`.
pub struct CostTracker {
    /// The run whose costs are being tracked.
    run_id: RunId,
    /// Optional maximum USD spend for this run (`None` = unlimited).
    max_per_run_usd: Option<f64>,
    /// Accumulated cost so far (in USD).
    total_usd: f64,
    /// Accumulated input tokens across all turns.
    total_input_tokens: u64,
    /// Accumulated output tokens across all turns.
    total_output_tokens: u64,
    /// Cost estimator (boxed for flexibility).
    #[cfg(feature = "payment")]
    estimator: polkagent_payment::CostEstimator,
    /// Payment store for persisting cost records.
    #[cfg(feature = "payment")]
    payment_store: Option<std::sync::Arc<dyn polkagent_payment::PaymentStore>>,
}

impl CostTracker {
    /// Create a `CostTracker` with no budget limit and no payment store.
    ///
    /// This is useful when payment tracking is enabled at compile time but
    /// you want to run without enforcement (e.g. for tests or internal runs).
    #[must_use]
    pub fn unbounded(run_id: RunId) -> Self {
        Self {
            run_id,
            max_per_run_usd: None,
            total_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            #[cfg(feature = "payment")]
            estimator: polkagent_payment::CostEstimator::new(),
            #[cfg(feature = "payment")]
            payment_store: None,
        }
    }

    /// Create a `CostTracker` with an explicit per-run budget.
    #[must_use]
    pub fn with_budget(run_id: RunId, max_per_run_usd: f64) -> Self {
        Self {
            run_id,
            max_per_run_usd: Some(max_per_run_usd),
            total_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            #[cfg(feature = "payment")]
            estimator: polkagent_payment::CostEstimator::new(),
            #[cfg(feature = "payment")]
            payment_store: None,
        }
    }

    /// Attach a `PaymentStore` for persisting cost records.
    ///
    /// Only available when the `payment` feature is enabled.
    #[cfg(feature = "payment")]
    #[must_use]
    pub fn with_store(
        mut self,
        store: std::sync::Arc<dyn polkagent_payment::PaymentStore>,
    ) -> Self {
        self.payment_store = Some(store);
        self
    }

    /// Check whether starting a new turn with the given parameters would
    /// exceed the per-run budget.
    ///
    /// This estimates the *worst-case* cost of the upcoming turn: all
    /// `max_tokens` are assumed to be output tokens (conservative).
    ///
    /// Returns `None` if the budget check passes (the turn may proceed).
    /// Returns `Some(BudgetExceededReason)` if the budget would be exceeded.
    ///
    /// When the `payment` feature is disabled, this always returns `None`.
    #[allow(unused_variables)]
    pub fn check_budget(
        &self,
        provider: &str,
        model: &str,
        max_tokens: u32,
    ) -> Option<BudgetExceededReason> {
        let limit = self.max_per_run_usd?;

        #[cfg(feature = "payment")]
        {
            // Worst-case estimate: all max_tokens are output.
            let estimated = self
                .estimator
                .estimate(provider, model, 0, u64::from(max_tokens));
            let projected = self.total_usd + estimated;
            if projected > limit {
                return Some(BudgetExceededReason {
                    reason: format!(
                        "projected spend ${projected:.6} would exceed per-run budget ${limit:.6}"
                    ),
                    spent_usd: self.total_usd,
                });
            }
        }

        // Without the `payment` feature we have no estimator; skip.
        #[cfg(not(feature = "payment"))]
        {
            if self.total_usd > limit {
                return Some(BudgetExceededReason {
                    reason: format!(
                        "accumulated spend ${:.6} exceeds per-run budget ${limit:.6}",
                        self.total_usd
                    ),
                    spent_usd: self.total_usd,
                });
            }
        }

        None
    }

    /// Record the actual cost of a completed turn and update accumulated totals.
    ///
    /// Returns `Some(BudgetExceededReason)` if the budget is now exceeded after
    /// recording this turn's cost. The caller should terminate the run.
    ///
    /// When the `payment` feature is disabled, only the in-memory totals are
    /// updated; no `PaymentStore` persistence occurs.
    #[allow(unused_variables)]
    pub async fn record_turn_cost(
        &mut self,
        provider: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Option<BudgetExceededReason> {
        #[cfg(feature = "payment")]
        let turn_usd = self
            .estimator
            .estimate(provider, model, input_tokens, output_tokens);

        #[cfg(not(feature = "payment"))]
        let turn_usd = 0.0_f64;

        self.total_usd += turn_usd;
        self.total_input_tokens += input_tokens;
        self.total_output_tokens += output_tokens;

        // Persist to payment store (best-effort; errors are logged but do not
        // fail the run).
        #[cfg(feature = "payment")]
        if let Some(store) = &self.payment_store {
            let record = polkagent_payment::CostRecord {
                run_id: self.run_id.to_string(),
                provider: provider.to_owned(),
                model: model.to_owned(),
                input_tokens,
                output_tokens,
                estimated_usd: turn_usd,
                recorded_at: chrono::Utc::now(),
            };
            if let Err(err) = store.record_cost(record).await {
                tracing::warn!(run_id = %self.run_id, %err, "failed to persist cost record");
            }
        }

        // Check if total spend now exceeds the budget.
        if let Some(limit) = self.max_per_run_usd {
            if self.total_usd > limit {
                return Some(BudgetExceededReason {
                    reason: format!(
                        "run budget exhausted: spent ${:.6} > limit ${limit:.6}",
                        self.total_usd
                    ),
                    spent_usd: self.total_usd,
                });
            }
        }

        None
    }

    /// Returns the accumulated USD cost for this run.
    #[must_use]
    pub fn total_usd(&self) -> f64 {
        self.total_usd
    }

    /// Returns the accumulated input token count.
    #[must_use]
    pub fn total_input_tokens(&self) -> u64 {
        self.total_input_tokens
    }

    /// Returns the accumulated output token count.
    #[must_use]
    pub fn total_output_tokens(&self) -> u64 {
        self.total_output_tokens
    }
}

impl std::fmt::Debug for CostTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CostTracker")
            .field("run_id", &self.run_id)
            .field("max_per_run_usd", &self.max_per_run_usd)
            .field("total_usd", &self.total_usd)
            .field("total_input_tokens", &self.total_input_tokens)
            .field("total_output_tokens", &self.total_output_tokens)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use polkagent_core::RunId;

    use super::*;

    // ---- CostTracker::check_budget -----------------------------------------

    #[test]
    fn check_budget_allows_when_no_limit() {
        let tracker = CostTracker::unbounded(RunId::new());
        let result = tracker.check_budget("anthropic", "claude-sonnet-4", 4096);
        assert!(result.is_none(), "unbounded tracker should always allow");
    }

    #[test]
    fn check_budget_allows_when_under_limit() {
        // $1 limit, $0 spent so far, very small token count.
        let tracker = CostTracker::with_budget(RunId::new(), 1.0);
        // With the `payment` feature: 1 input + 1 output at $3/$15 per million
        // = effectively zero cost; without the feature also returns None.
        let result = tracker.check_budget("anthropic", "claude-sonnet-4", 1);
        assert!(
            result.is_none(),
            "tiny token request should be under budget"
        );
    }

    #[tokio::test]
    async fn record_turn_cost_accumulates_totals() {
        let mut tracker = CostTracker::unbounded(RunId::new());
        tracker
            .record_turn_cost("anthropic", "claude-sonnet-4", 100, 50)
            .await;
        tracker
            .record_turn_cost("anthropic", "claude-sonnet-4", 200, 80)
            .await;

        assert_eq!(tracker.total_input_tokens(), 300);
        assert_eq!(tracker.total_output_tokens(), 130);
        // Total USD depends on whether `payment` feature is enabled; just
        // verify it is non-negative and finite.
        assert!(tracker.total_usd() >= 0.0);
        assert!(tracker.total_usd().is_finite());
    }

    #[tokio::test]
    async fn record_turn_cost_returns_exceeded_when_over_budget() {
        // Budget = $0.0001 USD (tiny).
        let mut tracker = CostTracker::with_budget(RunId::new(), 0.0001);

        // Record a turn that will likely push the total over the limit when
        // the `payment` feature is on (large token counts). Without the
        // feature, total_usd stays 0.0 and budget check is based on
        // accumulated 0.0 vs. limit, so we need to also test the
        // non-feature path.
        let _result = tracker
            .record_turn_cost("anthropic", "claude-sonnet-4", 1_000_000, 500_000)
            .await;

        // With `payment` feature: spending will be >> $0.0001 → exceeded.
        // Without `payment` feature: total_usd = 0.0 → not exceeded.
        // Test both cases gracefully.
        #[cfg(feature = "payment")]
        assert!(
            result.is_some(),
            "large spend should exceed tiny budget when payment feature is on"
        );
    }

    #[tokio::test]
    async fn budget_not_exceeded_within_limit() {
        // Enough budget for 1M tokens at Claude rates.
        let mut tracker = CostTracker::with_budget(RunId::new(), 100.0);
        let result = tracker
            .record_turn_cost("anthropic", "claude-sonnet-4", 1000, 500)
            .await;
        assert!(
            result.is_none(),
            "small spend should not exceed $100 budget"
        );
    }

    #[tokio::test]
    async fn check_budget_after_accumulation_reflects_total() {
        let mut tracker = CostTracker::with_budget(RunId::new(), 0.001);

        // Record a tiny spend first.
        tracker
            .record_turn_cost("anthropic", "claude-sonnet-4", 10, 5)
            .await;

        // The pre-turn check should see the accumulated spend.
        // With payment feature, even 1 output token at $15/M is $0.000015 —
        // well within $0.001. Without the feature, total_usd = 0.0.
        let result = tracker.check_budget("anthropic", "claude-sonnet-4", 1);
        // Either way should pass for such small amounts.
        assert!(result.is_none(), "tiny post-accumulation check should pass");
    }

    #[test]
    fn unbounded_has_zero_initial_cost() {
        let tracker = CostTracker::unbounded(RunId::new());
        assert_eq!(tracker.total_usd(), 0.0);
        assert_eq!(tracker.total_input_tokens(), 0);
        assert_eq!(tracker.total_output_tokens(), 0);
    }

    #[test]
    fn debug_impl_does_not_panic() {
        let tracker = CostTracker::unbounded(RunId::new());
        let s = format!("{tracker:?}");
        assert!(s.contains("CostTracker"));
    }
}
