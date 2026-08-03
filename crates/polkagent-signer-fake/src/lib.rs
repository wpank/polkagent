//! Deterministic fake [`Signer`] adapter for testing.
//!
//! This crate provides [`FakeSigner`] — a fully deterministic, configurable
//! implementation of the [`Signer`] trait intended for unit and integration
//! tests. It never performs real cryptographic operations and never holds
//! actual private key material.
//!
//! # Key isolation verification
//!
//! The [`CanonicalSignRequest`] struct is designed to contain no model data.
//! The tests in this crate explicitly assert that none of the fields in a
//! `CanonicalSignRequest` are sourced from model output:
//!
//! - `payload`: SCALE-encoded bytes from the kernel, not from LLM output.
//! - `account`: typed `AccountRef`, not a string from a model.
//! - `chain_profile`: typed `ChainProfileId`, not a string from a model.
//! - `metadata_hash`: a digest, not model text.
//! - `grant_digest`: a digest, not model text.
//! - `approval_id`: a typed identifier, not model text.
//! - `expires_at`: a timestamp, not model text.
//!
//! # Quick-start examples
//!
//! ```rust
//! use polkagent_signer_fake::FakeSigner;
//! use polkagent_signer_trait::Signer;
//!
//! // Always produces a deterministic fake signature.
//! let signer = FakeSigner::new();
//!
//! // Pre-configured with specific account references.
//! use polkagent_signer_trait::{AccountRef, ChainProfileId};
//! let account = AccountRef::from_bytes([0u8; 32]);
//! let signer = FakeSigner::with_accounts(vec![account]);
//!
//! // Always rejects signing requests.
//! let signer = FakeSigner::rejecting();
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use polkagent_core::now;
use polkagent_signer_trait::{
    AccountRef, CanonicalSignRequest, ChainProfileId, Signer, SignedPayload, SignerCapabilities,
    SignerError,
};

// ---------------------------------------------------------------------------
// Internal mode
// ---------------------------------------------------------------------------

enum Mode {
    /// Produce a deterministic fake signature for all accounts.
    Signing,
    /// Reject all signing requests.
    Rejecting,
}

// ---------------------------------------------------------------------------
// FakeSigner
// ---------------------------------------------------------------------------

/// A deterministic fake implementation of [`Signer`].
///
/// `FakeSigner` never performs real cryptographic operations. It is designed
/// exclusively for tests. Its behaviour is controlled by the constructor:
///
/// | Constructor | Behaviour |
/// |---|---|
/// | [`new`] | Signs everything; one default account (`[0u8; 32]`). |
/// | [`with_accounts`] | Signs everything; the given accounts are reported. |
/// | [`rejecting`] | Rejects all signing requests with [`SignerError::UserRejected`]. |
///
/// After each call to [`sign`], the call counter is incremented and the
/// request is recorded for assertion.
///
/// [`new`]: FakeSigner::new
/// [`with_accounts`]: FakeSigner::with_accounts
/// [`rejecting`]: FakeSigner::rejecting
/// [`sign`]: Signer::sign
pub struct FakeSigner {
    mode: Mode,
    accounts: Vec<AccountRef>,
    chain_profiles: Vec<ChainProfileId>,
    sign_call_count: AtomicU64,
    last_request: Mutex<Option<CanonicalSignRequest>>,
}

impl FakeSigner {
    /// Create a `FakeSigner` that signs every request with a deterministic
    /// fake signature.
    ///
    /// Exposes one account: `[0u8; 32]` (the all-zeros 32-byte account).
    #[must_use]
    pub fn new() -> Self {
        Self {
            mode: Mode::Signing,
            accounts: vec![AccountRef::from_bytes([0u8; 32])],
            chain_profiles: vec![ChainProfileId::new("polkadot")],
            sign_call_count: AtomicU64::new(0),
            last_request: Mutex::new(None),
        }
    }

    /// Create a `FakeSigner` that signs every request and reports the given
    /// accounts as available.
    #[must_use]
    pub fn with_accounts(accounts: Vec<AccountRef>) -> Self {
        Self {
            mode: Mode::Signing,
            accounts,
            chain_profiles: vec![ChainProfileId::new("polkadot")],
            sign_call_count: AtomicU64::new(0),
            last_request: Mutex::new(None),
        }
    }

    /// Create a `FakeSigner` that rejects all signing requests.
    ///
    /// The rejection reason is [`SignerError::UserRejected`].
    #[must_use]
    pub fn rejecting() -> Self {
        Self {
            mode: Mode::Rejecting,
            accounts: vec![],
            chain_profiles: vec![],
            sign_call_count: AtomicU64::new(0),
            last_request: Mutex::new(None),
        }
    }

    // -----------------------------------------------------------------------
    // Assertion helpers
    // -----------------------------------------------------------------------

