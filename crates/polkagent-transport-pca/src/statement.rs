//! C3: Statement store integration for bulletin CID anchoring.
//!
//! This module defines the trait and data types for anchoring message
//! content to a content-addressable store (e.g. IPFS) via content
//! identifiers (CIDs).
//!
//! # Purpose
//!
//! Anchoring provides **non-repudiation** and **auditability**: once a
//! message is anchored, any party holding the CID can independently verify
//! the content. This is useful for:
//!
//! - Regulatory compliance (provable audit trails).
//! - Cross-organisational message verification.
//! - Offline verification without access to the live transport.
//!
//! # External Infrastructure Required
//!
//! The [`StatementStore`] trait intentionally abstracts over the storage
//! backend. Possible implementations include:
//!
//! - **IPFS** — content-addressed, decentralised. The CID is an IPFS
//!   multihash of the content.
//! - **Arweave / Filecoin** — permanent storage with on-chain anchoring.
//! - **Local file store** — for testing and air-gapped deployments.
//!
//! This crate ships an [`InMemoryStatementStore`] for testing. Production
//! backends should be provided by a separate crate that depends on the
//! relevant storage SDK.

use std::collections::{BTreeMap, HashMap};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::error::PcaError;

// ---------------------------------------------------------------------------
// Content Identifier
// ---------------------------------------------------------------------------

/// A content identifier (CID) referencing an anchored statement.
///
/// The format depends on the backing store (e.g. IPFS CIDv1, a SHA-256
/// hex string for local stores). This type is intentionally opaque.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Cid(pub String);

impl Cid {
    /// Create a CID from a string.
    pub fn new(cid: impl Into<String>) -> Self {
        Self(cid.into())
    }

    /// Return the CID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Cid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ---------------------------------------------------------------------------
// Bulletin entry
// ---------------------------------------------------------------------------

/// A bulletin entry representing a single anchored statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulletinEntry {
    /// The content identifier for this entry.
    pub cid: Cid,
    /// The conversation this entry belongs to.
    pub conversation_id: String,
    /// Monotonic sequence number within the conversation.
    pub sequence: u64,
    /// Epoch-millisecond timestamp of when the statement was created.
    pub timestamp_ms: u64,
    /// SHA-256 hash of the plaintext payload (for integrity verification).
    pub payload_hash: [u8; 32],
}

/// An anchored statement with a cryptographic signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatementAnchor {
    /// The bulletin entry.
    pub entry: BulletinEntry,
    /// The SS58 address of the signer.
    pub signer: String,
    /// Signature over the entry (format depends on the identity scheme).
    pub signature: Vec<u8>,
    /// Whether this anchor has been confirmed by the backing store.
    pub confirmed: bool,
}

// ---------------------------------------------------------------------------
// StatementStore trait
// ---------------------------------------------------------------------------

/// Trait for statement store backends (C3).
///
/// Implementations are responsible for persisting bulletin entries and
/// providing lookup/verification.
///
/// # Contract
///
/// - [`anchor`](StatementStore::anchor) must be **idempotent**: re-anchoring
///   the same entry (by CID) should succeed without creating a duplicate.
/// - [`verify`](StatementStore::verify) checks both the signature validity
///   and that the CID matches the stored payload hash.
/// - [`lookup`](StatementStore::lookup) returns `None` for unknown CIDs
///   (never errors on a miss).
pub trait StatementStore: Send + Sync {
    /// Anchor a bulletin entry and return the signed statement.
    ///
    /// The implementation should:
    /// 1. Persist the entry to the backing store.
    /// 2. Sign the entry with the local identity.
    /// 3. Return the anchor with `confirmed = true` once durable.
    fn anchor(&self, entry: BulletinEntry, signer: &str) -> Result<StatementAnchor, PcaError>;

    /// Verify that a statement anchor is valid.
    ///
    /// Returns `true` if the signature is valid and the CID matches the
    /// stored content.
    fn verify(&self, anchor: &StatementAnchor) -> Result<bool, PcaError>;

    /// Look up a bulletin entry by CID.
    fn lookup(&self, cid: &Cid) -> Result<Option<BulletinEntry>, PcaError>;

    /// List all bulletin entries for a conversation, ordered by sequence.
    fn list_by_conversation(&self, conversation_id: &str) -> Result<Vec<BulletinEntry>, PcaError>;
}

