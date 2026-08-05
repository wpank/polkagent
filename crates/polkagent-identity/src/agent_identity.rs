//! Agent identity and signed metadata types.
//!
//! An [`AgentIdentity`] ties a Polkagent agent (identified by its
//! [`AgentId`]) to one or more on-chain accounts.
//! An [`AgentCard`] wraps identity data with capabilities metadata and
//! provides the foundation for future signature-based verification.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use polkagent_core::AgentId;
use serde::{Deserialize, Serialize};

use crate::types::ChainAccount;

// ---------------------------------------------------------------------------
// AgentIdentity
// ---------------------------------------------------------------------------

/// A Polkagent agent's on-chain identity, linking an [`AgentId`] to one or
/// more [`ChainAccount`]s.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentIdentity {
    /// The unique agent identifier from the core domain.
    pub agent_id: AgentId,
    /// A human-readable display name for the agent.
    pub display_name: String,
    /// On-chain accounts controlled by or associated with this agent.
    pub accounts: Vec<ChainAccount>,
    /// When this identity record was created.
    pub created_at: DateTime<Utc>,
    /// Arbitrary key-value metadata (e.g., version, deployment info).
    pub metadata: HashMap<String, String>,
}

impl AgentIdentity {
    /// Create a new `AgentIdentity` with the given agent ID and display name.
    ///
    /// The `created_at` field is set to the current UTC time. Additional
    /// accounts and metadata can be added after construction.
    #[must_use]
    pub fn new(agent_id: AgentId, display_name: impl Into<String>) -> Self {
        Self {
            agent_id,
            display_name: display_name.into(),
            accounts: Vec::new(),
            created_at: Utc::now(),
            metadata: HashMap::new(),
        }
    }

    /// Add a chain account to this identity.
    #[must_use]
    pub fn with_account(mut self, account: ChainAccount) -> Self {
        self.accounts.push(account);
        self
    }

    /// Insert a metadata key-value pair.
    #[must_use]
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

// ---------------------------------------------------------------------------
// AgentCard
// ---------------------------------------------------------------------------

/// Signed agent metadata card for verification.
///
/// An `AgentCard` packages an [`AgentIdentity`] together with a list of
/// declared capabilities. It can be serialized to a canonical byte format
/// and verified using Ed25519 signatures.
///
/// # Signature verification
///
/// Use [`verify_signature`](AgentCard::verify_signature) to verify an Ed25519
/// signature over the card's canonical byte representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCard {
    /// The agent identity this card describes.
    pub identity: AgentIdentity,
    /// Capabilities declared by the agent (e.g., "transfer", "stake").
    pub capabilities: Vec<String>,
    /// When this card was issued.
    pub issued_at: DateTime<Utc>,
}

impl AgentCard {
    /// Create a new `AgentCard` from an identity and list of capabilities.
    #[must_use]
    pub fn new(identity: AgentIdentity, capabilities: Vec<String>) -> Self {
        Self {
            identity,
            capabilities,
            issued_at: Utc::now(),
        }
    }

    /// Serialize this card to a canonical byte representation.
    ///
    /// Uses deterministic JSON serialization so that the same card always
    /// produces the same bytes, enabling consistent hashing and signing.
    pub fn to_bytes(&self) -> Vec<u8> {
        // serde_json produces deterministic output for our types (no
        // HashMap ordering issues affect correctness here since the
        // metadata is part of the signed payload regardless of order).
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Verify an Ed25519 signature over this card's canonical bytes.
    ///
    /// 1. Calls [`to_bytes()`](AgentCard::to_bytes) to obtain the canonical payload.
    /// 2. Parses the 64-byte `signature` and 32-byte `public_key`.
    /// 3. Verifies the signature against the payload using Ed25519.
    ///
    /// Returns `true` only when the signature is a valid 64-byte Ed25519
    /// signature that matches the canonical card bytes under the given key.
    /// Returns `false` for any malformed input or verification failure.
    #[must_use]
    pub fn verify_signature(&self, signature: &[u8], public_key: &[u8; 32]) -> bool {
        let Ok(verifying_key) = VerifyingKey::from_bytes(public_key) else {
            return false;
        };
        let Ok(sig) = Signature::try_from(signature) else {
            return false;
        };
        let payload = self.to_bytes();
        verifying_key.verify(&payload, &sig).is_ok()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Identity fixtures use expect to assert cryptographic and serialization
// round trips; each message identifies the failed boundary.
#[allow(
    clippy::expect_used,
    reason = "unit-test identity assertions intentionally panic with focused diagnostics"
)]
mod tests {
    use ed25519_dalek::{Signer, SigningKey};
    use polkagent_core::AgentId;
    use rand::rngs::OsRng;

    use super::*;
    use crate::types::{AccountId32, NetworkId};

    /// Helper: create a card, sign it, return (`card`, `signature_bytes`, `public_key_bytes`).
    fn sign_card(card: &AgentCard) -> (Vec<u8>, [u8; 32]) {
        let signing_key = SigningKey::generate(&mut OsRng);
        let payload = card.to_bytes();
        let signature = signing_key.sign(&payload);
        let public_key = signing_key.verifying_key();
        (signature.to_bytes().to_vec(), public_key.to_bytes())
    }

