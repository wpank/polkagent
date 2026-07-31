//! Tool execution integration tests.
//!
//! Exercises ToolRegistry registration, grant-gated execution, rejection
//! without grant, and output capture across handler implementations.

use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde_json::{json, Value};

use polkagent_core::config::DataClassification;
use polkagent_core::ids::{AgentId, GrantId, RunId, StepId};
use polkagent_grant::grant::{EffectSet, GrantLimits, ResolvedGrant};
use polkagent_tool::{ToolContext, ToolError, ToolHandler, ToolRegistry, ToolResult, ToolSpec};

// ---------------------------------------------------------------------------
// Test tool implementations
// ---------------------------------------------------------------------------

/// A tool that echoes its JSON input back.
struct EchoTool;

#[async_trait]
impl ToolHandler for EchoTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "test.echo".to_string(),
            description: "Echoes input back as output.".to_string(),
            input_schema: json!({ "type": "object" }),
            required_grant: None,
            output_classification: DataClassification::Public,
        }
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            output: input,
            classification: DataClassification::Public,
            artifacts: vec![],
        })
    }
}

/// A tool that requires a specific grant.
struct PrivilegedTool;

#[async_trait]
impl ToolHandler for PrivilegedTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "test.privileged".to_string(),
            description: "Requires a grant to execute.".to_string(),
            input_schema: json!({ "type": "object" }),
            required_grant: Some("test/privileged-action".to_string()),
            output_classification: DataClassification::Internal,
        }
    }

    async fn execute(&self, _input: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            output: json!({ "status": "privileged-ok" }),
            classification: DataClassification::Internal,
            artifacts: vec![],
        })
    }
}

/// A tool that always fails during execution.
struct FailingTool;

#[async_trait]
impl ToolHandler for FailingTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "test.failing".to_string(),
            description: "Always fails.".to_string(),
            input_schema: json!({ "type": "object" }),
            required_grant: None,
            output_classification: DataClassification::Public,
        }
    }

    async fn execute(&self, _input: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        Err(ToolError::ExecutionFailed {
            reason: "intentional failure".to_string(),
        })
    }
}

/// A tool that captures the ToolContext fields.
struct ContextCaptureTool {
    captured: std::sync::Mutex<Option<ToolContext>>,
}

impl ContextCaptureTool {
    fn new() -> Self {
        Self {
            captured: std::sync::Mutex::new(None),
        }
    }

    fn last_context(&self) -> Option<ToolContext> {
        self.captured.lock().expect("lock").clone()
    }
}

#[async_trait]
impl ToolHandler for ContextCaptureTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "test.ctx".to_string(),
            description: "Captures context.".to_string(),
            input_schema: json!({ "type": "object" }),
            required_grant: None,
            output_classification: DataClassification::Public,
        }
    }

    async fn execute(&self, _input: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        *self.captured.lock().expect("lock") = Some(ctx.clone());
        Ok(ToolResult {
            output: json!({ "captured": true }),
            classification: DataClassification::Public,
            artifacts: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn empty_ctx() -> ToolContext {
    ToolContext {
        run_id: RunId::new(),
        agent_id: AgentId::new(),
        step_id: StepId::new(),
        grants: vec![],
    }
}

fn ctx_with_grant(action: &str) -> ToolContext {
    let grant = ResolvedGrant {
        grant_id: GrantId::new(),
        principal: "test-agent".to_string(),
        action: action.to_string(),
        resource: "**".to_string(),
        allowed_effects: EffectSet::new(vec![action.to_string()]),
        limits: GrantLimits::default(),
        expires_at: Utc::now() + Duration::hours(1),
    };
    ToolContext {
        grants: vec![grant],
        ..empty_ctx()
    }
}

fn make_expired_grant(action: &str) -> ResolvedGrant {
    ResolvedGrant {
        grant_id: GrantId::new(),
        principal: "test-agent".to_string(),
        action: action.to_string(),
        resource: "**".to_string(),
        allowed_effects: EffectSet::new(vec![action.to_string()]),
        limits: GrantLimits::default(),
        expires_at: Utc::now() - Duration::hours(1),
    }
}

// ---------------------------------------------------------------------------
// Registration tests
// ---------------------------------------------------------------------------

#[test]
fn empty_registry_has_zero_tools() {
    let reg = ToolRegistry::new();
    assert!(reg.is_empty());
    assert_eq!(reg.len(), 0);
    assert!(reg.list().is_empty());
    assert!(reg.names().is_empty());
}

#[test]
fn register_single_tool_increments_count() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(EchoTool));
    assert_eq!(reg.len(), 1);
    assert!(!reg.is_empty());
}

#[test]
fn register_multiple_tools_all_visible() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(EchoTool));
    reg.register(Box::new(PrivilegedTool));
    reg.register(Box::new(FailingTool));

    assert_eq!(reg.len(), 3);

    let names = reg.names();
    assert!(names.contains(&"test.echo".to_string()));
    assert!(names.contains(&"test.privileged".to_string()));
    assert!(names.contains(&"test.failing".to_string()));
}

