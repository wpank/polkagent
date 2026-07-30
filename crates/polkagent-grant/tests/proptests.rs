//! Property-based tests for polkagent-grant.
//!
//! These tests use `proptest` to verify invariants of the policy evaluation
//! engine, budget tracker, and gate composition system across randomly
//! generated inputs.

use std::collections::HashMap;

use proptest::prelude::*;

use polkagent_grant::gate::{ComposedGate, Gate, GateRequest, GateResult};
use polkagent_grant::policy::{
    evaluate, pattern_matches, Effect, EvaluationContext, PolicyDecision, PolicyRule, PolicySet,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a `PolicyRule` with the given effect, patterns, and no conditions.
fn rule(id: &str, effect: Effect, actions: &[&str], resources: &[&str]) -> PolicyRule {
    PolicyRule {
        id: id.to_string(),
        effect,
        action_patterns: actions.iter().map(|s| s.to_string()).collect(),
        resource_patterns: resources.iter().map(|s| s.to_string()).collect(),
        conditions: HashMap::new(),
    }
}

fn empty_ctx() -> EvaluationContext {
    EvaluationContext {
        attributes: HashMap::new(),
        evaluated_at: None,
    }
}

/// Build a minimal [`GateRequest`].
fn gate_req(principal: &str, resource: &str) -> GateRequest {
    GateRequest {
        principal: principal.to_string(),
        action: "test/action".to_string(),
        resource: resource.to_string(),
        amount: None,
        metadata: HashMap::new(),
    }
}

/// Strategy for a non-empty path segment: 1-8 alphanumeric chars, no `/` or `*`.
fn segment() -> impl Strategy<Value = String> {
    "[a-z0-9]{1,8}"
}

/// Strategy for a slash-separated path with 1-4 segments.
fn path_value() -> impl Strategy<Value = String> {
    prop::collection::vec(segment(), 1..=4).prop_map(|segs| segs.join("/"))
}

// ===========================================================================
// 1. Policy evaluation properties
// ===========================================================================

proptest! {
    // ----- 1a: Empty policy set always denies (default deny) ----------------

    #[test]
    fn empty_policy_always_denies(
        action in path_value(),
        resource in path_value(),
    ) {
        let set = PolicySet::default();
        let decision = evaluate(&set, &action, &resource, &empty_ctx());
        prop_assert!(
            matches!(decision, PolicyDecision::Deny { .. }),
            "empty policy set must deny for action={action}, resource={resource}"
        );
    }

    // ----- 1b: Adding a Deny rule never makes a previously denied action allowed ----

    #[test]
    fn adding_deny_never_allows_previously_denied(
        action in path_value(),
        resource in path_value(),
        extra_action in path_value(),
        extra_resource in path_value(),
    ) {
        // Start with a set that has one allow rule (may or may not match).
        let allow = rule("allow", Effect::Allow, &["specific/action"], &["specific/resource"]);
        let set_before = PolicySet::new(vec![allow.clone()]);
        let decision_before = evaluate(&set_before, &action, &resource, &empty_ctx());

        // Now add a deny rule on top.
        let deny = rule("deny", Effect::Deny, &[&extra_action], &[&extra_resource]);
        let set_after = PolicySet::new(vec![allow, deny]);
        let decision_after = evaluate(&set_after, &action, &resource, &empty_ctx());

        // If the action was denied before, it must still be denied after adding
        // a Deny rule. (A new Deny rule can only deny more things, never fewer.)
        if matches!(decision_before, PolicyDecision::Deny { .. }) {
            prop_assert!(
                matches!(decision_after, PolicyDecision::Deny { .. }),
                "adding a deny rule must not allow a previously denied action"
            );
        }
    }

    // ----- 1c: Deny-overrides — if any rule denies, result is deny ----------

    #[test]
    fn deny_overrides_all_allows(
        action in path_value(),
        resource in path_value(),
        num_allows in 1usize..=5,
    ) {
        let mut rules = Vec::new();

        // Add N allow-all rules.
        for i in 0..num_allows {
            rules.push(rule(&format!("allow-{i}"), Effect::Allow, &["**"], &["**"]));
        }
        // Add one deny rule matching the exact action and resource.
        rules.push(rule("deny-exact", Effect::Deny, &[&action], &[&resource]));

        let set = PolicySet::new(rules);
        let decision = evaluate(&set, &action, &resource, &empty_ctx());

        prop_assert!(
            matches!(decision, PolicyDecision::Deny { .. }),
            "deny must override {} allow rules for action={action}, resource={resource}",
            num_allows
        );
    }

    // ----- 1d: Exact pattern always matches itself ---------------------------

    #[test]
    fn exact_pattern_matches_itself(value in path_value()) {
        prop_assert!(
            pattern_matches(&value, &value),
            "pattern '{value}' must match itself"
        );
    }

    // ----- 1e: `*` never matches across `/` separators ----------------------

    #[test]
    fn star_never_matches_across_slash(
        prefix in segment(),
        seg1 in segment(),
        seg2 in segment(),
    ) {
        let pattern = format!("{prefix}/*");
        let value = format!("{prefix}/{seg1}/{seg2}");

        prop_assert!(
            !pattern_matches(&pattern, &value),
            "single * in '{pattern}' must not match '{value}' (crosses /)"
        );
    }

    // ----- 1f: `**` matches any depth ----------------------------------------

    #[test]
    fn double_star_matches_any_depth(
        prefix in segment(),
        value in path_value(),
    ) {
        let pattern = format!("{prefix}/**");
        let target = format!("{prefix}/{value}");

        prop_assert!(
            pattern_matches(&pattern, &target),
            "'**' in '{pattern}' must match '{target}'"
        );
    }
}

// ===========================================================================
// 2. Budget tracking properties
// ===========================================================================

// These tests exercise the async BudgetTracker. proptest doesn't natively
// support async, so we use `tokio::runtime::Runtime::block_on` inside each
// test case.

mod budget_props {
    use super::*;
    use polkagent_grant::budget::BudgetTracker;
    use polkagent_core::ids::{AgentId, RunId};

    proptest! {
        // ----- 2a: Spending within budget always succeeds --------------------

        #[test]
        fn spend_within_budget_succeeds(
            max_spend in 1u64..=1_000_000,
            amount in 1u64..=1_000_000,
        ) {
            // Only test when the amount fits.
            prop_assume!(amount <= max_spend);

            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let tracker = BudgetTracker::new();
                let agent = AgentId::new();
                let run = RunId::new();
                tracker.configure(agent, max_spend).await;

                let result = tracker.record_spend(agent, run, amount).await;
                prop_assert!(
                    result.is_ok(),
                    "spending {amount} within budget {max_spend} must succeed"
                );
                Ok(())
            })?;
        }

        // ----- 2b: Spending over budget always fails -------------------------

        #[test]
        fn spend_over_budget_fails(
            max_spend in 0u64..=999_999,
            overshoot in 1u64..=1_000_000,
        ) {
            let amount = max_spend.saturating_add(overshoot);
            // Guard against overflow wrapping back to a valid amount.
            prop_assume!(amount > max_spend);

            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let tracker = BudgetTracker::new();
                let agent = AgentId::new();
                let run = RunId::new();
                tracker.configure(agent, max_spend).await;

                let result = tracker.record_spend(agent, run, amount).await;
                prop_assert!(
                    result.is_err(),
                    "spending {amount} over budget {max_spend} must fail"
                );
                Ok(())
            })?;
        }

        // ----- 2c: Total spent never exceeds max_spend -----------------------

        #[test]
        fn total_spent_never_exceeds_max(
            max_spend in 100u64..=100_000,
            amounts in prop::collection::vec(1u64..=200, 1..=10),
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let tracker = BudgetTracker::new();
                let agent = AgentId::new();
                tracker.configure(agent, max_spend).await;

                for (i, &amount) in amounts.iter().enumerate() {
                    let run = RunId::new();
                    let _ = tracker.record_spend(agent, run, amount).await;
                    // After each attempt, check the invariant.
                    let status = tracker.get_remaining(agent).await.unwrap();
                    prop_assert!(
                        status.spent <= max_spend,
                        "after attempt {i}: spent={} must not exceed max_spend={max_spend}",
                        status.spent
                    );
                }
                Ok(())
            })?;
        }

        // ----- 2d: remaining = max - spent (always non-negative) -------------

        #[test]
        fn remaining_equals_max_minus_spent(
            max_spend in 1u64..=1_000_000,
            amount in 0u64..=1_000_000,
        ) {
            prop_assume!(amount <= max_spend);

            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let tracker = BudgetTracker::new();
                let agent = AgentId::new();
                let run = RunId::new();
                tracker.configure(agent, max_spend).await;

                tracker.record_spend(agent, run, amount).await.unwrap();
                let status = tracker.get_remaining(agent).await.unwrap();

                prop_assert_eq!(
                    status.remaining,
                    max_spend - amount,
                    "remaining must equal max_spend - spent"
                );
                prop_assert!(
                    status.remaining <= max_spend,
                    "remaining must be non-negative (at most max_spend)"
                );
                Ok(())
            })?;
        }

        // ----- 2e: Recording 0 spend always succeeds and doesn't change balance ----

        #[test]
        fn zero_spend_is_noop(max_spend in 0u64..=1_000_000) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let tracker = BudgetTracker::new();
                let agent = AgentId::new();
                let run = RunId::new();
                tracker.configure(agent, max_spend).await;

                let result = tracker.record_spend(agent, run, 0).await;
                prop_assert!(result.is_ok(), "0 spend must always succeed");

                let status = tracker.get_remaining(agent).await.unwrap();
                prop_assert_eq!(status.spent, 0, "0 spend must not change balance");
                prop_assert_eq!(
                    status.remaining, max_spend,
                    "remaining must equal max_spend after 0 spend"
                );
                Ok(())
            })?;
        }
    }
}

