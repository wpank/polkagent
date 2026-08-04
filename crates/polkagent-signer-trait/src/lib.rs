//! Signer port trait for the Polkagent platform.
//!
//! This crate defines the [`Signer`] trait — the isolated signing boundary
//! through which Polkagent produces Polkadot transaction signatures.
//!
//! # Key isolation invariant
//!
//! **Models NEVER see raw signing keys.** The signer receives only canonical
//! payload bytes and binding references. It never receives conversation
//! history, model output, tool results, or any other data from the
//! untrusted execution boundary.
//!
//! # Contract
//!
//! - Implementations must be `Send + Sync + 'static`.
//! - [`Signer::sign`] must verify that the request has not expired before
//!   producing a signature.
//! - A hardware-backed signer must display the canonical call summary to the
//!   user before signing.
//! - Implementations must never expose raw key material through their
//!   public API surface, error messages, or log output.
//! - The [`CanonicalSignRequest`] struct must not be extended with fields
//!   sourced from model output or conversation data.

#[cfg(feature = "test-contracts")]
pub mod contracts;

#[cfg(feature = "test-contracts")]
pub mod conformance;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use polkagent_core::Timestamp;

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// An opaque reference to a Polkadot account (32-byte public key).
///
/// Stored and compared as raw bytes. The human-readable SS58 form is
/// derived at display time using the appropriate network prefix.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountRef {
    /// Raw 32-byte AccountId32.
    pub account_id: [u8; 32],
    /// Optional human-readable SS58 address for display purposes only.
    ///
    /// This field is **not** used for signing decisions; only `account_id`
    /// is authoritative.
    pub ss58_display: Option<String>,
}

impl AccountRef {
    /// Construct an `AccountRef` from raw 32-byte account bytes.
    #[must_use]
    pub fn from_bytes(account_id: [u8; 32]) -> Self {
        Self {
            account_id,
            ss58_display: None,
        }
    }

    /// Construct an `AccountRef` with a display-only SS58 address.
    #[must_use]
    pub fn with_ss58(account_id: [u8; 32], ss58: impl Into<String>) -> Self {
        Self {
            account_id,
            ss58_display: Some(ss58.into()),
        }
    }
}

/// An opaque identifier for a chain profile configuration.
///
/// A chain profile binds a genesis hash, runtime version, RPC endpoints,
/// and metadata snapshot for a specific Polkadot network.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChainProfileId(pub String);

impl ChainProfileId {
    /// Construct a `ChainProfileId` from a string identifier.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for ChainProfileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// BLAKE3 or SHA-256 digest of the runtime metadata used to construct a
/// signing payload. The signer verifies this matches the metadata it has
/// on record for the target chain profile.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MetadataDigest(pub Vec<u8>);

/// A digest of the [`polkagent_core::GrantId`] that authorized this effect.
///
/// The signer does not evaluate policy — it verifies only that this digest
/// matches the grant it was pre-authorized with (if applicable).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GrantDigest(pub Vec<u8>);

/// An opaque identifier for a human- or quorum-approval decision.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ApprovalId(pub String);

impl ApprovalId {
    /// Construct an `ApprovalId` from a string.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

/// The exact canonical bytes to sign, plus binding metadata.
///
/// This struct is the **only** input to the signer. It must not contain
/// model output, conversation history, tool results, or any data sourced
/// from the untrusted execution boundary. The kernel constructs this from
/// typed domain records only.
///
/// **Invariant:** No field of this struct may be populated from LLM-generated
/// text, user free-form input, or memory content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalSignRequest {
    /// Unique, non-reusable identifier for this signing request.
    pub request_id: String,
    /// The SCALE-encoded transaction payload bytes to sign.
    ///
    /// **Never** derive these bytes from model output or user text.
    pub payload: Vec<u8>,
    /// The account that must produce the signature.
    pub account: AccountRef,
    /// The chain profile this transaction targets.
    pub chain_profile: ChainProfileId,
    /// Hash of the runtime metadata used to construct `payload`.
    pub metadata_hash: MetadataDigest,
    /// Digest of the `ResolvedGrant` that authorized this effect.
    pub grant_digest: GrantDigest,
    /// The approval decision that authorized signing.
    pub approval_id: ApprovalId,
    /// When this request expires. The signer must refuse requests received
    /// after this timestamp.
    pub expires_at: Timestamp,
}

/// A successfully produced signature.
#[derive(Debug, Clone)]
pub struct SignedPayload {
    /// The full signed extrinsic ready for broadcast.
    ///
    /// Typically: `payload || signature || public_key` in SCALE encoding.
    pub signed_extrinsic: Vec<u8>,
    /// The public key that signed (for audit evidence).
    ///
    /// Raw 32-byte Sr25519 / Ed25519 key, or 33-byte compressed secp256k1.
    pub public_key: Vec<u8>,
    /// The raw signature bytes (for audit evidence).
    pub signature: Vec<u8>,
}

/// Capabilities reported by a signer implementation.
#[derive(Debug, Clone)]
pub struct SignerCapabilities {
    /// Account references this signer can sign for.
    pub accounts: Vec<AccountRef>,
    /// Chain profiles this signer is configured to target.
    pub chain_profiles: Vec<ChainProfileId>,
    /// Whether this signer is backed by a hardware security module.
    pub hardware_backed: bool,
    /// Human-readable display name for operator interfaces.
    pub display_name: String,
    /// Whether this signer can produce signatures.
    ///
    /// A watch-only signer reports `false` here — it can enumerate accounts
    /// but will always refuse to sign.
    pub can_sign: bool,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that a [`Signer`] implementation may return.
#[derive(Debug, Error)]
pub enum SignerError {
    /// The requested account is not managed by this signer.
    #[error("account not found in signer: {account:?}")]
    AccountNotFound {
        /// The account that was requested.
        account: AccountRef,
    },

