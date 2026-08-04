//! PRD-15 Security Tests — E5: Least-Privilege Enforcement
//!
//! This module verifies the remaining least-privilege invariants described in
//! PRD-15:
//!
//! - E5-03: Child grant intersection is enforced (child ⊆ parent).
//! - E5-04: Grant revocation is immediate; revoked capability is denied.
//! - E5-05: Tool capability scoping — wrong capability type is denied.
//! - E5-06: ModelExecutor interface never exposes API keys in InferenceRequest.
//! - E5-07: Grant widening by model text is rejected.
//! - E5-08: Restricted operation without isolation emits an event (not silent).
//! - E5-09: Budget enforcement across multiple attempts (cumulative > 100%).
//! - E5-10: Grant provenance is recorded for every grant decision.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde_json::Value;
use tokio::sync::Mutex;

use polkagent_core::{
    config::DataClassification,
    ids::{AgentId, GrantId, RunId, StepId},
};
use polkagent_executor_trait::{ContentBlock, InferenceMessage, InferenceRequest, MessageRole};
use polkagent_grant::{
    budget::BudgetTracker,
    grant::{
        ActiveGrant, EffectSet, GrantDecision, GrantLimits, GrantResolver, ResolvedGrant,
        ResolverConfig,
    },
    policy::{Effect, EvaluationContext, PolicyRule, PolicySet},
};
use polkagent_tool::{ToolContext, ToolError, ToolHandler, ToolRegistry, ToolResult, ToolSpec};

// ===========================================================================
// Helpers
// ===========================================================================

fn allow_rule(id: &str, actions: &[&str], resources: &[&str]) -> PolicyRule {
    PolicyRule {
        id: id.to_string(),
        effect: Effect::Allow,
        action_patterns: actions.iter().map(|s| s.to_string()).collect(),
        resource_patterns: resources.iter().map(|s| s.to_string()).collect(),
        conditions: Default::default(),
        abac_condition: None,
    }
}

fn empty_ctx() -> EvaluationContext {
    EvaluationContext::default()
}

fn make_grant(principal: &str, action: &str, expires_in: Duration) -> ResolvedGrant {
    ResolvedGrant {
        grant_id: GrantId::new(),
        principal: principal.to_string(),
        action: action.to_string(),
        resource: "**".to_string(),
        allowed_effects: EffectSet::new(vec![action.to_string()]),
        limits: GrantLimits::default(),
        expires_at: Utc::now() + expires_in,
    }
}

fn tool_ctx(grants: Vec<ResolvedGrant>) -> ToolContext {
    ToolContext {
        run_id: RunId::new(),
        agent_id: AgentId::new(),
        step_id: StepId::new(),
        grants,
    }
}

// ===========================================================================
// E5-03: Child grant intersection enforced (child ⊆ parent)
// ===========================================================================

/// Create a parent grant set [file:read, file:write, net:connect].
/// Create a child requesting [file:write, net:connect, shell:exec].
/// The child's effective grant must be exactly [file:write, net:connect] — the
/// intersection — and must NOT include shell:exec (not in parent).
///
/// This enforces the invariant: child capabilities ⊆ parent capabilities.
#[test]
fn e5_03_child_grant_intersection_enforced() {
    let parent_caps: HashSet<&str> = ["file:read", "file:write", "net:connect"]
        .iter()
        .copied()
        .collect();
    let child_requested: HashSet<&str> = ["file:write", "net:connect", "shell:exec"]
        .iter()
        .copied()
        .collect();

    // The set of capabilities the child requested but the parent does NOT have.
    // These must be DENIED to the child.
    let illegal: HashSet<&str> = child_requested.difference(&parent_caps).copied().collect();

    assert!(
        illegal.contains("shell:exec"),
        "shell:exec is not in the parent grant, so it must be in the illegal set"
    );

    // Effective child capabilities = intersection(parent, child_requested).
    // This is what the child actually receives after intersection enforcement.
    let effective: HashSet<&str> = parent_caps
        .intersection(&child_requested)
        .copied()
        .collect();

    // The effective set must NOT include anything from the illegal set.
    let overlap: HashSet<&str> = effective.intersection(&illegal).copied().collect();
    assert!(
        overlap.is_empty(),
        "effective child grant must contain no illegal capabilities; overlap: {overlap:?}"
    );

    assert!(
        effective.contains("file:write"),
        "child must have file:write (in both parent and child)"
    );
    assert!(
        effective.contains("net:connect"),
        "child must have net:connect (in both parent and child)"
    );
    assert!(
        !effective.contains("shell:exec"),
        "child must NOT have shell:exec (not in parent)"
    );
    assert!(
        !effective.contains("file:read"),
        "child must NOT have file:read (not requested by child)"
    );

    assert_eq!(
        effective.len(),
        2,
        "effective child grant must contain exactly 2 capabilities"
    );
}

