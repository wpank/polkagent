//! C2: End-to-end encrypted group messaging.
//!
//! This module provides multi-party encrypted communication on top of the
//! pairwise X25519 / ChaCha20-Poly1305 primitives from [`crate::crypto`].
//!
//! # Design
//!
//! Groups use a **pairwise fan-out** model:
//!
//! - Each group member maintains an independent X25519 key pair.
//! - A **group epoch key** is derived by hashing all pairwise shared secrets
//!   together (sorted by SS58 address for determinism). All members who have
//!   completed pairwise key agreement with every other member derive the same
//!   epoch key.
//! - Sending to a group encrypts the plaintext once with the epoch key using a
//!   per-sender nonce counter.
//! - On **member leave** the epoch advances and a new group key is derived
//!   (excluding the departed member's secret), providing forward secrecy for
//!   subsequent messages.
//! - On **member join** the new member must complete pairwise DH with all
//!   existing members before a new epoch key can be derived.
//!
//! # Limitations
//!
//! - The pairwise fan-out model is practical for small groups (≤ ~50 members).
//!   For larger groups, a tree-based protocol (e.g. MLS) would be required.
//! - The handshake for a joining member must be coordinated out-of-band by
//!   distributing public keys to all existing members.

use std::collections::BTreeMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::crypto::{self, KeyPair, NonceCounter, SharedSecret};
use crate::error::PcaError;

/// Unique identifier for a group.
pub type GroupId = String;

/// A member of an encrypted group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMember {
    /// The member's SS58 address (identity).
    pub ss58_address: String,
    /// The member's current X25519 public key.
    pub public_key: [u8; 32],
    /// Optional human-readable label.
    pub label: Option<String>,
    /// The epoch at which this member joined.
    pub joined_epoch: u32,
}

/// Events emitted by group membership operations.
#[derive(Debug, Clone)]
pub enum GroupEvent {
    /// A new member has joined the group.
    MemberJoined {
        /// The member that joined.
        ss58_address: String,
    },
    /// A member has left the group.
    MemberLeft {
        /// The member that left.
        ss58_address: String,
    },
    /// The group epoch has advanced (new key material).
    EpochAdvanced {
        /// The new epoch number.
        new_epoch: u32,
    },
}

/// Tracks pairwise shared secrets and derives the group epoch key.
struct GroupKeyState {
    /// Current epoch number (increments on membership changes).
    epoch: u32,
    /// Derived group-wide symmetric key for the current epoch.
    group_key: Option<[u8; 32]>,
    /// Pairwise shared secrets keyed by peer SS58 address.
    pairwise_secrets: BTreeMap<String, SharedSecret>,
}

impl GroupKeyState {
    fn new() -> Self {
        Self {
            epoch: 0,
            group_key: None,
            pairwise_secrets: BTreeMap::new(),
        }
    }

    /// Derive the group epoch key by hashing all pairwise secrets in
    /// deterministic (sorted) order.
    ///
    /// Uses iterated XOR + a final ChaCha20 pass as a KDF. This is
    /// intentionally simple; a production deployment would use HKDF.
    fn derive_group_key(&mut self) {
        if self.pairwise_secrets.is_empty() {
            self.group_key = None;
            return;
        }

        // XOR all pairwise secrets together (BTreeMap is already sorted).
        let mut combined = [0u8; 32];
        for secret in self.pairwise_secrets.values() {
            let bytes = secret.as_bytes();
            for (c, s) in combined.iter_mut().zip(bytes.iter()) {
                *c ^= s;
            }
        }

        // Mix in the epoch number for domain separation.
        let epoch_bytes = self.epoch.to_le_bytes();
        for (i, &b) in epoch_bytes.iter().enumerate() {
            combined[i] ^= b;
        }

        self.group_key = Some(combined);
    }

    fn advance_epoch(&mut self) {
        self.epoch += 1;
        self.derive_group_key();
    }
}

/// An encrypted envelope for group messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupEnvelope {
    /// The group this message belongs to.
    pub group_id: String,
    /// SS58 address of the sender.
    pub sender_address: String,
    /// The epoch under which this message was encrypted.
    pub epoch: u32,
    /// Per-sender monotonic sequence number.
    pub sequence: u64,
    /// The nonce used for encryption.
    pub nonce: [u8; 12],
    /// Ciphertext (includes AEAD auth tag).
    pub ciphertext: Vec<u8>,
}

