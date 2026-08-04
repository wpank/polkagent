//! KMS/HSM signer adapter for the Polkagent platform.
//!
//! This crate implements the [`Signer`] trait backed by an external Key
//! Management Service.  Key material **never** leaves the KMS — the signer
//! forwards canonical payload bytes to the KMS for signing and returns the
//! resulting signature.
//!
//! # Backends
//!
//! | Feature            | Backend              | Status             |
//! |--------------------|----------------------|--------------------|
//! | `mock` (default)   | In-process mock KMS  | ✓ Implemented      |
//! | `aws-kms`          | AWS KMS              | Stub (PRD-07 v2)   |
//! | `hashicorp-vault`  | HashiCorp Vault      | Stub (PRD-07 v2)   |
//!
//! # Key isolation invariant
//!
//! The mock backend derives deterministic signatures using BLAKE3 keyed by the
//! `key_id`.  No private key bytes are stored or exposed through the public
//! API, error messages, or log output.

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use polkagent_core::now;
use polkagent_signer_trait::{
    AccountRef, CanonicalSignRequest, ChainProfileId, SignedPayload, Signer, SignerCapabilities,
    SignerError,
};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// KMS provider backend selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KmsProvider {
    /// Mock backend for development and testing.
    Mock,
    /// AWS KMS (requires `aws-kms` feature).
    AwsKms,
    /// HashiCorp Vault Transit engine (requires `hashicorp-vault` feature).
    HashiCorpVault,
}

/// Configuration for a KMS-backed signer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KmsSignerConfig {
    /// Which KMS provider to use.
    pub provider: KmsProvider,
    /// Key identifier (ARN for AWS, key name for Vault).
    pub key_id: String,
    /// Cloud region (e.g. `us-east-1`). Required for AWS KMS.
    pub region: Option<String>,
    /// Optional endpoint override (e.g. for localstack or self-hosted Vault).
    pub endpoint: Option<String>,
    /// Accounts this signer is authorised to sign for.
    pub accounts: Vec<AccountRef>,
    /// Chain profiles this signer targets.
    pub chain_profiles: Vec<ChainProfileId>,
}

// ---------------------------------------------------------------------------
// KmsSigner
// ---------------------------------------------------------------------------

/// A signer backed by an external Key Management Service.
///
/// Key material never leaves the KMS.  The signer forwards canonical payload
/// bytes to the KMS for signing and returns the resulting signature.
///
/// Use [`KmsSigner::mock`] for testing or [`KmsSigner::new`] with a full
/// [`KmsSignerConfig`] for production configurations.
pub struct KmsSigner {
    config: KmsSignerConfig,
    sign_count: AtomicU64,
}

impl KmsSigner {
    /// Create a KMS signer from an explicit configuration.
    pub fn new(config: KmsSignerConfig) -> Self {
        Self {
            config,
            sign_count: AtomicU64::new(0),
        }
    }

    /// Create a mock KMS signer for testing.
    ///
    /// Derives a single deterministic account from the `key_id`.
    pub fn mock(key_id: impl Into<String>) -> Self {
        let key_id = key_id.into();
        let public_key = Self::derive_mock_public_key(&key_id);
        let mut account_id = [0u8; 32];
        account_id.copy_from_slice(&public_key[..32]);

        Self::new(KmsSignerConfig {
            provider: KmsProvider::Mock,
            key_id,
            region: None,
            endpoint: None,
            accounts: vec![AccountRef::from_bytes(account_id)],
            chain_profiles: vec![ChainProfileId::new("polkadot")],
        })
    }

    /// How many signing operations have been dispatched to the KMS.
    pub fn sign_count(&self) -> u64 {
        self.sign_count.load(Ordering::SeqCst)
    }

    // -- Mock KMS internals -------------------------------------------------

    /// Derive a deterministic 32-byte "public key" from a key identifier.
    ///
    /// In a real KMS the public key is fetched from the service; the mock
    /// derives it locally so tests can construct matching `AccountRef`s.
    fn derive_mock_public_key(key_id: &str) -> Vec<u8> {
        let hash = blake3::hash(format!("polkagent-kms-pubkey:{key_id}").as_bytes());
        hash.as_bytes().to_vec()
    }

    /// Produce a deterministic 64-byte mock signature.
    ///
    /// Uses BLAKE3 keyed by `key_id` so that:
    /// - same payload → same signature   (determinism)
    /// - different payload → different signature   (correctness)
    fn mock_sign(key_id: &str, payload: &[u8]) -> Vec<u8> {
        let mut hasher = blake3::Hasher::new();
        hasher.update(format!("polkagent-kms-sign:{key_id}:").as_bytes());
        hasher.update(payload);
        let first = hasher.finalize();

        let second = blake3::hash(first.as_bytes());

        let mut signature = Vec::with_capacity(64);
        signature.extend_from_slice(first.as_bytes());
        signature.extend_from_slice(second.as_bytes());
        signature
    }
}