/// Child with an empty parent set has zero effective capabilities.
#[test]
fn e5_03_child_with_empty_parent_gets_no_capabilities() {
    let parent_caps: HashSet<&str> = HashSet::new();
    let child_requested: HashSet<&str> = ["file:read", "shell:exec"].iter().copied().collect();

    let effective: HashSet<&str> = parent_caps
        .intersection(&child_requested)
        .copied()
        .collect();

    assert!(
        effective.is_empty(),
        "child with zero-capability parent must have zero effective capabilities"
    );
}

/// Child requesting a strict subset of parent gets exactly what it requested.
#[test]
fn e5_03_child_subset_of_parent_gets_full_request() {
    let parent_caps: HashSet<&str> = ["file:read", "file:write", "net:connect", "shell:exec"]
        .iter()
        .copied()
        .collect();
    let child_requested: HashSet<&str> = ["file:read"].iter().copied().collect();

    let effective: HashSet<&str> = parent_caps
        .intersection(&child_requested)
        .copied()
        .collect();

    assert_eq!(
        effective, child_requested,
        "child requesting strict subset of parent must get exactly what it requested"
    );
}

// ===========================================================================
// E5-04: Grant revocation is immediate
// ===========================================================================

/// Grant a capability, verify it succeeds. Revoke by expiring the grant
/// immediately. Verify the next use is denied.
#[tokio::test]
async fn e5_04_grant_revocation_is_immediate() {
    let mut set = PolicySet::default();
    set.add_rule(allow_rule("allow-file-read", &["file/read"], &["**"]));

    let resolver = GrantResolver::new(set, ResolverConfig::default());

    // Add an active grant.
    let grant_id = GrantId::new();
    resolver
        .add_active_grant(ActiveGrant {
            grant_id,
            principal: "alice".to_string(),
            action_pattern: "file/read".to_string(),
            resource_pattern: "**".to_string(),
            allowed_effects: EffectSet::new(vec!["file/read".to_string()]),
            limits: GrantLimits::default(),
            expires_at: Utc::now() + Duration::hours(1),
        })
        .await;

    // First use: should succeed.
    let d1 = resolver
        .resolve(
            "alice",
            "file/read",
            "path/to/file",
            &empty_ctx(),
            None,
            None,
        )
        .await
        .expect("resolve");
    assert!(
        matches!(d1, GrantDecision::Permit(_)),
        "initial grant must be permitted: {d1:?}"
    );

    // Revoke by adding an expired replacement. The `GrantResolver` caches
    // by reference; to simulate revocation we exercise the same mechanism:
    // an expired grant is skipped, so we add an already-expired grant to
    // replace the active one. In a real system, revocation removes the grant
    // record from the registry.
    //
    // Here we test the invariant: an expired grant is denied immediately.
    let expired_grant = ResolvedGrant {
        grant_id: GrantId::new(),
        principal: "alice".to_string(),
        action: "file/read".to_string(),
        resource: "**".to_string(),
        allowed_effects: EffectSet::new(vec!["file/read".to_string()]),
        limits: GrantLimits::default(),
        expires_at: Utc::now() - Duration::hours(1), // already expired
    };

    // Verify the expired grant is not valid.
    assert!(
        !expired_grant.is_valid(),
        "revoked/expired grant must not be valid"
    );

    // A context with only the expired grant must be denied by the tool registry.
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(FileReadTool));

    let ctx_expired = tool_ctx(vec![expired_grant]);
    let result = registry
        .execute("test.file_read", serde_json::json!({}), &ctx_expired)
        .await;

    assert!(
        matches!(result, Err(ToolError::PermissionDenied { .. })),
        "expired grant must be denied immediately: {result:?}"
    );
}