#[test]
fn get_returns_registered_handler() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(EchoTool));

    assert!(reg.get("test.echo").is_some(), "registered tool must be findable by get");
    assert!(reg.get("nonexistent").is_none(), "absent tool must return None");
}

#[test]
fn register_same_name_replaces_handler() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(EchoTool));
    reg.register(Box::new(EchoTool)); // second registration of same name
    assert_eq!(reg.len(), 1, "duplicate registration must replace, not add");
}

#[test]
fn list_returns_all_specs() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(EchoTool));
    reg.register(Box::new(PrivilegedTool));

    let specs = reg.list();
    assert_eq!(specs.len(), 2);
}

#[test]
fn to_tool_definitions_converts_all_specs() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(EchoTool));
    reg.register(Box::new(PrivilegedTool));

    let defs = reg.to_tool_definitions();
    assert_eq!(defs.len(), 2);

    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"test.echo"));
    assert!(names.contains(&"test.privileged"));
}

// ---------------------------------------------------------------------------
// Execution without grants
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_tool_without_grant_requirement_succeeds() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(EchoTool));

    let input = json!({ "message": "hello world" });
    let result = reg
        .execute("test.echo", input.clone(), &empty_ctx())
        .await
        .expect("execute should succeed");

    assert_eq!(result.output, input, "output must equal input for echo tool");
    assert_eq!(result.classification, DataClassification::Public);
}

#[tokio::test]
async fn execute_returns_not_found_for_unknown_tool() {
    let reg = ToolRegistry::new();
    let result = reg
        .execute("no.such.tool", json!({}), &empty_ctx())
        .await;
    assert!(
        matches!(result, Err(ToolError::NotFound { .. })),
        "unknown tool name must return NotFound"
    );
}

#[tokio::test]
async fn execute_propagates_handler_error() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(FailingTool));

    let result = reg
        .execute("test.failing", json!({}), &empty_ctx())
        .await;
    assert!(
        matches!(result, Err(ToolError::ExecutionFailed { .. })),
        "handler error must propagate through registry"
    );
}

// ---------------------------------------------------------------------------
// Grant-gated execution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_grant_gated_tool_without_grant_is_denied() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(PrivilegedTool));

    // No grants in context.
    let result = reg
        .execute("test.privileged", json!({}), &empty_ctx())
        .await;
    assert!(
        matches!(result, Err(ToolError::PermissionDenied { .. })),
        "grant-gated tool must deny when no grants present"
    );
}

#[tokio::test]
async fn execute_grant_gated_tool_with_valid_grant_succeeds() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(PrivilegedTool));

    let ctx = ctx_with_grant("test/privileged-action");
    let result = reg
        .execute("test.privileged", json!({}), &ctx)
        .await
        .expect("should succeed with grant");

    assert_eq!(
        result.output,
        json!({ "status": "privileged-ok" }),
        "privileged tool must succeed with valid grant"
    );
}

#[tokio::test]
async fn execute_grant_gated_tool_with_wrong_grant_is_denied() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(PrivilegedTool));

    // Grant for a different action.
    let ctx = ctx_with_grant("test/different-action");
    let result = reg
        .execute("test.privileged", json!({}), &ctx)
        .await;
    assert!(
        matches!(result, Err(ToolError::PermissionDenied { .. })),
        "wrong grant action must not satisfy grant requirement"
    );
}

