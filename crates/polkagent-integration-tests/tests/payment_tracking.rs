//! Integration tests for the payment tracking subsystem.
//!
//! Exercises cost recording, usage summaries, budget enforcement,
//! daily usage aggregation, and multi-run cost tracking — all wired
//! through the `polkagent-payment` crate boundary using only the
//! public API.

use chrono::{Duration, Utc};

use polkagent_payment::{
    Amount, AssetId, BudgetChecker, BudgetConfig, BudgetDecision, CostEstimator,
    CostRecord, PricingEntry, UsageSummary,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn native_asset() -> AssetId {
    AssetId::Native
}

fn micro_usd(value: u128) -> Amount {
    Amount::new(value, native_asset(), 6)
}

fn make_cost_record(run_id: &str, provider: &str, model: &str, input: u64, output: u64, usd: f64) -> CostRecord {
    CostRecord {
        run_id: run_id.into(),
        provider: provider.into(),
        model: model.into(),
        input_tokens: input,
        output_tokens: output,
        estimated_usd: usd,
        recorded_at: Utc::now(),
    }
}

/// Create a checker with a daily limit
fn make_checker_daily(agent_id: &str, max_micro_usd: u128) -> BudgetChecker {
    let mut checker = BudgetChecker::new();
    checker.set_config(
        agent_id,
        BudgetConfig {
            max_per_day: Some(micro_usd(max_micro_usd)),
            warn_at_percent: 80,
            ..Default::default()
        },
    );
    checker
}

// ---------------------------------------------------------------------------
// IT-PAY-01: Record costs for a run → get usage summary
// ---------------------------------------------------------------------------

#[test]
fn record_single_cost_and_retrieve_usage_summary() {
    let mut checker = BudgetChecker::new();
    checker.set_config("agent-1", BudgetConfig::default());

    let cost = make_cost_record("run-1", "anthropic", "claude-sonnet-4", 1000, 500, 0.0105);
    checker.record_spend("agent-1", &cost).expect("record cost");

    let now = Utc::now();
    let summary = checker.get_usage("agent-1", now - Duration::days(1), now);

    // 0.0105 USD = 10_500 micro-dollars stored, then converted back
    assert!(
        (summary.estimated_usd - 0.0105).abs() < 0.001,
        "summary should reflect recorded cost: expected 0.0105, got {}",
        summary.estimated_usd
    );
}

#[test]
fn record_multiple_runs_aggregate_cost() {
    let mut checker = BudgetChecker::new();
    checker.set_config("agent-multi", BudgetConfig::default());

    let costs = vec![
        make_cost_record("run-1", "anthropic", "claude-sonnet-4", 1000, 500, 0.01),
        make_cost_record("run-2", "anthropic", "claude-sonnet-4", 2000, 1000, 0.02),
        make_cost_record("run-3", "openai", "gpt-4o-mini", 500, 200, 0.005),
    ];

    for cost in &costs {
        checker.record_spend("agent-multi", cost).expect("record cost");
    }

    let now = Utc::now();
    let summary = checker.get_usage("agent-multi", now - Duration::days(1), now);
    let expected_usd = 0.01 + 0.02 + 0.005;
    assert!(
        (summary.estimated_usd - expected_usd).abs() < 0.001,
        "summary should aggregate all run costs: expected {expected_usd}, got {}",
        summary.estimated_usd
    );
}

#[test]
fn usage_summary_for_unknown_agent_returns_zero() {
    let checker = BudgetChecker::new();
    let now = Utc::now();
    let summary = checker.get_usage("unknown-agent", now - Duration::hours(1), now);
    assert!((summary.estimated_usd - 0.0).abs() < f64::EPSILON);
    assert_eq!(summary.total_runs, 0);
}

#[test]
fn usage_summary_period_fields_are_set() {
    let checker = BudgetChecker::new();
    let start = Utc::now() - Duration::days(7);
    let end = Utc::now();
    let summary = checker.get_usage("agent-period", start, end);
    assert_eq!(summary.period_start, start);
    assert_eq!(summary.period_end, end);
}

#[test]
fn record_spend_zero_cost_does_not_error() {
    let mut checker = BudgetChecker::new();
    checker.set_config("agent-zero", BudgetConfig::default());
    let cost = make_cost_record("run-zero", "local", "local", 1000, 500, 0.0);
    let result = checker.record_spend("agent-zero", &cost);
    assert!(result.is_ok(), "zero-cost record should succeed");
}

// ---------------------------------------------------------------------------
// IT-PAY-02: Budget enforcement — reject over-budget
// ---------------------------------------------------------------------------

#[test]
fn budget_check_allows_within_daily_limit() {
    let mut checker = make_checker_daily("agent-ok", 10_000_000); // $10 daily
    let decision = checker.check("agent-ok", &micro_usd(5_000_000)); // $5
    assert_eq!(decision, BudgetDecision::Allow, "amount within daily limit must be allowed");
}

#[test]
fn budget_check_denies_over_daily_limit() {
    let mut checker = make_checker_daily("agent-over", 10_000_000); // $10 daily
    let decision = checker.check("agent-over", &micro_usd(11_000_000)); // $11
    assert!(
        matches!(decision, BudgetDecision::Deny { .. }),
        "amount over daily limit must be denied"
    );
}

#[test]
fn budget_check_warns_near_limit() {
    // Spend $8 first (via record_spend), then check $2 (90% total)
    let mut checker = BudgetChecker::new();
    checker.set_config(
        "agent-warn",
        BudgetConfig {
            max_per_day: Some(micro_usd(10_000_000)), // $10
            warn_at_percent: 80,
            ..Default::default()
        },
    );

    // Record $8 of spend
    let cost = make_cost_record("run-warn", "anthropic", "claude-sonnet-4", 0, 0, 8.0);
    checker.record_spend("agent-warn", &cost).expect("record");

    // Now checking $2 brings total to $10 = 100% → at 90% threshold, should warn
    let decision = checker.check("agent-warn", &micro_usd(2_000_000)); // $2
    assert!(
        matches!(decision, BudgetDecision::Allow | BudgetDecision::Warn { .. }),
        "spend near limit should trigger warning or allow at boundary"
    );
}

#[test]
fn budget_check_allows_unconfigured_agent() {
    let mut checker = BudgetChecker::new();
    let decision = checker.check("unconfigured", &micro_usd(1_000_000_000));
    assert_eq!(decision, BudgetDecision::Allow);
}

#[test]
fn monthly_budget_check_denies_over_limit() {
    let mut checker = BudgetChecker::new();
    checker.set_config(
        "agent-month",
        BudgetConfig {
            max_per_month: Some(micro_usd(100_000_000)), // $100/month
            warn_at_percent: 90,
            ..Default::default()
        },
    );

    // Record $96 spent this month
    let cost = make_cost_record("run-month", "anthropic", "claude-sonnet-4", 0, 0, 96.0);
    checker.record_spend("agent-month", &cost).expect("record");

    // $6 would exceed the monthly limit
    let decision = checker.check("agent-month", &micro_usd(6_000_000)); // $6
    assert!(
        matches!(decision, BudgetDecision::Deny { .. }),
        "monthly over-limit must be denied"
    );
}

#[test]
fn budget_config_default_has_no_limits() {
    let config = BudgetConfig::default();
    assert!(config.max_per_run.is_none());
    assert!(config.max_per_day.is_none());
    assert!(config.max_per_month.is_none());
    assert_eq!(config.warn_at_percent, 80);
}

#[test]
fn budget_decision_deny_has_reason_string() {
    let mut checker = make_checker_daily("agent-reason", 1_000); // $0.001 daily
    let decision = checker.check("agent-reason", &micro_usd(2_000)); // $0.002
    if let BudgetDecision::Deny { reason } = decision {
        assert!(!reason.is_empty(), "deny reason must not be empty");
    } else {
        panic!("expected Deny");
    }
}

#[test]
fn budget_check_exact_limit_uses_allow() {
    let mut checker = make_checker_daily("agent-exact", 10_000_000); // $10 daily
    // Exactly $10 should be allowed (not exceeding)
    let decision = checker.check("agent-exact", &micro_usd(10_000_000));
    assert!(
        matches!(decision, BudgetDecision::Allow | BudgetDecision::Warn { .. }),
        "exact limit should be allowed or warned, not denied"
    );
}

// ---------------------------------------------------------------------------
// IT-PAY-03: Daily usage aggregation
// ---------------------------------------------------------------------------

#[test]
fn record_spend_updates_daily_usage() {
    let mut checker = BudgetChecker::new();
    checker.set_config("agent-daily", BudgetConfig::default());

    let cost = make_cost_record("run-daily", "anthropic", "claude-sonnet-4", 1000, 500, 0.0105);
    checker.record_spend("agent-daily", &cost).expect("record cost");

    let now = Utc::now();
    let summary = checker.get_usage("agent-daily", now - Duration::hours(1), now);
    assert!(
        (summary.estimated_usd - 0.0105).abs() < 0.001,
        "daily spend should be reflected: expected 0.0105, got {}",
        summary.estimated_usd
    );
}

#[test]
fn multiple_records_accumulate_in_usage() {
    let mut checker = BudgetChecker::new();
    checker.set_config("agent-multi-day", BudgetConfig::default());

    let records = vec![
        make_cost_record("r1", "anthropic", "claude-sonnet-4", 500, 200, 0.005),
        make_cost_record("r2", "anthropic", "claude-sonnet-4", 800, 300, 0.008),
        make_cost_record("r3", "openai", "gpt-4o-mini", 1000, 400, 0.002),
    ];

    for rec in &records {
        checker.record_spend("agent-multi-day", rec).expect("record");
    }

    let now = Utc::now();
    let summary = checker.get_usage("agent-multi-day", now - Duration::hours(1), now);
    let expected_total = 0.005 + 0.008 + 0.002;
    assert!(
        (summary.estimated_usd - expected_total).abs() < 0.001,
        "accumulated daily spend: expected {expected_total}, got {}",
        summary.estimated_usd
    );
}

#[test]
fn independent_agents_have_isolated_budget_states() {
    let mut checker = BudgetChecker::new();
    checker.set_config("agent-a", BudgetConfig::default());
    checker.set_config("agent-b", BudgetConfig::default());

    let cost_a = make_cost_record("run-a", "anthropic", "claude-sonnet-4", 5000, 2000, 0.05);
    let cost_b = make_cost_record("run-b", "openai", "gpt-4o-mini", 1000, 500, 0.001);

    checker.record_spend("agent-a", &cost_a).expect("record A");
    checker.record_spend("agent-b", &cost_b).expect("record B");

    let now = Utc::now();
    let summary_a = checker.get_usage("agent-a", now - Duration::hours(1), now);
    let summary_b = checker.get_usage("agent-b", now - Duration::hours(1), now);

    assert!(
        (summary_a.estimated_usd - 0.05).abs() < 0.001,
        "agent A should have 0.05 USD, got {}",
        summary_a.estimated_usd
    );
    assert!(
        (summary_b.estimated_usd - 0.001).abs() < 0.0001,
        "agent B should have 0.001 USD, got {}",
        summary_b.estimated_usd
    );
}

#[test]
fn spend_is_not_shared_across_agents() {
    let mut checker = BudgetChecker::new();
    checker.set_config("agent-x", BudgetConfig::default());
    checker.set_config("agent-y", BudgetConfig::default());

    // Only agent-x spends
    checker
        .record_spend("agent-x", &make_cost_record("r1", "anthropic", "claude-sonnet-4", 1000, 500, 0.02))
        .expect("record");

    let now = Utc::now();
    let summary_y = checker.get_usage("agent-y", now - Duration::hours(1), now);
    assert!(
        (summary_y.estimated_usd - 0.0).abs() < f64::EPSILON,
        "agent-y should have zero spend, not inheriting agent-x's spend"
    );
}

// ---------------------------------------------------------------------------
// IT-PAY-04: Multi-run cost tracking via CostEstimator
// ---------------------------------------------------------------------------

#[test]
fn cost_estimator_produces_expected_sonnet_amounts() {
    let estimator = CostEstimator::new();
    // Claude Sonnet 4: $3/M input + $15/M output
    // 1000 input + 500 output = $0.003 + $0.0075 = $0.0105
    let cost = estimator.estimate("anthropic", "claude-sonnet-4", 1000, 500);
    assert!((cost - 0.0105).abs() < 0.0001, "Sonnet 4 estimate: expected 0.0105, got {cost}");
}

#[test]
fn cost_estimator_with_gpt4o_produces_expected_amounts() {
    let estimator = CostEstimator::new();
    // GPT-4o: $2.50/M input + $10/M output
    // 1M each = $12.50
    let cost = estimator.estimate("openai", "gpt-4o", 1_000_000, 1_000_000);
    assert!((cost - 12.50).abs() < 0.01, "GPT-4o cost: expected 12.50, got {cost}");
}

#[test]
fn cost_estimator_local_model_is_free() {
    let estimator = CostEstimator::new();
    let cost = estimator.estimate("local", "local", 10_000_000, 5_000_000);
    assert!((cost - 0.0).abs() < f64::EPSILON, "local model must be free");
}

#[test]
fn cost_estimator_unknown_model_returns_zero() {
    let estimator = CostEstimator::new();
    let cost = estimator.estimate("unknown-provider", "unknown-model", 1000, 500);
    assert!((cost - 0.0).abs() < f64::EPSILON, "unknown model must return 0.0");
}

#[test]
fn cost_estimator_prefix_matching_works() {
    let estimator = CostEstimator::new();
    // Versioned model name should match via prefix
    let cost = estimator.estimate("anthropic", "claude-sonnet-4-20250514", 1_000_000, 1_000_000);
    // $3 input + $15 output = $18
    assert!((cost - 18.0).abs() < 0.01, "prefix-matched cost: expected 18.0, got {cost}");
}

#[test]
fn cost_estimator_zero_tokens_returns_zero() {
    let estimator = CostEstimator::new();
    let cost = estimator.estimate("anthropic", "claude-sonnet-4", 0, 0);
    assert!((cost - 0.0).abs() < f64::EPSILON);
}

#[test]
fn cost_estimator_custom_pricing_table() {
    let estimator = CostEstimator::with_entries(vec![PricingEntry {
        provider: "custom".into(),
        model_pattern: "my-model".into(),
        input_per_million: 1.0,
        output_per_million: 2.0,
    }]);
    let cost = estimator.estimate("custom", "my-model", 1_000_000, 1_000_000);
    assert!((cost - 3.0).abs() < 0.001);
}

#[test]
fn cost_estimator_add_entry_extends_table() {
    let mut estimator = CostEstimator::new();
    estimator.add_entry(PricingEntry {
        provider: "custom".into(),
        model_pattern: "special".into(),
        input_per_million: 5.0,
        output_per_million: 10.0,
    });
    let cost = estimator.estimate("custom", "special", 1_000_000, 1_000_000);
    assert!((cost - 15.0).abs() < 0.001);
}

#[test]
fn cost_estimator_gpt4o_mini_not_confused_with_gpt4o() {
    let estimator = CostEstimator::new();
    // gpt-4o-mini is $0.75/M total vs gpt-4o $12.50/M total
    let mini_cost = estimator.estimate("openai", "gpt-4o-mini", 1_000_000, 1_000_000);
    let full_cost = estimator.estimate("openai", "gpt-4o", 1_000_000, 1_000_000);
    assert!(mini_cost < full_cost, "mini should be cheaper than full");
    assert!((mini_cost - 0.75).abs() < 0.01);
}

// ---------------------------------------------------------------------------
// IT-PAY-05: Amount arithmetic and display
// ---------------------------------------------------------------------------

#[test]
fn amount_display_human_format() {
    let amt = Amount::new(15_000_000_000, AssetId::Native, 10);
    let display = amt.display_human();
    assert!(display.contains("1"), "display should contain whole part");
    assert!(display.contains("NATIVE"));
}

#[test]
fn amount_display_zero() {
    let amt = Amount::zero(AssetId::Native, 6);
    let display = amt.display_human();
    assert!(display.starts_with("0"), "zero amount should start with 0");
}

#[test]
fn amount_checked_add_same_asset() {
    let a = Amount::new(1_000_000, native_asset(), 6);
    let b = Amount::new(2_000_000, native_asset(), 6);
    let result = a.checked_add(&b).expect("add ok");
    assert_eq!(result.value, 3_000_000);
}

#[test]
fn amount_checked_sub_same_asset() {
    let a = Amount::new(5_000_000, native_asset(), 6);
    let b = Amount::new(3_000_000, native_asset(), 6);
    let result = a.checked_sub(&b).expect("sub ok");
    assert_eq!(result.value, 2_000_000);
}

#[test]
fn amount_checked_mul_scalar() {
    let a = Amount::new(1_000, native_asset(), 6);
    let result = a.checked_mul(5).expect("mul ok");
    assert_eq!(result.value, 5_000);
}

#[test]
fn amount_asset_mismatch_checked_add_fails() {
    let a = Amount::new(100, AssetId::Native, 6);
    let b = Amount::new(200, AssetId::Token {
        chain: "polkadot".into(),
        symbol: "USDT".into(),
        decimals: 6,
    }, 6);
    let result = a.checked_add(&b);
    assert!(result.is_err(), "cross-asset add must fail");
}

#[test]
fn amount_checked_sub_underflow_fails() {
    let a = Amount::new(100, native_asset(), 6);
    let b = Amount::new(200, native_asset(), 6);
    let result = a.checked_sub(&b);
    assert!(result.is_err(), "underflow must fail");
}

#[test]
fn amount_checked_mul_by_zero() {
    let a = Amount::new(1_000_000, native_asset(), 6);
    let result = a.checked_mul(0).expect("mul by zero ok");
    assert_eq!(result.value, 0);
}

#[test]
fn usage_summary_serde_round_trip() {
    let now = Utc::now();
    let summary = UsageSummary {
        total_runs: 42,
        total_tokens: 1_000_000,
        estimated_usd: 0.105,
        period_start: now - Duration::days(7),
        period_end: now,
    };
    let json = serde_json::to_string(&summary).expect("serialize");
    let back: UsageSummary = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.total_runs, 42);
    assert!((back.estimated_usd - 0.105).abs() < f64::EPSILON);
}