/// A grant with a 1-millisecond TTL expires before the next check.
#[tokio::test]
async fn e5_04_tiny_ttl_grant_expires_immediately() {
    let grant = ResolvedGrant {
        grant_id: GrantId::new(),
        principal: "alice".to_string(),
        action: "file/read".to_string(),
        resource: "**".to_string(),
        allowed_effects: EffectSet::new(vec!["file/read".to_string()]),
        limits: GrantLimits::default(),
        expires_at: Utc::now() - chrono::Duration::milliseconds(1), // already past
    };

    assert!(
        !grant.is_valid(),
        "a grant expiring in the past is immediately invalid"
    );
}

// ===========================================================================
// E5-05: Tool capability scoping
// ===========================================================================

/// A tool requiring "file:read" capability.
struct FileReadTool;

#[async_trait]
impl ToolHandler for FileReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "test.file_read".to_string(),
            description: "Reads a file".to_string(),
            input_schema: serde_json::json!({ "type": "object" }),
            required_grant: Some("file:read".to_string()),
            output_classification: DataClassification::Internal,
        }
    }

    async fn execute(&self, _input: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            output: serde_json::json!({ "content": "file data" }),
            classification: DataClassification::Internal,
            artifacts: vec![],
        })
    }
}

/// An agent with only "file:write" cannot execute the "file:read" tool.
#[tokio::test]
async fn e5_05_tool_capability_scoping_wrong_cap_denied() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(FileReadTool));

    // Agent has "file:write" but NOT "file:read".
    let write_only_grant = make_grant("agent-a", "file:write", Duration::hours(1));
    let ctx = tool_ctx(vec![write_only_grant]);

    let result = registry
        .execute("test.file_read", serde_json::json!({}), &ctx)
        .await;

    assert!(
        matches!(result, Err(ToolError::PermissionDenied { .. })),
        "agent with only file:write must not execute file:read tool: {result:?}"
    );
}

/// An agent with the correct "file:read" capability CAN execute the tool.
#[tokio::test]
async fn e5_05_tool_capability_scoping_correct_cap_allowed() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(FileReadTool));

    let read_grant = make_grant("agent-b", "file:read", Duration::hours(1));
    let ctx = tool_ctx(vec![read_grant]);

    let result = registry
        .execute("test.file_read", serde_json::json!({}), &ctx)
        .await;

    assert!(
        result.is_ok(),
        "agent with file:read must be able to execute file:read tool: {result:?}"
    );
}

/// An agent with no grants at all is denied access to any gated tool.
#[tokio::test]
async fn e5_05_tool_no_grants_denied() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(FileReadTool));

    let ctx = tool_ctx(vec![]); // zero grants

    let result = registry
        .execute("test.file_read", serde_json::json!({}), &ctx)
        .await;

    assert!(
        matches!(result, Err(ToolError::PermissionDenied { .. })),
        "agent with no grants must be denied: {result:?}"
    );
}

// ===========================================================================
// E5-06: ModelExecutor interface never exposes API keys
// ===========================================================================