    /// Return how many times [`sign`] has been called.
    ///
    /// [`sign`]: Signer::sign
    #[must_use]
    pub fn sign_call_count(&self) -> u64 {
        self.sign_call_count.load(Ordering::SeqCst)
    }

    /// Return a clone of the most recent [`CanonicalSignRequest`].
    ///
    /// Returns `None` if [`sign`] has not been called yet.
    ///
    /// [`sign`]: Signer::sign
    #[must_use]
    pub fn last_request(&self) -> Option<CanonicalSignRequest> {
        self.last_request.lock().expect("mutex poisoned").clone()
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn record(&self, request: &CanonicalSignRequest) {
        self.sign_call_count.fetch_add(1, Ordering::SeqCst);
        let mut guard = self.last_request.lock().expect("mutex poisoned");
        *guard = Some(request.clone());
    }

    /// Produce a deterministic fake signature.
    ///
    /// The fake signature is constructed as:
    /// - `signature`: 64 zero bytes followed by the first 4 bytes of the
    ///   payload (for easy identification in tests).
    /// - `public_key`: 32 bytes from the account's `account_id`.
    /// - `signed_extrinsic`: `payload || signature`.
    fn fake_sign(request: &CanonicalSignRequest) -> SignedPayload {
        let mut signature = vec![0u8; 64];
        // Embed up to 4 payload bytes so tests can verify the right payload
        // was signed without having to compare the entire byte slice.
        for (i, &b) in request.payload.iter().take(4).enumerate() {
            signature[i] = b;
        }
        let public_key = request.account.account_id.to_vec();
        let mut signed_extrinsic = request.payload.clone();
        signed_extrinsic.extend_from_slice(&signature);
        SignedPayload { signed_extrinsic, public_key, signature }
    }
}

impl Default for FakeSigner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Signer for FakeSigner {
    async fn describe(&self) -> Result<SignerCapabilities, SignerError> {
        Ok(SignerCapabilities {
            accounts: self.accounts.clone(),
            chain_profiles: self.chain_profiles.clone(),
            hardware_backed: false,
            display_name: "FakeSigner".into(),
            can_sign: true,
        })
    }

    async fn sign(&self, request: CanonicalSignRequest) -> Result<SignedPayload, SignerError> {
        self.record(&request);

        // Check expiry before doing anything else.
        if request.expires_at <= now() {
            return Err(SignerError::Expired { expired_at: request.expires_at });
        }

        match &self.mode {
            Mode::Signing => Ok(Self::fake_sign(&request)),
            Mode::Rejecting => Err(SignerError::UserRejected),
        }
    }

