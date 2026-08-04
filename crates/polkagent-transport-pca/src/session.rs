//! Encrypted session management for PCA transport.
//!
//! A [`Session`] represents an authenticated, encrypted communication channel
//! between two peers. Sessions are created via a handshake that performs
//! X25519 key agreement and supports periodic key rotation.
//!
//! # Lifecycle
//!
//! 1. **Handshake** — [`Session::initiate`] or [`Session::accept`] creates a
//!    session by exchanging ephemeral public keys and deriving a shared secret.
//! 2. **Active** — Messages are encrypted and decrypted using the session key.
//! 3. **Key rotation** — [`Session::rotate_key`] generates a new key pair and
//!    derives a fresh shared secret while preserving message ordering.
//! 4. **Expiry** — Sessions expire after a configurable timeout or when
//!    explicitly closed.

use std::time::{Duration, Instant};

use tracing::{debug, warn};
use uuid::Uuid;
use x25519_dalek::PublicKey;

use crate::crypto::{self, KeyPair, NonceCounter, SharedSecret};
use crate::error::PcaError;

/// The state of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// The handshake is in progress.
    Handshaking,
    /// The session is active and can encrypt/decrypt messages.
    Active,
    /// The session is undergoing key rotation.
    Rotating,
    /// The session has expired or been closed.
    Expired,
}

/// An encrypted communication session between two peers.
pub struct Session {
    /// Unique session identifier.
    id: String,
    /// Current session state.
    state: SessionState,
    /// The local key pair for this session.
    local_keypair: KeyPair,
    /// The remote peer's public key.
    peer_public_key: Option<PublicKey>,
    /// The derived shared secret used as the encryption key.
    shared_secret: Option<SharedSecret>,
    /// Nonce counter for outgoing messages (encrypt direction).
    encrypt_nonce: NonceCounter,
    /// Nonce counter for incoming messages (decrypt direction).
    decrypt_nonce: NonceCounter,
    /// The SS58 address of the local peer.
    local_address: String,
    /// The SS58 address of the remote peer.
    peer_address: Option<String>,
    /// When the session was created.
    created_at: Instant,
    /// When the session was last active.
    last_active: Instant,
    /// Session timeout duration.
    timeout: Duration,
    /// Number of key rotations performed.
    rotation_count: u32,
}

impl Session {
    /// Initiate a new session as the initiator side of the handshake.
    ///
    /// Returns the session and the local public key that should be sent to the
    /// peer to complete the handshake.
    pub fn initiate(local_address: String, timeout: Duration) -> (Self, [u8; 32]) {
        let keypair = KeyPair::generate();
        let public_key_bytes = keypair.public_key_bytes();
        let now = Instant::now();
        let session = Self {
            id: Uuid::now_v7().to_string(),
            state: SessionState::Handshaking,
            local_keypair: keypair,
            peer_public_key: None,
            shared_secret: None,
            encrypt_nonce: NonceCounter::new(),
            decrypt_nonce: NonceCounter::new(),
            local_address,
            peer_address: None,
            created_at: now,
            last_active: now,
            timeout,
            rotation_count: 0,
        };
        debug!(session_id = %session.id, "initiated handshake");
        (session, public_key_bytes)
    }

    /// Complete the initiator-side handshake by providing the peer's public key.
    pub fn complete_handshake(
        &mut self,
        peer_public_key_bytes: &[u8; 32],
        peer_address: String,
    ) -> Result<(), PcaError> {
        if self.state != SessionState::Handshaking {
            return Err(PcaError::KeyExchangeFailed {
                reason: format!("cannot complete handshake in state {:?}", self.state),
            });
        }

        let peer_public = PublicKey::from(*peer_public_key_bytes);
        let shared = self.local_keypair.diffie_hellman(&peer_public);

        self.peer_public_key = Some(peer_public);
        self.shared_secret = Some(shared);
        self.peer_address = Some(peer_address);
        self.state = SessionState::Active;
        self.last_active = Instant::now();

        debug!(session_id = %self.id, "handshake completed");
        Ok(())
    }