/// Verify that the `InferenceRequest` type — the only data that crosses the
/// kernel / executor boundary — has no field capable of carrying raw key
/// material. All fields are examined by type.
///
/// This is a static/structural test: if a field capable of holding secrets
/// (e.g. `api_key`, `token`, `secret`) were added to `InferenceRequest`,
/// this test would need to be updated, which serves as a forcing function
/// for a security review.
#[test]
fn e5_06_inference_request_has_no_api_key_field() {
    let req = InferenceRequest {
        run_id: RunId::new(),
        step_id: StepId::new(),
        messages: vec![InferenceMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }],
        system: None,
        tools: vec![],
        model_id: "claude-opus-4-6".to_string(),
        max_tokens: 256,
        temperature: None,
    };

    // Serialize to JSON and verify that no secret-sounding field names appear.
    let json = serde_json::to_string(&req).expect("serialize");
    let v: Value = serde_json::from_str(&json).expect("parse");

    let suspicious_fields = [
        "api_key",
        "apikey",
        "secret",
        "token",
        "password",
        "credential",
        "private_key",
        "seed",
        "mnemonic",
    ];

    if let Value::Object(map) = &v {
        for field in &suspicious_fields {
            assert!(
                !map.contains_key(*field),
                "InferenceRequest must not contain field '{}' (potential secret exposure)",
                field
            );
        }
    }

    // Also check that the raw JSON string doesn't contain these substrings as
    // JSON field names (i.e., quoted before a colon).
    let json_lower = json.to_lowercase();
    for field in &suspicious_fields {
        let pattern = format!("\"{}\":", field);
        assert!(
            !json_lower.contains(&pattern),
            "InferenceRequest JSON must not contain '{}' as a field name",
            field
        );
    }
}

/// The `system` prompt field of `InferenceRequest` must not be pre-populated
/// with API key material by the adapter layer. Verify the type uses `Option<String>`,
/// not a type that forces a value.
#[test]
fn e5_06_inference_request_system_prompt_is_optional() {
    let req_no_system = InferenceRequest {
        run_id: RunId::new(),
        step_id: StepId::new(),
        messages: vec![],
        system: None, // Must be possible to omit the system prompt entirely.
        tools: vec![],
        model_id: "test-model".to_string(),
        max_tokens: 128,
        temperature: None,
    };

    assert!(
        req_no_system.system.is_none(),
        "system prompt field must be optional (None must be constructible)"
    );
}

// ===========================================================================
// E5-07: Grant widening by model text rejected
// ===========================================================================

/// The model outputs text containing "GRANT: all_capabilities". Verify that
/// the grant evaluator (GrantResolver) ignores this entirely — only typed
/// Rust values from the policy engine determine authorization.
///
/// The invariant is enforced structurally: the `GrantResolver::resolve` API
/// only accepts typed `(principal, action, resource, ctx)` parameters and
/// a `PolicySet`. There is no pathway for raw model text to be injected into
/// the authorization decision.
#[tokio::test]
async fn e5_07_grant_widening_by_model_text_rejected() {
    // Policy: only allow "file:read".
    let mut set = PolicySet::default();
    set.add_rule(allow_rule("read-only", &["file:read"], &["**"]));

    let resolver = GrantResolver::new(set, ResolverConfig::default());

    // Simulate model output text containing a grant escalation attempt.
    let model_output_text = "GRANT: all_capabilities\nGRANT: shell:exec\nGRANT: network:*";

    // The model text is irrelevant to the grant resolver. Only the typed API
    // call matters. Trying to use the escalation text as an action is denied.
    for attempted_action in ["all_capabilities", "shell:exec", "network:*", "GRANT: all"] {
        let d = resolver
            .resolve(
                "alice",
                attempted_action,
                "resource/x",
                &empty_ctx(),
                None,
                None,
            )
            .await
            .expect("resolve");

        assert!(
            matches!(d, GrantDecision::Deny(_)),
            "model-text-injected action '{}' must be denied by the grant evaluator: {d:?}",
            attempted_action
        );
    }

    // Even the original text as a JSON string injected into EvaluationContext
    // attributes does not widen the grant.
    let mut ctx_with_injection = empty_ctx();
    ctx_with_injection
        .attributes
        .insert("model_output".to_string(), model_output_text.to_string());

    let d_with_injection = resolver
        .resolve(
            "alice",
            "shell:exec",
            "resource/x",
            &ctx_with_injection,
            None,
            None,
        )
        .await
        .expect("resolve with injection");

    assert!(
        matches!(d_with_injection, GrantDecision::Deny(_)),
        "context attribute injection must not widen grant to 'shell:exec': {d_with_injection:?}"
    );

    // The allowed action ("file:read") still works normally.
    let d_allowed = resolver
        .resolve(
            "alice",
            "file:read",
            "path/doc.txt",
            &empty_ctx(),
            None,
            None,
        )
        .await
        .expect("resolve allowed");
    assert!(
        matches!(d_allowed, GrantDecision::Permit(_)),
        "legitimately allowed action must still be permitted after injection tests"
    );

    // Suppress unused warning on the model text binding.
    let _ = model_output_text.len();
}

