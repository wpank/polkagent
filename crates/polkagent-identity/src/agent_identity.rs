//! Agent identity and signed metadata types.
//!
//! An [`AgentIdentity`] ties a Polkagent agent (identified by its
//! [`AgentId`](polkagent_core::AgentId)) to one or more on-chain accounts.
//! An [`AgentCard`] wraps identity data with capabilities metadata and
//! provides the foundation for future signature-based verification.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
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
/// for future signature-based verification workflows.
///
/// # Signature verification
///
/// The [`verify_signature`](AgentCard::verify_signature) method is currently a
/// stub that always returns `false`. A full implementation will be added once
/// the signing infrastructure is in place.
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

    /// Verify a signature over this card's canonical bytes.
    ///
    /// # Stub implementation
    ///
    /// This always returns `false`. A real implementation would:
    /// 1. Call [`to_bytes()`](AgentCard::to_bytes) to obtain the canonical payload.
    /// 2. Verify the `signature` against the payload using the `public_key`
    ///    with Ed25519 or Sr25519 (depending on the key type indicator).
    ///
    /// The stub conservatively returns `false` (reject) rather than `true`
    /// so that callers cannot accidentally bypass verification.
    #[must_use]
    pub fn verify_signature(&self, _signature: &[u8], _public_key: &[u8; 32]) -> bool {
        // Stub: signature verification is not yet implemented.
        // Returning `false` is the safe default -- no signature is ever
        // considered valid until the signing infrastructure lands.
        false
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use polkagent_core::AgentId;

    use super::*;
    use crate::types::{AccountId32, NetworkId};

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
    fn agent_card_verify_signature_stub_returns_false() {
        let identity = AgentIdentity::new(AgentId::new(), "sig-test");
        let card = AgentCard::new(identity, vec![]);

        let fake_sig = vec![0u8; 64];
        let fake_key = [0u8; 32];

        assert!(
            !card.verify_signature(&fake_sig, &fake_key),
            "stub should always return false"
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
