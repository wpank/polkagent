//! Grant + effect integration test.
//!
//! Verifies that the policy evaluation engine works end-to-end with budget
//! tracking and gate composition: allowed actions produce Allow decisions,
//! denied actions produce Deny decisions, and the budget gate tracks
//! cumulative spend.

// This assertion-oriented integration target uses `expect`/`unwrap` to identify
// the exact cross-crate fixture step or behavioral contract that failed.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashMap;

use polkagent_grant::gate::{
    AllowlistField, AllowlistGate, BudgetGate, ComposedGate, Gate, GateRequest, GateResult,
};
use polkagent_grant::policy::{
    evaluate, Effect, EvaluationContext, PolicyDecision, PolicyRule, PolicySet,
};

// ---------------------------------------------------------------------------
// Helper: build a GateRequest
// ---------------------------------------------------------------------------

fn gate_req(principal: &str, action: &str, resource: &str, amount: Option<u64>) -> GateRequest {
    GateRequest {
        principal: principal.to_string(),
        action: action.to_string(),
        resource: resource.to_string(),
        amount,
        metadata: HashMap::default(),
    }
}

// ---------------------------------------------------------------------------
// Policy: allow + deny rules
// ---------------------------------------------------------------------------

#[test]
fn policy_allows_configured_action() {
    let mut policy_set = PolicySet::default();
    policy_set.add_rule(PolicyRule {
        id: "allow-query".to_string(),
        effect: Effect::Allow,
        action_patterns: vec!["chain/query".to_string()],
        resource_patterns: vec!["**".to_string()],
        conditions: HashMap::default(),
        abac_condition: None,
    });

    let ctx = EvaluationContext::default();
    let decision = evaluate(&policy_set, "chain/query", "account/alice", &ctx);
    assert_eq!(decision, PolicyDecision::Allow);
}

#[test]
fn policy_denies_unconfigured_action() {
    let mut policy_set = PolicySet::default();
    policy_set.add_rule(PolicyRule {
        id: "allow-query".to_string(),
        effect: Effect::Allow,
        action_patterns: vec!["chain/query".to_string()],
        resource_patterns: vec!["**".to_string()],
        conditions: HashMap::default(),
        abac_condition: None,
    });

    let ctx = EvaluationContext::default();
    let decision = evaluate(&policy_set, "chain/transfer", "account/alice", &ctx);
    assert!(
        matches!(decision, PolicyDecision::Deny { .. }),
        "chain/transfer is not allowed by the policy"
    );
}

#[test]
fn explicit_deny_overrides_allow() {
    let mut policy_set = PolicySet::default();
    policy_set.add_rule(PolicyRule {
        id: "allow-all".to_string(),
        effect: Effect::Allow,
        action_patterns: vec!["**".to_string()],
        resource_patterns: vec!["**".to_string()],
        conditions: HashMap::default(),
        abac_condition: None,
    });
    policy_set.add_rule(PolicyRule {
        id: "deny-transfer".to_string(),
        effect: Effect::Deny,
        action_patterns: vec!["chain/transfer".to_string()],
        resource_patterns: vec!["**".to_string()],
        conditions: HashMap::default(),
        abac_condition: None,
    });

    let ctx = EvaluationContext::default();

    // Query should be allowed.
    assert_eq!(
        evaluate(&policy_set, "chain/query", "account/alice", &ctx),
        PolicyDecision::Allow
    );

    // Transfer should be denied.
    assert!(matches!(
        evaluate(&policy_set, "chain/transfer", "account/alice", &ctx),
        PolicyDecision::Deny { .. }
    ));
}

