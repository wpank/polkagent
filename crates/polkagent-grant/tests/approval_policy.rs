#![allow(
    clippy::expect_used,
    reason = "approval policy fixtures should fail at the exact setup or decision boundary"
)]

use std::sync::Arc;

use async_trait::async_trait;
use polkagent_grant::{
    load_policy_file, Effect, EvaluationContext, Gate, GateRequest, GateResult, GrantDecision,
    GrantResolver, PolicyRule, PolicySet, ResolverConfig,
};

fn rule(id: &str, effect: Effect) -> PolicyRule {
    PolicyRule {
        id: id.to_owned(),
        effect,
        action_patterns: vec!["tool.write".to_owned()],
        resource_patterns: vec!["workspace/**".to_owned()],
        conditions: Default::default(),
        abac_condition: None,
    }
}

async fn resolve(resolver: &GrantResolver) -> GrantDecision {
    resolver
        .resolve(
            "agent-1",
            "tool.write",
            "workspace/src/lib.rs",
            &EvaluationContext::default(),
            None,
            None,
        )
        .await
        .expect("resolver infrastructure should remain available")
}

#[tokio::test]
async fn approval_policy_default_deny_never_escalates() {
    let resolver = GrantResolver::new(PolicySet::default(), ResolverConfig::default());
    let decision = resolve(&resolver).await;

    assert!(matches!(decision, GrantDecision::Deny(_)));
}

#[tokio::test]
async fn approval_policy_explicit_deny_beats_escalation_and_allow() {
    let resolver = GrantResolver::new(
        PolicySet::new(vec![
            rule("permit", Effect::Allow),
            rule("approval", Effect::RequireApproval),
            rule("deny", Effect::Deny),
        ]),
        ResolverConfig::default(),
    );
    let decision = resolve(&resolver).await;

    assert!(matches!(decision, GrantDecision::Deny(_)));
}

#[tokio::test]
async fn approval_policy_escalation_beats_allow() {
    let resolver = GrantResolver::new(
        PolicySet::new(vec![
            rule("permit", Effect::Allow),
            rule("approval", Effect::RequireApproval),
        ]),
        ResolverConfig::default(),
    );
    let decision = resolve(&resolver).await;

    assert!(matches!(decision, GrantDecision::RequireApproval(_)));
}

#[tokio::test]
async fn approval_policy_permit_produces_exact_scope() {
    let resolver = GrantResolver::new(
        PolicySet::new(vec![rule("permit", Effect::Allow)]),
        ResolverConfig::default(),
    );
    let decision = resolve(&resolver).await;

    let GrantDecision::Permit(grant) = decision else {
        panic!("explicit permit should produce a resolved grant");
    };
    assert_eq!(grant.principal, "agent-1");
    assert_eq!(grant.action, "tool.write");
    assert_eq!(grant.resource, "workspace/src/lib.rs");
    assert!(grant.allowed_effects.contains("tool.write"));
}

struct EscalatingGate;

#[async_trait]
impl Gate for EscalatingGate {
    async fn check(&self, _request: &GateRequest) -> GateResult {
        GateResult::Escalate {
            reason: "operator review required".to_owned(),
        }
    }

    fn name(&self) -> &str {
        "EscalatingGate"
    }
}

#[tokio::test]
async fn approval_policy_gate_escalates_only_after_policy_permit() {
    let resolver = GrantResolver::new(
        PolicySet::new(vec![rule("permit", Effect::Allow)]),
        ResolverConfig::default(),
    )
    .with_gate(Arc::new(EscalatingGate))
    .await;

    assert!(matches!(
        resolve(&resolver).await,
        GrantDecision::RequireApproval(_)
    ));

    let denied = GrantResolver::new(PolicySet::default(), ResolverConfig::default())
        .with_gate(Arc::new(EscalatingGate))
        .await;
    assert!(matches!(resolve(&denied).await, GrantDecision::Deny(_)));
}

#[test]
fn approval_policy_effect_is_strictly_serializable() {
    let temp = tempfile::tempdir().expect("create policy fixture directory");
    let valid_path = temp.path().join("approval.toml");
    std::fs::write(
        &valid_path,
        r#"
[[rules]]
id = "review-write"
effect = "require_approval"
action_patterns = ["tool.write"]
resource_patterns = ["workspace/**"]
"#,
    )
    .expect("write approval policy");

    let loaded = load_policy_file(&valid_path).expect("load approval policy");
    assert_eq!(loaded.rules.len(), 1);
    assert_eq!(loaded.rules[0].effect, Effect::RequireApproval);

    let unknown_path = temp.path().join("unknown.toml");
    std::fs::write(
        &unknown_path,
        r#"
[[rules]]
id = "review-write"
effect = "require_approval"
action_patterns = ["tool.write"]
resource_patterns = ["workspace/**"]
surprise_authority = true
"#,
    )
    .expect("write malformed policy");
    assert!(
        load_policy_file(&unknown_path).is_err(),
        "unknown authority fields must fail closed"
    );
}
