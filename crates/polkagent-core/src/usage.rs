//! Usage tracking and cost accounting types.
//!
//! This module defines the core vocabulary for recording LLM and tool usage
//! across the Polkagent platform. Each inference call, tool invocation, or
//! harness interaction produces a [`UsageRecord`] that is accumulated for
//! billing, audit, and budget enforcement.
//!
//! # Key types
//!
//! | Type | Purpose |
//! |------|---------|
//! | [`UsageRecord`] | A single usage observation (one inference call, one tool run) |
//! | [`UsageSource`] | Where the usage originated (executor, tool, harness) |
//! | [`Cost`] | Monetary cost with an estimation flag |
//! | [`Budget`] | Resource limits for a run or turn |
//! | [`BudgetScope`] | Whether a budget limit applies per-run or per-turn |

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::ids::{RunId, StepId, UsageRecordId};
use crate::turn::TokenUsage;
use crate::types::Timestamp;

// ---------------------------------------------------------------------------
// UsageSource
// ---------------------------------------------------------------------------

/// Where a usage charge originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    /// Usage from an LLM executor (model inference).
    Executor,
    /// Usage from a tool invocation.
    Tool,
    /// Usage from a harness (e.g. Claude Code, Cursor).
    Harness,
}

impl std::fmt::Display for UsageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Executor => write!(f, "executor"),
            Self::Tool => write!(f, "tool"),
            Self::Harness => write!(f, "harness"),
        }
    }
}

// ---------------------------------------------------------------------------
// Cost
// ---------------------------------------------------------------------------

/// A monetary cost value with an estimation flag.
///
/// When `estimated` is `true`, the cost was derived from published pricing
/// tables and may differ from the actual bill. When `false`, it was taken
/// from a provider's usage API.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    /// The cost amount in US dollars.
    pub amount_usd: f64,
    /// Whether this cost is an estimate (true) or confirmed (false).
    pub estimated: bool,
}

impl Cost {
    /// Create a confirmed (non-estimated) cost.
    #[must_use]
    pub fn confirmed(amount_usd: f64) -> Self {
        Self {
            amount_usd,
            estimated: false,
        }
    }

    /// Create an estimated cost.
    #[must_use]
    pub fn estimated(amount_usd: f64) -> Self {
        Self {
            amount_usd,
            estimated: true,
        }
    }

    /// A zero cost.
    pub const ZERO: Self = Self {
        amount_usd: 0.0,
        estimated: false,
    };
}

impl Default for Cost {
    fn default() -> Self {
        Self::ZERO
    }
}

// ---------------------------------------------------------------------------
// UsageRecord
// ---------------------------------------------------------------------------

/// A single usage observation produced during run execution.
///
/// Each LLM inference call, tool invocation, or harness interaction produces
/// one `UsageRecord`. Records are accumulated per-run for budget enforcement
/// and billing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRecord {
    /// Stable identifier for this usage record.
    pub id: UsageRecordId,
    /// Where this usage originated.
    pub source: UsageSource,
    /// The run this usage belongs to.
    pub run_id: RunId,
    /// The step within the run that incurred this usage.
    pub step_id: StepId,
    /// Token breakdown (input, output, cache).
    pub tokens: TokenUsage,
    /// Monetary cost.
    pub cost: Cost,
    /// Wall-clock duration of the operation.
    pub duration: Duration,
    /// When this usage was recorded.
    pub timestamp: Timestamp,
}

// ---------------------------------------------------------------------------
// BudgetScope
// ---------------------------------------------------------------------------

/// Whether a budget limit applies per-run or per-turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetScope {
    /// The limit applies to the entire run.
    PerRun,
    /// The limit applies to each individual turn.
    PerTurn,
}

impl std::fmt::Display for BudgetScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PerRun => write!(f, "per_run"),
            Self::PerTurn => write!(f, "per_turn"),
        }
    }
}

// ---------------------------------------------------------------------------
// Budget
// ---------------------------------------------------------------------------

/// Resource limits for a run or turn.
///
/// All limits are optional; `None` means no limit for that resource.
/// The [`BudgetEnforcer`](in polkagent-run) checks these before and after
/// each executor call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Budget {
    /// Maximum USD spend allowed.
    pub max_cost_usd: Option<f64>,
    /// Maximum total tokens (input + output) allowed.
    pub max_tokens: Option<u64>,
    /// Per-run limits (applied to accumulated totals across all turns).
    pub per_run: Option<BudgetLimits>,
    /// Per-turn limits (applied to each individual turn independently).
    pub per_turn: Option<BudgetLimits>,
}

/// Granular limits that can be applied at either the run or turn level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BudgetLimits {
    /// Maximum USD spend.
    pub max_cost_usd: Option<f64>,
    /// Maximum total tokens.
    pub max_tokens: Option<u64>,
    /// Maximum input tokens.
    pub max_input_tokens: Option<u64>,
    /// Maximum output tokens.
    pub max_output_tokens: Option<u64>,
}