#[test]
fn policy_with_conditions_requires_matching_context() {
    let mut policy_set = PolicySet::default();
    let mut rule = PolicyRule {
        id: "allow-polkadot-only".to_string(),
        effect: Effect::Allow,
        action_patterns: vec!["chain/**".to_string()],
        resource_patterns: vec!["**".to_string()],
        conditions: HashMap::default(),
        abac_condition: None,
    };
    rule.conditions
        .insert("network".to_string(), "polkadot".to_string());
    policy_set.add_rule(rule);

    // Without matching context: deny.
    let no_ctx = EvaluationContext::default();
    assert!(matches!(
        evaluate(&policy_set, "chain/query", "res", &no_ctx),
        PolicyDecision::Deny { .. }
    ));

    // With matching context: allow.
    let mut ctx = EvaluationContext::default();
    ctx.attributes
        .insert("network".to_string(), "polkadot".to_string());
    assert_eq!(
        evaluate(&policy_set, "chain/query", "res", &ctx),
        PolicyDecision::Allow
    );

    // With wrong context: deny.
    let mut wrong_ctx = EvaluationContext::default();
    wrong_ctx
        .attributes
        .insert("network".to_string(), "kusama".to_string());
    assert!(matches!(
        evaluate(&policy_set, "chain/query", "res", &wrong_ctx),
        PolicyDecision::Deny { .. }
    ));
}

// ---------------------------------------------------------------------------
// Budget gate tracks cumulative spend
// ---------------------------------------------------------------------------

#[tokio::test]
async fn budget_gate_tracks_cumulative_spend() {
    let gate = BudgetGate::new(1000);

    // First transfer: 600 units (within budget).
    let r1 = gate
        .check(&gate_req(
            "agent-1",
            "chain/transfer",
            "account/bob",
            Some(600),
        ))
        .await;
    assert_eq!(r1, GateResult::Allow);

    // Second transfer: 500 units (total 1100 > 1000 budget).
    let r2 = gate
        .check(&gate_req(
            "agent-1",
            "chain/transfer",
            "account/bob",
            Some(500),
        ))
        .await;
    assert!(
        matches!(r2, GateResult::Deny { .. }),
        "cumulative 1100 exceeds budget of 1000"
    );

    // Different principal: should have independent budget.
    let r3 = gate
        .check(&gate_req(
            "agent-2",
            "chain/transfer",
            "account/bob",
            Some(900),
        ))
        .await;
    assert_eq!(
        r3,
        GateResult::Allow,
        "agent-2 has a separate budget from agent-1"
    );
}

// ---------------------------------------------------------------------------
// Composed gate: Sequential(AllowlistGate, BudgetGate)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn composed_gate_allowlist_plus_budget() {
    let gate = ComposedGate::Sequential(vec![
        Box::new(AllowlistGate::new(
            AllowlistField::Resource,
            vec!["account/bob".to_string()],
        )),
        Box::new(BudgetGate::new(500)),
    ]);

    // Allowed resource within budget.
    let r1 = gate
        .check(&gate_req(
            "agent-1",
            "chain/transfer",
            "account/bob",
            Some(200),
        ))
        .await;
    assert_eq!(r1, GateResult::Allow);

    // Disallowed resource.
    let r2 = gate
        .check(&gate_req(
            "agent-1",
            "chain/transfer",
            "account/eve",
            Some(100),
        ))
        .await;
    assert!(
        matches!(r2, GateResult::Deny { .. }),
        "account/eve is not on the allowlist"
    );

    // Allowed resource but exceeds budget.
    let r3 = gate
        .check(&gate_req(
            "agent-1",
            "chain/transfer",
            "account/bob",
            Some(400),
        ))
        .await;
    assert!(
        matches!(r3, GateResult::Deny { .. }),
        "cumulative 600 exceeds budget of 500"
    );
}

// ---------------------------------------------------------------------------
// Empty policy set defaults to deny
// ---------------------------------------------------------------------------

#[test]
fn empty_policy_set_denies_everything() {
    let set = PolicySet::default();
    let ctx = EvaluationContext::default();
    let decision = evaluate(&set, "any/action", "any/resource", &ctx);
    assert!(
        matches!(decision, PolicyDecision::Deny { .. }),
        "empty policy set must default to deny"
    );
}
