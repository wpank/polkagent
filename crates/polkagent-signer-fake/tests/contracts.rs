//! Contract and conformance tests for [`FakeSigner`].
//!
//! These tests verify that the fake signer implementation conforms to the
//! [`Signer`] trait contract and the shared PRD-15 conformance suite defined
//! in `polkagent-signer-trait`.

use polkagent_signer_fake::FakeSigner;
use polkagent_signer_trait::{conformance, contracts, AccountRef};

// ---------------------------------------------------------------------------
// Legacy contracts (kept for backwards compatibility)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_returns_accounts() {
    let signer = FakeSigner::new();
    contracts::test_describe_returns_accounts(&signer).await;
}

#[tokio::test]
async fn sign_returns_payload() {
    let signer = FakeSigner::new();
    let account = AccountRef::from_bytes([0u8; 32]);
    contracts::test_sign_returns_payload(&signer, account).await;
}

#[tokio::test]
async fn sign_payload_matches_request() {
    let signer = FakeSigner::new();
    let account = AccountRef::from_bytes([0u8; 32]);
    contracts::test_sign_payload_matches_request(&signer, account).await;
}

#[tokio::test]
async fn health_returns_result() {
    let signer = FakeSigner::new();
    contracts::test_health_returns_result(&signer).await;
}

// ---------------------------------------------------------------------------
// PRD-15 conformance suite
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conformance_describe_returns_accounts() {
    let signer = FakeSigner::new();
    conformance::test_describe_returns_accounts(&signer).await;
}

#[tokio::test]
async fn conformance_sign_returns_signature() {
    let signer = FakeSigner::new();
    let account = AccountRef::from_bytes([0u8; 32]);
    conformance::test_sign_returns_signature(&signer, account).await;
}

#[tokio::test]
async fn conformance_sign_deterministic_for_same_input() {
    let signer = FakeSigner::new();
    let account = AccountRef::from_bytes([0u8; 32]);
    conformance::test_sign_deterministic_for_same_input(&signer, account).await;
}

#[tokio::test]
async fn conformance_sign_different_inputs_different_signatures() {
    let signer = FakeSigner::new();
    let account = AccountRef::from_bytes([0u8; 32]);
    conformance::test_sign_different_inputs_different_signatures(&signer, account).await;
}

#[tokio::test]
async fn conformance_verify_valid_signature() {
    let signer = FakeSigner::new();
    let account = AccountRef::from_bytes([0u8; 32]);
    conformance::test_verify_valid_signature(&signer, account).await;
}

#[tokio::test]
async fn conformance_verify_invalid_signature_fails() {
    let signer = FakeSigner::new();
    let account = AccountRef::from_bytes([0u8; 32]);
    conformance::test_verify_invalid_signature_fails(&signer, account).await;
}

#[tokio::test]
async fn conformance_signer_never_sees_plaintext_secret() {
    let signer = FakeSigner::new();
    let account = AccountRef::from_bytes([0u8; 32]);
    conformance::test_signer_never_sees_plaintext_secret(&signer, account).await;
}

#[tokio::test]
async fn conformance_health_returns_result() {
    let signer = FakeSigner::new();
    conformance::test_health_returns_result(&signer).await;
}
