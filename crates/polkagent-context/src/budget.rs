//! Token budget management for context windows.
//!
//! A [`TokenBudget`] partitions a model's total context window into named
//! sections (system prompt, tools, memory, conversation, response reserve),
//! ensuring that the assembled context never exceeds the model's limit.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::{ContextError, ContextResult};

// ---------------------------------------------------------------------------
// SectionAllocation
// ---------------------------------------------------------------------------

/// Token allocation for a single context section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionAllocation {
    /// Maximum tokens allocated to this section.
    pub max_tokens: u32,
    /// Minimum tokens guaranteed to this section (never truncated below this).
    pub min_tokens: u32,
}

impl SectionAllocation {
    /// Create a new allocation with the given maximum and minimum.
    #[must_use]
    pub fn new(max_tokens: u32, min_tokens: u32) -> Self {
        Self {
            max_tokens,
            min_tokens,
        }
    }

    /// Create an allocation where the minimum equals the maximum (fixed).
    #[must_use]
    pub fn fixed(tokens: u32) -> Self {
        Self {
            max_tokens: tokens,
            min_tokens: tokens,
        }
    }
}

// ---------------------------------------------------------------------------
// TokenBudget
// ---------------------------------------------------------------------------

/// Manages token allocation across context sections.
///
/// The budget tracks a total token limit and named allocations for each
/// section. The response reserve is subtracted first, and the remaining
/// tokens are distributed among the content sections.
///
/// # Section names
///
/// By convention the following section names are used:
///
/// - `system_prompt` — the agent's system prompt
/// - `tools` — tool/function descriptions
/// - `memory` — retrieved memory entries
/// - `conversation` — conversation history messages
/// - `response_reserve` — tokens reserved for the model's response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBudget {
    /// Total tokens available for the entire context window (including
    /// response reserve).
    total_tokens: u32,
    /// Per-section allocations.
    allocations: HashMap<String, SectionAllocation>,
}

/// Minimum total budget: 128 tokens.
const MIN_BUDGET: u32 = 128;

impl TokenBudget {
    /// Create a new budget with the given total token limit.
    ///
    /// Returns an error if the total is below the minimum budget.
    pub fn new(total_tokens: u32) -> ContextResult<Self> {
        if total_tokens < MIN_BUDGET {
            return Err(ContextError::BudgetTooSmall {
                budget: total_tokens,
                minimum: MIN_BUDGET,
            });
        }
        Ok(Self {
            total_tokens,
            allocations: HashMap::new(),
        })
    }

    /// Set the allocation for a named section.
    pub fn set_allocation(&mut self, section: &str, allocation: SectionAllocation) {
        self.allocations.insert(section.to_string(), allocation);
    }

    /// Get the allocation for a named section.
    #[must_use]
    pub fn allocation(&self, section: &str) -> Option<&SectionAllocation> {
        self.allocations.get(section)
    }

    /// Return the total token limit.
    #[must_use]
    pub fn total_tokens(&self) -> u32 {
        self.total_tokens
    }

    /// Return the tokens available for content (total minus response reserve).
    #[must_use]
    pub fn content_tokens(&self) -> u32 {
        let reserve = self
            .allocations
            .get("response_reserve")
            .map_or(0, |a| a.max_tokens);
        self.total_tokens.saturating_sub(reserve)
    }

    /// Return the response reserve allocation.
    #[must_use]
    pub fn response_reserve(&self) -> u32 {
        self.allocations
            .get("response_reserve")
            .map_or(0, |a| a.max_tokens)
    }

    /// Return the sum of all section maximum allocations (excluding response
    /// reserve).
    #[must_use]
    pub fn total_allocated(&self) -> u32 {
        self.allocations
            .iter()
            .filter(|(k, _)| k.as_str() != "response_reserve")
            .map(|(_, v)| v.max_tokens)
            .sum()
    }

    /// Check whether the sum of all allocations (including response reserve)
    /// exceeds the total budget.
    #[must_use]
    pub fn is_over_allocated(&self) -> bool {
        let total: u32 = self.allocations.values().map(|a| a.max_tokens).sum();
        total > self.total_tokens
    }

    /// Return all section names and their allocations.
    #[must_use]
    pub fn allocations(&self) -> &HashMap<String, SectionAllocation> {
        &self.allocations
    }

