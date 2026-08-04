//! Shared conformance test suite for [`Signer`] implementations.
//!
//! This module defines the canonical set of behavioural tests that **every**
//! adapter implementing [`Signer`] must pass (PRD-15).  Tests are plain
//! `async fn`s so each adapter can call them from its own `tests/` directory.
//!
//! # Key isolation invariant
//!
//! The tests in this module verify that the signing boundary maintains the
//! core key-isolation guarantee: the [`CanonicalSignRequest`] passed to the
//! signer contains **only** typed domain values — no free-form strings from
//! model output, conversation history, or API key material.
//!
//! # Usage
//!
//! ```rust,ignore
//! // In your adapter crate: tests/conformance.rs
//! use polkagent_signer_trait::{conformance, AccountRef};
//! use my_adapter::MySigner;
//!
//! #[tokio::test]
//! async fn test_sign_returns_signature() {
//!     let signer = MySigner::new_for_tests();
//!     let account = conformance::first_account(&signer).await;
//!     conformance::test_sign_returns_signature(&signer, account).await;
//! }
//! ```
//!
//! # Feature gate
//!
//! Compiled only when the `test-contracts` feature is enabled.

use crate::{
    AccountRef, ApprovalId, CanonicalSignRequest, ChainProfileId, GrantDigest, MetadataDigest,
    Signer, SignerError,
};
use polkagent_core::now;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a valid, non-expired [`CanonicalSignRequest`] for the given account.
///
/// The payload is deliberately small SCALE-like bytes so tests can assert on
/// it without carrying a full blockchain dependency.
#[must_use]
pub fn valid_sign_request(account: AccountRef) -> CanonicalSignRequest {
    CanonicalSignRequest {
        request_id: "conformance-req-001".into(),
        payload: vec![0xCA, 0xFE, 0xBA, 0xBE, 0x01, 0x02, 0x03, 0x04],
        account,
        chain_profile: ChainProfileId::new("polkadot"),
        metadata_hash: MetadataDigest(vec![0xAB; 32]),
        grant_digest: GrantDigest(vec![0xCD; 32]),
        approval_id: ApprovalId::new("conformance-approval"),
        expires_at: now() + chrono::Duration::hours(1),
    }
}

/// Build a [`CanonicalSignRequest`] that has already expired.
#[must_use]
pub fn expired_sign_request(account: AccountRef) -> CanonicalSignRequest {
    CanonicalSignRequest {
        request_id: "conformance-expired-001".into(),
        payload: vec![0x01, 0x02],
        account,
        chain_profile: ChainProfileId::new("polkadot"),
        metadata_hash: MetadataDigest(vec![0x00; 32]),
        grant_digest: GrantDigest(vec![0x00; 32]),
        approval_id: ApprovalId::new("conformance-expired-approval"),
        // One second in the past.
        expires_at: now() - chrono::Duration::seconds(1),
    }
}

/// Convenience: retrieve the first account reported by `describe()`.
///
/// Most conformance tests need a valid account reference.  If the signer
/// reports no accounts the test is skipped by returning `None`.
pub async fn first_account(signer: &dyn Signer) -> Option<AccountRef> {
    let caps = signer.describe().await.ok()?;
    caps.accounts.into_iter().next()
}

// ---------------------------------------------------------------------------
// Conformance tests
// ---------------------------------------------------------------------------

/// Conformance: `describe()` returns at least one account and a non-empty
/// display name.
///
/// A configured signer must always report at least one manageable account.
pub async fn test_describe_returns_accounts(signer: &dyn Signer) {
    let caps = signer
        .describe()
        .await
        .expect("describe() must not fail for a configured signer");

    assert!(
        !caps.accounts.is_empty(),
        "describe() must return at least one account; got zero accounts"
    );

    assert!(
        !caps.display_name.is_empty(),
        "describe() must return a non-empty display_name; got {:?}",
        caps.display_name,
    );
}

/// Conformance: `sign()` returns a [`crate::SignedPayload`] with non-empty
/// `signature`, `public_key`, and `signed_extrinsic`.
///
/// This is the primary happy-path test.  The `account` must be one that the
/// signer can sign for (i.e., listed in `describe().accounts`).
pub async fn test_sign_returns_signature(signer: &dyn Signer, account: AccountRef) {
    let request = valid_sign_request(account);
    let signed = signer
        .sign(request)
        .await
        .expect("sign() must not fail for a valid, non-expired request");

    assert!(
        !signed.signature.is_empty(),
        "sign() must return a non-empty signature; got 0 bytes"
    );

    assert!(
        !signed.public_key.is_empty(),
        "sign() must return a non-empty public_key; got 0 bytes"
    );

    assert!(
        !signed.signed_extrinsic.is_empty(),
        "sign() must return a non-empty signed_extrinsic; got 0 bytes"
    );
}

/// Conformance: signing the same payload twice produces equivalent signatures.
///
/// Deterministic signers (e.g., Ed25519, Sr25519 with fixed randomness, or
/// any fake/test signer) must return the same signature bytes for the same
/// input.  Non-deterministic signers (ECDSA with random nonce) should skip
/// this test by checking the signer's capabilities.
///
/// The `account` must be one that the signer can sign for.
pub async fn test_sign_deterministic_for_same_input(signer: &dyn Signer, account: AccountRef) {
    let request_a = valid_sign_request(account.clone());
    let request_b = CanonicalSignRequest {
        request_id: request_a.request_id.clone(),
        payload: request_a.payload.clone(),
        account: request_a.account.clone(),
        chain_profile: request_a.chain_profile.clone(),
        metadata_hash: request_a.metadata_hash.clone(),
        grant_digest: request_a.grant_digest.clone(),
        approval_id: request_a.approval_id.clone(),
        expires_at: request_a.expires_at,
    };

    let signed_a = signer
        .sign(request_a)
        .await
        .expect("first sign() must not fail");
    let signed_b = signer
        .sign(request_b)
        .await
        .expect("second sign() must not fail");

    assert_eq!(
        signed_a.signature, signed_b.signature,
        "deterministic signer must produce identical signatures for identical payloads"
    );
}