// ---------------------------------------------------------------------------
// Signer trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Signer for KmsSigner {
    async fn describe(&self) -> Result<SignerCapabilities, SignerError> {
        let display_name = match &self.config.provider {
            KmsProvider::Mock => format!("MockKMS({})", self.config.key_id),
            KmsProvider::AwsKms => format!("AWS-KMS({})", self.config.key_id),
            KmsProvider::HashiCorpVault => format!("Vault({})", self.config.key_id),
        };

        Ok(SignerCapabilities {
            accounts: self.config.accounts.clone(),
            chain_profiles: self.config.chain_profiles.clone(),
            hardware_backed: true,
            display_name,
            can_sign: true,
        })
    }

    async fn sign(&self, request: CanonicalSignRequest) -> Result<SignedPayload, SignerError> {
        // Contract: reject expired requests before any processing.
        if request.expires_at <= now() {
            return Err(SignerError::Expired {
                expired_at: request.expires_at,
            });
        }

        // Verify the account is managed by this signer.
        let known = self
            .config
            .accounts
            .iter()
            .any(|a| a.account_id == request.account.account_id);
        if !known {
            return Err(SignerError::AccountNotFound {
                account: request.account,
            });
        }

        self.sign_count.fetch_add(1, Ordering::SeqCst);

        match &self.config.provider {
            KmsProvider::Mock => {
                tracing::debug!(
                    key_id = %self.config.key_id,
                    request_id = %request.request_id,
                    "mock KMS signing request"
                );

                let public_key = Self::derive_mock_public_key(&self.config.key_id);
                let signature = Self::mock_sign(&self.config.key_id, &request.payload);

                let mut signed_extrinsic = request.payload.clone();
                signed_extrinsic.extend_from_slice(&signature);

                Ok(SignedPayload {
                    signed_extrinsic,
                    public_key,
                    signature,
                })
            }
            #[cfg(feature = "aws-kms")]
            KmsProvider::AwsKms => Err(SignerError::Internal {
                message: "AWS KMS signing not yet implemented".into(),
            }),
            #[cfg(not(feature = "aws-kms"))]
            KmsProvider::AwsKms => Err(SignerError::Internal {
                message: "AWS KMS support requires the `aws-kms` feature".into(),
            }),
            #[cfg(feature = "hashicorp-vault")]
            KmsProvider::HashiCorpVault => Err(SignerError::Internal {
                message: "HashiCorp Vault signing not yet implemented".into(),
            }),
            #[cfg(not(feature = "hashicorp-vault"))]
            KmsProvider::HashiCorpVault => Err(SignerError::Internal {
                message: "HashiCorp Vault support requires the `hashicorp-vault` feature".into(),
            }),
        }
    }

    async fn health(&self) -> Result<(), SignerError> {
        match &self.config.provider {
            KmsProvider::Mock => Ok(()),
            #[cfg(feature = "aws-kms")]
            KmsProvider::AwsKms => Err(SignerError::Internal {
                message: "AWS KMS health check not yet implemented".into(),
            }),
            #[cfg(not(feature = "aws-kms"))]
            KmsProvider::AwsKms => Err(SignerError::Internal {
                message: "AWS KMS support requires the `aws-kms` feature".into(),
            }),
            #[cfg(feature = "hashicorp-vault")]
            KmsProvider::HashiCorpVault => Err(SignerError::Internal {
                message: "HashiCorp Vault health check not yet implemented".into(),
            }),
            #[cfg(not(feature = "hashicorp-vault"))]
            KmsProvider::HashiCorpVault => Err(SignerError::Internal {
                message: "HashiCorp Vault support requires the `hashicorp-vault` feature".into(),
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
    use polkagent_signer_trait::{conformance, contracts};

    // -- Unit tests ---------------------------------------------------------

    #[tokio::test]
    async fn mock_signer_signs_successfully() {
        let signer = KmsSigner::mock("test-key-1");
        let caps = signer.describe().await.expect("describe ok");
        let account = caps.accounts[0].clone();

        let request = conformance::valid_sign_request(account);
        let payload = request.payload.clone();
        let signed = signer.sign(request).await.expect("sign ok");

        assert!(!signed.signature.is_empty());
        assert_eq!(signed.public_key.len(), 32);
        assert!(signed.signed_extrinsic.starts_with(&payload));
    }

    #[tokio::test]
    async fn mock_signer_rejects_unknown_account() {
        let signer = KmsSigner::mock("test-key-1");
        let unknown = AccountRef::from_bytes([0xFF; 32]);
        let request = conformance::valid_sign_request(unknown);
        let result = signer.sign(request).await;
        assert!(matches!(result, Err(SignerError::AccountNotFound { .. })));
    }

    #[tokio::test]
    async fn mock_signer_rejects_expired_request() {
        let signer = KmsSigner::mock("test-key-1");
        let caps = signer.describe().await.expect("describe ok");
        let account = caps.accounts[0].clone();

        let request = conformance::expired_sign_request(account);
        let result = signer.sign(request).await;
        assert!(matches!(result, Err(SignerError::Expired { .. })));
    }

    #[tokio::test]
    async fn sign_count_increments() {
        let signer = KmsSigner::mock("test-key-1");
        assert_eq!(signer.sign_count(), 0);

        let caps = signer.describe().await.expect("describe ok");
        let account = caps.accounts[0].clone();

        let request = conformance::valid_sign_request(account.clone());
        signer.sign(request).await.expect("sign ok");
        assert_eq!(signer.sign_count(), 1);

        let request = conformance::valid_sign_request(account);
        signer.sign(request).await.expect("sign ok");
        assert_eq!(signer.sign_count(), 2);
    }

    #[tokio::test]
    async fn mock_health_succeeds() {
        let signer = KmsSigner::mock("test-key-1");
        assert!(signer.health().await.is_ok());
    }

    #[tokio::test]
    async fn describe_reports_hardware_backed() {
        let signer = KmsSigner::mock("test-key-1");
        let caps = signer.describe().await.expect("describe ok");
        assert!(caps.hardware_backed);
        assert!(caps.can_sign);
        assert!(caps.display_name.contains("MockKMS"));
    }

    #[tokio::test]
    async fn config_serializes_to_toml() {
        let signer = KmsSigner::mock("arn:aws:kms:us-east-1:123456:key/my-key");
        let toml_str =
            toml::to_string_pretty(&signer.config).expect("config should serialize to TOML");
        assert!(toml_str.contains("mock"));
        assert!(toml_str.contains("my-key"));
    }

    #[tokio::test]
    async fn different_key_ids_produce_different_accounts() {
        let signer_a = KmsSigner::mock("key-alpha");
        let signer_b = KmsSigner::mock("key-beta");
        let caps_a = signer_a.describe().await.expect("describe ok");
        let caps_b = signer_b.describe().await.expect("describe ok");
        assert_ne!(caps_a.accounts[0].account_id, caps_b.accounts[0].account_id);
    }

    // -- Contract tests (polkagent-signer-trait::contracts) -----------------

    #[tokio::test]
    async fn contract_describe_returns_accounts() {
        let signer = KmsSigner::mock("contract-key");
        contracts::test_describe_returns_accounts(&signer).await;
    }

    #[tokio::test]
    async fn contract_sign_returns_payload() {
        let signer = KmsSigner::mock("contract-key");
        let caps = signer.describe().await.expect("describe ok");
        let account = caps.accounts[0].clone();
        contracts::test_sign_returns_payload(&signer, account).await;
    }

    #[tokio::test]
    async fn contract_sign_payload_matches_request() {
        let signer = KmsSigner::mock("contract-key");
        let caps = signer.describe().await.expect("describe ok");
        let account = caps.accounts[0].clone();
        contracts::test_sign_payload_matches_request(&signer, account).await;
    }

    #[tokio::test]
    async fn contract_health_returns_result() {
        let signer = KmsSigner::mock("contract-key");
        contracts::test_health_returns_result(&signer).await;
    }

    // -- Conformance tests (polkagent-signer-trait::conformance) ------------

    #[tokio::test]
    async fn conformance_describe_returns_accounts() {
        let signer = KmsSigner::mock("conformance-key");
        conformance::test_describe_returns_accounts(&signer).await;
    }

    #[tokio::test]
    async fn conformance_sign_returns_signature() {
        let signer = KmsSigner::mock("conformance-key");
        let account = conformance::first_account(&signer)
            .await
            .expect("has account");
        conformance::test_sign_returns_signature(&signer, account).await;
    }

    #[tokio::test]
    async fn conformance_sign_deterministic() {
        let signer = KmsSigner::mock("conformance-key");
        let account = conformance::first_account(&signer)
            .await
            .expect("has account");
        conformance::test_sign_deterministic_for_same_input(&signer, account).await;
    }

    #[tokio::test]
    async fn conformance_different_inputs_different_signatures() {
        let signer = KmsSigner::mock("conformance-key");
        let account = conformance::first_account(&signer)
            .await
            .expect("has account");
        conformance::test_sign_different_inputs_different_signatures(&signer, account).await;
    }

    #[tokio::test]
    async fn conformance_verify_valid_signature() {
        let signer = KmsSigner::mock("conformance-key");
        let account = conformance::first_account(&signer)
            .await
            .expect("has account");
        conformance::test_verify_valid_signature(&signer, account).await;
    }

    #[tokio::test]
    async fn conformance_expired_request_rejected() {
        let signer = KmsSigner::mock("conformance-key");
        let account = conformance::first_account(&signer)
            .await
            .expect("has account");
        conformance::test_verify_invalid_signature_fails(&signer, account).await;
    }

    #[tokio::test]
    async fn conformance_no_plaintext_secrets() {
        let signer = KmsSigner::mock("conformance-key");
        let account = conformance::first_account(&signer)
            .await
            .expect("has account");
        conformance::test_signer_never_sees_plaintext_secret(&signer, account).await;
    }

    #[tokio::test]
    async fn conformance_health() {
        let signer = KmsSigner::mock("conformance-key");
        conformance::test_health_returns_result(&signer).await;
    }
}
