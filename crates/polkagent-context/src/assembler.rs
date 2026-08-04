//! Context assembler — builds complete context from component parts.
//!
//! [`ContextAssembler`] is the high-level entry point for constructing a
//! model's context window. It combines a system prompt, tool descriptions,
//! memory context, conversation history, and user input into a single
//! [`AssembledContext`] that respects the token budget.

use serde::{Deserialize, Serialize};
use tracing::info;

use crate::budget::TokenBudget;
use crate::builder::ContextBuilder;
use crate::error::ContextResult;
use crate::estimator::TokenEstimator;
use crate::section::ContextSection;
use crate::template::TemplateEngine;

// ---------------------------------------------------------------------------
// AssembledContext
// ---------------------------------------------------------------------------

/// The fully assembled context ready for model execution.
///
/// Contains the ordered sections, the total estimated token count, and the
/// budget that was used during assembly.
#[derive(Debug, Clone)]
pub struct AssembledContext {
    /// The sections that make up the context, in presentation order.
    pub sections: Vec<ContextSection>,
    /// Total estimated tokens across all sections.
    pub total_tokens: u32,
    /// The budget used during assembly.
    pub budget: TokenBudget,
}

impl AssembledContext {
    /// Render the full context as a single string with section separators.
    #[must_use]
    pub fn render(&self) -> String {
        self.sections
            .iter()
            .map(|s| s.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Return only the system prompt section content, if present.
    #[must_use]
    pub fn system_prompt(&self) -> Option<&str> {
        self.sections
            .iter()
            .find(|s| s.kind == crate::section::SectionKind::SystemPrompt)
            .map(|s| s.content.as_str())
    }

    /// Return only the user input section content, if present.
    #[must_use]
    pub fn user_input(&self) -> Option<&str> {
        self.sections
            .iter()
            .find(|s| s.kind == crate::section::SectionKind::UserInput)
            .map(|s| s.content.as_str())
    }

    /// Return the number of tokens remaining before the content budget is
    /// exhausted.
    #[must_use]
    pub fn remaining_tokens(&self) -> u32 {
        self.budget
            .content_tokens()
            .saturating_sub(self.total_tokens)
    }

    /// Return the number of sections in the assembled context.
    #[must_use]
    pub fn section_count(&self) -> usize {
        self.sections.len()
    }
}

// ---------------------------------------------------------------------------
// ContextAssembler
// ---------------------------------------------------------------------------

/// High-level assembler that orchestrates context construction.
///
/// Wraps a [`TokenBudget`], [`TokenEstimator`], and [`TemplateEngine`] to
/// provide a convenient `assemble` method. For one-shot construction,
/// prefer [`ContextBuilder`] directly.
///
/// # Example
///
/// ```rust,no_run
/// use polkagent_context::assembler::ContextAssembler;
/// use polkagent_context::budget::TokenBudget;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let budget = TokenBudget::with_defaults(8192)?;
/// let assembler = ContextAssembler::new(budget);
/// let context = assembler.assemble(
///     Some("You are {{agent_name}}."),
///     &["transfer: Transfer tokens"],
///     &["User prefers dark mode"],
///     &["User: Hi", "Assistant: Hello!"],
///     Some("What is my balance?"),
/// )?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct ContextAssembler {
    /// The token budget governing assembly.
    budget: TokenBudget,
    /// Token estimator.
    estimator: TokenEstimator,
    /// Template engine for system prompt variable resolution.
    template_engine: TemplateEngine,
}

impl ContextAssembler {
    /// Create a new assembler with the given budget and default settings.
    #[must_use]
    pub fn new(budget: TokenBudget) -> Self {
        Self {
            budget,
            estimator: TokenEstimator::default(),
            template_engine: TemplateEngine::new(),
        }
    }

    /// Set a custom token estimator.
    #[must_use]
    pub fn with_estimator(mut self, estimator: TokenEstimator) -> Self {
        self.estimator = estimator;
        self
    }

    /// Set a custom template engine.
    #[must_use]
    pub fn with_template_engine(mut self, engine: TemplateEngine) -> Self {
        self.template_engine = engine;
        self
    }

    /// Set a template variable.
    pub fn set_template_var(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.template_engine.set(name, value);
    }

    /// Return a reference to the budget.
    #[must_use]
    pub fn budget(&self) -> &TokenBudget {
        &self.budget
    }

    /// Return a reference to the estimator.
    #[must_use]
    pub fn estimator(&self) -> &TokenEstimator {
        &self.estimator
    }