    /// Accept a session as the responder side of the handshake.
    ///
    /// Given the initiator's public key, this performs the DH exchange and
    /// returns the responder's public key for the initiator to complete.
    pub fn accept(
        local_address: String,
        peer_public_key_bytes: &[u8; 32],
        peer_address: String,
        timeout: Duration,
    ) -> Result<(Self, [u8; 32]), PcaError> {
        let keypair = KeyPair::generate();
        let local_public = keypair.public_key_bytes();

        let peer_public = PublicKey::from(*peer_public_key_bytes);
        let shared = keypair.diffie_hellman(&peer_public);

        let now = Instant::now();
        let session = Self {
            id: Uuid::now_v7().to_string(),
            state: SessionState::Active,
            local_keypair: keypair,
            peer_public_key: Some(peer_public),
            shared_secret: Some(shared),
            encrypt_nonce: NonceCounter::new(),
            decrypt_nonce: NonceCounter::new(),
            local_address,
            peer_address: Some(peer_address),
            created_at: now,
            last_active: now,
            timeout,
            rotation_count: 0,
        };

        debug!(session_id = %session.id, "accepted handshake");
        Ok((session, local_public))
    }

    /// Encrypt a plaintext message using the session's shared secret.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<EncryptedEnvelope, PcaError> {
        self.ensure_active()?;

        let key = self
            .shared_secret
            .as_ref()
            .ok_or_else(|| PcaError::KeyExchangeFailed {
                reason: "no shared secret established".into(),
            })?;

        let nonce = self
            .encrypt_nonce
            .next_nonce()
            .ok_or_else(|| PcaError::NonceExhausted {
                session_id: self.id.clone(),
            })?;

        let sequence = self.encrypt_nonce.current() - 1;
        let ciphertext = crypto::encrypt(key.as_bytes(), &nonce, plaintext)?;

        self.last_active = Instant::now();