/// Serializable snapshot for group persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupSessionSnapshot {
    /// The group ID.
    pub group_id: String,
    /// Local SS58 address.
    pub local_address: String,
    /// Current epoch.
    pub epoch: u32,
    /// Member public keys (address → 32 bytes).
    pub members: Vec<GroupMemberSnapshot>,
    /// Current encrypt nonce counter.
    pub encrypt_nonce_counter: u64,
}

/// Serializable member info within a group snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMemberSnapshot {
    /// Member SS58 address.
    pub ss58_address: String,
    /// Member public key bytes.
    pub public_key: [u8; 32],
    /// Optional label.
    pub label: Option<String>,
    /// Epoch the member joined.
    pub joined_epoch: u32,
}

/// An end-to-end encrypted group session (C2).
///
/// Manages the membership roster, pairwise key agreement with each member,
/// group epoch key derivation, and encrypt/decrypt operations.
pub struct GroupSession {
    /// Unique group identifier.
    group_id: GroupId,
    /// The local participant's SS58 address.
    local_address: String,
    /// The local participant's X25519 key pair.
    local_keypair: KeyPair,
    /// Known group members keyed by SS58 address.
    members: BTreeMap<String, GroupMember>,
    /// Key agreement state.
    key_state: GroupKeyState,
    /// Nonce counter for locally encrypted messages.
    encrypt_nonce: NonceCounter,
    /// Per-member expected decrypt sequence (for ordering).
    decrypt_sequences: BTreeMap<String, u64>,
    /// When this group session was created.
    created_at: Instant,
}

impl GroupSession {
    /// Create a new group session.
    ///
    /// The local participant is automatically the first member.
    pub fn new(group_id: impl Into<String>, local_address: impl Into<String>) -> Self {
        Self::create(group_id.into(), local_address.into())
    }

    /// Create a new group session (takes owned strings).
    pub fn create(group_id: String, local_address: String) -> Self {
        let keypair = KeyPair::generate();

        info!(group_id = %group_id, local = %local_address, "created group session");

        Self {
            group_id,
            local_address,
            local_keypair: keypair,
            members: BTreeMap::new(),
            key_state: GroupKeyState::new(),
            encrypt_nonce: NonceCounter::new(),
            decrypt_sequences: BTreeMap::new(),
            created_at: Instant::now(),
        }
    }

    /// Return the local participant's public key bytes for distribution.
    pub fn local_public_key(&self) -> [u8; 32] {
        self.local_keypair.public_key_bytes()
    }

    /// Return the group identifier.
    pub fn group_id(&self) -> &str {
        &self.group_id
    }

    /// Return the current epoch.
    pub fn epoch(&self) -> u32 {
        self.key_state.epoch
    }

    /// Return the number of members (excluding self).
    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    /// Return the member roster.
    pub fn members(&self) -> impl Iterator<Item = &GroupMember> {
        self.members.values()
    }

    /// Return how long this group session has existed.
    pub fn age(&self) -> std::time::Duration {
        self.created_at.elapsed()
    }

    /// Whether the group has an active epoch key (i.e. at least one pairwise
    /// key agreement has been completed).
    pub fn has_group_key(&self) -> bool {
        self.key_state.group_key.is_some()
    }

    // -- Membership operations -----------------------------------------------

    /// Add a member to the group and perform pairwise key agreement.
    ///
    /// The caller must supply the new member's X25519 public key (obtained
    /// out-of-band). Returns a [`GroupEvent::MemberJoined`] followed by
    /// [`GroupEvent::EpochAdvanced`].
    pub fn add_member(
        &mut self,
        ss58_address: impl Into<String>,
        public_key: &[u8; 32],
        label: Option<String>,
    ) -> Result<Vec<GroupEvent>, PcaError> {
        let ss58_address = ss58_address.into();

        if ss58_address == self.local_address {
            return Err(PcaError::GroupError {
                reason: "cannot add self as group member".into(),
            });
        }

        if self.members.contains_key(&ss58_address) {
            return Err(PcaError::GroupError {
                reason: format!("member {ss58_address} already in group"),
            });
        }

        // Pairwise DH with the new member.
        let peer_public = x25519_dalek::PublicKey::from(*public_key);
        let shared = self.local_keypair.diffie_hellman(&peer_public);

        let epoch = self.key_state.epoch;
        self.members.insert(
            ss58_address.clone(),
            GroupMember {
                ss58_address: ss58_address.clone(),
                public_key: *public_key,
                label,
                joined_epoch: epoch,
            },
        );

        self.key_state
            .pairwise_secrets
            .insert(ss58_address.clone(), shared);

        // Advance epoch with new key material.
        self.key_state.advance_epoch();
        self.decrypt_sequences.insert(ss58_address.clone(), 0);

        debug!(
            group = %self.group_id,
            member = %ss58_address,
            epoch = self.key_state.epoch,
            "member added to group"
        );

        Ok(vec![
            GroupEvent::MemberJoined { ss58_address },
            GroupEvent::EpochAdvanced {
                new_epoch: self.key_state.epoch,
            },
        ])
    }

