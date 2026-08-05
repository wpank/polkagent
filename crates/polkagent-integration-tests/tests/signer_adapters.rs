//! Cross-crate integration tests for signer adapter behaviours.
//!
//! These tests verify the `polkagent-signer-fake` crate against the
//! `polkagent-signer-trait` contract, exercising signing, rejection,
//! account enumeration, and expiry enforcement from outside the crate
//! boundary.

// This assertion-oriented integration target uses `expect`/`unwrap` to identify
// the exact cross-crate fixture step or behavioral contract that failed.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use polkagent_core::now;
use polkagent_signer_fake::FakeSigner;
use polkagent_signer_trait::{
    AccountRef, ApprovalId, CanonicalSignRequest, ChainProfileId, GrantDigest, MetadataDigest,
    Signer, SignerError,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn valid_request(account: AccountRef) -> CanonicalSignRequest {
    CanonicalSignRequest {
        request_id: "integ-req-001".into(),
        payload: vec![0xDE, 0xAD, 0xBE, 0xEF, 0xCA],
        account,
        chain_profile: ChainProfileId::new("polkadot"),
        metadata_hash: MetadataDigest(vec![0xAB; 32]),
        grant_digest: GrantDigest(vec![0xCD; 32]),
        approval_id: ApprovalId::new("integ-approval-001"),
        expires_at: now() + chrono::Duration::hours(1),
    }
}

// ---------------------------------------------------------------------------
// FakeSigner::new() — sign() returns deterministic signature
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fake_signer_sign_returns_deterministic_signature() {
    let signer = FakeSigner::new();
    let account = AccountRef::from_bytes([0u8; 32]);
    let req = valid_request(account.clone());
    let payload = req.payload.clone();

    let signed_payload = signer.sign(req).await.expect("sign should succeed");

    // Signature is 64 bytes with the first 4 bytes mirroring the payload.
    assert_eq!(signed_payload.signature.len(), 64);
    assert_eq!(signed_payload.signature[0], 0xDE);
    assert_eq!(signed_payload.signature[1], 0xAD);
    assert_eq!(signed_payload.signature[2], 0xBE);
    assert_eq!(signed_payload.signature[3], 0xEF);

    // Public key matches the signing account.
    assert_eq!(signed_payload.public_key.len(), 32);
    assert_eq!(signed_payload.public_key, account.account_id.to_vec());

    // signed_extrinsic = payload || signature
    assert_eq!(
        &signed_payload.signed_extrinsic[..payload.len()],
        payload.as_slice()
    );
    assert_eq!(
        &signed_payload.signed_extrinsic[payload.len()..],
        signed_payload.signature.as_slice()
    );
}

#[tokio::test]
async fn fake_signer_sign_is_deterministic_across_calls() {
    let signer = FakeSigner::new();
    let account = AccountRef::from_bytes([0u8; 32]);

    let signed1 = signer
        .sign(valid_request(account.clone()))
        .await
        .expect("sign 1");
    let signed2 = signer.sign(valid_request(account)).await.expect("sign 2");

    // Same payload + account should produce identical signatures.
    assert_eq!(signed1.signature, signed2.signature);
    assert_eq!(signed1.public_key, signed2.public_key);
}

// ---------------------------------------------------------------------------
// FakeSigner::rejecting() — denies all requests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fake_signer_rejecting_denies_all_requests() {
    let signer = FakeSigner::rejecting();
    let account = AccountRef::from_bytes([0u8; 32]);
    let result = signer.sign(valid_request(account)).await;

    assert!(
        matches!(result, Err(SignerError::UserRejected)),
        "expected UserRejected, got: {result:?}"
    );
}

#[tokio::test]
async fn fake_signer_rejecting_health_returns_error() {
    let signer = FakeSigner::rejecting();
    let result = signer.health().await;
    assert!(result.is_err(), "health should fail for rejecting signer");
}

#[tokio::test]
async fn fake_signer_rejecting_still_counts_calls() {
    let signer = FakeSigner::rejecting();
    let account = AccountRef::from_bytes([0u8; 32]);

    assert_eq!(signer.sign_call_count(), 0);
    let _ = signer.sign(valid_request(account.clone())).await;
    assert_eq!(signer.sign_call_count(), 1);
    let _ = signer.sign(valid_request(account)).await;
    assert_eq!(signer.sign_call_count(), 2);
}