impl Budget {
    /// A budget with no limits.
    pub const UNLIMITED: Self = Self {
        max_cost_usd: None,
        max_tokens: None,
        per_run: None,
        per_turn: None,
    };

    /// Returns `true` if no limits are set.
    #[must_use]
    pub fn is_unlimited(&self) -> bool {
        self.max_cost_usd.is_none()
            && self.max_tokens.is_none()
            && self.per_run.is_none()
            && self.per_turn.is_none()
    }
}

// ---------------------------------------------------------------------------
// UsageSummary
// ---------------------------------------------------------------------------

/// Accumulated usage totals for a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UsageSummary {
    /// Total input tokens across all records.
    pub total_input_tokens: u64,
    /// Total output tokens across all records.
    pub total_output_tokens: u64,
    /// Total tokens (input + output).
    pub total_tokens: u64,
    /// Total cache-read tokens.
    pub total_cache_read_tokens: u64,
    /// Total cache-write tokens.
    pub total_cache_write_tokens: u64,
    /// Total cost in USD.
    pub total_cost_usd: f64,
    /// Number of usage records accumulated.
    pub record_count: u64,
}

impl UsageSummary {
    /// Accumulate a usage record into this summary.
    pub fn record(&mut self, usage: &UsageRecord) {
        self.total_input_tokens = self
            .total_input_tokens
            .saturating_add(u64::from(usage.tokens.input_tokens));
        self.total_output_tokens = self
            .total_output_tokens
            .saturating_add(u64::from(usage.tokens.output_tokens));
        self.total_tokens = self
            .total_tokens
            .saturating_add(u64::from(usage.tokens.total_tokens));
        self.total_cache_read_tokens = self
            .total_cache_read_tokens
            .saturating_add(u64::from(usage.tokens.cache_read_tokens));
        self.total_cache_write_tokens = self
            .total_cache_write_tokens
            .saturating_add(u64::from(usage.tokens.cache_write_tokens));
        self.total_cost_usd += usage.cost.amount_usd;
        self.record_count = self.record_count.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{RunId, StepId, UsageRecordId};
    use crate::types::now;

    fn sample_record() -> UsageRecord {
        UsageRecord {
            id: UsageRecordId::new(),
            source: UsageSource::Executor,
            run_id: RunId::new(),
            step_id: StepId::new(),
            tokens: TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                total_tokens: 150,
                cache_read_tokens: 10,
                cache_write_tokens: 5,
                cost_usd: Some(0.01),
            },
            cost: Cost::estimated(0.01),
            duration: Duration::from_millis(250),
            timestamp: now(),
        }
    }

    // --- UsageSource ---

    #[test]
    fn usage_source_display() {
        assert_eq!(UsageSource::Executor.to_string(), "executor");
        assert_eq!(UsageSource::Tool.to_string(), "tool");
        assert_eq!(UsageSource::Harness.to_string(), "harness");
    }

    #[test]
    fn usage_source_serde_round_trip() {
        for source in [UsageSource::Executor, UsageSource::Tool, UsageSource::Harness] {
            let json = serde_json::to_string(&source).expect("serialize");
            let back: UsageSource = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(source, back);
        }
    }

    #[test]
    fn usage_source_serde_values() {
        assert_eq!(
            serde_json::to_string(&UsageSource::Executor).unwrap(),
            r#""executor""#
        );
        assert_eq!(
            serde_json::to_string(&UsageSource::Tool).unwrap(),
            r#""tool""#
        );
        assert_eq!(
            serde_json::to_string(&UsageSource::Harness).unwrap(),
            r#""harness""#
        );
    }

    // --- Cost ---