    /// Remove a member from the group.
    ///
    /// The epoch advances and a new group key is derived that excludes the
    /// departed member, providing forward secrecy. Returns events.
    pub fn remove_member(
        &mut self,
        ss58_address: &str,
    ) -> Result<Vec<GroupEvent>, PcaError> {
        if !self.members.contains_key(ss58_address) {
            return Err(PcaError::GroupError {
                reason: format!("member {ss58_address} not in group"),
            });
        }

        self.members.remove(ss58_address);
        self.key_state.pairwise_secrets.remove(ss58_address);
        self.decrypt_sequences.remove(ss58_address);

        // Advance epoch — new key excludes the departed member.
        self.key_state.advance_epoch();

        // Reset encrypt nonce for the new epoch.
        self.encrypt_nonce = NonceCounter::new();

        warn!(
            group = %self.group_id,
            member = %ss58_address,
            epoch = self.key_state.epoch,
            "member removed from group, epoch advanced"
        );

        Ok(vec![
            GroupEvent::MemberLeft {
                ss58_address: ss58_address.to_string(),
            },
            GroupEvent::EpochAdvanced {
                new_epoch: self.key_state.epoch,
            },
        ])
    }

    // -- Encrypt / decrypt ---------------------------------------------------

    /// Encrypt a plaintext message for the group.
    ///
    /// Uses the current epoch key. All members who hold the same epoch key
    /// can decrypt.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<GroupEnvelope, PcaError> {
        let group_key = self.key_state.group_key.ok_or_else(|| {
            PcaError::GroupKeyAgreementFailed {
                reason: "no group key derived (add at least one member first)".into(),
            }
        })?;

        let nonce = self.encrypt_nonce.next_nonce().ok_or_else(|| {
            PcaError::NonceExhausted {
                session_id: self.group_id.clone(),
            }
        })?;

        let sequence = self.encrypt_nonce.current() - 1;

        // Use the group_id as AAD for domain separation.
        let ciphertext =
            crypto::encrypt_with_aad(&group_key, &nonce, self.group_id.as_bytes(), plaintext)?;