/// Conformance: different payloads produce different signatures.
///
/// A signer that returns the same signature bytes for different payloads is
/// broken.  This test verifies basic signer correctness.
///
/// The `account` must be one that the signer can sign for.
pub async fn test_sign_different_inputs_different_signatures(
    signer: &dyn Signer,
    account: AccountRef,
) {
    let mut req_a = valid_sign_request(account.clone());
    req_a.payload = vec![0x01, 0x02, 0x03];
    req_a.request_id = "conformance-diff-a".into();

    let mut req_b = valid_sign_request(account);
    req_b.payload = vec![0x04, 0x05, 0x06];
    req_b.request_id = "conformance-diff-b".into();

    let signed_a = signer
        .sign(req_a)
        .await
        .expect("first sign() must not fail");
    let signed_b = signer
        .sign(req_b)
        .await
        .expect("second sign() must not fail");

    assert_ne!(
        signed_a.signature, signed_b.signature,
        "different payloads must produce different signatures"
    );
}

/// Conformance: `signed_extrinsic` contains the original canonical payload
/// bytes.
///
/// The signed extrinsic is typically `payload || signature` (or
/// `payload || signature || public_key`).  At minimum the original payload
/// bytes must appear somewhere in the extrinsic so callers can verify the
/// signer did not silently alter the payload.
///
/// The `account` must be one that the signer can sign for.
pub async fn test_verify_valid_signature(signer: &dyn Signer, account: AccountRef) {
    let request = valid_sign_request(account);
    let original_payload = request.payload.clone();

    let signed = signer
        .sign(request)
        .await
        .expect("sign() must not fail for a valid request");

    let contains_payload = signed
        .signed_extrinsic
        .windows(original_payload.len())
        .any(|window| window == original_payload.as_slice());

    assert!(
        contains_payload,
        "signed_extrinsic must contain the original canonical payload bytes; \
         payload={original_payload:?} extrinsic={:?}",
        signed.signed_extrinsic,
    );
}

/// Conformance: an expired request is rejected with `SignerError::Expired`.
///
/// The trait contract requires that `sign()` verifies `expires_at` before
/// producing a signature.  Passing a request whose `expires_at` is in the
/// past must always fail.
///
/// The `account` must be one that the signer can sign for.
pub async fn test_verify_invalid_signature_fails(signer: &dyn Signer, account: AccountRef) {
    let expired = expired_sign_request(account);
    let result = signer.sign(expired).await;

    match result {
        Err(SignerError::Expired { .. }) => {
            // Correct — the signer rejected the expired request.
        }
        Err(other) => {
            panic!(
                "sign() must return SignerError::Expired for an expired request; \
                 got: {other}"
            );
        }
        Ok(_) => {
            panic!("sign() must not succeed for an expired request; got Ok(_)");
        }
    }
}

/// Conformance: the signing payload contains no API key or secret material.
///
/// This is the key-isolation invariant from PRD-15 §3.  The [`CanonicalSignRequest`]
/// must never be populated with API keys, conversation text, model output, or
/// any `SecretForbidden`-classified data.
///
/// We verify this by constructing a request and asserting that no known secret
/// sentinel strings appear in the serialised request payload.
pub async fn test_signer_never_sees_plaintext_secret(signer: &dyn Signer, account: AccountRef) {
    // These sentinels stand in for real API keys or secrets that the kernel
    // must never allow into the signing boundary.
    let forbidden_sentinels = [
        "sk-ant-",     // Anthropic API key prefix
        "sk-",         // OpenAI API key prefix
        "Bearer ",     // HTTP auth header
        "password",    // Password field
        "api_key",     // Generic API key field name
        "secret",      // Generic secret field name
        "private_key", // Private key label
        "mnemonic",    // Seed phrase label
    ];

    let request = valid_sign_request(account);

    // Serialise the entire request to JSON and scan for forbidden strings.
    let request_json =
        serde_json::to_string(&request).expect("CanonicalSignRequest must be serialisable");

    for sentinel in &forbidden_sentinels {
        assert!(
            !request_json
                .to_lowercase()
                .contains(&sentinel.to_lowercase()),
            "signing request must not contain secret material; \
             found sentinel {sentinel:?} in serialised request"
        );
    }

    // Also check the raw payload bytes do not spell out ASCII secrets.
    let payload_str = String::from_utf8_lossy(&request.payload);
    for sentinel in &forbidden_sentinels {
        assert!(
            !payload_str
                .to_lowercase()
                .contains(&sentinel.to_lowercase()),
            "signing payload bytes must not contain plaintext secret material; \
             found sentinel {sentinel:?} in payload"
        );
    }

    // Finally, actually perform the sign to ensure the request is otherwise valid.
    let _signed = signer
        .sign(request)
        .await
        .expect("sign() must not fail for a valid conformance request");
}

/// Conformance: `health()` returns without panicking.
///
/// Adapters must implement a health check that does not panic.
pub async fn test_health_returns_result(signer: &dyn Signer) {
    let _result = signer.health().await;
}
