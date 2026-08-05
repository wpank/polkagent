//! Fluent builder API for constructing context.
//!
//! [`ContextBuilder`] provides a chainable interface for incrementally
//! building up the sections that compose a model's context window.
//!
//! # Example
//!
//! ```rust,no_run
//! use polkagent_context::builder::ContextBuilder;
//! use polkagent_context::budget::TokenBudget;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let budget = TokenBudget::with_defaults(8192)?;
//! let assembled = ContextBuilder::new(budget)
//!     .system_prompt("You are a helpful agent on {{chain_name}}.")
//!     .tool_description("transfer: Transfer tokens between accounts")
//!     .tool_description("balance: Query account balance")
//!     .memory_entry("User prefers dark mode")
//!     .memory_entry("Last transaction was 10 DOT")
//!     .conversation_message("User: Hello!")
//!     .conversation_message("Assistant: Hi there!")
//!     .user_input("What is my balance?")
//!     .build()?;
//! # Ok(())
//! # }
//! ```

use crate::assembler::AssembledContext;
use crate::budget::TokenBudget;
use crate::error::{ContextError, ContextResult};
use crate::estimator::TokenEstimator;
use crate::section::{ContextSection, SectionKind};
use crate::template::TemplateEngine;
use crate::truncation::truncate_to_budget;

// ---------------------------------------------------------------------------
// ContextBuilder
// ---------------------------------------------------------------------------

/// Fluent builder for assembling a context window.
#[derive(Debug)]
pub struct ContextBuilder {
    /// Token budget governing section allocations.
    budget: TokenBudget,
    /// Token estimator (configurable ratio).
    estimator: TokenEstimator,
    /// Template engine for variable resolution.
    template_engine: TemplateEngine,
    /// Raw system prompt text (may contain `{{variables}}`).
    system_prompt: Option<String>,
    /// Tool description strings.
    tool_descriptions: Vec<String>,
    /// Memory entry strings.
    memory_entries: Vec<String>,
    /// Conversation message strings (chronological order).
    conversation_messages: Vec<String>,
    /// The current user input.
    user_input: Option<String>,
}

impl ContextBuilder {
    /// Create a new builder with the given token budget.
    #[must_use]
    pub fn new(budget: TokenBudget) -> Self {
        Self {
            budget,
            estimator: TokenEstimator::default(),
            template_engine: TemplateEngine::new(),
            system_prompt: None,
            tool_descriptions: Vec::new(),
            memory_entries: Vec::new(),
            conversation_messages: Vec::new(),
            user_input: None,
        }
    }

    /// Set a custom token estimator.
    #[must_use]
    pub fn estimator(mut self, estimator: TokenEstimator) -> Self {
        self.estimator = estimator;
        self
    }

    /// Set the template engine for system prompt variable resolution.
    #[must_use]
    pub fn template_engine(mut self, engine: TemplateEngine) -> Self {
        self.template_engine = engine;
        self
    }

    /// Set a template variable for system prompt rendering.
    #[must_use]
    pub fn template_var(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.template_engine.set(name, value);
        self
    }

    /// Set the system prompt text.
    #[must_use]
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Add a single tool description.
    #[must_use]
    pub fn tool_description(mut self, desc: impl Into<String>) -> Self {
        self.tool_descriptions.push(desc.into());
        self
    }

    /// Add multiple tool descriptions at once.
    #[must_use]
    pub fn tools(mut self, descriptions: Vec<String>) -> Self {
        self.tool_descriptions.extend(descriptions);
        self
    }

    /// Add a single memory entry.
    #[must_use]
    pub fn memory_entry(mut self, entry: impl Into<String>) -> Self {
        self.memory_entries.push(entry.into());
        self
    }

    /// Add multiple memory entries at once.
    #[must_use]
    pub fn memory(mut self, entries: Vec<String>) -> Self {
        self.memory_entries.extend(entries);
        self
    }

    /// Add a single conversation message string.
    #[must_use]
    pub fn conversation_message(mut self, msg: impl Into<String>) -> Self {
        self.conversation_messages.push(msg.into());
        self
    }

    /// Add multiple conversation messages at once.
    #[must_use]
    pub fn conversation(mut self, messages: Vec<String>) -> Self {
        self.conversation_messages.extend(messages);
        self
    }

    /// Set the current user input.
    #[must_use]
    pub fn user_input(mut self, input: impl Into<String>) -> Self {
        self.user_input = Some(input.into());
        self
    }