    /// Create a budget with sensible defaults for a given total token limit.
    ///
    /// Default allocation ratios (of content tokens, i.e. total minus reserve):
    /// - `system_prompt`: 15%
    /// - `tools`: 15%
    /// - `memory`: 15%
    /// - `conversation`: 55%
    /// - `response_reserve`: 4096 tokens (or 25% of total if total < 16384)
    pub fn with_defaults(total_tokens: u32) -> ContextResult<Self> {
        let mut budget = Self::new(total_tokens)?;

        let reserve = if total_tokens < 16384 {
            total_tokens / 4
        } else {
            4096
        };
        budget.set_allocation("response_reserve", SectionAllocation::fixed(reserve));

        let content = total_tokens.saturating_sub(reserve);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let system = (f64::from(content) * 0.15) as u32;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let tools = (f64::from(content) * 0.15) as u32;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let memory = (f64::from(content) * 0.15) as u32;
        let conversation = content.saturating_sub(system + tools + memory);

        budget.set_allocation("system_prompt", SectionAllocation::new(system, 64));
        budget.set_allocation("tools", SectionAllocation::new(tools, 0));
        budget.set_allocation("memory", SectionAllocation::new(memory, 0));
        budget.set_allocation("conversation", SectionAllocation::new(conversation, 64));

        Ok(budget)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Context unit tests intentionally panic at the exact fixture or invariant
// boundary that failed so assembly regressions remain easy to diagnose.
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "context unit-test assertions intentionally panic with focused diagnostics"
)]
mod tests {
    use super::*;

    #[test]
    fn new_budget_rejects_too_small() {
        let err = TokenBudget::new(10).unwrap_err();
        assert!(err.to_string().contains("too small"));
    }

    #[test]
    fn new_budget_accepts_minimum() {
        let budget = TokenBudget::new(128).expect("should accept 128");
        assert_eq!(budget.total_tokens(), 128);
    }

    #[test]
    fn set_and_get_allocation() {
        let mut budget = TokenBudget::new(1000).expect("budget");
        budget.set_allocation("system_prompt", SectionAllocation::new(200, 50));
        let alloc = budget.allocation("system_prompt").expect("should exist");
        assert_eq!(alloc.max_tokens, 200);
        assert_eq!(alloc.min_tokens, 50);
    }

    #[test]
    fn missing_allocation_returns_none() {
        let budget = TokenBudget::new(1000).expect("budget");
        assert!(budget.allocation("nonexistent").is_none());
    }

    #[test]
    fn content_tokens_subtracts_reserve() {
        let mut budget = TokenBudget::new(1000).expect("budget");
        budget.set_allocation("response_reserve", SectionAllocation::fixed(200));
        assert_eq!(budget.content_tokens(), 800);
    }

    #[test]
    fn content_tokens_without_reserve() {
        let budget = TokenBudget::new(1000).expect("budget");
        assert_eq!(budget.content_tokens(), 1000);
    }

    #[test]
    fn response_reserve_returns_allocation() {
        let mut budget = TokenBudget::new(1000).expect("budget");
        budget.set_allocation("response_reserve", SectionAllocation::fixed(300));
        assert_eq!(budget.response_reserve(), 300);
    }

    #[test]
    fn is_over_allocated_detects_overflow() {
        let mut budget = TokenBudget::new(200).expect("budget");
        budget.set_allocation("system_prompt", SectionAllocation::new(150, 0));
        budget.set_allocation("tools", SectionAllocation::new(100, 0));
        assert!(budget.is_over_allocated());
    }

    #[test]
    fn is_over_allocated_false_when_fits() {
        let mut budget = TokenBudget::new(1000).expect("budget");
        budget.set_allocation("system_prompt", SectionAllocation::new(200, 0));
        budget.set_allocation("tools", SectionAllocation::new(200, 0));
        assert!(!budget.is_over_allocated());
    }

    #[test]
    fn with_defaults_creates_valid_budget() {
        let budget = TokenBudget::with_defaults(8192).expect("default budget");
        assert_eq!(budget.total_tokens(), 8192);
        assert!(budget.allocation("system_prompt").is_some());
        assert!(budget.allocation("tools").is_some());
        assert!(budget.allocation("memory").is_some());
        assert!(budget.allocation("conversation").is_some());
        assert!(budget.allocation("response_reserve").is_some());
        assert!(!budget.is_over_allocated());
    }

    #[test]
    fn with_defaults_small_budget_uses_percentage_reserve() {
        let budget = TokenBudget::with_defaults(512).expect("small budget");
        assert_eq!(budget.response_reserve(), 128); // 512 / 4
    }

    #[test]
    fn with_defaults_large_budget_uses_fixed_reserve() {
        let budget = TokenBudget::with_defaults(100_000).expect("large budget");
        assert_eq!(budget.response_reserve(), 4096);
    }

    #[test]
    fn total_allocated_excludes_reserve() {
        let mut budget = TokenBudget::new(1000).expect("budget");
        budget.set_allocation("system_prompt", SectionAllocation::new(200, 0));
        budget.set_allocation("response_reserve", SectionAllocation::fixed(300));
        assert_eq!(budget.total_allocated(), 200);
    }

    #[test]
    fn fixed_allocation_has_equal_min_max() {
        let alloc = SectionAllocation::fixed(100);
        assert_eq!(alloc.max_tokens, 100);
        assert_eq!(alloc.min_tokens, 100);
    }
}
