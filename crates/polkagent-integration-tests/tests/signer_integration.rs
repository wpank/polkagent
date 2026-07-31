//! IT-08: Signer integration tests.
//!
//! Exercises FakeSigner: sign, canonical bytes enforcement, signature
//! artifact structure, no secrets in payload, sign→verify round trip.

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
        request_id: "req-integration-test".into(),
        payload: vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03, 0x04],
        account,
        chain_profile: ChainProfileId::new("polkadot"),
        metadata_hash: MetadataDigest(vec![0xAB; 32]),
        grant_digest: GrantDigest(vec![0xCD; 32]),
        approval_id: ApprovalId::new("approval-it-08"),
        expires_at: now() + chrono::Duration::hours(1),
    }
}

fn default_account() -> AccountRef {
    AccountRef::from_bytes([0u8; 32])
}

// ---------------------------------------------------------------------------
// IT-08-A: Sign produces a valid signed payload
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sign_succeeds_with_valid_request() {
    let signer = FakeSigner::new();
    let account = default_account();
    let result = signer.sign(valid_request(account)).await;
    assert!(result.is_ok(), "sign must succeed for a valid request");
}

#[tokio::test]
async fn sign_produces_non_empty_signature() {
    let signer = FakeSigner::new();
    let signed = signer
        .sign(valid_request(default_account()))
        .await
        .expect("sign ok");

    assert!(!signed.signature.is_empty(), "signature must not be empty");
    assert!(!signed.public_key.is_empty(), "public key must not be empty");
    assert!(!signed.signed_extrinsic.is_empty(), "signed_extrinsic must not be empty");
}

#[tokio::test]
async fn sign_returns_64_byte_signature() {
    let signer = FakeSigner::new();
    let signed = signer
        .sign(valid_request(default_account()))
        .await
        .expect("sign ok");

    assert_eq!(signed.signature.len(), 64, "fake signature must be 64 bytes");
}

#[tokio::test]
async fn sign_returns_32_byte_public_key() {
    let signer = FakeSigner::new();
    let signed = signer
        .sign(valid_request(default_account()))
        .await
        .expect("sign ok");

    assert_eq!(signed.public_key.len(), 32, "public key must be 32 bytes");
}

// ---------------------------------------------------------------------------
// IT-08-B: Canonical bytes — payload bytes embedded in signature
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fake_signer_embeds_payload_bytes_in_signature() {
    let signer = FakeSigner::new();
    let payload = vec![0xDE, 0xAD, 0xBE, 0xEF];
    let mut req = valid_request(default_account());
    req.payload = payload.clone();

    let signed = signer.sign(req).await.expect("sign");

    // FakeSigner copies the first 4 payload bytes into signature[0..4].
    assert_eq!(signed.signature[0], 0xDE);
    assert_eq!(signed.signature[1], 0xAD);
    assert_eq!(signed.signature[2], 0xBE);
    assert_eq!(signed.signature[3], 0xEF);
}

#[tokio::test]
async fn signed_extrinsic_contains_original_payload() {
    let signer = FakeSigner::new();
    let payload = vec![1u8, 2, 3, 4, 5];
    let mut req = valid_request(default_account());
    req.payload = payload.clone();

    let signed = signer.sign(req).await.expect("sign");

    // signed_extrinsic = payload || signature
    assert!(
        signed.signed_extrinsic.starts_with(&payload),
        "signed_extrinsic must begin with the original payload bytes"
    );
}

#[tokio::test]
async fn signed_extrinsic_length_is_payload_plus_signature() {
    let signer = FakeSigner::new();
    let payload_len = 8;
    let mut req = valid_request(default_account());
    req.payload = vec![0xAA; payload_len];

    let signed = signer.sign(req).await.expect("sign");

    assert_eq!(
        signed.signed_extrinsic.len(),
        payload_len + 64,
        "signed_extrinsic length must equal payload length + 64-byte signature"
    );
}

// ---------------------------------------------------------------------------
// IT-08-C: No secrets in payload — request fields are typed domain values
// ---------------------------------------------------------------------------