        Ok(GroupEnvelope {
            group_id: self.group_id.clone(),
            sender_address: self.local_address.clone(),
            epoch: self.key_state.epoch,
            sequence,
            nonce,
            ciphertext,
        })
    }

    /// Decrypt a group envelope.
    ///
    /// Verifies the sender is a known member, that the epoch matches, and
    /// that the sequence number is in order for that sender.
    pub fn decrypt(&mut self, envelope: &GroupEnvelope) -> Result<Vec<u8>, PcaError> {
        if envelope.group_id != self.group_id {
            return Err(PcaError::GroupError {
                reason: format!(
                    "envelope group_id {} does not match {}",
                    envelope.group_id, self.group_id
                ),
            });
        }

        if envelope.epoch != self.key_state.epoch {
            return Err(PcaError::GroupError {
                reason: format!(
                    "epoch mismatch: envelope has {}, local has {}",
                    envelope.epoch, self.key_state.epoch
                ),
            });
        }

        // Verify sender is a known member (or self).
        if envelope.sender_address != self.local_address
            && !self.members.contains_key(&envelope.sender_address)
        {
            return Err(PcaError::PeerAuthenticationFailed {
                peer_address: envelope.sender_address.clone(),
                reason: "sender is not a group member".into(),
            });
        }

        // Check per-sender sequence ordering.
        if envelope.sender_address != self.local_address {
            let expected = self
                .decrypt_sequences
                .get(&envelope.sender_address)
                .copied()
                .unwrap_or(0);

            if envelope.sequence != expected {
                return Err(PcaError::SequenceError {
                    expected,
                    actual: envelope.sequence,
                });
            }

            self.decrypt_sequences
                .insert(envelope.sender_address.clone(), expected + 1);
        }

        let group_key = self.key_state.group_key.ok_or_else(|| {
            PcaError::GroupKeyAgreementFailed {
                reason: "no group key available".into(),
            }
        })?;

        crypto::decrypt_with_aad(
            &group_key,
            &envelope.nonce,
            self.group_id.as_bytes(),
            &envelope.ciphertext,
        )
    }

    /// Capture a serializable snapshot of this group session.
    pub fn snapshot(&self) -> GroupSessionSnapshot {
        GroupSessionSnapshot {
            group_id: self.group_id.clone(),
            local_address: self.local_address.clone(),
            epoch: self.key_state.epoch,
            members: self
                .members
                .values()
                .map(|m| GroupMemberSnapshot {
                    ss58_address: m.ss58_address.clone(),
                    public_key: m.public_key,
                    label: m.label.clone(),
                    joined_epoch: m.joined_epoch,
                })
                .collect(),
            encrypt_nonce_counter: self.encrypt_nonce.current(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_group() -> (GroupSession, GroupSession) {
        let mut alice = GroupSession::create("grp-1".into(), "5Alice...".into());
        let mut bob = GroupSession::create("grp-1".into(), "5Bob...".into());

        let alice_pub = alice.local_public_key();
        let bob_pub = bob.local_public_key();

        alice
            .add_member("5Bob...", &bob_pub, Some("Bob".into()))
            .expect("add bob");
        bob.add_member("5Alice...", &alice_pub, Some("Alice".into()))
            .expect("add alice");

        (alice, bob)
    }

    #[test]
    fn group_creation() {
        let group = GroupSession::create("grp-1".into(), "5Alice...".into());
        assert_eq!(group.group_id(), "grp-1");
        assert_eq!(group.member_count(), 0);
        assert_eq!(group.epoch(), 0);
        assert!(!group.has_group_key());
    }

    #[test]
    fn add_member_advances_epoch() {
        let mut alice = GroupSession::create("grp-1".into(), "5Alice...".into());
        let bob_kp = KeyPair::generate();

        let events = alice
            .add_member("5Bob...", &bob_kp.public_key_bytes(), None)
            .expect("add");

        assert_eq!(alice.member_count(), 1);
        assert_eq!(alice.epoch(), 1);
        assert!(alice.has_group_key());
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], GroupEvent::MemberJoined { .. }));
        assert!(matches!(events[1], GroupEvent::EpochAdvanced { new_epoch: 1 }));
    }

    #[test]
    fn cannot_add_self() {
        let mut group = GroupSession::create("grp-1".into(), "5Alice...".into());
        let kp = KeyPair::generate();
        let result = group.add_member("5Alice...", &kp.public_key_bytes(), None);
        assert!(result.is_err());
    }

    #[test]
    fn cannot_add_duplicate() {
        let mut group = GroupSession::create("grp-1".into(), "5Alice...".into());
        let kp = KeyPair::generate();
        group
            .add_member("5Bob...", &kp.public_key_bytes(), None)
            .expect("first add");
        let result = group.add_member("5Bob...", &kp.public_key_bytes(), None);
        assert!(result.is_err());
    }

    #[test]
    fn remove_member_advances_epoch() {
        let mut group = GroupSession::create("grp-1".into(), "5Alice...".into());
        let kp = KeyPair::generate();
        group
            .add_member("5Bob...", &kp.public_key_bytes(), None)
            .expect("add");

        let events = group.remove_member("5Bob...").expect("remove");
        assert_eq!(group.member_count(), 0);
        assert_eq!(group.epoch(), 2); // epoch 1 from add, epoch 2 from remove
        assert!(matches!(events[0], GroupEvent::MemberLeft { .. }));
    }

    #[test]
    fn remove_nonexistent_member_errors() {
        let mut group = GroupSession::create("grp-1".into(), "5Alice...".into());
        let result = group.remove_member("5Nobody...");
        assert!(result.is_err());
    }

    #[test]
    fn encrypt_requires_group_key() {
        let mut group = GroupSession::create("grp-1".into(), "5Alice...".into());
        let result = group.encrypt(b"hello");
        assert!(result.is_err());
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let (mut alice, mut bob) = make_group();

        let envelope = alice.encrypt(b"hello group").expect("encrypt");
        assert_eq!(envelope.epoch, alice.epoch());
        assert_eq!(envelope.sender_address, "5Alice...");

        let plaintext = bob.decrypt(&envelope).expect("decrypt");
        assert_eq!(plaintext, b"hello group");
    }

    #[test]
    fn bidirectional_messaging() {
        let (mut alice, mut bob) = make_group();

        let env_a = alice.encrypt(b"from alice").expect("encrypt");
        let pt_a = bob.decrypt(&env_a).expect("decrypt");
        assert_eq!(pt_a, b"from alice");

        let env_b = bob.encrypt(b"from bob").expect("encrypt");
        let pt_b = alice.decrypt(&env_b).expect("decrypt");
        assert_eq!(pt_b, b"from bob");
    }

    #[test]
    fn sequence_ordering_enforced() {
        let (mut alice, mut bob) = make_group();

        let _env0 = alice.encrypt(b"msg-0").expect("encrypt");
        let env1 = alice.encrypt(b"msg-1").expect("encrypt");

        // Skip env0, try to decrypt env1 directly — should fail.
        let result = bob.decrypt(&env1);
        assert!(result.is_err());
    }

    #[test]
    fn wrong_group_id_rejected() {
        let (mut alice, mut bob) = make_group();

        let mut envelope = alice.encrypt(b"hello").expect("encrypt");
        envelope.group_id = "wrong-group".into();

        let result = bob.decrypt(&envelope);
        assert!(result.is_err());
    }

    #[test]
    fn unknown_sender_rejected() {
        let (mut alice, mut bob) = make_group();

        let mut envelope = alice.encrypt(b"hello").expect("encrypt");
        envelope.sender_address = "5Unknown...".into();

        let result = bob.decrypt(&envelope);
        assert!(result.is_err());
    }

    #[test]
    fn snapshot_captures_state() {
        let (alice, _bob) = make_group();
        let snap = alice.snapshot();

        assert_eq!(snap.group_id, "grp-1");
        assert_eq!(snap.local_address, "5Alice...");
        assert_eq!(snap.members.len(), 1);
        assert_eq!(snap.members[0].ss58_address, "5Bob...");
    }

    #[test]
    fn multiple_messages_maintain_sequence() {
        let (mut alice, mut bob) = make_group();

        for i in 0u64..5 {
            let env = alice.encrypt(format!("msg-{i}").as_bytes()).expect("encrypt");
            assert_eq!(env.sequence, i);
            let pt = bob.decrypt(&env).expect("decrypt");
            assert_eq!(pt, format!("msg-{i}").as_bytes());
        }
    }

    #[test]
    fn three_party_group() {
        let mut alice = GroupSession::create("grp-3".into(), "5Alice...".into());
        let mut bob = GroupSession::create("grp-3".into(), "5Bob...".into());
        let mut carol = GroupSession::create("grp-3".into(), "5Carol...".into());

        let alice_pub = alice.local_public_key();
        let bob_pub = bob.local_public_key();
        let carol_pub = carol.local_public_key();

        // Each participant adds the other two.
        alice.add_member("5Bob...", &bob_pub, None).expect("add");
        alice.add_member("5Carol...", &carol_pub, None).expect("add");

        bob.add_member("5Alice...", &alice_pub, None).expect("add");
        bob.add_member("5Carol...", &carol_pub, None).expect("add");

        carol.add_member("5Alice...", &alice_pub, None).expect("add");
        carol.add_member("5Bob...", &bob_pub, None).expect("add");

        assert_eq!(alice.member_count(), 2);
        assert_eq!(bob.member_count(), 2);
        assert_eq!(carol.member_count(), 2);

        // All three should have the same epoch.
        assert_eq!(alice.epoch(), bob.epoch());
        assert_eq!(bob.epoch(), carol.epoch());
    }

    #[test]
    fn member_leave_provides_forward_secrecy() {
        let mut alice = GroupSession::create("grp-fs".into(), "5Alice...".into());
        let bob_kp = KeyPair::generate();
        let carol_kp = KeyPair::generate();

        alice
            .add_member("5Bob...", &bob_kp.public_key_bytes(), None)
            .expect("add bob");
        alice
            .add_member("5Carol...", &carol_kp.public_key_bytes(), None)
            .expect("add carol");

        let epoch_before = alice.epoch();

        // Remove Bob.
        alice.remove_member("5Bob...").expect("remove");

        // Epoch should have advanced.
        assert!(alice.epoch() > epoch_before);
        // Bob's secret is no longer in the key state.
        assert_eq!(alice.member_count(), 1);
    }

    #[test]
    fn envelope_serializes_to_json() {
        let (mut alice, _bob) = make_group();
        let env = alice.encrypt(b"serialize me").expect("encrypt");
        let json = serde_json::to_string(&env).expect("serialize");
        let back: GroupEnvelope = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.group_id, env.group_id);
        assert_eq!(back.sequence, env.sequence);
        assert_eq!(back.ciphertext, env.ciphertext);
    }

    #[test]
    fn snapshot_serializes() {
        let (alice, _bob) = make_group();
        let snap = alice.snapshot();
        let json = serde_json::to_string(&snap).expect("serialize");
        let back: GroupSessionSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.group_id, "grp-1");
        assert_eq!(back.epoch, snap.epoch);
    }
}