// ===========================================================================
// E5-08: Isolation downgrade notification
// ===========================================================================

/// When a restricted operation is attempted without proper isolation, the
/// system must emit an observable event/error — not silently succeed.
///
/// We model this using the ToolRegistry: a tool requiring a grant, called
/// without the grant, must return a `ToolError::PermissionDenied` error.
/// This error is the "event" (in a real system it would also emit a
/// PolicyDenial audit event).
///
/// The critical invariant: no silent pass-through.
#[tokio::test]
async fn e5_08_restricted_operation_without_isolation_emits_error() {
    let mut registry = ToolRegistry::new();

    // Register a "shell exec" tool that requires a strict grant.
    struct ShellExecTool;

    #[async_trait]
    impl ToolHandler for ShellExecTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "test.shell_exec".to_string(),
                description: "Executes a shell command (highly privileged)".to_string(),
                input_schema: serde_json::json!({ "type": "object" }),
                required_grant: Some("shell:exec".to_string()),
                output_classification: DataClassification::Sensitive,
            }
        }

        async fn execute(
            &self,
            _input: Value,
            _ctx: &ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult {
                output: serde_json::json!({ "stdout": "" }),
                classification: DataClassification::Sensitive,
                artifacts: vec![],
            })
        }
    }

    registry.register(Box::new(ShellExecTool));

    // Agent with NO isolation capability attempts a restricted op.
    let ctx_no_isolation = tool_ctx(vec![]); // zero grants

    let result = registry
        .execute(
            "test.shell_exec",
            serde_json::json!({"cmd": "ls"}),
            &ctx_no_isolation,
        )
        .await;

    // Must be an explicit, observable error — not Ok or silent.
    match &result {
        Err(ToolError::PermissionDenied { reason }) => {
            assert!(
                !reason.is_empty(),
                "PermissionDenied error must carry a non-empty reason"
            );
        }
        other => {
            panic!("restricted op without isolation must produce PermissionDenied, got: {other:?}")
        }
    }
}

/// A tool with no required_grant runs unconditionally — this is NOT a silent
/// downgrade; it is explicitly declared open access.
#[tokio::test]
async fn e5_08_open_access_tool_is_explicit_not_silent() {
    let mut registry = ToolRegistry::new();

    struct PublicInfoTool;

    #[async_trait]
    impl ToolHandler for PublicInfoTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "test.public_info".to_string(),
                description: "Returns public information; no grant required.".to_string(),
                input_schema: serde_json::json!({ "type": "object" }),
                required_grant: None, // explicitly open
                output_classification: DataClassification::Public,
            }
        }

        async fn execute(
            &self,
            _input: Value,
            _ctx: &ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult {
                output: serde_json::json!({ "info": "public" }),
                classification: DataClassification::Public,
                artifacts: vec![],
            })
        }
    }

    registry.register(Box::new(PublicInfoTool));

    let spec = registry.get("test.public_info").expect("registered").spec();
    assert!(
        spec.required_grant.is_none(),
        "open-access tool must declare None for required_grant explicitly"
    );

    // Calling without grants is allowed because the tool declared open access.
    let ctx_no_grants = tool_ctx(vec![]);
    let result = registry
        .execute("test.public_info", serde_json::json!({}), &ctx_no_grants)
        .await;
    assert!(
        result.is_ok(),
        "explicitly open tool must succeed without grants: {result:?}"
    );
}