        Ok(EncryptedEnvelope {
            session_id: self.id.clone(),
            sequence,
            nonce,
            ciphertext,
        })
    }

    /// Decrypt a ciphertext message using the session's shared secret.
    pub fn decrypt(&mut self, envelope: &EncryptedEnvelope) -> Result<Vec<u8>, PcaError> {
        self.ensure_active()?;

        let expected_seq = self.decrypt_nonce.current();
        if envelope.sequence != expected_seq {
            return Err(PcaError::SequenceError {
                expected: expected_seq,
                actual: envelope.sequence,
            });
        }

        let key = self
            .shared_secret
            .as_ref()
            .ok_or_else(|| PcaError::KeyExchangeFailed {
                reason: "no shared secret established".into(),
            })?;

        // Advance the decrypt nonce counter to stay in sync.
        let _ = self.decrypt_nonce.next_nonce();

        let plaintext = crypto::decrypt(key.as_bytes(), &envelope.nonce, &envelope.ciphertext)?;
        self.last_active = Instant::now();

        Ok(plaintext)
    }

    /// Rotate the session key by generating a new key pair.
    ///
    /// Returns the new public key that must be sent to the peer. The peer
    /// should call [`apply_rotation`] with this public key to derive the new
    /// shared secret.
    ///
    /// [`apply_rotation`]: Session::apply_rotation
    pub fn rotate_key(&mut self) -> Result<[u8; 32], PcaError> {
        self.ensure_active()?;
        self.state = SessionState::Rotating;

        let new_keypair = KeyPair::generate();
        let new_public = new_keypair.public_key_bytes();
        self.local_keypair = new_keypair;

        debug!(
            session_id = %self.id,
            rotation = self.rotation_count + 1,
            "initiating key rotation"
        );

        Ok(new_public)
    }

    /// Apply a key rotation initiated by the peer.
    ///
    /// This is called when the peer sends a new public key during key rotation.
    pub fn apply_rotation(&mut self, new_peer_public_bytes: &[u8; 32]) -> Result<(), PcaError> {
        if self.state != SessionState::Active && self.state != SessionState::Rotating {
            return Err(PcaError::KeyRotationFailed {
                reason: format!("cannot rotate in state {:?}", self.state),
            });
        }

        let new_peer_public = PublicKey::from(*new_peer_public_bytes);
        let new_shared = self.local_keypair.diffie_hellman(&new_peer_public);

        self.peer_public_key = Some(new_peer_public);
        self.shared_secret = Some(new_shared);
        self.encrypt_nonce = NonceCounter::new();
        self.decrypt_nonce = NonceCounter::new();
        self.rotation_count += 1;
        self.state = SessionState::Active;
        self.last_active = Instant::now();

        debug!(
            session_id = %self.id,
            rotation = self.rotation_count,
            "key rotation applied"
        );

        Ok(())
    }

    /// Check if the session has expired based on the configured timeout.
    pub fn is_expired(&self) -> bool {
        self.state == SessionState::Expired || self.last_active.elapsed() > self.timeout
    }

    /// Mark the session as expired.
    pub fn expire(&mut self) {
        if self.state != SessionState::Expired {
            warn!(session_id = %self.id, "session expired");
            self.state = SessionState::Expired;
        }
    }

    /// Return the session identifier.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Return the current session state.
    pub fn state(&self) -> SessionState {
        self.state
    }

    /// Return the local SS58 address.
    pub fn local_address(&self) -> &str {
        &self.local_address
    }

    /// Return the peer SS58 address, if established.
    pub fn peer_address(&self) -> Option<&str> {
        self.peer_address.as_deref()
    }

    /// Return the number of key rotations performed.
    pub fn rotation_count(&self) -> u32 {
        self.rotation_count
    }

    /// Return how long the session has been alive.
    pub fn age(&self) -> Duration {
        self.created_at.elapsed()
    }

    /// Return how long since the session was last active.
    pub fn idle_duration(&self) -> Duration {
        self.last_active.elapsed()
    }

    /// Verify that the session is in an active state.
    fn ensure_active(&self) -> Result<(), PcaError> {
        match self.state {
            SessionState::Active => {
                if self.is_expired() {
                    Err(PcaError::SessionExpired {
                        session_id: self.id.clone(),
                    })
                } else {
                    Ok(())
                }
            }
            SessionState::Expired => Err(PcaError::SessionExpired {
                session_id: self.id.clone(),
            }),
            other => Err(PcaError::KeyExchangeFailed {
                reason: format!("session is in state {other:?}, not Active"),
            }),
        }
    }
}

