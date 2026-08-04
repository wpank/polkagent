//! Phase 4 Acceptance Tests — Multisig Edge Cases
//!
//! Tests multisig-specific risk patterns and edge cases using the risk gate
//! pipeline and action card builder. Verifies threshold changes, pure-proxy
//! revocation, and multisig-in-batch scenarios.
//!
//! - MS-01: Multisig.as_multi hidden in batch triggers BatchHiding.
//! - MS-02: Multisig.approve_as_multi hidden in batch triggers BatchHiding.
//! - MS-03: Multisig.cancel_as_multi hidden in batch triggers BatchHiding.
//! - MS-04: Proxy.remove_proxies (revoke all) hidden in batch is critical.
//! - MS-05: Proxy.kill_pure (pure-proxy kill) hidden in batch.
//! - MS-06: Threshold change via multisig wrapping multisig config.
//! - MS-07: Pure-proxy creation hidden in batch.
//! - MS-08: Multiple proxy operations in single batch.
//! - MS-09: ActionCard correctly tags multisig-related risk flags.
//! - MS-10: Batch of only multisig approvals with no hidden calls is still flagged.

use chrono::Utc;
use uuid::Uuid;

use polkagent_payment::{
    Amount, AssetId, BatchHidingDetector, CompositeRiskGate, PaymentIntent, PaymentStatus,
    RiskCode, RiskGate, RiskSeverity,
};

use polkagent_card::{
    ActionCardBuilder, RiskFlag, RiskFlagType, RiskLevel, SectionSource, Severity,
};

// =========================================================================
// Helpers
// =========================================================================

const ALICE: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";

fn make_intent_with_calls(calls: serde_json::Value) -> PaymentIntent {
    PaymentIntent {
        id: Uuid::now_v7(),
        agent_id: "agent-multisig-test".into(),
        run_id: "run-ms-1".into(),
        amount: Amount::new(1_000, AssetId::Native, 10),
        recipient: ALICE.into(),
        idempotency_key: Uuid::now_v7().to_string(),
        created_at: Utc::now(),
        status: PaymentStatus::Pending,
        metadata: Some(serde_json::json!({ "calls": calls })),
    }
}

fn transfer_call() -> serde_json::Value {
    serde_json::json!({"pallet": "Balances", "call": "transfer_keep_alive", "args": {}})
}

// =========================================================================
// MS-01: Multisig.as_multi hidden in batch
// =========================================================================

#[test]
fn ms_01_as_multi_hidden_in_batch() {
    let intent = make_intent_with_calls(serde_json::json!([
        transfer_call(),
        {"pallet": "Multisig", "call": "as_multi", "args": {"threshold": 2, "call": "0xdead"}}
    ]));
    let findings = BatchHidingDetector.assess(&intent);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, RiskCode::BatchHiding);
}

// =========================================================================
// MS-02: Multisig.approve_as_multi hidden in batch
// =========================================================================

#[test]
fn ms_02_approve_as_multi_hidden_in_batch() {
    let intent = make_intent_with_calls(serde_json::json!([
        transfer_call(),
        {"pallet": "Multisig", "call": "approve_as_multi", "args": {"threshold": 2}}
    ]));
    let findings = BatchHidingDetector.assess(&intent);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, RiskCode::BatchHiding);
}

// =========================================================================
// MS-03: Multisig.cancel_as_multi hidden in batch
// =========================================================================

#[test]
fn ms_03_cancel_as_multi_hidden_in_batch() {
    let intent = make_intent_with_calls(serde_json::json!([
        transfer_call(),
        {"pallet": "Multisig", "call": "cancel_as_multi", "args": {"threshold": 2}}
    ]));
    let findings = BatchHidingDetector.assess(&intent);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, RiskCode::BatchHiding);
}

// =========================================================================
// MS-04: Proxy.remove_proxies (revoke all) hidden in batch
// =========================================================================

#[test]
fn ms_04_remove_proxies_hidden_in_batch() {
    let intent = make_intent_with_calls(serde_json::json!([
        transfer_call(),
        {"pallet": "Proxy", "call": "remove_proxies", "args": {}}
    ]));
    let findings = BatchHidingDetector.assess(&intent);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, RiskCode::BatchHiding);
    assert_eq!(findings[0].severity, RiskSeverity::Critical);
}

// =========================================================================
// MS-05: Proxy.kill_pure (pure-proxy destruction) hidden in batch
// =========================================================================