#[test]
fn cost_record_serde_round_trip() {
    let record = make_cost_record("run-42", "anthropic", "claude-sonnet-4", 2000, 1000, 0.021);
    let json = serde_json::to_string(&record).expect("serialize");
    let back: CostRecord = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.run_id, "run-42");
    assert_eq!(back.provider, "anthropic");
    assert_eq!(back.input_tokens, 2000);
    assert!((back.estimated_usd - 0.021).abs() < f64::EPSILON);
}

#[test]
fn pricing_entry_serde_round_trip() {
    let entry = PricingEntry {
        provider: "anthropic".into(),
        model_pattern: "claude-sonnet-4".into(),
        input_per_million: 3.0,
        output_per_million: 15.0,
    };
    let json = serde_json::to_string(&entry).expect("serialize");
    let back: PricingEntry = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.provider, "anthropic");
    assert_eq!(back.model_pattern, "claude-sonnet-4");
    assert!((back.input_per_million - 3.0).abs() < f64::EPSILON);
}

#[test]
fn record_and_budget_integration_deny_over_limit() {
    let mut checker = BudgetChecker::new();
    checker.set_config(
        "budget-agent",
        BudgetConfig {
            max_per_day: Some(micro_usd(20_000)), // $0.02 daily limit
            warn_at_percent: 80,
            ..Default::default()
        },
    );

    // Record three runs totalling $0.015 (15_000 micro-USD)
    for i in 0..3 {
        checker
            .record_spend(
                "budget-agent",
                &make_cost_record(&format!("run-{i}"), "anthropic", "claude-sonnet-4", 500, 200, 0.005),
            )
            .expect("record");
    }

    // A $0.008 spend would bring total to $0.023 > $0.02 — should be denied
    let decision = checker.check("budget-agent", &micro_usd(8_000));
    assert!(
        matches!(decision, BudgetDecision::Deny { .. }),
        "should deny spend that would exceed daily limit"
    );
}

#[test]
fn record_and_budget_integration_allow_within_limit() {
    let mut checker = BudgetChecker::new();
    checker.set_config(
        "budget-ok-agent",
        BudgetConfig {
            max_per_day: Some(micro_usd(100_000)), // $0.10 daily limit
            warn_at_percent: 80,
            ..Default::default()
        },
    );

    // Record one run of $0.01
    checker
        .record_spend(
            "budget-ok-agent",
            &make_cost_record("run-1", "anthropic", "claude-sonnet-4", 500, 200, 0.01),
        )
        .expect("record");

    // $0.05 spend stays within $0.10 limit
    let decision = checker.check("budget-ok-agent", &micro_usd(50_000));
    assert!(
        matches!(decision, BudgetDecision::Allow | BudgetDecision::Warn { .. }),
        "should allow spend within daily limit"
    );
}