    /// Build the assembled context, applying template rendering and
    /// truncation.
    ///
    /// # Errors
    ///
    /// Returns [`ContextError`] if:
    /// - Template rendering fails (unknown variables).
    /// - The context cannot be truncated to fit the budget (extremely small
    ///   budgets).
    pub fn build(self) -> ContextResult<AssembledContext> {
        let mut sections = Vec::new();

        // 1. System prompt (render templates).
        if let Some(prompt) = &self.system_prompt {
            let rendered = self.template_engine.render(prompt)?;
            sections.push(ContextSection::new(SectionKind::SystemPrompt, rendered));
        }

        // 2. Tool descriptions (joined by blank lines).
        if !self.tool_descriptions.is_empty() {
            let content = self.tool_descriptions.join("\n\n");
            sections.push(ContextSection::new(SectionKind::ToolDescriptions, content));
        }

        // 3. Memory entries (joined by newlines).
        if !self.memory_entries.is_empty() {
            let content = self.memory_entries.join("\n");
            sections.push(ContextSection::new(SectionKind::MemoryContext, content));
        }

        // 4. Conversation history (joined by newlines).
        if !self.conversation_messages.is_empty() {
            let content = self.conversation_messages.join("\n");
            sections.push(ContextSection::new(
                SectionKind::ConversationHistory,
                content,
            ));
        }

        // 5. User input.
        if let Some(input) = &self.user_input {
            sections.push(ContextSection::new(SectionKind::UserInput, input.clone()));
        }

        // Apply truncation to fit within the content budget.
        let content_budget = self.budget.content_tokens();
        let total_tokens = truncate_to_budget(&mut sections, content_budget, &self.estimator);

        // Check if we are still over budget after truncation.
        if total_tokens > content_budget {
            return Err(ContextError::BudgetExceeded {
                total: total_tokens,
                budget: content_budget,
            });
        }

        Ok(AssembledContext {
            sections,
            total_tokens,
            budget: self.budget,
        })
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

    fn budget(total: u32) -> TokenBudget {
        TokenBudget::with_defaults(total).expect("budget")
    }

    #[test]
    fn empty_builder_produces_empty_context() {
        let ctx = ContextBuilder::new(budget(1024)).build().expect("build");
        assert!(ctx.sections.is_empty());
        assert_eq!(ctx.total_tokens, 0);
    }

    #[test]
    fn system_prompt_included() {
        let ctx = ContextBuilder::new(budget(4096))
            .system_prompt("You are helpful.")
            .build()
            .expect("build");
        assert_eq!(ctx.sections.len(), 1);
        assert_eq!(ctx.sections[0].kind, SectionKind::SystemPrompt);
        assert_eq!(ctx.sections[0].content, "You are helpful.");
    }

    #[test]
    fn template_variables_resolved() {
        let ctx = ContextBuilder::new(budget(4096))
            .template_var("agent_name", "TestBot")
            .system_prompt("You are {{agent_name}}.")
            .build()
            .expect("build");
        assert_eq!(ctx.sections[0].content, "You are TestBot.");
    }

    #[test]
    fn unknown_template_variable_errors() {
        let result = ContextBuilder::new(budget(4096))
            .system_prompt("Hello {{unknown_var}}")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn tool_descriptions_joined() {
        let ctx = ContextBuilder::new(budget(4096))
            .tool_description("Tool A")
            .tool_description("Tool B")
            .build()
            .expect("build");
        assert_eq!(ctx.sections.len(), 1);
        assert!(ctx.sections[0].content.contains("Tool A"));
        assert!(ctx.sections[0].content.contains("Tool B"));
        assert!(ctx.sections[0].content.contains("\n\n")); // separated by blank line
    }

    #[test]
    fn memory_entries_joined() {
        let ctx = ContextBuilder::new(budget(4096))
            .memory_entry("Fact 1")
            .memory_entry("Fact 2")
            .build()
            .expect("build");
        assert_eq!(ctx.sections[0].kind, SectionKind::MemoryContext);
        assert!(ctx.sections[0].content.contains("Fact 1\nFact 2"));
    }

    #[test]
    fn conversation_messages_joined() {
        let ctx = ContextBuilder::new(budget(4096))
            .conversation_message("User: Hi")
            .conversation_message("Assistant: Hello!")
            .build()
            .expect("build");
        assert_eq!(ctx.sections[0].kind, SectionKind::ConversationHistory);
    }

    #[test]
    fn user_input_included() {
        let ctx = ContextBuilder::new(budget(4096))
            .user_input("What is my balance?")
            .build()
            .expect("build");
        assert_eq!(ctx.sections[0].kind, SectionKind::UserInput);
        assert_eq!(ctx.sections[0].content, "What is my balance?");
    }

    #[test]
    fn all_sections_included() {
        let ctx = ContextBuilder::new(budget(8192))
            .system_prompt("System prompt text")
            .tool_description("tool1")
            .memory_entry("memory1")
            .conversation_message("msg1")
            .user_input("input")
            .build()
            .expect("build");
        assert_eq!(ctx.sections.len(), 5);
    }

    #[test]
    fn bulk_setters_work() {
        let ctx = ContextBuilder::new(budget(4096))
            .tools(vec!["A".into(), "B".into()])
            .memory(vec!["M1".into(), "M2".into()])
            .conversation(vec!["C1".into(), "C2".into()])
            .build()
            .expect("build");
        assert_eq!(ctx.sections.len(), 3);
    }

    #[test]
    fn total_tokens_is_correct() {
        let ctx = ContextBuilder::new(budget(8192))
            .system_prompt("abcd") // 4 chars = 1 token
            .user_input("abcdefgh") // 8 chars = 2 tokens
            .build()
            .expect("build");
        assert_eq!(ctx.total_tokens, 3);
    }

    #[test]
    fn custom_estimator_applied() {
        let ctx = ContextBuilder::new(budget(8192))
            .estimator(TokenEstimator::new(2.0)) // 2 chars per token
            .system_prompt("abcd") // 4 chars / 2 = 2 tokens
            .build()
            .expect("build");
        assert_eq!(ctx.total_tokens, 2);
    }
}
