//! Criterion benchmarks for the grant/policy evaluation performance-critical paths.
//!
//! Covers:
//! - Simple allow: single rule, matching action/resource
//! - Complex deny-overrides: 10 rules, one deny at position 9
//! - Grant intersection via GrantResolver::resolve
//! - Budget check via BudgetTracker
//!
//! PRD-15 performance benchmarks.

use chrono::{Duration, Utc};
use criterion::{criterion_group, criterion_main, Criterion};

use polkagent_core::ids::{AgentId, GrantId, RunId};
use polkagent_grant::{
    budget::BudgetTracker,
    grant::{ActiveGrant, EffectSet, GrantLimits, GrantResolver, ResolverConfig},
    policy::{Effect, EvaluationContext, PolicyRule, PolicySet, evaluate},
};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn make_allow_rule(id: &str, action: &str, resource: &str) -> PolicyRule {
    PolicyRule {
        id: id.to_string(),
        effect: Effect::Allow,
        action_patterns: vec![action.to_string()],
        resource_patterns: vec![resource.to_string()],
        conditions: Default::default(),
        abac_condition: None,
    }
}

fn make_deny_rule(id: &str, action: &str, resource: &str) -> PolicyRule {
    PolicyRule {
        id: id.to_string(),
        effect: Effect::Deny,
        action_patterns: vec![action.to_string()],
        resource_patterns: vec![resource.to_string()],
        conditions: Default::default(),
        abac_condition: None,
    }
}

/// One allow rule, action "chain/transfer", resource "account/**".
fn simple_allow_set() -> PolicySet {
    PolicySet::new(vec![
        make_allow_rule("allow-transfer", "chain/transfer", "account/**"),
    ])
}

/// 10 rules: first 9 are allow for unrelated actions, rule 10 is a deny.
fn complex_deny_set() -> PolicySet {
    let mut rules: Vec<PolicyRule> = (0..9)
        .map(|i| make_allow_rule(&format!("allow-{i}"), &format!("action/{i}"), "**"))
        .collect();
    rules.push(make_deny_rule("deny-final", "chain/transfer", "account/**"));
    PolicySet::new(rules)
}

fn simple_ctx() -> EvaluationContext {
    EvaluationContext::default()
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

fn bench_policy_simple_allow(c: &mut Criterion) {
    let set = simple_allow_set();
    let ctx = simple_ctx();

    c.bench_function("policy_evaluate_simple_allow", |b| {
        b.iter(|| {
            let decision = evaluate(&set, "chain/transfer", "account/alice", &ctx);
            criterion::black_box(decision);
        });
    });
}

fn bench_policy_complex_deny(c: &mut Criterion) {
    let set = complex_deny_set();
    let ctx = simple_ctx();

    c.bench_function("policy_evaluate_complex_deny_overrides_10_rules", |b| {
        b.iter(|| {
            let decision = evaluate(&set, "chain/transfer", "account/alice", &ctx);
            // Should be Deny because the final rule denies this action/resource.
            criterion::black_box(decision);
        });
    });
}

fn bench_grant_intersection(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let allow_set = simple_allow_set();
    let resolver = GrantResolver::new(allow_set, {
        // Disable context freshness check for benchmarks.
        ResolverConfig {
            max_context_age: None,
            default_grant_ttl: Duration::hours(1),
        }
    });

    let ctx = EvaluationContext::default();

    c.bench_function("grant_resolver_resolve_policy_only", |b| {
        b.iter(|| {
            rt.block_on(async {
                let decision = resolver
                    .resolve("alice", "chain/transfer", "account/bob", &ctx, None, None)
                    .await
                    .expect("resolve");
                criterion::black_box(decision);
            });
        });
    });

    // With a pre-issued active grant — exercises the active-grant lookup path.
    let resolver_with_grant = rt.block_on(async {
        let r = GrantResolver::new(PolicySet::default(), ResolverConfig {
            max_context_age: None,
            default_grant_ttl: Duration::hours(1),
        });
        r.add_active_grant(ActiveGrant {
            grant_id: GrantId::new(),
            principal: "alice".to_string(),
            action_pattern: "chain/**".to_string(),
            resource_pattern: "account/**".to_string(),
            allowed_effects: EffectSet::new(["chain.transfer".to_string()]),
            limits: GrantLimits::default(),
            expires_at: Utc::now() + Duration::hours(1),
        })
        .await;
        r
    });

    c.bench_function("grant_resolver_resolve_active_grant_hit", |b| {
        b.iter(|| {
            rt.block_on(async {
                let decision = resolver_with_grant
                    .resolve("alice", "chain/transfer", "account/bob", &ctx, None, None)
                    .await
                    .expect("resolve");
                criterion::black_box(decision);
            });
        });
    });
}

fn bench_budget_check(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let tracker = BudgetTracker::new();
    let agent = AgentId::new();
    rt.block_on(async {
        tracker.configure(agent, 1_000_000_000).await;
    });

    c.bench_function("budget_check_within_budget", |b| {
        b.iter(|| {
            rt.block_on(async {
                let ok = tracker.check_budget(agent, 500).await.expect("check");
                criterion::black_box(ok);
            });
        });
    });

    // Record spend first so we have non-zero state.
    let run_id = RunId::new();
    rt.block_on(async {
        tracker
            .record_spend(agent, run_id, 100_000)
            .await
            .expect("record");
    });

    c.bench_function("budget_check_after_spend", |b| {
        b.iter(|| {
            rt.block_on(async {
                let ok = tracker.check_budget(agent, 500).await.expect("check");
                criterion::black_box(ok);
            });
        });
    });

    c.bench_function("budget_record_spend", |b| {
        let tracker2 = BudgetTracker::new();
        let agent2 = AgentId::new();
        rt.block_on(async {
            tracker2.configure(agent2, 1_000_000_000_000).await;
        });
        let run2 = RunId::new();
        b.iter(|| {
            rt.block_on(async {
                tracker2
                    .record_spend(agent2, run2, 1)
                    .await
                    .expect("record");
            });
        });
    });
}

criterion_group!(
    benches,
    bench_policy_simple_allow,
    bench_policy_complex_deny,
    bench_grant_intersection,
    bench_budget_check,
);
criterion_main!(benches);