#[tokio::test]
async fn execute_grant_gated_tool_with_expired_grant_is_denied() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(PrivilegedTool));

    let expired_grant = make_expired_grant("test/privileged-action");
    let ctx = ToolContext {
        grants: vec![expired_grant],
        ..empty_ctx()
    };

    let result = reg
        .execute("test.privileged", json!({}), &ctx)
        .await;
    assert!(
        matches!(result, Err(ToolError::PermissionDenied { .. })),
        "expired grant must not satisfy grant requirement"
    );
}

#[tokio::test]
async fn execute_grant_gated_tool_with_multiple_grants_one_matching_succeeds() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(PrivilegedTool));

    // Provide two grants: one wrong, one correct.
    let grants = vec![
        ResolvedGrant {
            grant_id: GrantId::new(),
            principal: "agent".to_string(),
            action: "other/action".to_string(),
            resource: "**".to_string(),
            allowed_effects: EffectSet::new(vec!["other/action".to_string()]),
            limits: GrantLimits::default(),
            expires_at: Utc::now() + Duration::hours(1),
        },
        ResolvedGrant {
            grant_id: GrantId::new(),
            principal: "agent".to_string(),
            action: "test/privileged-action".to_string(),
            resource: "**".to_string(),
            allowed_effects: EffectSet::new(vec!["test/privileged-action".to_string()]),
            limits: GrantLimits::default(),
            expires_at: Utc::now() + Duration::hours(1),
        },
    ];
    let ctx = ToolContext { grants, ..empty_ctx() };

    let result = reg.execute("test.privileged", json!({}), &ctx).await;
    assert!(result.is_ok(), "any valid matching grant must satisfy the requirement");
}

// ---------------------------------------------------------------------------
// Output capture / context passing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_passes_correct_run_id_to_handler() {
    let capture = std::sync::Arc::new(ContextCaptureTool::new());
    let capture_clone = std::sync::Arc::clone(&capture);

    let mut reg = ToolRegistry::new();
    // We can't easily wrap Arc<ContextCaptureTool> as ToolHandler because
    // register takes Box<dyn ToolHandler>. Use a wrapper instead.
    struct ArcWrapper(std::sync::Arc<ContextCaptureTool>);
    #[async_trait]
    impl ToolHandler for ArcWrapper {
        fn spec(&self) -> ToolSpec {
            self.0.spec()
        }
        async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
            self.0.execute(input, ctx).await
        }
    }
    reg.register(Box::new(ArcWrapper(capture_clone)));

    let run_id = RunId::new();
    let ctx = ToolContext {
        run_id,
        agent_id: AgentId::new(),
        step_id: StepId::new(),
        grants: vec![],
    };
    reg.execute("test.ctx", json!({}), &ctx)
        .await
        .expect("execute");

    let captured = capture.last_context().expect("context must be captured");
    assert_eq!(captured.run_id, run_id, "handler must receive the correct run_id");
}

#[tokio::test]
async fn execute_output_classification_matches_handler_spec() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(PrivilegedTool));

    let ctx = ctx_with_grant("test/privileged-action");
    let result = reg
        .execute("test.privileged", json!({}), &ctx)
        .await
        .expect("ok");

    assert_eq!(
        result.classification,
        DataClassification::Internal,
        "output classification must match handler's declared classification"
    );
}

// ---------------------------------------------------------------------------
// Tool spec metadata
// ---------------------------------------------------------------------------

#[test]
fn tool_spec_required_grant_is_optional() {
    let spec = EchoTool.spec();
    assert!(spec.required_grant.is_none(), "EchoTool must have no required grant");
}

#[test]
fn tool_spec_required_grant_is_present_for_privileged_tool() {
    let spec = PrivilegedTool.spec();
    assert!(spec.required_grant.is_some(), "PrivilegedTool must declare a required grant");
    assert_eq!(spec.required_grant.unwrap(), "test/privileged-action");
}

#[test]
fn tool_spec_name_matches_registered_name() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(EchoTool));

    let specs = reg.list();
    let spec = specs.iter().find(|s| s.name == "test.echo").expect("spec");
    assert_eq!(spec.name, "test.echo");
}
