//! Reusable contract tests for [`Signer`] implementations.
//!
//! This module provides test functions that verify any implementation of
//! [`Signer`] conforms to the expected behaviour defined in the trait
//! contract. Each function accepts a reference to an implementor and exercises
//! one aspect of the contract.
//!
//! # Usage
//!
//! In your adapter crate's integration test:
//!
//! ```rust,ignore
//! use polkagent_signer_trait::contracts;
//!
//! #[tokio::test]
//! async fn describe_returns_accounts() {
//!     let signer = MySigner::new();
//!     contracts::test_describe_returns_accounts(&signer).await;
//! }
//! ```
//!
//! # Feature gate
//!
//! This module is only available when the `test-contracts` feature is enabled.

// Contract helpers are assertion functions: a failed prerequisite should stop
// immediately with the operation-specific message supplied at each call site.
#![allow(
    clippy::expect_used,
    reason = "contract assertions intentionally panic with operation-specific diagnostics"
)]

use crate::{
    AccountRef, ApprovalId, CanonicalSignRequest, ChainProfileId, GrantDigest, MetadataDigest,
    Signer,
};
use polkagent_core::now;

/// Build a valid [`CanonicalSignRequest`] targeting the given account.
///
/// The request has a one-hour expiry and a small payload. Contract tests
/// should not depend on the specific content beyond structural validity.
#[must_use]
pub fn valid_sign_request(account: AccountRef) -> CanonicalSignRequest {
    CanonicalSignRequest {
        request_id: "contract-test-req-001".into(),
        payload: vec![0xCA, 0xFE, 0xBA, 0xBE, 0x01, 0x02, 0x03, 0x04],
        account,
        chain_profile: ChainProfileId::new("polkadot"),
        metadata_hash: MetadataDigest(vec![0xAB; 32]),
        grant_digest: GrantDigest(vec![0xCD; 32]),
        approval_id: ApprovalId::new("contract-test-approval"),
        expires_at: now() + chrono::Duration::hours(1),
    }
}

/// Contract: `describe()` returns at least one account.
///
/// A well-configured signer must report at least one account it can sign for.
/// The returned [`crate::SignerCapabilities`] must contain a non-empty `accounts`
/// list and a non-empty `display_name`.
pub async fn test_describe_returns_accounts(signer: &dyn Signer) {
    let caps = signer
        .describe()
        .await
        .expect("describe() must not fail for a configured signer");

    assert!(
        !caps.accounts.is_empty(),
        "describe() must return at least one account"
    );

    assert!(
        !caps.display_name.is_empty(),
        "describe() must return a non-empty display_name"
    );
}

/// Contract: `sign()` returns a [`crate::SignedPayload`] with a non-empty signature.
///
/// When given a valid, non-expired request for a known account, `sign()` must
/// produce a signed payload containing a non-empty `signature` and a
/// non-empty `public_key`.
///
/// The `account` parameter must be an account that this signer can sign for
/// (i.e., one listed in `describe().accounts`).
pub async fn test_sign_returns_payload(signer: &dyn Signer, account: AccountRef) {
    let request = valid_sign_request(account);
    let signed_payload = signer
        .sign(request)
        .await
        .expect("sign() must not fail for a valid, non-expired request");

    assert!(
        !signed_payload.signature.is_empty(),
        "sign() must return a non-empty signature"
    );

    assert!(
        !signed_payload.public_key.is_empty(),
        "sign() must return a non-empty public_key"
    );

    assert!(
        !signed_payload.signed_extrinsic.is_empty(),
        "sign() must return a non-empty signed_extrinsic"
    );
}

/// Contract: signed payload contains the original canonical bytes.
///
/// The `signed_extrinsic` returned by `sign()` must contain the original
/// `payload` bytes from the request. This ensures the signer did not
/// silently modify the payload before signing.
///
/// The `account` parameter must be an account that this signer can sign for.
pub async fn test_sign_payload_matches_request(signer: &dyn Signer, account: AccountRef) {
    let request = valid_sign_request(account);
    let original_payload = request.payload.clone();

    let signed_payload = signer
        .sign(request)
        .await
        .expect("sign() must not fail for a valid, non-expired request");

    // The signed_extrinsic must contain the original payload bytes.
    // Typically, signed_extrinsic = payload || signature, so the payload
    // should appear as a prefix.
    let contains_payload = signed_payload
        .signed_extrinsic
        .windows(original_payload.len())
        .any(|window| window == original_payload.as_slice());

    assert!(
        contains_payload,
        "signed_extrinsic must contain the original canonical payload bytes"
    );
}

/// Contract: `health()` returns without panicking.
///
/// A healthy signer must return `Ok(())`. This test verifies that the
/// health check completes without panic. Whether it returns `Ok` or `Err`
/// depends on the signer's configuration, but it must not panic.
pub async fn test_health_returns_result(signer: &dyn Signer) {
    // We only care that this does not panic.
    let _result = signer.health().await;
}
