//! Phase 4 Acceptance Tests — Risk Gate Fixture Corpus
//!
//! Exercises the risk gate pipeline (`BatchHidingDetector`, `HomoglyphDetector`,
//! `HighValueDetector`, `CompositeRiskGate`) against 10+ dangerous-call patterns
//! combining batch, proxy, multisig, and governance pallets.
//!
//! - RG-01: Batch hiding a `System.set_code` call.
//! - RG-02: Batch hiding a `Proxy.add_proxy` call.
//! - RG-03: Batch hiding a `Multisig.as_multi` call.
//! - RG-04: Proxy call wrapping a transfer (`ProxyCall` risk code).
//! - RG-05: Batch with nested `Utility.batch_all` (recursive batch).
//! - RG-06: Batch hiding `Staking.nominate` among transfers.
//! - RG-07: Batch hiding `Democracy.vote` among transfers.
//! - RG-08: Homoglyph + batch hiding combined.
//! - RG-09: High value + batch hiding + homoglyph triple trigger.
//! - RG-10: Batch with only `Balances.transfer_all` calls is clean.
//! - RG-11: Empty metadata with high value triggers only `HighValue`.
//! - RG-12: Batch hiding `Sudo.sudo` call.

use chrono::Utc;
use uuid::Uuid;

use polkagent_payment::{
    Amount, AssetId, BatchHidingDetector, CompositeRiskGate, PaymentIntent, PaymentStatus,
    RiskCode, RiskGate, RiskSeverity,
};

// =========================================================================
// Helpers
// =========================================================================

fn dot() -> AssetId {
    AssetId::Native
}

fn make_intent(recipient: &str, planck: u128) -> PaymentIntent {
    PaymentIntent {
        id: Uuid::now_v7(),
        agent_id: "agent-risk-test".into(),
        run_id: "run-risk-1".into(),
        amount: Amount::new(planck, dot(), 10),
        recipient: recipient.into(),
        idempotency_key: Uuid::now_v7().to_string(),
        created_at: Utc::now(),
        status: PaymentStatus::Pending,
        metadata: None,
    }
}

fn make_intent_with_calls(
    recipient: &str,
    planck: u128,
    calls: serde_json::Value,
) -> PaymentIntent {
    let mut intent = make_intent(recipient, planck);
    let mut metadata = serde_json::Map::new();
    metadata.insert("calls".to_string(), calls);
    intent.metadata = Some(serde_json::Value::Object(metadata));
    intent
}

const ALICE: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";

fn transfer_call() -> serde_json::Value {
    serde_json::json!({"pallet": "Balances", "call": "transfer_keep_alive", "args": {}})
}

fn gate() -> CompositeRiskGate {
    CompositeRiskGate::with_defaults(100_000_000_000)
}

// =========================================================================
// RG-01: Batch hiding a System.set_code call
// =========================================================================

#[test]
fn rg_01_batch_hiding_system_set_code() {
    let intent = make_intent_with_calls(
        ALICE,
        1_000,
        serde_json::json!([
            transfer_call(),
            {"pallet": "System", "call": "set_code", "args": {"code": "0x1234"}}
        ]),
    );
    let findings = BatchHidingDetector.assess(&intent);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, RiskCode::BatchHiding);
    assert_eq!(findings[0].severity, RiskSeverity::Critical);
}

// =========================================================================
// RG-02: Batch hiding a Proxy.add_proxy call
// =========================================================================

#[test]
fn rg_02_batch_hiding_proxy_add_proxy() {
    let intent = make_intent_with_calls(
        ALICE,
        1_000,
        serde_json::json!([
            transfer_call(),
            {"pallet": "Proxy", "call": "add_proxy", "args": {"proxy": ALICE, "proxy_type": "Any"}}
        ]),
    );
    let findings = BatchHidingDetector.assess(&intent);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, RiskCode::BatchHiding);
    assert!(findings[0].message.contains("1 non-transfer call(s)"));
}

// =========================================================================
// RG-03: Batch hiding a Multisig.as_multi call
// =========================================================================

#[test]
fn rg_03_batch_hiding_multisig_as_multi() {
    let intent = make_intent_with_calls(
        ALICE,
        1_000,
        serde_json::json!([
            transfer_call(),
            {"pallet": "Multisig", "call": "as_multi", "args": {"threshold": 2}}
        ]),
    );
    let findings = BatchHidingDetector.assess(&intent);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, RiskCode::BatchHiding);
}

// =========================================================================
// RG-04: Proxy wrapping a transfer hides inside batch
// =========================================================================

#[test]
fn rg_04_proxy_wrapped_in_batch() {
    let intent = make_intent_with_calls(
        ALICE,
        1_000,
        serde_json::json!([
            transfer_call(),
            {"pallet": "Proxy", "call": "proxy", "args": {"real": ALICE, "call_data": "0xdead"}}
        ]),
    );
    let findings = BatchHidingDetector.assess(&intent);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, RiskCode::BatchHiding);
}