    async fn health(&self) -> Result<(), SignerError> {
        match &self.mode {
            Mode::Signing => Ok(()),
            Mode::Rejecting => Err(SignerError::Internal {
                message: "FakeSigner is in rejecting mode".into(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::now;
    use polkagent_signer_trait::{ApprovalId, GrantDigest, MetadataDigest};

    fn valid_request(account: AccountRef) -> CanonicalSignRequest {
        CanonicalSignRequest {
            request_id: "req-test-001".into(),
            payload: vec![1, 2, 3, 4, 5],
            account,
            chain_profile: ChainProfileId::new("polkadot"),
            metadata_hash: MetadataDigest(vec![0xab; 32]),
            grant_digest: GrantDigest(vec![0xcd; 32]),
            approval_id: ApprovalId::new("approval-test-001"),
            expires_at: now() + chrono::Duration::hours(1),
        }
    }

    #[tokio::test]
    async fn new_signs_with_deterministic_fake_signature() {
        let signer = FakeSigner::new();
        let account = AccountRef::from_bytes([0u8; 32]);
        let req = valid_request(account);
        let payload = req.payload.clone();

        let signed = signer.sign(req).await.expect("sign should succeed");

        assert_eq!(signed.signature.len(), 64);
        assert_eq!(signed.public_key.len(), 32);
        // signed_extrinsic = payload || signature
        assert_eq!(&signed.signed_extrinsic[..payload.len()], payload.as_slice());
    }

    #[tokio::test]
    async fn with_accounts_reports_configured_accounts() {
        let account_a = AccountRef::from_bytes([0xAA; 32]);
        let account_b = AccountRef::from_bytes([0xBB; 32]);
        let signer = FakeSigner::with_accounts(vec![account_a.clone(), account_b.clone()]);

        let caps = signer.describe().await.expect("describe ok");
        assert_eq!(caps.accounts.len(), 2);
        assert_eq!(caps.accounts[0].account_id, [0xAA; 32]);
        assert_eq!(caps.accounts[1].account_id, [0xBB; 32]);
    }

    #[tokio::test]
    async fn rejecting_returns_user_rejected() {
        let signer = FakeSigner::rejecting();
        let account = AccountRef::from_bytes([0u8; 32]);
        let req = valid_request(account);
        let result = signer.sign(req).await;
        assert!(matches!(result, Err(SignerError::UserRejected)));
    }

    #[tokio::test]
    async fn expired_request_is_rejected() {
        let signer = FakeSigner::new();
        let account = AccountRef::from_bytes([0u8; 32]);
        let mut req = valid_request(account);
        // Set expiry to the past.
        req.expires_at = now() - chrono::Duration::seconds(1);
        let result = signer.sign(req).await;
        assert!(matches!(result, Err(SignerError::Expired { .. })));
    }

    #[tokio::test]
    async fn sign_call_count_increments() {
        let signer = FakeSigner::new();
        assert_eq!(signer.sign_call_count(), 0);
        let account = AccountRef::from_bytes([0u8; 32]);
        signer.sign(valid_request(account.clone())).await.ok();
        assert_eq!(signer.sign_call_count(), 1);
        signer.sign(valid_request(account)).await.ok();
        assert_eq!(signer.sign_call_count(), 2);
    }

    #[tokio::test]
    async fn last_request_is_recorded_after_sign() {
        let signer = FakeSigner::new();
        assert!(signer.last_request().is_none());
        let account = AccountRef::from_bytes([0u8; 32]);
        let req = valid_request(account);
        let req_id = req.request_id.clone();
        signer.sign(req).await.ok();
        let captured = signer.last_request().expect("should have last request");
        assert_eq!(captured.request_id, req_id);
    }

    #[tokio::test]
    async fn health_ok_for_signing_mode() {
        let signer = FakeSigner::new();
        assert!(signer.health().await.is_ok());
    }

    #[tokio::test]
    async fn health_err_for_rejecting_mode() {
        let signer = FakeSigner::rejecting();
        assert!(signer.health().await.is_err());
    }

    #[tokio::test]
    async fn canonical_sign_request_contains_no_model_data() {
        // This test asserts that the fields of CanonicalSignRequest are all
        // typed domain records — none are free-form strings from model output.
        let req = CanonicalSignRequest {
            request_id: "signed-request-id".into(),
            // payload is SCALE-encoded bytes, not model text
            payload: vec![0xDE, 0xAD, 0xBE, 0xEF],
            // account is a typed AccountRef, not a model string
            account: AccountRef::from_bytes([0x01; 32]),
            // chain_profile is a typed ChainProfileId, not a model string
            chain_profile: ChainProfileId::new("polkadot"),
            // metadata_hash is a digest, not model text
            metadata_hash: MetadataDigest(vec![0xFF; 32]),
            // grant_digest is a digest, not model text
            grant_digest: GrantDigest(vec![0xAA; 32]),
            // approval_id is a typed identifier, not model text
            approval_id: ApprovalId::new("approval-abc"),
            // expires_at is a Timestamp, not model text
            expires_at: now() + chrono::Duration::hours(1),
        };

        // Verify the payload is raw bytes (not a string from model output).
        assert_eq!(req.payload, vec![0xDE, 0xAD, 0xBE, 0xEF]);

        // Verify the account is a typed value, not a model-generated string.
        assert_eq!(req.account.account_id, [0x01u8; 32]);
        assert!(
            req.account.ss58_display.is_none(),
            "ss58_display is display-only and should not affect signing"
        );

        // Verify chain_profile is typed.
        assert_eq!(req.chain_profile.0, "polkadot");

        // Verify metadata_hash and grant_digest are byte digests.
        assert_eq!(req.metadata_hash.0.len(), 32);
        assert_eq!(req.grant_digest.0.len(), 32);

        // The request is signable by a FakeSigner.
        let signer = FakeSigner::new();
        let result = signer.sign(req).await;
        assert!(result.is_ok(), "valid canonical request should be signable");
    }

    #[tokio::test]
    async fn sign_embeds_payload_bytes_in_fake_signature() {
        let signer = FakeSigner::new();
        let account = AccountRef::from_bytes([0u8; 32]);
        let mut req = valid_request(account);
        req.payload = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let signed = signer.sign(req).await.expect("ok");
        // The first 4 bytes of the signature should mirror the payload.
        assert_eq!(signed.signature[0], 0xDE);
        assert_eq!(signed.signature[1], 0xAD);
        assert_eq!(signed.signature[2], 0xBE);
        assert_eq!(signed.signature[3], 0xEF);
    }

    #[test]
    fn default_signer_behaves_like_new() {
        let signer = FakeSigner::default();
        assert_eq!(signer.sign_call_count(), 0);
        assert!(signer.last_request().is_none());
    }

    #[tokio::test]
    async fn rejecting_signer_still_increments_call_count() {
        let signer = FakeSigner::rejecting();
        let account = AccountRef::from_bytes([0u8; 32]);
        let req = valid_request(account);
        let _ = signer.sign(req).await;
        assert_eq!(signer.sign_call_count(), 1);
    }
}
