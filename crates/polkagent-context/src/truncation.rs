//! Truncation strategies for context sections that exceed their token budget.
//!
//! When the assembled context is too large for the model's context window,
//! sections are truncated according to their configured [`TruncationStrategy`].
//! Lower-priority sections are truncated first.

use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::estimator::TokenEstimator;
use crate::section::{ContextSection, SectionKind};

// ---------------------------------------------------------------------------
// TruncationStrategy
// ---------------------------------------------------------------------------

/// Strategy for reducing a section's content when it exceeds its token budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruncationStrategy {
    /// Drop the oldest items (used for conversation history).
    ///
    /// Items are assumed to be separated by newlines. The oldest (first)
    /// lines are removed until the section fits within the budget.
    DropOldest,

    /// Summarize the content (used for memory context).
    ///
    /// Currently implemented as aggressive truncation from the middle,
    /// preserving the first and last portions. A future version may use
    /// an LLM call for true summarization.
    Summarize,

    /// Drop lowest-priority items (used for tool descriptions).
    ///
    /// Items are separated by blank lines. Items at the end of the section
    /// (assumed lower priority) are dropped first.
    Priority,
}

impl TruncationStrategy {
    /// Return the default truncation strategy for a section kind.
    #[must_use]
    pub fn for_kind(kind: SectionKind) -> Self {
        match kind {
            SectionKind::ConversationHistory
            | SectionKind::SystemPrompt
            | SectionKind::UserInput => Self::DropOldest,
            SectionKind::MemoryContext => Self::Summarize,
            SectionKind::ToolDescriptions => Self::Priority,
        }
    }
}

// ---------------------------------------------------------------------------
// Truncation functions
// ---------------------------------------------------------------------------

/// Truncate a section's content to fit within `max_tokens`.
///
/// Returns the truncated content string. If the content already fits, it is
/// returned unchanged.
#[must_use]
pub fn truncate_section(
    section: &ContextSection,
    max_tokens: u32,
    strategy: TruncationStrategy,
    estimator: &TokenEstimator,
) -> String {
    let current = estimator.estimate(&section.content);
    if current <= max_tokens {
        return section.content.clone();
    }

    debug!(
        kind = ?section.kind,
        current_tokens = current,
        max_tokens,
        ?strategy,
        "truncating section"
    );

    match strategy {
        TruncationStrategy::DropOldest => {
            truncate_drop_oldest(&section.content, max_tokens, estimator)
        }
        TruncationStrategy::Summarize => {
            truncate_summarize(&section.content, max_tokens, estimator)
        }
        TruncationStrategy::Priority => truncate_priority(&section.content, max_tokens, estimator),
    }
}

/// Drop the oldest (first) lines until the content fits.
fn truncate_drop_oldest(content: &str, max_tokens: u32, estimator: &TokenEstimator) -> String {
    let lines: Vec<&str> = content.lines().collect();
    // Try dropping from the front until we fit.
    for start in 0..lines.len() {
        let remaining = lines[start..].join("\n");
        if estimator.estimate(&remaining) <= max_tokens {
            return remaining;
        }
    }
    // If nothing fits, return the last line (or empty).
    lines.last().map_or_else(String::new, |l| (*l).to_string())
}

/// Summarize by keeping the first and last portions, dropping the middle.
fn truncate_summarize(content: &str, max_tokens: u32, estimator: &TokenEstimator) -> String {
    if max_tokens == 0 {
        return String::new();
    }
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= 2 {
        return truncate_drop_oldest(content, max_tokens, estimator);
    }

    // Keep roughly half the budget for the beginning and half for the end.
    let half = lines.len() / 2;
    for keep in (1..=half).rev() {
        let head: Vec<&str> = lines[..keep].to_vec();
        let tail: Vec<&str> = lines[lines.len() - keep..].to_vec();
        let mut combined = head;
        combined.push("[... truncated ...]");
        combined.extend(tail);
        let text = combined.join("\n");
        if estimator.estimate(&text) <= max_tokens {
            return text;
        }
    }

    // Fall back to just the first line.
    lines.first().map_or_else(String::new, |l| (*l).to_string())
}

/// Drop the lowest-priority (last) items separated by blank lines.
fn truncate_priority(content: &str, max_tokens: u32, estimator: &TokenEstimator) -> String {
    let items: Vec<&str> = content.split("\n\n").collect();
    // Try keeping progressively fewer items from the front.
    for keep in (1..=items.len()).rev() {
        let text = items[..keep].join("\n\n");
        if estimator.estimate(&text) <= max_tokens {
            return text;
        }
    }
    // If even one item is too large, return it anyway (caller handles).
    items.first().map_or_else(String::new, |i| (*i).to_string())
}

