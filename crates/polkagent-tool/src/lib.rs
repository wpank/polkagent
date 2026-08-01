//! `polkagent-tool` — Tool registry, handler trait, and built-in tool
//! implementations for the Polkagent platform.
//!
//! This crate provides the tool dispatch layer that sits between the model
//! executor and the agent kernel. When a model requests a tool call, the kernel
//! resolves it through the [`ToolRegistry`], which:
//!
//! 1. Looks up the [`ToolHandler`] by name.
//! 2. Checks that the calling agent holds a valid grant (if the tool requires
//!    one).
//! 3. Dispatches execution and returns a typed [`ToolResult`].
//!
//! # Built-in tools
//!
//! The [`builtin`] module ships file, shell, and memory-search tools. Register
//! them all with [`builtin::register_builtins`].
//!
//! # Custom tools
//!
//! Implement [`ToolHandler`] and register via [`ToolRegistry::register`]:
//!
//! ```rust,no_run
//! use async_trait::async_trait;
//! use serde_json::Value;
//! use polkagent_core::config::DataClassification;
//! use polkagent_tool::registry::{ToolContext, ToolError, ToolHandler, ToolResult, ToolSpec};
//!
//! struct MyTool;
//!
//! #[async_trait]
//! impl ToolHandler for MyTool {
//!     fn spec(&self) -> ToolSpec {
//!         ToolSpec {
//!             name: "my.custom.tool".to_string(),
//!             description: "Does something custom.".to_string(),
//!             input_schema: serde_json::json!({ "type": "object" }),
//!             required_grant: None,
//!             output_classification: DataClassification::Public,
//!         }
//!     }
//!
//!     async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
//!         Ok(ToolResult {
//!             output: serde_json::json!({ "status": "done" }),
//!             classification: DataClassification::Public,
//!             artifacts: vec![],
//!         })
//!     }
//! }
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

pub mod batch;
pub mod builtin;
pub mod registry;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use batch::{BatchExecutionMode, BatchToolExecutor, BatchToolResult, ToolInvocation, ToolInvocationResult};
pub use builtin::register_builtins;
pub use registry::{ToolContext, ToolError, ToolHandler, ToolRegistry, ToolResult, ToolSpec};
