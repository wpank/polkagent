//! Tool registry, handler trait, and execution context.
//!
//! The [`ToolRegistry`] is a named collection of [`ToolHandler`] implementations.
//! The kernel dispatches tool calls from the model executor through this registry,
//! which resolves the handler by name, validates grants, and returns a typed
//! [`ToolResult`].
//!
//! # Grant checking
//!
//! Each [`ToolSpec`] may declare a `required_grant` pattern. When present, the
//! registry verifies that the calling agent's [`ToolContext`] contains a matching
//! [`ResolvedGrant`] before dispatching execution. Tools without a required
//! grant run unconditionally.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tracing::{debug, warn};

use polkagent_config::SecurityConfig;
use polkagent_core::config::DataClassification;
use polkagent_core::ids::{AgentId, ArtifactId, RunId, StepId};
use polkagent_executor_trait::ToolDefinition;
use polkagent_grant::grant::ResolvedGrant;

// ---------------------------------------------------------------------------
// ToolSpec
// ---------------------------------------------------------------------------

/// Metadata describing a single tool available in the registry.
///
/// This is the tool's "card" — name, description, JSON Schema for input, and
/// security metadata. The registry converts these into [`ToolDefinition`]s
/// for the model executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Unique, namespaced tool name (e.g. `"polkagent.file.read"`).
    pub name: String,

    /// Human-readable description shown to the model.
    pub description: String,

    /// JSON Schema for the tool's input parameters.
    pub input_schema: Value,

    /// Grant pattern required to execute this tool, if any.
    ///
    /// When `Some`, the registry checks the [`ToolContext::grants`] for a
    /// matching grant before dispatching. When `None`, the tool executes
    /// without grant checks.
    pub required_grant: Option<String>,

    /// Classification tier for the tool's output data.
    pub output_classification: DataClassification,
}

// ---------------------------------------------------------------------------
// ToolResult
// ---------------------------------------------------------------------------

/// The output produced by a successful tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// Serialized output data.
    pub output: Value,

    /// Classification of the output (inherited or elevated from the tool spec).
    pub classification: DataClassification,

    /// Identifiers of any artifacts created during execution.
    pub artifacts: Vec<ArtifactId>,
}

// ---------------------------------------------------------------------------
// ToolError
// ---------------------------------------------------------------------------

/// Errors that can occur during tool dispatch or execution.
#[derive(Debug, Error)]
pub enum ToolError {
    /// The caller lacks a required grant for this tool.
    #[error("permission denied: {reason}")]
    PermissionDenied {
        /// Why the grant check failed.
        reason: String,
    },

    /// The input provided by the model is invalid.
    #[error("invalid input: {reason}")]
    InvalidInput {
        /// What is wrong with the input.
        reason: String,
    },

    /// The tool failed during execution.
    #[error("execution failed: {reason}")]
    ExecutionFailed {
        /// Description of the failure.
        reason: String,
    },

    /// The tool execution timed out.
    #[error("tool execution timed out after {elapsed_ms}ms")]
    Timeout {
        /// Milliseconds elapsed before the timeout fired.
        elapsed_ms: u64,
    },

    /// The requested tool was not found in the registry.
    #[error("tool not found: {name}")]
    NotFound {
        /// The tool name that was requested.
        name: String,
    },
}

// ---------------------------------------------------------------------------
// ToolContext
// ---------------------------------------------------------------------------

/// Execution context passed to every tool handler invocation.
///
/// Carries identifiers for tracing and auditing, plus the resolved grants
/// available to the calling agent.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// The run this tool call belongs to.
    pub run_id: RunId,

    /// The agent executing this tool.
    pub agent_id: AgentId,

    /// The step within the run.
    pub step_id: StepId,

    /// Grants available to the agent for this invocation.
    pub grants: Vec<ResolvedGrant>,

    /// Security configuration governing filesystem access for this invocation.
    ///
    /// When `None`, file tools apply no path restrictions beyond the OS-level
    /// permissions of the running process. When `Some`, denied_paths and
    /// allowed_paths are enforced before any filesystem operation.
    pub security_config: Option<SecurityConfig>,
}

// ---------------------------------------------------------------------------
// ToolHandler trait
// ---------------------------------------------------------------------------

/// A handler for a single tool.
///
/// Implementations receive JSON input and a [`ToolContext`], and return a
/// [`ToolResult`] on success. The registry dispatches calls to the appropriate
/// handler after performing grant checks.
///
/// # Object safety
///
/// `ToolHandler` is object-safe and stored as `Box<dyn ToolHandler>` in the
/// registry.
#[async_trait]
pub trait ToolHandler: Send + Sync {
    /// Return the tool's specification (name, schema, grant requirements).
    fn spec(&self) -> ToolSpec;