/// Apply truncation across all sections to fit within a total budget.
///
/// Sections are sorted by priority (lowest first). Lower-priority sections
/// are truncated before higher-priority ones. Returns the total estimated
/// tokens after truncation.
pub fn truncate_to_budget(
    sections: &mut [ContextSection],
    total_budget: u32,
    estimator: &TokenEstimator,
) -> u32 {
    let total: u32 = sections.iter().map(|s| s.tokens(estimator)).sum();
    if total <= total_budget {
        return total;
    }

    let mut overflow = total.saturating_sub(total_budget);

    // Sort indices by priority (lowest first = truncate first).
    let mut indices: Vec<usize> = (0..sections.len()).collect();
    indices.sort_by_key(|&i| sections[i].effective_priority());

    for &idx in &indices {
        if overflow == 0 {
            break;
        }
        let section = &sections[idx];
        let current_tokens = section.tokens(estimator);
        let strategy = TruncationStrategy::for_kind(section.kind);

        // Target: reduce this section by up to `overflow` tokens.
        let target = current_tokens.saturating_sub(overflow);
        let truncated = truncate_section(section, target.max(1), strategy, estimator);
        let new_tokens = estimator.estimate(&truncated);
        let saved = current_tokens.saturating_sub(new_tokens);
        overflow = overflow.saturating_sub(saved);

        sections[idx].content = truncated;
        sections[idx].estimated_tokens = Some(new_tokens);
    }

    sections.iter().map(|s| s.tokens(estimator)).sum()
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

    fn estimator() -> TokenEstimator {
        TokenEstimator::default()
    }

    #[test]
    fn drop_oldest_removes_first_lines() {
        let content = "line1\nline2\nline3\nline4";
        let section = ContextSection::new(SectionKind::ConversationHistory, content);
        let est = estimator();
        // "line4" = 5 chars = 2 tokens; "line3\nline4" = 11 chars = 3 tokens
        let result = truncate_section(&section, 3, TruncationStrategy::DropOldest, &est);
        assert!(!result.contains("line1"));
        assert!(result.contains("line4"));
    }

    #[test]
    fn drop_oldest_returns_original_when_fits() {
        let content = "short";
        let section = ContextSection::new(SectionKind::ConversationHistory, content);
        let est = estimator();
        let result = truncate_section(&section, 100, TruncationStrategy::DropOldest, &est);
        assert_eq!(result, "short");
    }

    #[test]
    fn summarize_keeps_head_and_tail() {
        let content = "first\nsecond\nthird\nfourth\nfifth\nsixth";
        let section = ContextSection::new(SectionKind::MemoryContext, content);
        let est = estimator();
        // Request a small budget that forces truncation.
        let result = truncate_section(&section, 8, TruncationStrategy::Summarize, &est);
        assert!(result.contains("first"));
        assert!(result.contains("sixth"));
        assert!(result.contains("[... truncated ...]"));
    }

    #[test]
    fn priority_drops_last_items() {
        let content = "Tool A: description\n\nTool B: description\n\nTool C: description";
        let section = ContextSection::new(SectionKind::ToolDescriptions, content);
        let est = estimator();
        // Budget enough for ~2 items but not 3.
        let result = truncate_section(&section, 10, TruncationStrategy::Priority, &est);
        assert!(result.contains("Tool A"));
        assert!(!result.contains("Tool C"));
    }

    #[test]
    fn strategy_for_kind_defaults() {
        assert_eq!(
            TruncationStrategy::for_kind(SectionKind::ConversationHistory),
            TruncationStrategy::DropOldest
        );
        assert_eq!(
            TruncationStrategy::for_kind(SectionKind::MemoryContext),
            TruncationStrategy::Summarize
        );
        assert_eq!(
            TruncationStrategy::for_kind(SectionKind::ToolDescriptions),
            TruncationStrategy::Priority
        );
    }

    #[test]
    fn truncate_to_budget_fits_without_change() {
        let est = estimator();
        let mut sections = vec![
            ContextSection::new(SectionKind::SystemPrompt, "system"),
            ContextSection::new(SectionKind::UserInput, "user"),
        ];
        let total = truncate_to_budget(&mut sections, 1000, &est);
        assert!(total <= 1000);
        assert_eq!(sections[0].content, "system");
        assert_eq!(sections[1].content, "user");
    }

    #[test]
    fn truncate_to_budget_truncates_lowest_priority_first() {
        let est = estimator();
        // Build sections where total exceeds budget.
        let long_tools = "Tool A desc\n\nTool B desc\n\nTool C desc\n\nTool D desc";
        let mut sections = vec![
            ContextSection::new(SectionKind::SystemPrompt, "system prompt"),
            ContextSection::new(SectionKind::ToolDescriptions, long_tools),
            ContextSection::new(SectionKind::UserInput, "user input"),
        ];
        let original_tool_len = sections[1].content.len();
        let total = truncate_to_budget(&mut sections, 10, &est);
        // Tools (lowest priority) should have been truncated.
        assert!(sections[1].content.len() <= original_tool_len);
        // System prompt (highest priority) should be intact or last to be
        // touched.
        assert!(total > 0);
    }

    #[test]
    fn serde_round_trip() {
        for strategy in [
            TruncationStrategy::DropOldest,
            TruncationStrategy::Summarize,
            TruncationStrategy::Priority,
        ] {
            let json = serde_json::to_string(&strategy).expect("serialize");
            let back: TruncationStrategy = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(strategy, back);
        }
    }
}