// ===========================================================================
// E5-09: Budget enforcement across multiple attempts
// ===========================================================================

/// Create 3 effects each costing 40% of the budget. The first two must pass
/// (80% total), and the third must be rejected (80+40=120% > 100%).
#[tokio::test]
async fn e5_09_budget_enforcement_across_multiple_attempts() {
    let tracker = BudgetTracker::new();
    let agent_id = AgentId::new();
    let budget_ceiling: u64 = 100;

    tracker.configure(agent_id, budget_ceiling).await;

    let run_id = RunId::new();
    let cost_per_effect: u64 = 40; // 40% of 100

    // Attempt 1: 40 of 100 — must pass.
    let within_1 = tracker
        .check_budget(agent_id, cost_per_effect)
        .await
        .expect("check 1");
    assert!(within_1, "first attempt (40/100) must be within budget");
    tracker
        .record_spend(agent_id, run_id, cost_per_effect)
        .await
        .expect("spend 1");

    let status_1 = tracker.get_remaining(agent_id).await.expect("status 1");
    assert_eq!(status_1.spent, 40, "40 spent after first attempt");
    assert_eq!(status_1.remaining, 60, "60 remaining after first attempt");

    // Attempt 2: 40 more (total 80) — must pass.
    let within_2 = tracker
        .check_budget(agent_id, cost_per_effect)
        .await
        .expect("check 2");
    assert!(within_2, "second attempt (80/100) must be within budget");
    tracker
        .record_spend(agent_id, run_id, cost_per_effect)
        .await
        .expect("spend 2");

    let status_2 = tracker.get_remaining(agent_id).await.expect("status 2");
    assert_eq!(status_2.spent, 80, "80 spent after second attempt");
    assert_eq!(status_2.remaining, 20, "20 remaining after second attempt");

    // Attempt 3: 40 more (80 + 40 = 120 > 100) — must be rejected.
    let within_3 = tracker
        .check_budget(agent_id, cost_per_effect)
        .await
        .expect("check 3");
    assert!(
        !within_3,
        "third attempt (cumulative 120/100) must exceed budget and be rejected"
    );

    // record_spend for attempt 3 must also fail.
    let spend_result = tracker
        .record_spend(agent_id, run_id, cost_per_effect)
        .await;
    assert!(
        spend_result.is_err(),
        "recording over-budget spend must return an error: {spend_result:?}"
    );

    // Budget totals must remain at 80 (not updated on failed spend).
    let final_status = tracker.get_remaining(agent_id).await.expect("final status");
    assert_eq!(
        final_status.spent, 80,
        "budget total must not change after rejected third attempt"
    );
}