// ===========================================================================
// 3. Gate composition properties
// ===========================================================================

/// A deterministic gate that always returns the configured result.
struct FixedGate {
    result: GateResult,
    label: String,
}

impl FixedGate {
    fn allow(label: &str) -> Box<dyn Gate> {
        Box::new(Self {
            result: GateResult::Allow,
            label: label.to_string(),
        })
    }

    fn deny(label: &str) -> Box<dyn Gate> {
        Box::new(Self {
            result: GateResult::Deny {
                reason: format!("{label} denied"),
            },
            label: label.to_string(),
        })
    }

    fn escalate(label: &str) -> Box<dyn Gate> {
        Box::new(Self {
            result: GateResult::Escalate {
                reason: format!("{label} escalated"),
            },
            label: label.to_string(),
        })
    }
}

#[async_trait::async_trait]
impl Gate for FixedGate {
    async fn check(&self, _request: &GateRequest) -> GateResult {
        self.result.clone()
    }

    fn name(&self) -> &str {
        &self.label
    }
}

mod gate_props {
    use super::*;

    // ----- 3a: And(gates) denies if any gate denies --------------------------

    proptest! {
        #[test]
        fn and_denies_if_any_deny(
            deny_position in 0usize..5,
            total in 1usize..=5,
        ) {
            let deny_pos = deny_position % total;

            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let gates: Vec<Box<dyn Gate>> = (0..total)
                    .map(|i| {
                        if i == deny_pos {
                            FixedGate::deny(&format!("gate-{i}"))
                        } else {
                            FixedGate::allow(&format!("gate-{i}"))
                        }
                    })
                    .collect();

                let composed = ComposedGate::And(gates);
                let result = composed.check(&gate_req("alice", "res")).await;
                prop_assert!(
                    matches!(result, GateResult::Deny { .. }),
                    "And must deny when gate at position {deny_pos} of {total} denies"
                );
                Ok(())
            })?;
        }

        // ----- 3a (supplement): And with all allows returns Allow -----

        #[test]
        fn and_allows_when_all_allow(count in 1usize..=8) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let gates: Vec<Box<dyn Gate>> = (0..count)
                    .map(|i| FixedGate::allow(&format!("gate-{i}")))
                    .collect();

                let composed = ComposedGate::And(gates);
                let result = composed.check(&gate_req("alice", "res")).await;
                prop_assert!(
                    result == GateResult::Allow,
                    "And with {count} allow gates must allow"
                );
                Ok(())
            })?;
        }
    }

    // ----- 3b: Or(gates) allows if any gate allows ---------------------------

    proptest! {
        #[test]
        fn or_allows_if_any_allow(
            allow_position in 0usize..5,
            total in 1usize..=5,
        ) {
            let allow_pos = allow_position % total;

            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let gates: Vec<Box<dyn Gate>> = (0..total)
                    .map(|i| {
                        if i == allow_pos {
                            FixedGate::allow(&format!("gate-{i}"))
                        } else {
                            FixedGate::deny(&format!("gate-{i}"))
                        }
                    })
                    .collect();

                let composed = ComposedGate::Or(gates);
                let result = composed.check(&gate_req("alice", "res")).await;
                prop_assert!(
                    result == GateResult::Allow,
                    "Or must allow when gate at position {allow_pos} of {total} allows"
                );
                Ok(())
            })?;
        }

        // ----- 3b (supplement): Or with all denies returns Deny -----

        #[test]
        fn or_denies_when_all_deny(count in 1usize..=8) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let gates: Vec<Box<dyn Gate>> = (0..count)
                    .map(|i| FixedGate::deny(&format!("gate-{i}")))
                    .collect();

                let composed = ComposedGate::Or(gates);
                let result = composed.check(&gate_req("alice", "res")).await;
                prop_assert!(
                    matches!(result, GateResult::Deny { .. }),
                    "Or with {count} deny gates must deny"
                );
                Ok(())
            })?;
        }
    }

    // ----- 3c: Sequential gates short-circuit on first deny ------------------

    proptest! {
        #[test]
        fn sequential_short_circuits_on_first_deny(
            deny_position in 0usize..5,
            total in 1usize..=5,
        ) {
            let deny_pos = deny_position % total;

            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let gates: Vec<Box<dyn Gate>> = (0..total)
                    .map(|i| {
                        if i == deny_pos {
                            FixedGate::deny(&format!("gate-{i}"))
                        } else {
                            FixedGate::allow(&format!("gate-{i}"))
                        }
                    })
                    .collect();

                let composed = ComposedGate::Sequential(gates);
                let result = composed.check(&gate_req("alice", "res")).await;
                prop_assert!(
                    matches!(result, GateResult::Deny { .. }),
                    "Sequential must deny when gate at position {deny_pos} of {total} denies"
                );
                Ok(())
            })?;
        }

        #[test]
        fn sequential_short_circuits_on_first_escalate(
            escalate_position in 0usize..5,
            total in 1usize..=5,
        ) {
            let esc_pos = escalate_position % total;

            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let gates: Vec<Box<dyn Gate>> = (0..total)
                    .map(|i| {
                        if i == esc_pos {
                            FixedGate::escalate(&format!("gate-{i}"))
                        } else {
                            FixedGate::allow(&format!("gate-{i}"))
                        }
                    })
                    .collect();

                let composed = ComposedGate::Sequential(gates);
                let result = composed.check(&gate_req("alice", "res")).await;
                prop_assert!(
                    matches!(result, GateResult::Escalate { .. }),
                    "Sequential must escalate when gate at position {esc_pos} of {total} escalates"
                );
                Ok(())
            })?;
        }

        // ----- 3c (supplement): Sequential with all allows returns Allow -----

        #[test]
        fn sequential_allows_when_all_allow(count in 1usize..=8) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let gates: Vec<Box<dyn Gate>> = (0..count)
                    .map(|i| FixedGate::allow(&format!("gate-{i}")))
                    .collect();

                let composed = ComposedGate::Sequential(gates);
                let result = composed.check(&gate_req("alice", "res")).await;
                prop_assert!(
                    result == GateResult::Allow,
                    "Sequential with {count} allow gates must allow"
                );
                Ok(())
            })?;
        }
    }
}