/// An encrypted message envelope carrying ciphertext along with the
/// metadata needed to decrypt it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EncryptedEnvelope {
    /// Session identifier.
    pub session_id: String,
    /// Monotonic sequence number within the session.
    pub sequence: u64,
    /// The nonce used for this encryption.
    pub nonce: [u8; 12],
    /// The encrypted ciphertext (includes auth tag).
    pub ciphertext: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_timeout() -> Duration {
        Duration::from_secs(300)
    }

    #[test]
    fn initiate_creates_handshaking_session() {
        let (session, pub_key) = Session::initiate("5Alice...".into(), test_timeout());
        assert_eq!(session.state(), SessionState::Handshaking);
        assert!(!pub_key.iter().all(|&b| b == 0));
    }

    #[test]
    fn full_handshake_lifecycle() {
        // Alice initiates
        let (mut alice, alice_pub) = Session::initiate("5Alice...".into(), test_timeout());

        // Bob accepts
        let (mut bob, bob_pub) = Session::accept(
            "5Bob...".into(),
            &alice_pub,
            "5Alice...".into(),
            test_timeout(),
        )
        .expect("bob accept");

        // Alice completes
        alice
            .complete_handshake(&bob_pub, "5Bob...".into())
            .expect("alice complete");

        assert_eq!(alice.state(), SessionState::Active);
        assert_eq!(bob.state(), SessionState::Active);

        // Encrypt from Alice to Bob
        let plaintext = b"Hello Bob from Alice!";
        let envelope = alice.encrypt(plaintext).expect("encrypt");
        let decrypted = bob.decrypt(&envelope).expect("decrypt");
        assert_eq!(decrypted, plaintext);

        // Encrypt from Bob to Alice
        let reply = b"Hello Alice from Bob!";
        let envelope2 = bob.encrypt(reply).expect("encrypt");
        let decrypted2 = alice.decrypt(&envelope2).expect("decrypt");
        assert_eq!(decrypted2, reply);
    }

    #[test]
    fn encrypt_fails_in_handshaking_state() {
        let (mut session, _) = Session::initiate("5Alice...".into(), test_timeout());
        let result = session.encrypt(b"should fail");
        assert!(result.is_err());
    }

    #[test]
    fn decrypt_fails_with_out_of_order_sequence() {
        let (mut alice, alice_pub) = Session::initiate("5Alice...".into(), test_timeout());
        let (mut bob, bob_pub) = Session::accept(
            "5Bob...".into(),
            &alice_pub,
            "5Alice...".into(),
            test_timeout(),
        )
        .expect("accept");
        alice
            .complete_handshake(&bob_pub, "5Bob...".into())
            .expect("complete");

        // Encrypt two messages
        let _env0 = alice.encrypt(b"first").expect("encrypt");
        let env1 = alice.encrypt(b"second").expect("encrypt");

        // Try to decrypt the second message first (skipping sequence 0).
        let result = bob.decrypt(&env1);
        assert!(result.is_err());
        match result.unwrap_err() {
            PcaError::SequenceError { expected, actual } => {
                assert_eq!(expected, 0);
                assert_eq!(actual, 1);
            }
            other => panic!("expected SequenceError, got {other:?}"),
        }
    }

    #[test]
    fn key_rotation_works() {
        let (mut alice, alice_pub) = Session::initiate("5Alice...".into(), test_timeout());
        let (mut bob, bob_pub) = Session::accept(
            "5Bob...".into(),
            &alice_pub,
            "5Alice...".into(),
            test_timeout(),
        )
        .expect("accept");
        alice
            .complete_handshake(&bob_pub, "5Bob...".into())
            .expect("complete");

        // Send a message before rotation
        let env_pre = alice.encrypt(b"before rotation").expect("encrypt");
        let pt_pre = bob.decrypt(&env_pre).expect("decrypt");
        assert_eq!(pt_pre, b"before rotation");

        // Alice initiates key rotation
        let new_alice_pub = alice.rotate_key().expect("rotate");
        assert_eq!(alice.state(), SessionState::Rotating);

        // Bob applies the rotation
        bob.apply_rotation(&new_alice_pub).expect("apply rotation");

        // Alice also needs to apply Bob's perspective. In a full protocol
        // this would be bidirectional. For our test, Alice applies her own
        // rotation to complete it.
        let peer_pub_bytes = bob.local_keypair.public_key_bytes();
        alice
            .apply_rotation(&peer_pub_bytes)
            .expect("apply rotation");

        assert_eq!(alice.state(), SessionState::Active);
        assert_eq!(bob.state(), SessionState::Active);
        assert_eq!(alice.rotation_count(), 1);
        assert_eq!(bob.rotation_count(), 1);

        // Send a message after rotation
        let env_post = alice.encrypt(b"after rotation").expect("encrypt");
        let pt_post = bob.decrypt(&env_post).expect("decrypt");
        assert_eq!(pt_post, b"after rotation");
    }

    #[test]
    fn expired_session_rejects_operations() {
        let (mut alice, alice_pub) = Session::initiate("5Alice...".into(), test_timeout());
        let (_, bob_pub) = Session::accept(
            "5Bob...".into(),
            &alice_pub,
            "5Alice...".into(),
            test_timeout(),
        )
        .expect("accept");
        alice
            .complete_handshake(&bob_pub, "5Bob...".into())
            .expect("complete");

        alice.expire();
        assert_eq!(alice.state(), SessionState::Expired);

        let result = alice.encrypt(b"should fail");
        assert!(result.is_err());
    }

    #[test]
    fn session_reports_peer_info() {
        let (mut alice, alice_pub) = Session::initiate("5Alice...".into(), test_timeout());
        let (_, bob_pub) = Session::accept(
            "5Bob...".into(),
            &alice_pub,
            "5Alice...".into(),
            test_timeout(),
        )
        .expect("accept");
        alice
            .complete_handshake(&bob_pub, "5Bob...".into())
            .expect("complete");

        assert_eq!(alice.local_address(), "5Alice...");
        assert_eq!(alice.peer_address(), Some("5Bob..."));
    }

    #[test]
    fn session_id_is_nonempty() {
        let (session, _) = Session::initiate("5Alice...".into(), test_timeout());
        assert!(!session.id().is_empty());
    }

    #[test]
    fn multiple_messages_maintain_sequence() {
        let (mut alice, alice_pub) = Session::initiate("5Alice...".into(), test_timeout());
        let (mut bob, bob_pub) = Session::accept(
            "5Bob...".into(),
            &alice_pub,
            "5Alice...".into(),
            test_timeout(),
        )
        .expect("accept");
        alice
            .complete_handshake(&bob_pub, "5Bob...".into())
            .expect("complete");

        for i in 0..10u32 {
            let msg = format!("message-{i}");
            let envelope = alice.encrypt(msg.as_bytes()).expect("encrypt");
            assert_eq!(envelope.sequence, u64::from(i));
            let decrypted = bob.decrypt(&envelope).expect("decrypt");
            assert_eq!(String::from_utf8(decrypted).expect("utf8"), msg);
        }
    }

    #[test]
    fn rotation_resets_nonce_counters() {
        let (mut alice, alice_pub) = Session::initiate("5Alice...".into(), test_timeout());
        let (mut bob, bob_pub) = Session::accept(
            "5Bob...".into(),
            &alice_pub,
            "5Alice...".into(),
            test_timeout(),
        )
        .expect("accept");
        alice
            .complete_handshake(&bob_pub, "5Bob...".into())
            .expect("complete");

        // Send some messages to advance nonce counters.
        for _ in 0..5 {
            let env = alice.encrypt(b"data").expect("encrypt");
            bob.decrypt(&env).expect("decrypt");
        }

        // Rotate keys.
        let new_alice_pub = alice.rotate_key().expect("rotate");
        bob.apply_rotation(&new_alice_pub).expect("apply");
        let peer_pub = bob.local_keypair.public_key_bytes();
        alice.apply_rotation(&peer_pub).expect("apply");

        // After rotation, the first envelope should have sequence 0.
        let env = alice.encrypt(b"post-rotation").expect("encrypt");
        assert_eq!(env.sequence, 0);
        let pt = bob.decrypt(&env).expect("decrypt");
        assert_eq!(pt, b"post-rotation");
    }

    #[test]
    fn cannot_complete_handshake_twice() {
        let (mut alice, alice_pub) = Session::initiate("5Alice...".into(), test_timeout());
        let (_, bob_pub) = Session::accept(
            "5Bob...".into(),
            &alice_pub,
            "5Alice...".into(),
            test_timeout(),
        )
        .expect("accept");

        alice
            .complete_handshake(&bob_pub, "5Bob...".into())
            .expect("complete");

        // Second complete should fail because state is Active, not Handshaking.
        let result = alice.complete_handshake(&bob_pub, "5Bob...".into());
        assert!(result.is_err());
    }

    #[test]
    fn envelope_serializes_to_json() {
        let (mut alice, alice_pub) = Session::initiate("5Alice...".into(), test_timeout());
        let (_, bob_pub) = Session::accept(
            "5Bob...".into(),
            &alice_pub,
            "5Alice...".into(),
            test_timeout(),
        )
        .expect("accept");
        alice
            .complete_handshake(&bob_pub, "5Bob...".into())
            .expect("complete");

        let envelope = alice.encrypt(b"test message").expect("encrypt");
        let json = serde_json::to_string(&envelope).expect("serialize");
        let deserialized: EncryptedEnvelope = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.session_id, envelope.session_id);
        assert_eq!(deserialized.sequence, envelope.sequence);
        assert_eq!(deserialized.ciphertext, envelope.ciphertext);
    }
}