#[tokio::test]
async fn canonical_sign_request_fields_are_typed_not_model_strings() {
    // This test documents the security invariant: CanonicalSignRequest fields
    // are typed domain values, not free-form strings from model output.
    let req = CanonicalSignRequest {
        request_id: "typed-request-id".into(),
        // payload: SCALE-encoded bytes — not model text
        payload: vec![0x01, 0x02, 0x03, 0x04],
        // account: typed AccountRef — not a model-generated address string
        account: AccountRef::from_bytes([0x01; 32]),
        // chain_profile: typed ChainProfileId — not a model string
        chain_profile: ChainProfileId::new("polkadot"),
        // metadata_hash: digest — not model text
        metadata_hash: MetadataDigest(vec![0xFF; 32]),
        // grant_digest: digest — not model text
        grant_digest: GrantDigest(vec![0xAA; 32]),
        // approval_id: typed identifier — not model text
        approval_id: ApprovalId::new("ap-typed"),
        // expires_at: timestamp — not model text
        expires_at: now() + chrono::Duration::hours(1),
    };

    // Payload is raw bytes — it must not be interpretable as plain text.
    assert_eq!(req.payload, vec![0x01, 0x02, 0x03, 0x04]);

    // Account is a typed value backed by a fixed-length byte array.
    assert_eq!(req.account.account_id, [0x01u8; 32]);

    // ss58_display is display-only; absence means it cannot influence signing.
    assert!(
        req.account.ss58_display.is_none(),
        "ss58_display must not be present in the canonical sign request"
    );

    // MetadataDigest and GrantDigest are 32-byte blobs, not strings.
    assert_eq!(req.metadata_hash.0.len(), 32);
    assert_eq!(req.grant_digest.0.len(), 32);

    // The full request is signable.
    let signer = FakeSigner::new();
    let result = signer.sign(req).await;
    assert!(result.is_ok(), "valid typed request must be signable");
}

// ---------------------------------------------------------------------------
// IT-08-D: Sign → verify round trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sign_and_verify_signed_extrinsic_contains_payload() {
    let signer = FakeSigner::new();
    let payload = vec![0x10, 0x20, 0x30, 0x40];
    let mut req = valid_request(default_account());
    req.payload = payload.clone();

    let signed = signer.sign(req).await.expect("sign");

    // Verification: the signed_extrinsic contains the original payload.
    assert!(
        signed.signed_extrinsic.windows(payload.len()).any(|w| w == payload.as_slice()),
        "signed_extrinsic must contain the original payload"
    );

    // Verification: the public_key matches the account.
    assert_eq!(
        signed.public_key,
        default_account().account_id,
        "public_key must equal the signer account's account_id"
    );
}

#[tokio::test]
async fn different_payloads_produce_different_signatures() {
    let signer = FakeSigner::new();

    let mut req1 = valid_request(default_account());
    req1.payload = vec![0x01, 0x02, 0x03, 0x04];
    let signed1 = signer.sign(req1).await.expect("sign1");

    let mut req2 = valid_request(default_account());
    req2.payload = vec![0x05, 0x06, 0x07, 0x08];
    let signed2 = signer.sign(req2).await.expect("sign2");

    // The first 4 bytes of the signature embed the payload, so they must differ.
    assert_ne!(
        signed1.signature[..4],
        signed2.signature[..4],
        "different payloads must produce different signatures"
    );
}

// ---------------------------------------------------------------------------
// IT-08-E: Rejection scenarios
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rejecting_signer_returns_user_rejected_error() {
    let signer = FakeSigner::rejecting();
    let result = signer.sign(valid_request(default_account())).await;
    assert!(
        matches!(result, Err(SignerError::UserRejected)),
        "rejecting signer must return UserRejected"
    );
}

#[tokio::test]
async fn expired_request_is_rejected_by_signing_signer() {
    let signer = FakeSigner::new();
    let mut req = valid_request(default_account());
    // Set expiry to the past.
    req.expires_at = now() - chrono::Duration::seconds(5);

    let result = signer.sign(req).await;
    assert!(
        matches!(result, Err(SignerError::Expired { .. })),
        "expired request must be rejected even by signing signer"
    );
}