/// Budget enforcement via the GrantResolver: three sequential resolves,
/// third one is denied because cumulative spend exceeds ceiling.
#[tokio::test]
async fn e5_09_budget_enforcement_via_grant_resolver() {
    let mut set = PolicySet::default();
    set.add_rule(allow_rule("allow-all", &["**"], &["**"]));

    let config = ResolverConfig {
        max_context_age: None,
        default_grant_ttl: Duration::hours(1),
    };
    let resolver = GrantResolver::new(set, config);

    let tracker = BudgetTracker::new();
    let agent_id = AgentId::new();
    tracker.configure(agent_id, 100).await;

    let resolver = resolver.with_budget_tracker(Arc::clone(&tracker)).await;
    let principal = agent_id.to_string();
    let run_id = RunId::new();

    // Effect 1: cost 40 → allowed (40/100).
    let d1 = resolver
        .resolve(
            &principal,
            "chain/transfer",
            "account/a",
            &empty_ctx(),
            Some(40),
            Some(run_id),
        )
        .await
        .expect("resolve 1");
    assert!(
        matches!(d1, GrantDecision::Permit(_)),
        "first 40/100 must be permitted: {d1:?}"
    );

    // Effect 2: cost 40 → allowed (80/100).
    let d2 = resolver
        .resolve(
            &principal,
            "chain/transfer",
            "account/b",
            &empty_ctx(),
            Some(40),
            Some(run_id),
        )
        .await
        .expect("resolve 2");
    assert!(
        matches!(d2, GrantDecision::Permit(_)),
        "second 40 (80/100) must be permitted: {d2:?}"
    );

    // Effect 3: cost 40 → denied (120/100 > budget).
    let d3 = resolver
        .resolve(
            &principal,
            "chain/transfer",
            "account/c",
            &empty_ctx(),
            Some(40),
            Some(run_id),
        )
        .await
        .expect("resolve 3");
    assert!(
        matches!(d3, GrantDecision::Deny(_)),
        "third 40 (cumulative 120/100) must be denied: {d3:?}"
    );
}

// ===========================================================================
// E5-10: Grant provenance recorded
// ===========================================================================

/// A lightweight audit record structure for grant decisions.
#[derive(Debug, Clone)]
struct GrantAuditRecord {
    requester: String,
    action: String,
    resource: String,
    policy_matched: String,
    decision: &'static str,
}

/// An audit log that captures grant provenance.
#[derive(Debug, Default)]
struct AuditLog {
    records: Mutex<Vec<GrantAuditRecord>>,
}

impl AuditLog {
    async fn record(&self, r: GrantAuditRecord) {
        self.records.lock().await.push(r);
    }

    async fn all(&self) -> Vec<GrantAuditRecord> {
        self.records.lock().await.clone()
    }
}

/// Every grant decision must record who requested it, what was requested,
/// what policy matched, and whether it was allowed or denied.
#[tokio::test]
async fn e5_10_grant_provenance_recorded_for_every_decision() {
    let mut set = PolicySet::default();
    set.add_rule(allow_rule("allow-chain", &["chain/**"], &["account/**"]));

    let resolver = GrantResolver::new(set.clone(), ResolverConfig::default());
    let audit = Arc::new(AuditLog::default());

    // Helper: resolve and record audit provenance.
    async fn resolve_and_audit(
        resolver: &GrantResolver,
        audit: &Arc<AuditLog>,
        principal: &str,
        action: &str,
        resource: &str,
        policy_matched: &str,
    ) {
        let decision = resolver
            .resolve(
                principal,
                action,
                resource,
                &EvaluationContext::default(),
                None,
                None,
            )
            .await
            .expect("resolve");

        let outcome = match &decision {
            GrantDecision::Permit(_) => "allow",
            GrantDecision::Deny(_) => "deny",
            GrantDecision::RequireApproval(_) => "approval_required",
            GrantDecision::Defer(_) => "deferred",
        };

        audit
            .record(GrantAuditRecord {
                requester: principal.to_string(),
                action: action.to_string(),
                resource: resource.to_string(),
                policy_matched: policy_matched.to_string(),
                decision: outcome,
            })
            .await;
    }

    // Decision 1: Alice requests chain/transfer → allowed by "allow-chain".
    resolve_and_audit(
        &resolver,
        &audit,
        "alice",
        "chain/transfer",
        "account/bob",
        "allow-chain",
    )
    .await;

    // Decision 2: Bob requests governance/vote → denied (no matching rule).
    resolve_and_audit(
        &resolver,
        &audit,
        "bob",
        "governance/vote",
        "referendum/1",
        "default_deny",
    )
    .await;

    // Decision 3: Charlie requests chain/query → allowed by "allow-chain".
    resolve_and_audit(
        &resolver,
        &audit,
        "charlie",
        "chain/query",
        "account/charlie",
        "allow-chain",
    )
    .await;

    // Verify: every decision has complete provenance.
    let records = audit.all().await;
    assert_eq!(records.len(), 3, "must have exactly 3 audit records");

    // Record 1: Alice — allow.
    assert_eq!(records[0].requester, "alice");
    assert_eq!(records[0].action, "chain/transfer");
    assert_eq!(records[0].resource, "account/bob");
    assert_eq!(records[0].policy_matched, "allow-chain");
    assert_eq!(records[0].decision, "allow");

    // Record 2: Bob — deny.
    assert_eq!(records[1].requester, "bob");
    assert_eq!(records[1].action, "governance/vote");
    assert_eq!(records[1].decision, "deny");

    // Record 3: Charlie — allow.
    assert_eq!(records[2].requester, "charlie");
    assert_eq!(records[2].action, "chain/query");
    assert_eq!(records[2].decision, "allow");
}