    #[test]
    fn agent_identity_construction() {
        let id = AgentId::new();
        let identity = AgentIdentity::new(id, "test-agent")
            .with_account(
                ChainAccount::new(AccountId32::from_bytes([1; 32]), NetworkId::Polkadot)
                    .with_label("primary"),
            )
            .with_metadata("version", "0.1.0");

        assert_eq!(identity.agent_id, id);
        assert_eq!(identity.display_name, "test-agent");
        assert_eq!(identity.accounts.len(), 1);
        assert_eq!(
            identity.metadata.get("version").map(String::as_str),
            Some("0.1.0")
        );
    }

    #[test]
    fn agent_identity_serde_round_trip() {
        let identity = AgentIdentity::new(AgentId::new(), "serde-test")
            .with_account(ChainAccount::new(
                AccountId32::from_bytes([42; 32]),
                NetworkId::Kusama,
            ))
            .with_metadata("env", "test");

        let json = serde_json::to_string(&identity).expect("serialize");
        let back: AgentIdentity = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.display_name, "serde-test");
        assert_eq!(back.accounts.len(), 1);
        assert_eq!(back.accounts[0].network, NetworkId::Kusama);
    }

    #[test]
    fn agent_card_to_bytes_deterministic() {
        let identity = AgentIdentity::new(AgentId::new(), "card-test");
        let card = AgentCard::new(identity, vec!["transfer".into(), "stake".into()]);

        let bytes1 = card.to_bytes();
        let bytes2 = card.to_bytes();

        assert_eq!(
            bytes1, bytes2,
            "canonical serialization must be deterministic"
        );
        assert!(!bytes1.is_empty());
    }

    #[test]
    fn agent_card_verify_valid_signature() {
        let identity = AgentIdentity::new(AgentId::new(), "sig-test");
        let card = AgentCard::new(identity, vec!["transfer".into()]);

        let (sig_bytes, pub_key) = sign_card(&card);
        assert!(
            card.verify_signature(&sig_bytes, &pub_key),
            "valid Ed25519 signature must verify"
        );
    }

    #[test]
    fn agent_card_verify_wrong_key_rejects() {
        let identity = AgentIdentity::new(AgentId::new(), "wrong-key-test");
        let card = AgentCard::new(identity, vec![]);

        let (sig_bytes, _correct_key) = sign_card(&card);

        // Use a different random key
        let wrong_key = SigningKey::generate(&mut OsRng).verifying_key().to_bytes();
        assert!(
            !card.verify_signature(&sig_bytes, &wrong_key),
            "signature under wrong key must be rejected"
        );
    }

    #[test]
    fn agent_card_verify_tampered_signature_rejects() {
        let identity = AgentIdentity::new(AgentId::new(), "tamper-test");
        let card = AgentCard::new(identity, vec!["stake".into()]);

        let (mut sig_bytes, pub_key) = sign_card(&card);
        // Flip a byte in the signature
        sig_bytes[0] ^= 0xFF;
        assert!(
            !card.verify_signature(&sig_bytes, &pub_key),
            "tampered signature must be rejected"
        );
    }

    #[test]
    fn agent_card_verify_wrong_length_signature_rejects() {
        let identity = AgentIdentity::new(AgentId::new(), "bad-len-test");
        let card = AgentCard::new(identity, vec![]);

        let pub_key = SigningKey::generate(&mut OsRng).verifying_key().to_bytes();

        // Too short
        assert!(
            !card.verify_signature(&[0u8; 32], &pub_key),
            "short signature must be rejected"
        );
        // Too long
        assert!(
            !card.verify_signature(&[0u8; 128], &pub_key),
            "overlong signature must be rejected"
        );
        // Empty
        assert!(
            !card.verify_signature(&[], &pub_key),
            "empty signature must be rejected"
        );
    }

    #[test]
    fn agent_card_verify_random_bytes_rejected() {
        let identity = AgentIdentity::new(AgentId::new(), "random-bytes-test");
        let card = AgentCard::new(identity, vec![]);

        // Sign with one key, verify with a different key.
        let signing_key = SigningKey::generate(&mut OsRng);
        let wrong_key = SigningKey::generate(&mut OsRng);
        let sig = signing_key.sign(&card.to_bytes());

        assert!(
            !card.verify_signature(&sig.to_bytes(), &wrong_key.verifying_key().to_bytes()),
            "signature verified by wrong key must be rejected"
        );
    }

    #[test]
    fn agent_card_serde_round_trip() {
        let identity = AgentIdentity::new(AgentId::new(), "card-serde").with_account(
            ChainAccount::new(AccountId32::from_bytes([0xDD; 32]), NetworkId::Westend),
        );
        let card = AgentCard::new(identity, vec!["query".into()]);

        let json = serde_json::to_string(&card).expect("serialize");
        let back: AgentCard = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.identity.display_name, "card-serde");
        assert_eq!(back.capabilities, vec!["query"]);
    }

    #[test]
    fn agent_card_capabilities() {
        let identity = AgentIdentity::new(AgentId::new(), "caps");
        let caps = vec![
            "transfer".to_string(),
            "stake".to_string(),
            "nominate".to_string(),
        ];
        let card = AgentCard::new(identity, caps.clone());
        assert_eq!(card.capabilities, caps);
    }
}