// ---------------------------------------------------------------------------
// FakeSigner::with_accounts — reports specified accounts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fake_signer_with_accounts_reports_configured_accounts() {
    let acct_a = AccountRef::from_bytes([0xAA; 32]);
    let acct_b = AccountRef::from_bytes([0xBB; 32]);
    let acct_c = AccountRef::with_ss58(
        [0xCC; 32],
        "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
    );

    let signer = FakeSigner::with_accounts(vec![acct_a.clone(), acct_b.clone(), acct_c.clone()]);

    let caps = signer.describe().await.expect("describe should succeed");
    assert_eq!(caps.accounts.len(), 3);
    assert_eq!(caps.accounts[0].account_id, [0xAA; 32]);
    assert_eq!(caps.accounts[1].account_id, [0xBB; 32]);
    assert_eq!(caps.accounts[2].account_id, [0xCC; 32]);
    assert!(caps.accounts[2].ss58_display.is_some());

    // Hardware-backed should be false for fake signer.
    assert!(!caps.hardware_backed);
    assert_eq!(caps.display_name, "FakeSigner");
}

#[tokio::test]
async fn fake_signer_with_accounts_can_sign_for_any_account() {
    let acct_a = AccountRef::from_bytes([0xAA; 32]);
    let acct_b = AccountRef::from_bytes([0xBB; 32]);
    let signer = FakeSigner::with_accounts(vec![acct_a.clone(), acct_b.clone()]);

    // FakeSigner signs for any account regardless of which are configured.
    let signed_a = signer
        .sign(valid_request(acct_a))
        .await
        .expect("sign for account A");
    assert_eq!(signed_a.public_key, vec![0xAA; 32]);

    let signed_b = signer
        .sign(valid_request(acct_b))
        .await
        .expect("sign for account B");
    assert_eq!(signed_b.public_key, vec![0xBB; 32]);
}

// ---------------------------------------------------------------------------
// Sign request expiry is enforced
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fake_signer_rejects_expired_request() {
    let signer = FakeSigner::new();
    let account = AccountRef::from_bytes([0u8; 32]);

    let mut req = valid_request(account);
    // Set expiry to 1 second in the past.
    req.expires_at = now() - chrono::Duration::seconds(1);

    let result = signer.sign(req).await;
    assert!(
        matches!(result, Err(SignerError::Expired { .. })),
        "expected Expired error for past-dated request, got: {result:?}"
    );
}

#[tokio::test]
async fn fake_signer_accepts_future_expiry() {
    let signer = FakeSigner::new();
    let account = AccountRef::from_bytes([0u8; 32]);

    let mut req = valid_request(account);
    req.expires_at = now() + chrono::Duration::days(7);

    let result = signer.sign(req).await;
    assert!(
        result.is_ok(),
        "request with future expiry should be accepted"
    );
}

// ---------------------------------------------------------------------------
// Signer trait object safety — compile-time check
// ---------------------------------------------------------------------------

/// Compile-time verification that `FakeSigner` satisfies `Signer` via dyn.
#[allow(dead_code)]
fn _fake_signer_is_object_safe(_s: &dyn Signer) {}

/// Verify that `FakeSigner` can be used behind an Arc<dyn Signer>.
#[tokio::test]
async fn fake_signer_can_be_used_as_dyn_signer() {
    let signer: std::sync::Arc<dyn Signer> = std::sync::Arc::new(FakeSigner::new());

    let caps = signer.describe().await.expect("describe via dyn");
    assert_eq!(caps.accounts.len(), 1);

    let signed_payload = signer
        .sign(valid_request(AccountRef::from_bytes([0u8; 32])))
        .await
        .expect("sign via dyn");
    assert_eq!(signed_payload.signature.len(), 64);
}

// ---------------------------------------------------------------------------
// Request recording works across crate boundary
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fake_signer_records_last_request() {
    let signer = FakeSigner::new();
    assert!(signer.last_request().is_none());

    let account = AccountRef::from_bytes([0u8; 32]);
    let req = valid_request(account);
    let req_id = req.request_id.clone();

    signer.sign(req).await.expect("sign ok");

    let captured = signer.last_request().expect("should have recorded request");
    assert_eq!(captured.request_id, req_id);
}
