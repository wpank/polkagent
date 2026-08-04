//! Budget tracking for per-agent and per-run spending.
//!
//! [`BudgetTracker`] maintains in-memory spend records for each agent and each
//! run. It enforces a configurable maximum spend ceiling and exposes helpers
//! for checking headroom and recording actual expenditure.
//!
//! # Design notes
//!
//! - Amounts use [`u64`] representing the smallest indivisible unit of the
//!   relevant asset (e.g. planck for DOT, satoshi for BTC). Callers are
//!   responsible for converting larger denominations before calling these APIs.
//! - Thread safety is provided by [`tokio::sync::RwLock`] so the tracker can
//!   be shared via `Arc<BudgetTracker>` across async tasks.
//! - No persistence in Phase 1; budget state is in-memory only and resets on
//!   restart.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use polkagent_core::ids::{AgentId, RunId};

/// Errors that can occur during budget operations.
#[derive(Debug, Error)]
pub enum BudgetError {
    /// The agent has no budget configuration; cannot check or record spend.
    #[error("no budget configured for agent {0}")]
    NoBudgetConfigured(AgentId),

    /// The requested spend exceeds the configured ceiling.
    #[error("budget exceeded for agent {agent_id}: requested {requested}, remaining {remaining}")]
    BudgetExceeded {
        agent_id: AgentId,
        requested: u64,
        remaining: u64,
    },

    /// Amount would cause an integer overflow in the accumulated totals.
    #[error("spend amount would overflow total for agent {0}")]
    Overflow(AgentId),
}

/// The configured ceiling and current spend totals for a single agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetEntry {
    /// Maximum total spend permitted across all runs (in the asset's smallest
    /// unit).
    pub max_spend: u64,

    /// Accumulated spend so far.
    pub spent: u64,
}

impl BudgetEntry {
    /// Remaining headroom (saturating at zero — never negative).
    #[must_use]
    pub fn remaining(&self) -> u64 {
        self.max_spend.saturating_sub(self.spent)
    }

    /// Whether `amount` can be spent without exceeding `max_spend`.
    #[must_use]
    pub fn can_spend(&self, amount: u64) -> bool {
        self.spent.saturating_add(amount) <= self.max_spend
    }
}

/// Summary of remaining budget for an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetStatus {
    pub agent_id: AgentId,
    pub max_spend: u64,
    pub spent: u64,
    pub remaining: u64,
    /// Per-run spend breakdown (run_id → amount spent).
    pub per_run: HashMap<RunId, u64>,
}

/// In-memory budget tracker.
///
/// Wrap in `Arc<BudgetTracker>` to share across tasks.
#[derive(Debug, Default)]
pub struct BudgetTracker {
    /// Per-agent budget entries.
    budgets: RwLock<HashMap<AgentId, BudgetEntry>>,

    /// Per-agent, per-run spend records.
    run_spend: RwLock<HashMap<AgentId, HashMap<RunId, u64>>>,
}

impl BudgetTracker {
    /// Create a new, empty tracker.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Register a budget ceiling for `agent_id`.
    ///
    /// Overwrites any existing ceiling. Does **not** reset accumulated spend.
    pub async fn configure(&self, agent_id: AgentId, max_spend: u64) {
        let mut budgets = self.budgets.write().await;
        budgets
            .entry(agent_id)
            .and_modify(|e| e.max_spend = max_spend)
            .or_insert(BudgetEntry {
                max_spend,
                spent: 0,
            });
        debug!(%agent_id, max_spend, "budget configured");
    }

    /// Check whether `agent_id` can spend `amount` without exceeding its
    /// ceiling.
    ///
    /// Returns `Ok(true)` if within budget, `Ok(false)` if the ceiling would
    /// be exceeded but no error occurred, or an error when no budget is
    /// configured or an overflow would occur.
    ///
    /// # Errors
    ///
    /// - [`BudgetError::NoBudgetConfigured`] if `agent_id` is unknown.
    /// - [`BudgetError::Overflow`] if the arithmetic would overflow.
    pub async fn check_budget(&self, agent_id: AgentId, amount: u64) -> Result<bool, BudgetError> {
        let budgets = self.budgets.read().await;
        let entry = budgets
            .get(&agent_id)
            .ok_or(BudgetError::NoBudgetConfigured(agent_id))?;

        // Check for overflow before comparing.
        entry
            .spent
            .checked_add(amount)
            .ok_or(BudgetError::Overflow(agent_id))?;

        Ok(entry.can_spend(amount))
    }