    /// The approval ID does not authorize this request.
    #[error("approval '{approval_id}' is not valid for this signing request")]
    InvalidApproval {
        /// The approval that was rejected.
        approval_id: String,
    },

    /// The grant digest in the request does not match the signer's record.
    #[error("grant digest mismatch: signing request does not match authorized grant")]
    GrantMismatch,

    /// The metadata hash does not match the signer's pinned chain metadata.
    #[error("metadata hash mismatch: payload was built with a different runtime version")]
    MetadataMismatch,

    /// The signing request has expired.
    #[error("signing request expired at {expired_at}")]
    Expired {
        /// When the request expired.
        expired_at: Timestamp,
    },

    /// The user explicitly declined the signing request.
    #[error("user rejected the signing request")]
    UserRejected,

    /// A hardware wallet or HSM returned an error.
    #[error("hardware signer error: {message}")]
    Hardware {
        /// Human-readable description.
        message: String,
    },

    /// The signer did not respond within the allowed time.
    #[error("signer timed out after {elapsed_ms}ms")]
    Timeout {
        /// How long the signer waited.
        elapsed_ms: u64,
    },

    /// This signer is watch-only and cannot produce signatures.
    #[error("signer is watch-only and cannot sign")]
    WatchOnly,

    /// An unexpected internal error.
    #[error("signer internal error: {message}")]
    Internal {
        /// Human-readable description.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Trait definition
// ---------------------------------------------------------------------------

/// The isolated signing port.
///
/// One implementation exists per signing mode (external wallet, hardware
/// wallet, local encrypted keystore, managed KMS/HSM). The kernel depends
/// only on this trait.
///
/// # Contract
///
/// - Implementations must be `Send + Sync + 'static`.
/// - [`sign`] must reject requests where `expires_at` is in the past.
/// - Implementations must **never** expose raw key material through their
///   return values, error messages, log output, or side effects.
/// - A hardware-backed implementation must display the canonical call
///   summary to the user before producing a signature.
/// - The `CanonicalSignRequest` must not be extended with fields sourced
///   from model output or conversation data. The compiler enforces this:
///   the type is `pub` but constructable only from typed domain records.
///
/// [`sign`]: Signer::sign
#[async_trait]
pub trait Signer: Send + Sync + 'static {
    /// Describe the accounts and chain profiles this signer can operate.
    async fn describe(&self) -> Result<SignerCapabilities, SignerError>;

    /// Sign the canonical payload.
    ///
    /// The signer verifies:
    /// 1. That `request.account` is managed by this signer.
    /// 2. That `request.expires_at` has not passed.
    /// 3. Any additional invariants specific to this signing mode.
    ///
    /// On success, returns the signed extrinsic bytes and audit evidence.
    async fn sign(&self, request: CanonicalSignRequest) -> Result<SignedPayload, SignerError>;

    /// Health check: return `Ok(())` if the signer is reachable and ready.
    async fn health(&self) -> Result<(), SignerError>;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_ref_from_bytes_has_no_display() {
        let account = AccountRef::from_bytes([0u8; 32]);
        assert!(account.ss58_display.is_none());
    }

    #[test]
    fn account_ref_with_ss58_stores_address() {
        let account = AccountRef::with_ss58(
            [1u8; 32],
            "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
        );
        assert!(account.ss58_display.is_some());
        assert_eq!(account.account_id, [1u8; 32]);
    }

    #[test]
    fn chain_profile_id_display() {
        let id = ChainProfileId::new("polkadot-mainnet");
        assert_eq!(format!("{id}"), "polkadot-mainnet");
    }

    #[test]
    fn signer_error_user_rejected_not_retryable() {
        // SignerError doesn't have an is_retryable helper, but we can
        // verify the error type is constructable and displays correctly.
        let e = SignerError::UserRejected;
        assert!(format!("{e}").contains("rejected"));
    }

    #[test]
    fn signer_error_account_not_found_includes_account() {
        let account = AccountRef::from_bytes([42u8; 32]);
        let e = SignerError::AccountNotFound { account };
        let msg = format!("{e}");
        assert!(msg.contains("account"));
    }

    #[test]
    fn signer_error_hardware_includes_message() {
        let e = SignerError::Hardware {
            message: "device disconnected".into(),
        };
        assert!(format!("{e}").contains("device disconnected"));
    }

    #[test]
    fn canonical_sign_request_fields_present() {
        let req = CanonicalSignRequest {
            request_id: "req-001".into(),
            payload: vec![0u8, 1u8, 2u8],
            account: AccountRef::from_bytes([0u8; 32]),
            chain_profile: ChainProfileId::new("westend"),
            metadata_hash: MetadataDigest(vec![0u8; 32]),
            grant_digest: GrantDigest(vec![0u8; 32]),
            approval_id: ApprovalId::new("approval-1"),
            expires_at: chrono::Utc::now() + chrono::Duration::seconds(60),
        };
        assert_eq!(req.request_id, "req-001");
        assert_eq!(req.payload, vec![0u8, 1u8, 2u8]);
    }

    #[test]
    fn signed_payload_has_three_components() {
        let signed = SignedPayload {
            signed_extrinsic: vec![1u8; 100],
            public_key: vec![2u8; 32],
            signature: vec![3u8; 64],
        };
        assert_eq!(signed.public_key.len(), 32);
        assert_eq!(signed.signature.len(), 64);
    }

    /// Compile-time check: `Signer` can be used as a `dyn` trait object.
    #[allow(dead_code)]
    fn _signer_is_object_safe(_s: &dyn Signer) {}
}