    /// Execute the tool with the given input and context.
    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError>;
}

// ---------------------------------------------------------------------------
// ToolRegistry
// ---------------------------------------------------------------------------

/// A collection of named tool handlers.
///
/// The registry is the single dispatch point for all tool calls in a run. It
/// owns the handlers, resolves names, checks grants, and converts specs into
/// executor-compatible [`ToolDefinition`]s.
pub struct ToolRegistry {
    handlers: HashMap<String, Box<dyn ToolHandler>>,
}

impl ToolRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register a tool handler under the name declared in its spec.
    ///
    /// If a handler with the same name already exists, it is replaced and a
    /// warning is logged.
    pub fn register(&mut self, handler: Box<dyn ToolHandler>) {
        let name = handler.spec().name.clone();
        if self.handlers.contains_key(&name) {
            warn!(tool = %name, "replacing existing tool handler");
        }
        debug!(tool = %name, "tool registered");
        self.handlers.insert(name, handler);
    }

    /// Look up a handler by name.
    pub fn get(&self, name: &str) -> Option<&dyn ToolHandler> {
        self.handlers.get(name).map(|h| h.as_ref())
    }

    /// Return the specs of all registered tools.
    #[must_use]
    pub fn list(&self) -> Vec<ToolSpec> {
        self.handlers.values().map(|h| h.spec()).collect()
    }

    /// Return the names of all registered tools.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    /// Return the number of registered tools.
    #[must_use]
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Return `true` if the registry contains no tools.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    /// Execute a tool by name.
    ///
    /// 1. Resolves the handler.
    /// 2. Checks grants if the tool requires one.
    /// 3. Dispatches to the handler.
    pub async fn execute(
        &self,
        name: &str,
        input: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let handler = self.handlers.get(name).ok_or_else(|| ToolError::NotFound {
            name: name.to_string(),
        })?;

        // Grant check.
        let spec = handler.spec();
        if let Some(ref required) = spec.required_grant {
            let has_grant = context
                .grants
                .iter()
                .any(|g| g.is_valid() && polkagent_grant::pattern_matches(&g.action, required));
            if !has_grant {
                return Err(ToolError::PermissionDenied {
                    reason: format!(
                        "tool '{}' requires grant matching '{}' but none found in context",
                        name, required
                    ),
                });
            }
        }

        debug!(tool = %name, run = %context.run_id, "executing tool");
        handler.execute(input, context).await
    }

    /// Convert all registered tool specs into executor-compatible
    /// [`ToolDefinition`]s.
    ///
    /// This is used when building an [`polkagent_executor_trait::InferenceRequest`] to present the
    /// available tools to the model.
    #[must_use]
    pub fn to_tool_definitions(&self) -> Vec<ToolDefinition> {
        self.handlers
            .values()
            .map(|h| {
                let spec = h.spec();
                ToolDefinition {
                    name: spec.name,
                    description: spec.description,
                    input_schema_json: spec.input_schema.to_string(),
                }
            })
            .collect()
    }
}

