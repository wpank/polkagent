//! PRD-15 Security Tests: Grant security.
//!
//! These tests verify the authorization layer's security invariants:
//! default-deny, expiry enforcement, budget exhaustion, rate limiting,
//! deny-override semantics, undeclared capability rejection, and
//! wrong-principal rejection.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use polkagent_core::ids::{AgentId, GrantId, RunId};
use polkagent_grant::grant::{
    ActiveGrant, EffectSet, GrantDecision, GrantLimits, GrantResolver, ResolverConfig,
};
use polkagent_grant::policy::{Effect, EvaluationContext, PolicyRule, PolicySet};
use polkagent_grant::budget::BudgetTracker;
use polkagent_grant::gate::{
    AllowlistField, AllowlistGate, BudgetGate, Gate, GateRequest, GateResult, RateLimitGate,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn empty_ctx() -> EvaluationContext {
    EvaluationContext::default()
}

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

fn deny_rule(id: &str, actions: &[&str], resources: &[&str]) -> PolicyRule {
    PolicyRule {
        id: id.to_string(),
        effect: Effect::Deny,
        action_patterns: actions.iter().map(|s| s.to_string()).collect(),
        resource_patterns: resources.iter().map(|s| s.to_string()).collect(),
        conditions: Default::default(),
        abac_condition: None,
    }
}

// ===========================================================================
// Default-deny policy blocks all actions
// ===========================================================================

#[tokio::test]
async fn default_deny_empty_policy_blocks_all() {
    let resolver = GrantResolver::new(PolicySet::default(), ResolverConfig::default());
    let decision = resolver
        .resolve("alice", "chain/transfer", "account/bob", &empty_ctx(), None, None)
        .await
        .unwrap_or_else(|e| panic!("resolver error: {e}"));

    assert!(
        matches!(decision, GrantDecision::Deny(_)),
        "empty policy must default-deny all actions, got: {decision:?}"
    );
}

#[tokio::test]
async fn default_deny_unmatched_action_blocked() {
    let mut set = PolicySet::default();
    // Only allow "chain/query", nothing else.
    set.add_rule(allow_rule("allow-query", &["chain/query"], &["**"]));

    let resolver = GrantResolver::new(set, ResolverConfig::default());
    let decision = resolver
        .resolve("alice", "chain/transfer", "account/bob", &empty_ctx(), None, None)
        .await
        .unwrap_or_else(|e| panic!("resolver error: {e}"));

    assert!(
        matches!(decision, GrantDecision::Deny(_)),
        "undeclared action 'chain/transfer' must be denied when only 'chain/query' is allowed"
    );
}

// ===========================================================================
// Expired grants are rejected
// ===========================================================================

#[tokio::test]
async fn expired_grant_is_rejected() {
    let mut set = PolicySet::default();
    set.add_rule(allow_rule("r1", &["chain/**"], &["**"]));

    let resolver = GrantResolver::new(set, ResolverConfig::default());

    // Add an already-expired grant.
    resolver
        .add_active_grant(ActiveGrant {
            grant_id: GrantId::new(),
            principal: "alice".to_string(),
            action_pattern: "chain/**".to_string(),
            resource_pattern: "**".to_string(),
            allowed_effects: EffectSet::new(vec!["chain.transfer".to_string()]),
            limits: GrantLimits::default(),
            expires_at: Utc::now() - chrono::Duration::hours(1),
        })
        .await;

    let decision = resolver
        .resolve("alice", "chain/transfer", "account/bob", &empty_ctx(), None, None)
        .await
        .unwrap_or_else(|e| panic!("resolver error: {e}"));

    // Policy allows, so a new grant is synthesized (expired one is skipped).
    // The key point is the expired grant is NOT used.
    if let GrantDecision::Permit(grant) = &decision {
        assert!(
            grant.expires_at > Utc::now(),
            "expired grant must be ignored; synthesized grant must have future expiry"
        );
    } else {
        panic!("expected Permit with new grant, got: {decision:?}");
    }
}

// ===========================================================================
// Budget exhaustion blocks further spend
// ===========================================================================

#[tokio::test]
async fn budget_exhaustion_blocks_spend() {
    let mut set = PolicySet::default();
    set.add_rule(allow_rule("r1", &["**"], &["**"]));

    let config = ResolverConfig {
        max_context_age: None,
        default_grant_ttl: chrono::Duration::hours(1),
    };
    let resolver = GrantResolver::new(set, config);

    let tracker = BudgetTracker::new();
    let agent_id = AgentId::new();
    tracker.configure(agent_id, 100).await;

    let resolver = resolver.with_budget_tracker(Arc::clone(&tracker)).await;

    let principal = agent_id.to_string();
    let run_id = RunId::new();

    // First spend: 80 of 100 -- should succeed.
    let d1 = resolver
        .resolve(&principal, "chain/transfer", "account/bob", &empty_ctx(), Some(80), Some(run_id))
        .await
        .unwrap_or_else(|e| panic!("resolver error: {e}"));
    assert!(
        matches!(d1, GrantDecision::Permit(_)),
        "first spend (80/100) must be permitted"
    );

    // Second spend: 30 more -- 80+30=110 > 100 -- should be denied.
    let d2 = resolver
        .resolve(&principal, "chain/transfer", "account/bob", &empty_ctx(), Some(30), Some(run_id))
        .await
        .unwrap_or_else(|e| panic!("resolver error: {e}"));
    assert!(
        matches!(d2, GrantDecision::Deny(_)),
        "budget-exceeding spend (110/100) must be denied, got: {d2:?}"
    );
}

// ===========================================================================
// Rate limiting prevents burst abuse
// ===========================================================================

#[tokio::test]
async fn rate_limit_prevents_burst() {
    // RateLimitGate with capacity=3, no refill within test duration.
    let gate = RateLimitGate::new(3, 1, Duration::from_secs(3600));

    let req = GateRequest {
        principal: "alice".to_string(),
        action: "chain/transfer".to_string(),
        resource: "account/bob".to_string(),
        amount: None,
        metadata: Default::default(),
    };

    // Exhaust all 3 tokens.
    for i in 0..3 {
        let result = gate.check(&req).await;
        assert_eq!(
            result,
            GateResult::Allow,
            "request {i} within capacity must be allowed"
        );
    }

    // 4th request should be denied.
    let result = gate.check(&req).await;
    assert!(
        matches!(result, GateResult::Deny { .. }),
        "request beyond capacity must be rate-limited, got: {result:?}"
    );
}

// ===========================================================================
// Deny-override semantics (deny always wins)
// ===========================================================================

#[tokio::test]
async fn deny_always_overrides_allow_regardless_of_order() {
    // Scenario 1: Allow first, Deny second.
    let mut set1 = PolicySet::default();
    set1.add_rule(allow_rule("allow-all", &["**"], &["**"]));
    set1.add_rule(deny_rule("deny-transfer", &["chain/transfer"], &["**"]));

    let resolver1 = GrantResolver::new(set1, ResolverConfig::default());
    let d1 = resolver1
        .resolve("alice", "chain/transfer", "account/bob", &empty_ctx(), None, None)
        .await
        .unwrap_or_else(|e| panic!("resolver error: {e}"));
    assert!(
        matches!(d1, GrantDecision::Deny(_)),
        "deny must override allow (allow-first order)"
    );

    // Scenario 2: Deny first, Allow second.
    let mut set2 = PolicySet::default();
    set2.add_rule(deny_rule("deny-transfer", &["chain/transfer"], &["**"]));
    set2.add_rule(allow_rule("allow-all", &["**"], &["**"]));

    let resolver2 = GrantResolver::new(set2, ResolverConfig::default());
    let d2 = resolver2
        .resolve("alice", "chain/transfer", "account/bob", &empty_ctx(), None, None)
        .await
        .unwrap_or_else(|e| panic!("resolver error: {e}"));
    assert!(
        matches!(d2, GrantDecision::Deny(_)),
        "deny must override allow (deny-first order)"
    );
}

// ===========================================================================
// Undeclared capabilities are rejected
// ===========================================================================

#[tokio::test]
async fn undeclared_capability_rejected() {
    let mut set = PolicySet::default();
    // Only allow chain/query.
    set.add_rule(allow_rule("allow-query", &["chain/query"], &["account/**"]));

    let resolver = GrantResolver::new(set, ResolverConfig::default());

    // Attempt to use chain/transfer which is NOT declared.
    let decision = resolver
        .resolve("alice", "chain/transfer", "account/bob", &empty_ctx(), None, None)
        .await
        .unwrap_or_else(|e| panic!("resolver error: {e}"));
    assert!(
        matches!(decision, GrantDecision::Deny(_)),
        "undeclared capability must be rejected"
    );

    // Attempt to use governance/vote which is NOT declared.
    let decision2 = resolver
        .resolve("alice", "governance/vote", "referendum/1", &empty_ctx(), None, None)
        .await
        .unwrap_or_else(|e| panic!("resolver error: {e}"));
    assert!(
        matches!(decision2, GrantDecision::Deny(_)),
        "undeclared governance capability must be rejected"
    );
}

// ===========================================================================
// Grant with wrong principal is rejected
// ===========================================================================

#[tokio::test]
async fn grant_with_wrong_principal_is_rejected() {
    let mut set = PolicySet::default();
    set.add_rule(allow_rule("r1", &["chain/**"], &["**"]));

    let resolver = GrantResolver::new(set, ResolverConfig::default());

    // Add a grant for "alice" only.
    resolver
        .add_active_grant(ActiveGrant {
            grant_id: GrantId::new(),
            principal: "alice".to_string(),
            action_pattern: "chain/**".to_string(),
            resource_pattern: "**".to_string(),
            allowed_effects: EffectSet::new(vec!["chain.transfer".to_string()]),
            limits: GrantLimits::default(),
            expires_at: Utc::now() + chrono::Duration::hours(1),
        })
        .await;

    // "bob" tries to use alice's grant pattern -- should NOT reuse alice's grant.
    let decision = resolver
        .resolve("bob", "chain/transfer", "account/charlie", &empty_ctx(), None, None)
        .await
        .unwrap_or_else(|e| panic!("resolver error: {e}"));

    // Policy allows chain/**, so bob gets a NEW grant (not alice's).
    if let GrantDecision::Permit(grant) = &decision {
        assert_ne!(
            grant.principal, "alice",
            "bob must not receive alice's grant"
        );
        assert_eq!(grant.principal, "bob");
    } else {
        panic!("expected Permit for bob (policy allows), got: {decision:?}");
    }
}

// ===========================================================================
// Allowlist gate: empty allowlist denies everything (fail-closed)
// ===========================================================================

#[tokio::test]
async fn empty_allowlist_denies_all() {
    let gate = AllowlistGate::new(AllowlistField::Resource, std::iter::empty::<String>());
    let req = GateRequest {
        principal: "alice".to_string(),
        action: "chain/transfer".to_string(),
        resource: "any-address".to_string(),
        amount: None,
        metadata: Default::default(),
    };
    let result = gate.check(&req).await;
    assert!(
        matches!(result, GateResult::Deny { .. }),
        "empty allowlist must deny all (fail-closed)"
    );
}

// ===========================================================================
// Budget gate: cumulative spend tracking
// ===========================================================================

#[tokio::test]
async fn budget_gate_tracks_cumulative_spend() {
    let gate = BudgetGate::new(100);
    let req = |amount| GateRequest {
        principal: "alice".to_string(),
        action: "chain/transfer".to_string(),
        resource: "account/bob".to_string(),
        amount: Some(amount),
        metadata: Default::default(),
    };

    // Spend 60, then 50 -- cumulative 110 > 100.
    let r1 = gate.check(&req(60)).await;
    assert_eq!(r1, GateResult::Allow, "first spend (60/100) must be allowed");

    let r2 = gate.check(&req(50)).await;
    assert!(
        matches!(r2, GateResult::Deny { .. }),
        "cumulative spend (110/100) must be denied"
    );
}

// ===========================================================================
// Stale context is denied
// ===========================================================================

#[tokio::test]
async fn stale_context_produces_denial() {
    let mut set = PolicySet::default();
    set.add_rule(allow_rule("r1", &["**"], &["**"]));

    let config = ResolverConfig {
        max_context_age: Some(chrono::Duration::seconds(60)),
        default_grant_ttl: chrono::Duration::hours(1),
    };
    let resolver = GrantResolver::new(set, config);

    // Set evaluated_at to 10 minutes ago (exceeds 60s max).
    let stale_ts = (Utc::now() - chrono::Duration::minutes(10))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let ctx = EvaluationContext {
        attributes: Default::default(),
        typed_attributes: Default::default(),
        evaluated_at: Some(stale_ts),
    };

    let decision = resolver
        .resolve("alice", "chain/transfer", "account/bob", &ctx, None, None)
        .await
        .unwrap_or_else(|e| panic!("resolver error: {e}"));

    assert!(
        matches!(decision, GrantDecision::Deny(_)),
        "stale context must produce denial"
    );
}
