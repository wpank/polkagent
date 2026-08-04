//! `polkagent-context` — Context assembly for Polkagent model execution.
//!
//! This crate builds the system prompt, tool descriptions, memory context, and
//! conversation history into a coherent context window for model execution.
//! It manages a token budget to ensure the assembled context fits within a
//! model's context limit, truncating lower-priority sections first when the
//! budget is exceeded.
//!
//! # Architecture
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`assembler`] | [`ContextAssembler`]: high-level entry point |
//! | [`builder`] | [`ContextBuilder`]: fluent API for incremental construction |
//! | [`budget`] | [`TokenBudget`]: per-section token allocation |
//! | [`section`] | [`ContextSection`], [`SectionKind`]: typed content blocks |
//! | [`template`] | [`TemplateEngine`]: `{{variable}}` resolution |
//! | [`truncation`] | [`TruncationStrategy`]: overflow handling |
//! | [`estimator`] | [`TokenEstimator`]: character-based token counting |
//! | [`error`] | [`ContextError`] enum |
//!
//! # Quickstart
//!
//! ```rust,no_run
//! use polkagent_context::ContextAssembler;
//! use polkagent_context::budget::TokenBudget;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let budget = TokenBudget::with_defaults(8192)?;
//! let mut assembler = ContextAssembler::new(budget);
//! assembler.set_template_var("agent_name", "PolkaBot");
//!
//! let context = assembler.assemble(
//!     Some("You are {{agent_name}}, a helpful blockchain agent."),
//!     &["transfer: Transfer tokens", "balance: Query balance"],
//!     &["User prefers metric units"],
//!     &["User: Hello!", "Assistant: Hi there!"],
//!     Some("What is my balance?"),
//! )?;
//!
//! println!("Tokens used: {}/{}", context.total_tokens, context.budget.content_tokens());
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(
    missing_docs,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used
)]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod assembler;
pub mod budget;
pub mod builder;
pub mod error;
pub mod estimator;
pub mod section;
pub mod template;
pub mod truncation;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use assembler::{AssembledContext, AssemblerConfig, ContextAssembler};
pub use budget::{SectionAllocation, TokenBudget};
pub use builder::ContextBuilder;
pub use error::{ContextError, ContextResult};
pub use estimator::TokenEstimator;
pub use section::{ContextSection, SectionKind};
pub use template::TemplateEngine;
pub use truncation::TruncationStrategy;