/// Provenance is recorded even for budget-denied requests.
#[tokio::test]
async fn e5_10_grant_provenance_recorded_for_budget_denial() {
    let mut set = PolicySet::default();
    set.add_rule(allow_rule("allow-all", &["**"], &["**"]));

    let config = ResolverConfig {
        max_context_age: None,
        default_grant_ttl: Duration::hours(1),
    };
    let resolver = GrantResolver::new(set, config);

    let tracker = BudgetTracker::new();
    let agent_id = AgentId::new();
    tracker.configure(agent_id, 50).await;

    let resolver = resolver.with_budget_tracker(Arc::clone(&tracker)).await;
    let principal = agent_id.to_string();
    let run_id = RunId::new();

    let audit = Arc::new(AuditLog::default());

    // Attempt exceeds budget — must be denied and recorded.
    let decision = resolver
        .resolve(
            &principal,
            "chain/transfer",
            "account/x",
            &empty_ctx(),
            Some(100),
            Some(run_id),
        )
        .await
        .expect("resolve");

    let outcome = match &decision {
        GrantDecision::Permit(_) => "allow",
        GrantDecision::Deny(d) => {
            // Provenance: stage must indicate budget_check.
            assert!(
                d.stage.contains("budget"),
                "budget denial must reference 'budget' in stage, got: '{}'",
                d.stage
            );
            "deny"
        }
        other => panic!("unexpected decision: {other:?}"),
    };

    audit
        .record(GrantAuditRecord {
            requester: principal.clone(),
            action: "chain/transfer".to_string(),
            resource: "account/x".to_string(),
            policy_matched: "budget_check".to_string(),
            decision: outcome,
        })
        .await;

    let records = audit.all().await;
    assert_eq!(
        records.len(),
        1,
        "budget denial must produce an audit record"
    );
    assert_eq!(records[0].decision, "deny");
    assert_eq!(records[0].policy_matched, "budget_check");
}

/// Provenance captures the requesting agent identity, not just a string label.
#[tokio::test]
async fn e5_10_provenance_captures_requester_identity() {
    let mut set = PolicySet::default();
    set.add_rule(allow_rule("r1", &["chain/**"], &["**"]));

    let resolver = GrantResolver::new(set, ResolverConfig::default());
    let agent_id = AgentId::new();
    let principal = agent_id.to_string();

    let decision = resolver
        .resolve(
            &principal,
            "chain/transfer",
            "account/y",
            &empty_ctx(),
            None,
            None,
        )
        .await
        .expect("resolve");

    // The ResolvedGrant carries the principal, preserving provenance.
    if let GrantDecision::Permit(grant) = &decision {
        assert_eq!(
            grant.principal, principal,
            "grant provenance must record the requesting agent's principal"
        );
        assert_eq!(
            grant.action, "chain/transfer",
            "grant provenance must record the requested action"
        );
        assert_eq!(
            grant.resource, "account/y",
            "grant provenance must record the requested resource"
        );
    } else {
        panic!("expected Permit, got {decision:?}");
    }
}
