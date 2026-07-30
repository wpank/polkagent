//! Contract tests for [`FakeSigner`] via the reusable contract test suite.
//!
//! These tests verify that the fake signer implementation conforms to the
//! [`Signer`] trait contract defined in `polkagent-signer-trait`.

use polkagent_signer_fake::FakeSigner;
use polkagent_signer_trait::contracts;
use polkagent_signer_trait::AccountRef;

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