#[tokio::test]
async fn rejecting_signer_increments_call_count() {
    let signer = FakeSigner::rejecting();
    assert_eq!(signer.sign_call_count(), 0);

    let _ = signer.sign(valid_request(default_account())).await;
    assert_eq!(signer.sign_call_count(), 1);
}

// ---------------------------------------------------------------------------
// IT-08-F: Capabilities
// ---------------------------------------------------------------------------

#[tokio::test]
async fn default_signer_reports_one_account() {
    let signer = FakeSigner::new();
    let caps = signer.describe().await.expect("describe");
    assert_eq!(caps.accounts.len(), 1, "default signer must report one account");
    assert_eq!(caps.accounts[0].account_id, [0u8; 32]);
}

#[tokio::test]
async fn with_accounts_reports_all_configured_accounts() {
    let accounts = vec![
        AccountRef::from_bytes([0xAA; 32]),
        AccountRef::from_bytes([0xBB; 32]),
        AccountRef::from_bytes([0xCC; 32]),
    ];
    let signer = FakeSigner::with_accounts(accounts.clone());
    let caps = signer.describe().await.expect("describe");

    assert_eq!(caps.accounts.len(), 3);
    assert_eq!(caps.accounts[0].account_id, [0xAA; 32]);
    assert_eq!(caps.accounts[1].account_id, [0xBB; 32]);
    assert_eq!(caps.accounts[2].account_id, [0xCC; 32]);
}

#[tokio::test]
async fn rejecting_signer_reports_no_accounts() {
    let signer = FakeSigner::rejecting();
    let caps = signer.describe().await.expect("describe");
    assert!(
        caps.accounts.is_empty(),
        "rejecting signer must report no accounts"
    );
}

#[tokio::test]
async fn signing_signer_health_check_passes() {
    let signer = FakeSigner::new();
    assert!(signer.health().await.is_ok());
}

#[tokio::test]
async fn rejecting_signer_health_check_fails() {
    let signer = FakeSigner::rejecting();
    assert!(
        signer.health().await.is_err(),
        "rejecting signer must report unhealthy"
    );
}

// ---------------------------------------------------------------------------
// IT-08-G: Call count tracking
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sign_call_count_increments_on_each_call() {
    let signer = FakeSigner::new();
    let account = default_account();

    assert_eq!(signer.sign_call_count(), 0);

    for i in 1..=5u64 {
        signer.sign(valid_request(account.clone())).await.ok();
        assert_eq!(signer.sign_call_count(), i);
    }
}

#[tokio::test]
async fn last_request_is_recorded_after_sign() {
    let signer = FakeSigner::new();
    assert!(signer.last_request().is_none());

    let mut req = valid_request(default_account());
    req.request_id = "it-08-unique-id".into();
    signer.sign(req).await.ok();

    let captured = signer.last_request().expect("last request must be recorded");
    assert_eq!(captured.request_id, "it-08-unique-id");
}

#[tokio::test]
async fn last_request_updated_on_subsequent_sign_calls() {
    let signer = FakeSigner::new();
    let account = default_account();

    let mut req1 = valid_request(account.clone());
    req1.request_id = "req-first".into();
    signer.sign(req1).await.ok();

    let mut req2 = valid_request(account.clone());
    req2.request_id = "req-second".into();
    signer.sign(req2).await.ok();

    let captured = signer.last_request().expect("last request");
    assert_eq!(
        captured.request_id, "req-second",
        "last_request must reflect the most recent call"
    );
}

// ---------------------------------------------------------------------------
// IT-08-H: FakeSigner is not hardware-backed (security property)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fake_signer_is_not_hardware_backed() {
    let signer = FakeSigner::new();
    let caps = signer.describe().await.expect("describe");
    assert!(
        !caps.hardware_backed,
        "FakeSigner must not claim to be hardware-backed"
    );
}

#[tokio::test]
async fn fake_signer_display_name_is_fakesigner() {
    let signer = FakeSigner::new();
    let caps = signer.describe().await.expect("describe");
    assert_eq!(caps.display_name, "FakeSigner");
}
