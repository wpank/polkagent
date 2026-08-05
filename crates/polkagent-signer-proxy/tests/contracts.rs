//! Contract and conformance tests for [`ProxySigner`].
//!
//! These tests verify that the proxy signer implementation conforms to the
//! [`Signer`] trait contract and the shared PRD-15 conformance suite defined
//! in `polkagent-signer-trait`.

#![allow(
    clippy::expect_used,
    reason = "signer contract tests intentionally panic with focused diagnostics"
)]

use std::sync::Arc;

use polkagent_core::ids::{AgentId, RunId};
use polkagent_grant::budget::BudgetTracker;
use polkagent_signer_fake::FakeSigner;
use polkagent_signer_proxy::{ProxySigner, ProxySignerConfig};
use polkagent_signer_trait::{conformance, contracts, AccountRef, ChainProfileId, Signer};

async fn make_test_proxy() -> ProxySigner {
    let tracker = BudgetTracker::new();
    let agent = AgentId::new();
    let run = RunId::new();
    tracker.configure(agent, u64::MAX).await;

    let config = ProxySignerConfig {
        agent_id: agent,
        run_id: run,
        transfer_amount: None,
        dry_run_enabled: false,
        chain_profiles: vec![ChainProfileId::new("polkadot")],
    };

    ProxySigner::new(Box::new(FakeSigner::new()), tracker, config)
}

// ---------------------------------------------------------------------------
// Legacy contracts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_returns_accounts() {
    let signer = make_test_proxy().await;
    contracts::test_describe_returns_accounts(&signer).await;
}

#[tokio::test]
async fn sign_returns_payload() {
    let signer = make_test_proxy().await;
    let account = AccountRef::from_bytes([0u8; 32]);
    contracts::test_sign_returns_payload(&signer, account).await;
}

#[tokio::test]
async fn sign_payload_matches_request() {
    let signer = make_test_proxy().await;
    let account = AccountRef::from_bytes([0u8; 32]);
    contracts::test_sign_payload_matches_request(&signer, account).await;
}

#[tokio::test]
async fn health_returns_result() {
    let signer = make_test_proxy().await;
    contracts::test_health_returns_result(&signer).await;
}

// ---------------------------------------------------------------------------
// PRD-15 conformance suite
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conformance_describe_returns_accounts() {
    let signer = make_test_proxy().await;
    conformance::test_describe_returns_accounts(&signer).await;
}

#[tokio::test]
async fn conformance_sign_returns_signature() {
    let signer = make_test_proxy().await;
    let account = AccountRef::from_bytes([0u8; 32]);
    conformance::test_sign_returns_signature(&signer, account).await;
}

#[tokio::test]
async fn conformance_sign_deterministic_for_same_input() {
    let signer = make_test_proxy().await;
    let account = AccountRef::from_bytes([0u8; 32]);
    conformance::test_sign_deterministic_for_same_input(&signer, account).await;
}

#[tokio::test]
async fn conformance_sign_different_inputs_different_signatures() {
    let signer = make_test_proxy().await;
    let account = AccountRef::from_bytes([0u8; 32]);
    conformance::test_sign_different_inputs_different_signatures(&signer, account).await;
}

#[tokio::test]
async fn conformance_verify_valid_signature() {
    let signer = make_test_proxy().await;
    let account = AccountRef::from_bytes([0u8; 32]);
    conformance::test_verify_valid_signature(&signer, account).await;
}

#[tokio::test]
async fn conformance_verify_invalid_signature_fails() {
    let signer = make_test_proxy().await;
    let account = AccountRef::from_bytes([0u8; 32]);
    conformance::test_verify_invalid_signature_fails(&signer, account).await;
}

#[tokio::test]
async fn conformance_signer_never_sees_plaintext_secret() {
    let signer = make_test_proxy().await;
    let account = AccountRef::from_bytes([0u8; 32]);
    conformance::test_signer_never_sees_plaintext_secret(&signer, account).await;
}

#[tokio::test]
async fn conformance_health_returns_result() {
    let signer = make_test_proxy().await;
    conformance::test_health_returns_result(&signer).await;
}

// ---------------------------------------------------------------------------
// Budget enforcement integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn budget_enforcement_blocks_over_ceiling() {
    let tracker = BudgetTracker::new();
    let agent = AgentId::new();
    let run = RunId::new();
    tracker.configure(agent, 1_000).await;

    let config = ProxySignerConfig {
        agent_id: agent,
        run_id: run,
        transfer_amount: Some(2_000),
        dry_run_enabled: false,
        chain_profiles: vec![],
    };

    let proxy = ProxySigner::new(Box::new(FakeSigner::new()), Arc::clone(&tracker), config);

    let account = AccountRef::from_bytes([0u8; 32]);
    let request = polkagent_signer_trait::contracts::valid_sign_request(account);
    let err = proxy
        .sign(request)
        .await
        .expect_err("should be denied by budget");

    let msg = format!("{err}");
    assert!(
        msg.contains("budget"),
        "error should reference budget: {msg}"
    );
}

#[tokio::test]
async fn budget_enforcement_allows_within_ceiling() {
    let tracker = BudgetTracker::new();
    let agent = AgentId::new();
    let run = RunId::new();
    tracker.configure(agent, 10_000).await;

    let config = ProxySignerConfig {
        agent_id: agent,
        run_id: run,
        transfer_amount: Some(5_000),
        dry_run_enabled: false,
        chain_profiles: vec![],
    };

    let proxy = ProxySigner::new(Box::new(FakeSigner::new()), Arc::clone(&tracker), config);

    let account = AccountRef::from_bytes([0u8; 32]);
    let request = polkagent_signer_trait::contracts::valid_sign_request(account);
    let signed = proxy
        .sign(request)
        .await
        .expect("should succeed within budget");
    assert!(!signed.signature.is_empty());

    let status = tracker.get_remaining(agent).await.expect("status ok");
    assert_eq!(status.spent, 5_000, "spend should be recorded");
}