// ---------------------------------------------------------------------------
// In-memory implementation (testing)
// ---------------------------------------------------------------------------

/// An in-memory statement store for testing.
///
/// This implementation does **not** perform real cryptographic signing;
/// the "signature" is simply the signer address encoded as bytes. It is
/// not suitable for production use.
pub struct InMemoryStatementStore {
    entries: RwLock<BTreeMap<String, BulletinEntry>>,
    by_conversation: RwLock<HashMap<String, Vec<String>>>,
}

impl InMemoryStatementStore {
    /// Create a new empty in-memory store.
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(BTreeMap::new()),
            by_conversation: RwLock::new(HashMap::new()),
        }
    }

    /// Return the total number of anchored entries.
    pub fn len(&self) -> usize {
        self.entries.read().len()
    }

    /// Check whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.read().is_empty()
    }
}

impl Default for InMemoryStatementStore {
    fn default() -> Self {
        Self::new()
    }
}

impl StatementStore for InMemoryStatementStore {
    fn anchor(&self, entry: BulletinEntry, signer: &str) -> Result<StatementAnchor, PcaError> {
        let cid_str = entry.cid.0.clone();
        let conv_id = entry.conversation_id.clone();

        // Idempotent: if the CID already exists, return the existing entry.
        {
            let entries = self.entries.read();
            if let Some(existing) = entries.get(&cid_str) {
                return Ok(StatementAnchor {
                    entry: existing.clone(),
                    signer: signer.to_string(),
                    signature: signer.as_bytes().to_vec(),
                    confirmed: true,
                });
            }
        }

        self.entries.write().insert(cid_str.clone(), entry.clone());
        self.by_conversation
            .write()
            .entry(conv_id)
            .or_default()
            .push(cid_str);

        Ok(StatementAnchor {
            entry,
            signer: signer.to_string(),
            signature: signer.as_bytes().to_vec(),
            confirmed: true,
        })
    }

    fn verify(&self, anchor: &StatementAnchor) -> Result<bool, PcaError> {
        // In-memory "verification": check that the signer matches the
        // signature bytes and the CID exists.
        let sig_matches = anchor.signature == anchor.signer.as_bytes();
        let cid_exists = self.entries.read().contains_key(&anchor.entry.cid.0);
        Ok(sig_matches && cid_exists)
    }

    fn lookup(&self, cid: &Cid) -> Result<Option<BulletinEntry>, PcaError> {
        Ok(self.entries.read().get(&cid.0).cloned())
    }

    fn list_by_conversation(&self, conversation_id: &str) -> Result<Vec<BulletinEntry>, PcaError> {
        let by_conv = self.by_conversation.read();
        let entries = self.entries.read();

        let cids = match by_conv.get(conversation_id) {
            Some(cids) => cids,
            None => return Ok(Vec::new()),
        };

        let mut result: Vec<BulletinEntry> = cids
            .iter()
            .filter_map(|cid| entries.get(cid).cloned())
            .collect();

        result.sort_by_key(|e| e.sequence);
        Ok(result)
    }
}

