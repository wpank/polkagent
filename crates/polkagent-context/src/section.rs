//! Context sections with priorities and token estimation.
//!
//! A [`ContextSection`] represents a logical block of content within the
//! assembled context (system prompt, tool descriptions, memory entries,
//! conversation history, or user input). Each section has a [`SectionKind`]
//! that determines its default priority for truncation.

use serde::{Deserialize, Serialize};

use crate::estimator::TokenEstimator;

// ---------------------------------------------------------------------------
// SectionKind
// ---------------------------------------------------------------------------

/// The logical type of a context section.
///
/// Priority ordering (highest to lowest):
/// 1. `SystemPrompt` — always retained if possible
/// 2. `UserInput` — the current user message
/// 3. `ConversationHistory` — prior messages
/// 4. `MemoryContext` — retrieved memory entries
/// 5. `ToolDescriptions` — available tool schemas
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SectionKind {
    /// Available tool/function descriptions.
    ToolDescriptions = 0,
    /// Retrieved memory entries.
    MemoryContext = 1,
    /// Prior conversation messages.
    ConversationHistory = 2,
    /// The current user message.
    UserInput = 3,
    /// The agent's system prompt.
    SystemPrompt = 4,
}

impl SectionKind {
    /// Return the default priority for this section kind.
    ///
    /// Higher values mean higher priority (less likely to be truncated).
    #[must_use]
    pub fn default_priority(&self) -> u8 {
        match self {
            Self::SystemPrompt => 100,
            Self::UserInput => 90,
            Self::ConversationHistory => 70,
            Self::MemoryContext => 50,
            Self::ToolDescriptions => 30,
        }
    }

    /// Return the section name string used in budget allocation keys.
    #[must_use]
    pub fn budget_key(&self) -> &'static str {
        match self {
            Self::SystemPrompt => "system_prompt",
            Self::UserInput => "conversation",
            Self::ConversationHistory => "conversation",
            Self::MemoryContext => "memory",
            Self::ToolDescriptions => "tools",
        }
    }
}

// ---------------------------------------------------------------------------
// ContextSection
// ---------------------------------------------------------------------------

/// A single section of assembled context content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSection {
    /// The logical type of this section.
    pub kind: SectionKind,
    /// The text content of this section.
    pub content: String,
    /// Override priority (if `None`, uses [`SectionKind::default_priority`]).
    pub priority: Option<u8>,
    /// Cached estimated token count (lazily computed).
    pub estimated_tokens: Option<u32>,
}

impl ContextSection {
    /// Create a new section with the given kind and content.
    #[must_use]
    pub fn new(kind: SectionKind, content: impl Into<String>) -> Self {
        Self {
            kind,
            content: content.into(),
            priority: None,
            estimated_tokens: None,
        }
    }

    /// Set an explicit priority override.
    #[must_use]
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = Some(priority);
        self
    }

    /// Set a precomputed token estimate.
    #[must_use]
    pub fn with_estimated_tokens(mut self, tokens: u32) -> Self {
        self.estimated_tokens = Some(tokens);
        self
    }

    /// Return this section's effective priority.
    #[must_use]
    pub fn effective_priority(&self) -> u8 {
        self.priority.unwrap_or_else(|| self.kind.default_priority())
    }

    /// Estimate the token count, using the cached value if available.
    #[must_use]
    pub fn tokens(&self, estimator: &TokenEstimator) -> u32 {
        self.estimated_tokens
            .unwrap_or_else(|| estimator.estimate(&self.content))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_kind_priority_order() {
        assert!(SectionKind::SystemPrompt.default_priority() > SectionKind::UserInput.default_priority());
        assert!(SectionKind::UserInput.default_priority() > SectionKind::ConversationHistory.default_priority());
        assert!(SectionKind::ConversationHistory.default_priority() > SectionKind::MemoryContext.default_priority());
        assert!(SectionKind::MemoryContext.default_priority() > SectionKind::ToolDescriptions.default_priority());
    }

    #[test]
    fn new_section_uses_kind_defaults() {
        let section = ContextSection::new(SectionKind::SystemPrompt, "You are a helpful agent.");
        assert_eq!(section.kind, SectionKind::SystemPrompt);
        assert_eq!(section.effective_priority(), 100);
        assert!(section.priority.is_none());
    }

    #[test]
    fn with_priority_overrides_default() {
        let section = ContextSection::new(SectionKind::ToolDescriptions, "tools")
            .with_priority(99);
        assert_eq!(section.effective_priority(), 99);
    }

    #[test]
    fn with_estimated_tokens_caches_value() {
        let section = ContextSection::new(SectionKind::MemoryContext, "some memory")
            .with_estimated_tokens(42);
        let est = TokenEstimator::default();
        assert_eq!(section.tokens(&est), 42);
    }

    #[test]
    fn tokens_computes_when_not_cached() {
        let section = ContextSection::new(SectionKind::UserInput, "abcdefgh"); // 8 chars / 4 = 2
        let est = TokenEstimator::default();
        assert_eq!(section.tokens(&est), 2);
    }

    #[test]
    fn budget_key_maps_correctly() {
        assert_eq!(SectionKind::SystemPrompt.budget_key(), "system_prompt");
        assert_eq!(SectionKind::UserInput.budget_key(), "conversation");
        assert_eq!(SectionKind::ConversationHistory.budget_key(), "conversation");
        assert_eq!(SectionKind::MemoryContext.budget_key(), "memory");
        assert_eq!(SectionKind::ToolDescriptions.budget_key(), "tools");
    }

    #[test]
    fn section_kind_serde_round_trip() {
        for kind in [
            SectionKind::SystemPrompt,
            SectionKind::UserInput,
            SectionKind::ConversationHistory,
            SectionKind::MemoryContext,
            SectionKind::ToolDescriptions,
        ] {
            let json = serde_json::to_string(&kind).expect("serialize");
            let back: SectionKind = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(kind, back);
        }
    }
}