impl ToolRegistry {
    /// Execute a batch of tool invocations using the given mode and context.
    ///
    /// This is a convenience method that creates a [`crate::batch::BatchToolExecutor`],
    /// dispatches all invocations, and returns the aggregate result. It
    /// requires wrapping `self` in an `Arc` first; prefer constructing a
    /// [`crate::batch::BatchToolExecutor`] directly if you already have an `Arc<ToolRegistry>`.
    ///
    /// See [`crate::batch::BatchToolExecutor::execute`] for full documentation.
    pub async fn execute_batch(
        self: &Arc<Self>,
        invocations: Vec<crate::batch::ToolInvocation>,
        context: &ToolContext,
        mode: crate::batch::BatchExecutionMode,
    ) -> crate::batch::BatchToolResult {
        let executor = crate::batch::BatchToolExecutor::new(Arc::clone(self));
        executor.execute(invocations, context, mode).await
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tool_count", &self.handlers.len())
            .field("tools", &self.names())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use polkagent_core::ids::GrantId;
    use polkagent_grant::grant::{EffectSet, GrantLimits, ResolvedGrant};

    // -- Helpers --

    /// A minimal test tool that echoes its input.
    struct EchoTool;

    #[async_trait]
    impl ToolHandler for EchoTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "test.echo".to_string(),
                description: "Echoes input back".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "message": { "type": "string" }
                    },
                    "required": ["message"]
                }),
                required_grant: None,
                output_classification: DataClassification::Public,
            }
        }

        async fn execute(
            &self,
            input: Value,
            _context: &ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult {
                output: input,
                classification: DataClassification::Public,
                artifacts: vec![],
            })
        }
    }

    /// A tool that requires a grant.
    struct GrantedTool;

    #[async_trait]
    impl ToolHandler for GrantedTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "test.granted".to_string(),
                description: "Requires a grant".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
                required_grant: Some("test/granted".to_string()),
                output_classification: DataClassification::Internal,
            }
        }

        async fn execute(
            &self,
            _input: Value,
            _context: &ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult {
                output: serde_json::json!({ "status": "ok" }),
                classification: DataClassification::Internal,
                artifacts: vec![],
            })
        }
    }

    fn test_context(grants: Vec<ResolvedGrant>) -> ToolContext {
        ToolContext {
            run_id: RunId::new(),
            agent_id: AgentId::new(),
            step_id: StepId::new(),
            grants,
            security_config: None,
        }
    }

    fn make_grant(action: &str) -> ResolvedGrant {
        ResolvedGrant {
            grant_id: GrantId::new(),
            principal: "test-agent".to_string(),
            action: action.to_string(),
            resource: "**".to_string(),
            allowed_effects: EffectSet::new(vec![action.to_string()]),
            limits: GrantLimits::default(),
            expires_at: Utc::now() + Duration::hours(1),
        }
    }

    // -- Tests --

    #[test]
    fn empty_registry() {
        let reg = ToolRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.list().is_empty());
        assert!(reg.to_tool_definitions().is_empty());
    }

    #[test]
    fn register_and_get() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoTool));

        assert_eq!(reg.len(), 1);
        assert!(!reg.is_empty());
        assert!(reg.get("test.echo").is_some());
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn list_returns_all_specs() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoTool));
        reg.register(Box::new(GrantedTool));

        let specs = reg.list();
        assert_eq!(specs.len(), 2);

        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"test.echo"));
        assert!(names.contains(&"test.granted"));
    }

    #[test]
    fn to_tool_definitions_converts_specs() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoTool));

        let defs = reg.to_tool_definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "test.echo");
        assert_eq!(defs[0].description, "Echoes input back");
        assert!(!defs[0].input_schema_json.is_empty());
    }

    #[tokio::test]
    async fn execute_no_grant_tool() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoTool));

        let ctx = test_context(vec![]);
        let input = serde_json::json!({ "message": "hello" });

        let result = reg.execute("test.echo", input.clone(), &ctx).await;
        assert!(result.is_ok());

        let result = result.unwrap_or_else(|e| panic!("unexpected error: {e}"));
        assert_eq!(result.output, input);
        assert_eq!(result.classification, DataClassification::Public);
    }

    #[tokio::test]
    async fn execute_not_found() {
        let reg = ToolRegistry::new();
        let ctx = test_context(vec![]);

        let result = reg
            .execute("nonexistent", serde_json::json!({}), &ctx)
            .await;
        assert!(matches!(result, Err(ToolError::NotFound { .. })));
    }

    #[tokio::test]
    async fn execute_grant_required_and_missing() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(GrantedTool));

        let ctx = test_context(vec![]); // no grants
        let result = reg
            .execute("test.granted", serde_json::json!({}), &ctx)
            .await;
        assert!(matches!(result, Err(ToolError::PermissionDenied { .. })));
    }

    #[tokio::test]
    async fn execute_grant_required_and_present() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(GrantedTool));

        let ctx = test_context(vec![make_grant("test/granted")]);
        let result = reg
            .execute("test.granted", serde_json::json!({}), &ctx)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn execute_grant_required_but_expired() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(GrantedTool));

        let expired_grant = ResolvedGrant {
            grant_id: GrantId::new(),
            principal: "test-agent".to_string(),
            action: "test/granted".to_string(),
            resource: "**".to_string(),
            allowed_effects: EffectSet::new(vec!["test/granted".to_string()]),
            limits: GrantLimits::default(),
            expires_at: Utc::now() - Duration::hours(1), // expired
        };

        let ctx = test_context(vec![expired_grant]);
        let result = reg
            .execute("test.granted", serde_json::json!({}), &ctx)
            .await;
        assert!(
            matches!(result, Err(ToolError::PermissionDenied { .. })),
            "expired grant must not satisfy grant check"
        );
    }

    #[test]
    fn registry_debug_format() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoTool));
        let debug = format!("{reg:?}");
        assert!(debug.contains("ToolRegistry"));
        assert!(debug.contains("tool_count: 1"));
    }

    #[test]
    fn registry_replace_handler_keeps_count() {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoTool));
        reg.register(Box::new(EchoTool)); // same name
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn tool_error_display() {
        let e = ToolError::PermissionDenied {
            reason: "no grant".to_string(),
        };
        assert!(e.to_string().contains("permission denied"));

        let e = ToolError::NotFound {
            name: "foo".to_string(),
        };
        assert!(e.to_string().contains("foo"));
    }
}