    /// Record `amount` as spent by `agent_id` during `run_id`.
    ///
    /// Updates both the agent-level total and the per-run breakdown.
    ///
    /// # Errors
    ///
    /// - [`BudgetError::NoBudgetConfigured`] if `agent_id` is unknown.
    /// - [`BudgetError::BudgetExceeded`] if the spend would exceed the
    ///   ceiling. The internal totals are **not** updated in this case.
    /// - [`BudgetError::Overflow`] if the arithmetic would overflow.
    pub async fn record_spend(
        &self,
        agent_id: AgentId,
        run_id: RunId,
        amount: u64,
    ) -> Result<(), BudgetError> {
        // Acquire write lock for atomic check-and-update.
        let mut budgets = self.budgets.write().await;
        let entry = budgets
            .get_mut(&agent_id)
            .ok_or(BudgetError::NoBudgetConfigured(agent_id))?;

        let new_spent = entry
            .spent
            .checked_add(amount)
            .ok_or(BudgetError::Overflow(agent_id))?;

        if new_spent > entry.max_spend {
            let remaining = entry.remaining();
            warn!(
                %agent_id,
                %run_id,
                requested = amount,
                remaining,
                "budget exceeded"
            );
            return Err(BudgetError::BudgetExceeded {
                agent_id,
                requested: amount,
                remaining,
            });
        }

        entry.spent = new_spent;
        drop(budgets);

        // Update per-run breakdown.
        let mut run_spend = self.run_spend.write().await;
        let run_map = run_spend.entry(agent_id).or_default();
        let run_entry = run_map.entry(run_id).or_insert(0);
        // This cannot overflow: individual run totals are always ≤ agent total,
        // which we already checked.
        *run_entry = run_entry.saturating_add(amount);

        debug!(%agent_id, %run_id, amount, "spend recorded");
        Ok(())
    }

    /// Return the current budget status for `agent_id`.
    ///
    /// # Errors
    ///
    /// - [`BudgetError::NoBudgetConfigured`] if `agent_id` is unknown.
    pub async fn get_remaining(&self, agent_id: AgentId) -> Result<BudgetStatus, BudgetError> {
        let budgets = self.budgets.read().await;
        let entry = budgets
            .get(&agent_id)
            .ok_or(BudgetError::NoBudgetConfigured(agent_id))?;

        let run_spend = self.run_spend.read().await;
        let per_run = run_spend.get(&agent_id).cloned().unwrap_or_default();

        Ok(BudgetStatus {
            agent_id,
            max_spend: entry.max_spend,
            spent: entry.spent,
            remaining: entry.remaining(),
            per_run,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn configure_and_check_within_budget() {
        let tracker = BudgetTracker::new();
        let agent = AgentId::new();
        tracker.configure(agent, 1000).await;

        let ok = tracker.check_budget(agent, 500).await.unwrap();
        assert!(ok, "500 of 1000 should be within budget");
    }

    #[tokio::test]
    async fn check_exceeds_budget_returns_false() {
        let tracker = BudgetTracker::new();
        let agent = AgentId::new();
        tracker.configure(agent, 100).await;

        let ok = tracker.check_budget(agent, 101).await.unwrap();
        assert!(!ok, "101 > 100 should exceed budget");
    }

    #[tokio::test]
    async fn record_spend_updates_totals() {
        let tracker = BudgetTracker::new();
        let agent = AgentId::new();
        let run = RunId::new();
        tracker.configure(agent, 1000).await;

        tracker.record_spend(agent, run, 300).await.unwrap();
        let status = tracker.get_remaining(agent).await.unwrap();
        assert_eq!(status.spent, 300);
        assert_eq!(status.remaining, 700);
        assert_eq!(*status.per_run.get(&run).unwrap(), 300);
    }

    #[tokio::test]
    async fn record_spend_exceeds_budget_returns_error() {
        let tracker = BudgetTracker::new();
        let agent = AgentId::new();
        let run = RunId::new();
        tracker.configure(agent, 100).await;

        let err = tracker.record_spend(agent, run, 200).await.unwrap_err();
        assert!(
            matches!(err, BudgetError::BudgetExceeded { .. }),
            "expected BudgetExceeded"
        );

        // Totals must NOT have been updated.
        let status = tracker.get_remaining(agent).await.unwrap();
        assert_eq!(
            status.spent, 0,
            "totals must not update on over-budget spend"
        );
    }

    #[tokio::test]
    async fn no_budget_configured_returns_error() {
        let tracker = BudgetTracker::new();
        let agent = AgentId::new();

        assert!(matches!(
            tracker.check_budget(agent, 1).await.unwrap_err(),
            BudgetError::NoBudgetConfigured(_)
        ));
    }

    #[tokio::test]
    async fn cumulative_spend_across_runs() {
        let tracker = BudgetTracker::new();
        let agent = AgentId::new();
        let run1 = RunId::new();
        let run2 = RunId::new();
        tracker.configure(agent, 100).await;

        tracker.record_spend(agent, run1, 40).await.unwrap();
        tracker.record_spend(agent, run2, 40).await.unwrap();

        let status = tracker.get_remaining(agent).await.unwrap();
        assert_eq!(status.spent, 80);
        assert_eq!(status.remaining, 20);

        // 21 more would exceed the 20 remaining.
        let err = tracker.record_spend(agent, run1, 21).await.unwrap_err();
        assert!(matches!(err, BudgetError::BudgetExceeded { .. }));
    }

    #[tokio::test]
    async fn get_remaining_unknown_agent_returns_error() {
        let tracker = BudgetTracker::new();
        let agent = AgentId::new();

        assert!(matches!(
            tracker.get_remaining(agent).await.unwrap_err(),
            BudgetError::NoBudgetConfigured(_)
        ));
    }
}