// =========================================================================
// RG-05: Nested Utility.batch_all inside a batch
// =========================================================================

#[test]
fn rg_05_nested_utility_batch_all() {
    let intent = make_intent_with_calls(
        ALICE,
        1_000,
        serde_json::json!([
            transfer_call(),
            {"pallet": "Utility", "call": "batch_all", "args": {"calls": []}}
        ]),
    );
    let findings = BatchHidingDetector.assess(&intent);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, RiskCode::BatchHiding);
}

// =========================================================================
// RG-06: Batch hiding Staking.nominate among transfers
// =========================================================================

#[test]
fn rg_06_batch_hiding_staking_nominate() {
    let intent = make_intent_with_calls(
        ALICE,
        1_000,
        serde_json::json!([
            transfer_call(),
            {"pallet": "Balances", "call": "transfer_allow_death", "args": {}},
            {"pallet": "Staking", "call": "nominate", "args": {"targets": []}}
        ]),
    );
    let findings = BatchHidingDetector.assess(&intent);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, RiskCode::BatchHiding);
    assert!(findings[0].message.contains("1 non-transfer call(s)"));
    assert!(findings[0].message.contains("3 total calls"));
}

// =========================================================================
// RG-07: Batch hiding Democracy.vote among transfers
// =========================================================================

#[test]
fn rg_07_batch_hiding_democracy_vote() {
    let intent = make_intent_with_calls(
        ALICE,
        1_000,
        serde_json::json!([
            transfer_call(),
            {"pallet": "Democracy", "call": "vote", "args": {"ref_index": 42}}
        ]),
    );
    let findings = BatchHidingDetector.assess(&intent);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, RiskCode::BatchHiding);
}

// =========================================================================
// RG-08: Homoglyph + batch hiding combined
// =========================================================================

#[test]
fn rg_08_homoglyph_plus_batch_hiding() {
    let spoofed = "5Grwva\u{0415}F5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
    let intent = make_intent_with_calls(
        spoofed,
        1_000,
        serde_json::json!([
            transfer_call(),
            {"pallet": "Proxy", "call": "remove_proxy", "args": {}}
        ]),
    );
    let g = gate();
    let findings = g.assess(&intent);
    let codes: Vec<RiskCode> = findings.iter().map(|f| f.code).collect();
    assert!(codes.contains(&RiskCode::BatchHiding));
    assert!(codes.contains(&RiskCode::HomoglyphAddress));
    assert_eq!(findings.len(), 2);
}

// =========================================================================
// RG-09: Triple trigger — high value + batch hiding + homoglyph
// =========================================================================

#[test]
fn rg_09_triple_trigger_high_batch_homoglyph() {
    let spoofed = "5Grwva\u{0415}F5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
    let intent = make_intent_with_calls(
        spoofed,
        500_000_000_000,
        serde_json::json!([
            transfer_call(),
            {"pallet": "System", "call": "kill_storage", "args": {"keys": []}}
        ]),
    );
    let g = gate();
    let findings = g.assess(&intent);
    assert_eq!(findings.len(), 3, "all three detectors must fire");
    let codes: Vec<RiskCode> = findings.iter().map(|f| f.code).collect();
    assert!(codes.contains(&RiskCode::BatchHiding));
    assert!(codes.contains(&RiskCode::HomoglyphAddress));
    assert!(codes.contains(&RiskCode::HighValue));
}

// =========================================================================
// RG-10: Batch with only valid transfer variants is clean
// =========================================================================

#[test]
fn rg_10_batch_only_transfers_is_clean() {
    let intent = make_intent_with_calls(
        ALICE,
        1_000,
        serde_json::json!([
            {"pallet": "Balances", "call": "transfer_keep_alive", "args": {}},
            {"pallet": "Balances", "call": "transfer_allow_death", "args": {}},
            {"pallet": "Balances", "call": "transfer_all", "args": {}}
        ]),
    );
    let g = gate();
    let findings = g.assess(&intent);
    assert!(
        findings.is_empty(),
        "only safe transfer calls — no findings"
    );
}

// =========================================================================
// RG-11: High value without batch metadata triggers only HighValue
// =========================================================================

#[test]
fn rg_11_high_value_only() {
    let intent = make_intent(ALICE, 500_000_000_000);
    let g = gate();
    let findings = g.assess(&intent);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, RiskCode::HighValue);
    assert_eq!(findings[0].severity, RiskSeverity::Warning);
}

// =========================================================================
// RG-12: Batch hiding Sudo.sudo call
// =========================================================================

#[test]
fn rg_12_batch_hiding_sudo() {
    let intent = make_intent_with_calls(
        ALICE,
        1_000,
        serde_json::json!([
            transfer_call(),
            {"pallet": "Sudo", "call": "sudo", "args": {"call_data": "0xfeed"}}
        ]),
    );
    let findings = BatchHidingDetector.assess(&intent);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, RiskCode::BatchHiding);
    assert_eq!(findings[0].severity, RiskSeverity::Critical);
}