    /// Assemble a complete context from the provided parts.
    ///
    /// This is the primary entry point. It delegates to [`ContextBuilder`]
    /// internally, applying template rendering and truncation.
    ///
    /// # Errors
    ///
    /// Returns [`ContextError`](crate::error::ContextError) on template
    /// errors or budget violations.
    pub fn assemble(
        &self,
        system_prompt: Option<&str>,
        tool_descriptions: &[&str],
        memory_entries: &[&str],
        conversation_messages: &[&str],
        user_input: Option<&str>,
    ) -> ContextResult<AssembledContext> {
        let mut builder = ContextBuilder::new(self.budget.clone())
            .estimator(self.estimator.clone())
            .template_engine(self.template_engine.clone());

        if let Some(prompt) = system_prompt {
            builder = builder.system_prompt(prompt);
        }

        if !tool_descriptions.is_empty() {
            builder = builder.tools(tool_descriptions.iter().map(|s| (*s).to_string()).collect());
        }

        if !memory_entries.is_empty() {
            builder = builder.memory(memory_entries.iter().map(|s| (*s).to_string()).collect());
        }

        if !conversation_messages.is_empty() {
            builder = builder.conversation(
                conversation_messages
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
            );
        }

        if let Some(input) = user_input {
            builder = builder.user_input(input);
        }

        let context = builder.build()?;

        info!(
            total_tokens = context.total_tokens,
            section_count = context.section_count(),
            remaining = context.remaining_tokens(),
            "assembled context"
        );

        Ok(context)
    }
}

// ---------------------------------------------------------------------------
// AssemblerConfig (serializable configuration)
// ---------------------------------------------------------------------------

/// Serializable configuration for creating a [`ContextAssembler`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssemblerConfig {
    /// Total token budget for the context window.
    pub total_tokens: u32,
    /// Characters-per-token ratio for estimation.
    #[serde(default = "default_chars_per_token")]
    pub chars_per_token: f64,
    /// Whether the template engine should allow unknown variables.
    #[serde(default)]
    pub allow_unknown_variables: bool,
}

fn default_chars_per_token() -> f64 {
    4.0
}

impl AssemblerConfig {
    /// Build a [`ContextAssembler`] from this configuration.
    pub fn build(self) -> ContextResult<ContextAssembler> {
        let budget = TokenBudget::with_defaults(self.total_tokens)?;
        let estimator = TokenEstimator::new(self.chars_per_token);
        let mut template_engine = TemplateEngine::new();
        template_engine.set_allow_unknown(self.allow_unknown_variables);

        Ok(ContextAssembler {
            budget,
            estimator,
            template_engine,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assemble_empty_context() {
        let budget = TokenBudget::with_defaults(4096).expect("budget");
        let assembler = ContextAssembler::new(budget);
        let ctx = assembler
            .assemble(None, &[], &[], &[], None)
            .expect("assemble");
        assert_eq!(ctx.section_count(), 0);
        assert_eq!(ctx.total_tokens, 0);
    }

    #[test]
    fn assemble_full_context() {
        let budget = TokenBudget::with_defaults(8192).expect("budget");
        let mut assembler = ContextAssembler::new(budget);
        assembler.set_template_var("agent_name", "TestBot");
        let ctx = assembler
            .assemble(
                Some("You are {{agent_name}}."),
                &["tool1: do stuff"],
                &["memory fact"],
                &["User: hello"],
                Some("What can you do?"),
            )
            .expect("assemble");
        assert_eq!(ctx.section_count(), 5);
        assert_eq!(ctx.system_prompt(), Some("You are TestBot."));
        assert_eq!(ctx.user_input(), Some("What can you do?"));
        assert!(ctx.total_tokens > 0);
        assert!(ctx.remaining_tokens() > 0);
    }

    #[test]
    fn render_produces_combined_text() {
        let budget = TokenBudget::with_defaults(8192).expect("budget");
        let assembler = ContextAssembler::new(budget);
        let ctx = assembler
            .assemble(Some("System prompt."), &[], &[], &[], Some("User input."))
            .expect("assemble");
        let rendered = ctx.render();
        assert!(rendered.contains("System prompt."));
        assert!(rendered.contains("User input."));
    }

    #[test]
    fn assembler_config_round_trip() {
        let config = AssemblerConfig {
            total_tokens: 4096,
            chars_per_token: 3.5,
            allow_unknown_variables: true,
        };
        let json = serde_json::to_string(&config).expect("serialize");
        let back: AssemblerConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.total_tokens, 4096);
        assert!((back.chars_per_token - 3.5).abs() < f64::EPSILON);
        assert!(back.allow_unknown_variables);
    }

    #[test]
    fn assembler_config_build() {
        let config = AssemblerConfig {
            total_tokens: 4096,
            chars_per_token: 4.0,
            allow_unknown_variables: false,
        };
        let assembler = config.build().expect("build");
        assert_eq!(assembler.budget().total_tokens(), 4096);
    }

    #[test]
    fn with_estimator_changes_estimation() {
        let budget = TokenBudget::with_defaults(8192).expect("budget");
        let assembler = ContextAssembler::new(budget).with_estimator(TokenEstimator::new(2.0));
        let ctx = assembler
            .assemble(Some("abcdefgh"), &[], &[], &[], None)
            .expect("assemble");
        // 8 chars / 2.0 = 4 tokens
        assert_eq!(ctx.total_tokens, 4);
    }

    #[test]
    fn remaining_tokens_computed_correctly() {
        let budget = TokenBudget::with_defaults(8192).expect("budget");
        let assembler = ContextAssembler::new(budget.clone());
        let ctx = assembler
            .assemble(Some("abcd"), &[], &[], &[], None)
            .expect("assemble");
        let expected = budget.content_tokens() - 1; // "abcd" = 1 token
        assert_eq!(ctx.remaining_tokens(), expected);
    }
}
