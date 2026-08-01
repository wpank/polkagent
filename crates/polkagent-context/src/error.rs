//! Error types for the context assembly layer.

use thiserror::Error;

// ---------------------------------------------------------------------------
// ContextError
// ---------------------------------------------------------------------------

/// Errors that can occur during context assembly.
#[derive(Debug, Error)]
pub enum ContextError {
    /// The total token budget is zero or unreasonably small.
    #[error("token budget too small: {budget} tokens (minimum {minimum})")]
    BudgetTooSmall {
        /// The budget that was provided.
        budget: u32,
        /// The minimum acceptable budget.
        minimum: u32,
    },

    /// A required context section is missing.
    #[error("missing required section: {section}")]
    MissingSectionKind {
        /// Name of the missing section.
        section: String,
    },

    /// An unknown template variable was encountered.
    #[error("unknown template variable: {name}")]
    UnknownVariable {
        /// The variable name that could not be resolved.
        name: String,
    },

    /// A section's estimated token count exceeds its allocation.
    #[error("section {section} exceeds allocation: {estimated} > {allocated} tokens")]
    SectionOverflow {
        /// Name of the overflowing section.
        section: String,
        /// Estimated token count.
        estimated: u32,
        /// Allocated token count.
        allocated: u32,
    },

    /// The assembled context exceeds the total token budget even after
    /// truncation.
    #[error("context exceeds budget after truncation: {total} > {budget} tokens")]
    BudgetExceeded {
        /// Total tokens after truncation.
        total: u32,
        /// Maximum allowed budget.
        budget: u32,
    },

    /// Template rendering failed.
    #[error("template error: {message}")]
    TemplateRender {
        /// Description of the rendering failure.
        message: String,
    },

    /// Serialization/deserialization failure.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Convenience alias for context operations.
pub type ContextResult<T> = Result<T, ContextError>;
