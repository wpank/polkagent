//! **EXPERIMENTAL** — Integration test for the full earn → invoke → spend cycle.
//!
//! This exercises the PRD-08 §5.2 agent earn/spend mechanics:
//!
//! 1. Agent completes a skill task and receives a simulated micropayment
//! 2. Agent uses the payment to invoke a tool call (with x402 header)
//! 3. Earnings and spending are tracked in the payment ledger
//!
//! **Research prototype — not production-ready.**

// This end-to-end assertion target uses `expect` to identify the exact ledger,
// payment-header, or spend transition that violated the cycle contract.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use polkagent_payment::ledger::{Ledger, LedgerEntryKind, X402PaymentHeader};
use polkagent_payment::types::AssetId;

/// Full earn → invoke → spend cycle exercising all three mechanics.
#[test]
fn earn_invoke_spend_full_cycle() {
    // --- Setup: agent with empty ledger ---
    let mut ledger = Ledger::new("test-agent-007", AssetId::Native, 10);
    assert_eq!(ledger.balance_planck(), 0);

    // ---------------------------------------------------------------
    // Phase 1: Agent completes a skill task → earns micropayment
    // ---------------------------------------------------------------
    // Simulates: agent ran "governance-researcher" skill, receives reward.
    let earn_id = ledger
        .record_earning(
            "governance-researcher",
            10_000_000,
            "analyzed referendum #42",
        )
        .expect("earning should succeed");

    assert_eq!(ledger.balance_planck(), 10_000_000);
    assert_eq!(ledger.entries().len(), 1);

    // Verify the ledger entry is correct.
    let entry = &ledger.entries()[0];
    assert_eq!(entry.id, earn_id);
    assert!(matches!(
        &entry.kind,
        LedgerEntryKind::Earn { skill_name } if skill_name == "governance-researcher"
    ));

    // ---------------------------------------------------------------
    // Phase 2: Agent invokes a tool call with x402 payment header
    // ---------------------------------------------------------------
    // Simulates: agent calls an HTTP tool that requires payment.
    let header = ledger
        .prepare_x402_payment("chain_query", 3_000_000)
        .expect("x402 payment should succeed");

    // Verify the x402 header is well-formed.
    assert_eq!(header.scheme, "x402");
    assert_eq!(header.amount_planck, 3_000_000);
    assert_eq!(header.asset, "NATIVE");
    assert_eq!(header.payer, "test-agent-007");

    // The header value should be a parseable string.
    let hv = header.to_header_value();
    assert!(hv.starts_with("x402 "));
    assert!(hv.contains("amount=3000000"));
    assert!(hv.contains("asset=NATIVE"));
    assert!(hv.contains("payer=test-agent-007"));

    // Balance reduced after payment.
    assert_eq!(ledger.balance_planck(), 7_000_000);

    // ---------------------------------------------------------------
    // Phase 3: Agent earns again, then spends directly
    // ---------------------------------------------------------------
    ledger
        .record_earning("data-fetcher", 2_000_000, "fetched on-chain metrics")
        .expect("second earning");
    assert_eq!(ledger.balance_planck(), 9_000_000);

    ledger
        .record_spend("llm_inference", 1_500_000, "model inference call")
        .expect("direct spend");
    assert_eq!(ledger.balance_planck(), 7_500_000);

    // ---------------------------------------------------------------
    // Phase 4: Verify cumulative ledger state
    // ---------------------------------------------------------------
    assert_eq!(ledger.total_earned_planck(), 12_000_000);
    assert_eq!(ledger.total_spent_planck(), 4_500_000);
    assert_eq!(ledger.entries().len(), 4);
    assert_eq!(ledger.agent_id(), "test-agent-007");

    // Balance as Amount type.
    let bal = ledger.balance();
    assert_eq!(bal.value, 7_500_000);
    assert_eq!(bal.asset, AssetId::Native);
    assert_eq!(bal.decimals, 10);
}

/// Spending more than the balance is rejected.
#[test]
fn spend_rejected_on_insufficient_balance() {
    let mut ledger = Ledger::new("agent-broke", AssetId::Native, 10);
    ledger
        .record_earning("small-task", 500, "tiny reward")
        .expect("earn");

    // Direct spend exceeding balance.
    let err = ledger.record_spend("expensive_tool", 1000, "too much");
    assert!(err.is_err());
    assert_eq!(ledger.balance_planck(), 500); // unchanged

    // x402 payment exceeding balance.
    let err = ledger.prepare_x402_payment("another_tool", 1000);
    assert!(err.is_err());
    assert_eq!(ledger.balance_planck(), 500); // unchanged
}

/// x402 header can be serialized for HTTP transport.
#[test]
fn x402_header_serialization() {
    let header = X402PaymentHeader {
        scheme: "x402".into(),
        amount_planck: 42_000,
        asset: "NATIVE".into(),
        payer: "agent-xyz".into(),
        signature: "experimental-no-sig".into(),
    };

    // JSON round-trip (for logging / debugging).
    let json = serde_json::to_string(&header).expect("serialize");
    let back: X402PaymentHeader = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(header, back);

    // HTTP header value.
    let hv = header.to_header_value();
    assert_eq!(
        hv,
        "x402 amount=42000;asset=NATIVE;payer=agent-xyz;sig=experimental-no-sig"
    );
}

/// Multiple agents can have independent ledgers.
#[test]
fn independent_agent_ledgers() {
    let mut alice = Ledger::new("alice", AssetId::Native, 10);
    let mut bob = Ledger::new("bob", AssetId::Native, 10);

    alice
        .record_earning("skill-a", 10_000, "alice earned")
        .expect("alice earn");
    bob.record_earning("skill-b", 5_000, "bob earned")
        .expect("bob earn");

    assert_eq!(alice.balance_planck(), 10_000);
    assert_eq!(bob.balance_planck(), 5_000);

    alice
        .record_spend("tool-1", 3_000, "alice spent")
        .expect("alice spend");

    assert_eq!(alice.balance_planck(), 7_000);
    assert_eq!(bob.balance_planck(), 5_000); // bob unaffected
}