/// Helper: compute a simple payload hash for testing.
///
/// Uses a basic XOR-fold; production code should use SHA-256.
pub fn test_payload_hash(payload: &[u8]) -> [u8; 32] {
    let mut hash = [0u8; 32];
    for (i, &b) in payload.iter().enumerate() {
        hash[i % 32] ^= b;
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(cid: &str, conv: &str, seq: u64) -> BulletinEntry {
        BulletinEntry {
            cid: Cid::new(cid),
            conversation_id: conv.into(),
            sequence: seq,
            timestamp_ms: 1_000_000 + seq,
            payload_hash: test_payload_hash(format!("payload-{seq}").as_bytes()),
        }
    }

    #[test]
    fn cid_display() {
        let cid = Cid::new("bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi");
        assert_eq!(
            cid.to_string(),
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi"
        );
        assert_eq!(cid.as_str(), cid.0.as_str());
    }

    #[test]
    fn anchor_and_lookup() {
        let store = InMemoryStatementStore::new();
        let entry = make_entry("cid-1", "conv-a", 0);

        let anchor = store.anchor(entry.clone(), "5Alice...").expect("anchor");
        assert!(anchor.confirmed);
        assert_eq!(anchor.signer, "5Alice...");

        let looked_up = store.lookup(&Cid::new("cid-1")).expect("lookup");
        assert!(looked_up.is_some());
        assert_eq!(looked_up.as_ref().map(|e| e.sequence), Some(0));
    }

    #[test]
    fn anchor_is_idempotent() {
        let store = InMemoryStatementStore::new();
        let entry = make_entry("cid-1", "conv-a", 0);

        store.anchor(entry.clone(), "5Alice...").expect("first");
        store.anchor(entry, "5Alice...").expect("second");

        assert_eq!(store.len(), 1);
    }

    #[test]
    fn lookup_missing_returns_none() {
        let store = InMemoryStatementStore::new();
        let result = store.lookup(&Cid::new("nonexistent")).expect("lookup");
        assert!(result.is_none());
    }

    #[test]
    fn list_by_conversation_ordered() {
        let store = InMemoryStatementStore::new();
        // Insert out of order.
        store
            .anchor(make_entry("cid-3", "conv-a", 2), "5A...")
            .expect("anchor");
        store
            .anchor(make_entry("cid-1", "conv-a", 0), "5A...")
            .expect("anchor");
        store
            .anchor(make_entry("cid-2", "conv-a", 1), "5A...")
            .expect("anchor");

        let list = store.list_by_conversation("conv-a").expect("list");
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].sequence, 0);
        assert_eq!(list[1].sequence, 1);
        assert_eq!(list[2].sequence, 2);
    }

    #[test]
    fn list_by_conversation_empty() {
        let store = InMemoryStatementStore::new();
        let list = store.list_by_conversation("no-such-conv").expect("list");
        assert!(list.is_empty());
    }

    #[test]
    fn verify_valid_anchor() {
        let store = InMemoryStatementStore::new();
        let entry = make_entry("cid-1", "conv-a", 0);
        let anchor = store.anchor(entry, "5Alice...").expect("anchor");
        assert!(store.verify(&anchor).expect("verify"));
    }

    #[test]
    fn verify_fails_with_wrong_signer() {
        let store = InMemoryStatementStore::new();
        let entry = make_entry("cid-1", "conv-a", 0);
        let mut anchor = store.anchor(entry, "5Alice...").expect("anchor");
        anchor.signer = "5Mallory...".into(); // tamper
        assert!(!store.verify(&anchor).expect("verify"));
    }

    #[test]
    fn verify_fails_for_missing_cid() {
        let store = InMemoryStatementStore::new();
        let anchor = StatementAnchor {
            entry: make_entry("cid-unknown", "conv-a", 0),
            signer: "5Alice...".into(),
            signature: b"5Alice...".to_vec(),
            confirmed: true,
        };
        assert!(!store.verify(&anchor).expect("verify"));
    }

    #[test]
    fn empty_store() {
        let store = InMemoryStatementStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn bulletin_entry_serializes() {
        let entry = make_entry("cid-1", "conv-a", 42);
        let json = serde_json::to_string(&entry).expect("serialize");
        let back: BulletinEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.cid, entry.cid);
        assert_eq!(back.sequence, 42);
    }

    #[test]
    fn statement_anchor_serializes() {
        let store = InMemoryStatementStore::new();
        let entry = make_entry("cid-1", "conv-a", 0);
        let anchor = store.anchor(entry, "5Alice...").expect("anchor");

        let json = serde_json::to_string(&anchor).expect("serialize");
        let back: StatementAnchor = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.signer, "5Alice...");
        assert!(back.confirmed);
    }

    #[test]
    fn multiple_conversations() {
        let store = InMemoryStatementStore::new();
        store
            .anchor(make_entry("cid-a1", "conv-a", 0), "5A...")
            .expect("anchor");
        store
            .anchor(make_entry("cid-b1", "conv-b", 0), "5A...")
            .expect("anchor");
        store
            .anchor(make_entry("cid-a2", "conv-a", 1), "5A...")
            .expect("anchor");

        let list_a = store.list_by_conversation("conv-a").expect("list");
        assert_eq!(list_a.len(), 2);
        let list_b = store.list_by_conversation("conv-b").expect("list");
        assert_eq!(list_b.len(), 1);
    }

    #[test]
    fn test_payload_hash_deterministic() {
        let h1 = test_payload_hash(b"hello");
        let h2 = test_payload_hash(b"hello");
        assert_eq!(h1, h2);

        let h3 = test_payload_hash(b"world");
        assert_ne!(h1, h3);
    }
}