    #[test]
    fn cost_confirmed_is_not_estimated() {
        let c = Cost::confirmed(0.05);
        assert!(!c.estimated);
        assert!((c.amount_usd - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn cost_estimated_is_estimated() {
        let c = Cost::estimated(0.03);
        assert!(c.estimated);
        assert!((c.amount_usd - 0.03).abs() < f64::EPSILON);
    }

    #[test]
    fn cost_zero() {
        let c = Cost::ZERO;
        assert_eq!(c.amount_usd, 0.0);
        assert!(!c.estimated);
    }

    #[test]
    fn cost_default_is_zero() {
        let c = Cost::default();
        assert_eq!(c.amount_usd, 0.0);
        assert!(!c.estimated);
    }

    #[test]
    fn cost_serde_round_trip() {
        let c = Cost::estimated(1.23);
        let json = serde_json::to_string(&c).expect("serialize");
        let back: Cost = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(c, back);
    }

    // --- UsageRecord ---

    #[test]
    fn usage_record_serde_round_trip() {
        let rec = sample_record();
        let json = serde_json::to_string(&rec).expect("serialize");
        let back: UsageRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(rec.id, back.id);
        assert_eq!(rec.source, back.source);
        assert_eq!(rec.run_id, back.run_id);
        assert_eq!(rec.step_id, back.step_id);
        assert_eq!(rec.tokens.input_tokens, back.tokens.input_tokens);
        assert_eq!(rec.cost, back.cost);
    }

    #[test]
    fn usage_record_fields_populated() {
        let rec = sample_record();
        assert_eq!(rec.source, UsageSource::Executor);
        assert_eq!(rec.tokens.input_tokens, 100);
        assert_eq!(rec.tokens.output_tokens, 50);
        assert!(rec.cost.estimated);
    }

    // --- Budget ---

    #[test]
    fn budget_unlimited_is_unlimited() {
        assert!(Budget::UNLIMITED.is_unlimited());
    }

    #[test]
    fn budget_default_is_unlimited() {
        let b = Budget::default();
        assert!(b.is_unlimited());
    }

    #[test]
    fn budget_with_cost_limit_not_unlimited() {
        let b = Budget {
            max_cost_usd: Some(10.0),
            ..Default::default()
        };
        assert!(!b.is_unlimited());
    }

    #[test]
    fn budget_with_token_limit_not_unlimited() {
        let b = Budget {
            max_tokens: Some(100_000),
            ..Default::default()
        };
        assert!(!b.is_unlimited());
    }

    #[test]
    fn budget_with_per_run_not_unlimited() {
        let b = Budget {
            per_run: Some(BudgetLimits {
                max_cost_usd: Some(5.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(!b.is_unlimited());
    }

    #[test]
    fn budget_serde_round_trip() {
        let b = Budget {
            max_cost_usd: Some(10.0),
            max_tokens: Some(500_000),
            per_run: Some(BudgetLimits {
                max_cost_usd: Some(5.0),
                max_tokens: Some(250_000),
                max_input_tokens: None,
                max_output_tokens: Some(100_000),
            }),
            per_turn: None,
        };
        let json = serde_json::to_string(&b).expect("serialize");
        let back: Budget = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(b, back);
    }

    // --- BudgetScope ---

    #[test]
    fn budget_scope_display() {
        assert_eq!(BudgetScope::PerRun.to_string(), "per_run");
        assert_eq!(BudgetScope::PerTurn.to_string(), "per_turn");
    }

    #[test]
    fn budget_scope_serde_round_trip() {
        for scope in [BudgetScope::PerRun, BudgetScope::PerTurn] {
            let json = serde_json::to_string(&scope).expect("serialize");
            let back: BudgetScope = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(scope, back);
        }
    }

    // --- UsageSummary ---

    #[test]
    fn usage_summary_default_is_zero() {
        let s = UsageSummary::default();
        assert_eq!(s.total_input_tokens, 0);
        assert_eq!(s.total_output_tokens, 0);
        assert_eq!(s.total_tokens, 0);
        assert_eq!(s.total_cost_usd, 0.0);
        assert_eq!(s.record_count, 0);
    }

    #[test]
    fn usage_summary_records_single() {
        let mut s = UsageSummary::default();
        let rec = sample_record();
        s.record(&rec);
        assert_eq!(s.total_input_tokens, 100);
        assert_eq!(s.total_output_tokens, 50);
        assert_eq!(s.total_tokens, 150);
        assert_eq!(s.total_cache_read_tokens, 10);
        assert_eq!(s.total_cache_write_tokens, 5);
        assert!((s.total_cost_usd - 0.01).abs() < f64::EPSILON);
        assert_eq!(s.record_count, 1);
    }

    #[test]
    fn usage_summary_accumulates_multiple() {
        let mut s = UsageSummary::default();
        let rec1 = sample_record();
        let mut rec2 = sample_record();
        rec2.tokens.input_tokens = 200;
        rec2.tokens.output_tokens = 100;
        rec2.tokens.total_tokens = 300;
        rec2.cost = Cost::estimated(0.02);
        s.record(&rec1);
        s.record(&rec2);
        assert_eq!(s.total_input_tokens, 300);
        assert_eq!(s.total_output_tokens, 150);
        assert_eq!(s.total_tokens, 450);
        assert!((s.total_cost_usd - 0.03).abs() < f64::EPSILON);
        assert_eq!(s.record_count, 2);
    }

    #[test]
    fn usage_summary_serde_round_trip() {
        let mut s = UsageSummary::default();
        let rec = sample_record();
        s.record(&rec);
        let json = serde_json::to_string(&s).expect("serialize");
        let back: UsageSummary = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(s, back);
    }

    // --- BudgetLimits ---

    #[test]
    fn budget_limits_default_all_none() {
        let bl = BudgetLimits::default();
        assert!(bl.max_cost_usd.is_none());
        assert!(bl.max_tokens.is_none());
        assert!(bl.max_input_tokens.is_none());
        assert!(bl.max_output_tokens.is_none());
    }

    #[test]
    fn budget_limits_serde_round_trip() {
        let bl = BudgetLimits {
            max_cost_usd: Some(1.5),
            max_tokens: Some(10_000),
            max_input_tokens: Some(8_000),
            max_output_tokens: Some(2_000),
        };
        let json = serde_json::to_string(&bl).expect("serialize");
        let back: BudgetLimits = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(bl, back);
    }
}