#[test]
fn ms_05_kill_pure_hidden_in_batch() {
    let intent = make_intent_with_calls(serde_json::json!([
        transfer_call(),
        {"pallet": "Proxy", "call": "kill_pure", "args": {"spawner": ALICE, "index": 0}}
    ]));
    let findings = BatchHidingDetector.assess(&intent);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, RiskCode::BatchHiding);
}

// =========================================================================
// MS-06: Threshold change via nested multisig config in batch
// =========================================================================

#[test]
fn ms_06_threshold_change_via_nested_multisig() {
    let intent = make_intent_with_calls(serde_json::json!([
        transfer_call(),
        {"pallet": "Multisig", "call": "as_multi", "args": {
            "threshold": 1,
            "other_signatories": [ALICE],
            "call": "0x0000"
        }},
        {"pallet": "Proxy", "call": "add_proxy", "args": {"proxy": ALICE, "proxy_type": "Any"}}
    ]));
    let findings = BatchHidingDetector.assess(&intent);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, RiskCode::BatchHiding);
    let evidence = &findings[0].evidence;
    let hidden_count = evidence
        .get("hidden_calls")
        .and_then(|h| h.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(hidden_count, 2, "both multisig and proxy calls are hidden");
}

// =========================================================================
// MS-07: Pure-proxy creation hidden in batch
// =========================================================================

#[test]
fn ms_07_create_pure_proxy_hidden_in_batch() {
    let intent = make_intent_with_calls(serde_json::json!([
        transfer_call(),
        {"pallet": "Proxy", "call": "create_pure", "args": {"proxy_type": "Any", "index": 0}}
    ]));
    let findings = BatchHidingDetector.assess(&intent);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, RiskCode::BatchHiding);
}

// =========================================================================
// MS-08: Multiple proxy operations in single batch
// =========================================================================

#[test]
fn ms_08_multiple_proxy_operations_in_batch() {
    let intent = make_intent_with_calls(serde_json::json!([
        transfer_call(),
        {"pallet": "Proxy", "call": "add_proxy", "args": {"proxy": ALICE}},
        {"pallet": "Proxy", "call": "remove_proxy", "args": {"proxy": ALICE}},
        {"pallet": "Proxy", "call": "create_pure", "args": {"proxy_type": "Any"}}
    ]));
    let findings = BatchHidingDetector.assess(&intent);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, RiskCode::BatchHiding);
    assert!(findings[0].message.contains("3 non-transfer call(s)"));
}

// =========================================================================
// MS-09: ActionCard tags multisig-related risk flags correctly
// =========================================================================

#[test]
fn ms_09_action_card_multisig_risk_flags() {
    let card = ActionCardBuilder::new("Multisig Transfer")
        .add_canonical("Pallet", "Multisig.as_multi", SectionSource::Metadata)
        .add_canonical("Threshold", "2 of 3", SectionSource::Chain)
        .add_narrative("Explanation", "This is a multisig transfer requiring 2 signatories.")
        .add_risk_flag(RiskFlag::new(
            RiskFlagType::LargeApproval,
            "Multisig threshold change detected",
            Severity::High,
        ))
        .with_risk_level(RiskLevel::High)
        .with_payload_hash("0xdeadbeef")
        .build();

    assert_eq!(card.risk_level, RiskLevel::High);
    assert_eq!(card.risk_flags.len(), 1);
    assert_eq!(card.risk_flags[0].flag_type, RiskFlagType::LargeApproval);
    assert_eq!(card.canonical_sections.len(), 2);
    assert_eq!(card.narrative_sections.len(), 1);
}

// =========================================================================
// MS-10: Batch of only multisig approvals (non-transfer) is flagged
// =========================================================================

#[test]
fn ms_10_batch_only_multisig_calls_flagged() {
    let intent = make_intent_with_calls(serde_json::json!([
        {"pallet": "Multisig", "call": "approve_as_multi", "args": {"threshold": 2}},
        {"pallet": "Multisig", "call": "as_multi", "args": {"threshold": 3}}
    ]));
    let g = CompositeRiskGate::with_defaults(100_000_000_000);
    let findings = g.assess(&intent);
    assert!(!findings.is_empty(), "non-transfer-only batch must trigger");
    let codes: Vec<RiskCode> = findings.iter().map(|f| f.code).collect();
    assert!(codes.contains(&RiskCode::BatchHiding));
}
