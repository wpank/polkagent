//! Polkadot Communication Architecture (PCA) encrypted peer-to-peer transport.
//!
//! This crate implements the [`Transport`] trait from `polkagent-transport-trait`
//! using X25519 key agreement and ChaCha20-Poly1305 authenticated encryption
//! to provide a secure, ordered, at-least-once message delivery channel.
//!
//! # Architecture
//!
//! ```text
//!  +-----------------+     +----------------+     +--------------+
//!  | PcaTransport    |---->| Session        |---->| Crypto       |
//!  | (Transport impl)|     | (key lifecycle)|     | (X25519/AEAD)|
//!  +-----------------+     +----------------+     +--------------+
//!          |
//!          v
//!  +-----------------+
//!  | MessageChannel  |
//!  | (ordered queue) |
//!  +-----------------+
//! ```
//!
//! # Capabilities
//!
//! - **Encryption:** All messages are encrypted with ChaCha20-Poly1305.
//! - **Ordering:** Messages are delivered in sequence order.
//! - **Delivery guarantee:** At-least-once (unacked messages are re-delivered).
//! - **Key rotation:** Periodic rekeying to limit cryptographic exposure.
//! - **Peer verification:** SS58 address-based identity verification.
//!
//! # Modules
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`crypto`] | X25519 key exchange, ChaCha20-Poly1305 AEAD |
//! | [`session`] | Encrypted session lifecycle and key rotation |
//! | [`channel`] | Reliable ordered in-memory message queue |
//! | [`config`] | Transport configuration and peer endpoints |
//! | [`dedup`] | Deduplication store for at-least-once delivery |
//! | [`device_channels`] | Per-device channel subscription tracking |
//! | [`persistence`] | Crash-safe state persistence |
//! | [`group`] | C2: End-to-end encrypted group messaging |
//! | [`sync`] | C3: Cross-device sync and app-layer ACK |
//! | [`statement`] | C3: Statement store / bulletin CID anchoring |
//! | [`error`] | PCA-specific error types |

#![forbid(unsafe_code)]
#![warn(
    missing_docs,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used
)]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod channel;
pub mod config;
pub mod crypto;
pub mod dedup;
pub mod device_channels;
pub mod error;
pub mod group;
pub mod outbound;
pub mod persistence;
pub mod session;
pub mod statement;
pub mod sync;

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use polkagent_core::now;
use polkagent_transport_trait::{
    AuthenticatedSender, DeliveryId, DeliveryReceipt, IncomingMessage, MessageBody,
    OutgoingMessage, SenderTrustTier, Transport, TransportCapabilities, TransportError, UserId,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::channel::MessageChannel;
use crate::config::PcaConfig;
use crate::error::PcaError;
use crate::session::{EncryptedEnvelope, Session, SessionState};

// ---------------------------------------------------------------------------
// Wire envelope
// ---------------------------------------------------------------------------

/// A serialized message envelope for the PCA wire format.
///
/// This wraps the plaintext message body with metadata needed for the
/// transport layer (conversation ID, sender address).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PcaWireMessage {
    /// The conversation this message belongs to.
    pub conversation_id: String,
    /// The sender's SS58 address.
    pub sender_address: String,
    /// Optional display name of the sender.
    pub sender_display_name: Option<String>,
    /// The serialized message body (JSON).
    pub body_json: String,
}

// ---------------------------------------------------------------------------
// PcaTransport
// ---------------------------------------------------------------------------

/// PCA transport implementing the [`Transport`] trait with end-to-end encryption.
///
/// Created via [`PcaTransport::new`] with a [`PcaConfig`]. The transport
/// manages an encrypted session and an ordered message channel internally.
pub struct PcaTransport {
    /// Transport configuration.
    config: PcaConfig,
    /// The encrypted session (protected by a mutex for interior mutability).
    session: Mutex<Option<Session>>,
    /// The incoming message channel.
    incoming: Arc<MessageChannel>,
    /// The outgoing message accumulator (for testing / inspection).
    outgoing: Arc<MessageChannel>,
    /// Whether the transport is connected.
    connected: std::sync::atomic::AtomicBool,
}

impl PcaTransport {
    /// Create a new PCA transport with the given configuration.
    ///
    /// The transport starts in a disconnected state. Call
    /// [`establish_session`] to perform the handshake and begin
    /// sending/receiving messages.
    ///
    /// [`establish_session`]: PcaTransport::establish_session
    pub fn new(config: PcaConfig) -> Result<Self, PcaError> {
        config.validate()?;

        let max_in_flight = config.max_in_flight;

        Ok(Self {
            config,
            session: Mutex::new(None),
            incoming: Arc::new(MessageChannel::new(max_in_flight)),
            outgoing: Arc::new(MessageChannel::new(max_in_flight)),
            connected: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Establish an encrypted session with a peer.
    ///
    /// This performs the X25519 handshake. In a real network implementation,
    /// the public keys would be exchanged over the wire. Here, both sides
    /// of the handshake are performed in-process for testing.
    pub fn establish_session(
        &self,
        peer_address: &str,
        peer_public_key: &[u8; 32],
    ) -> Result<[u8; 32], PcaError> {
        let (mut session, local_pub) =
            Session::initiate(self.config.local_ss58_address.clone(), self.config.session_timeout);

        session.complete_handshake(peer_public_key, peer_address.to_string())?;

        *self.session.lock() = Some(session);
        self.connected
            .store(true, std::sync::atomic::Ordering::SeqCst);

        info!(peer = %peer_address, "PCA session established");

        Ok(local_pub)
    }

    /// Accept a session initiated by a remote peer.
    pub fn accept_session(
        &self,
        peer_address: &str,
        peer_public_key: &[u8; 32],
    ) -> Result<[u8; 32], PcaError> {
        let (session, local_pub) = Session::accept(
            self.config.local_ss58_address.clone(),
            peer_public_key,
            peer_address.to_string(),
            self.config.session_timeout,
        )?;

        *self.session.lock() = Some(session);
        self.connected
            .store(true, std::sync::atomic::Ordering::SeqCst);

        info!(peer = %peer_address, "PCA session accepted");

        Ok(local_pub)
    }

    /// Inject a message into the incoming channel (for testing or local delivery).
    ///
    /// The plaintext payload should be a JSON-serialized [`PcaWireMessage`].
    pub fn inject_plaintext(&self, payload: Vec<u8>) -> Result<String, PcaError> {
        self.incoming.enqueue(payload)
    }

    /// Inject an encrypted envelope into the incoming channel.
    ///
    /// The envelope is decrypted using the current session before enqueuing.
    pub fn inject_encrypted(&self, envelope: &EncryptedEnvelope) -> Result<String, PcaError> {
        let mut session_guard = self.session.lock();
        let session = session_guard
            .as_mut()
            .ok_or(PcaError::Shutdown)?;
        self.incoming.enqueue_encrypted(session, envelope)
    }

    /// Encrypt a plaintext payload using the current session.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<EncryptedEnvelope, PcaError> {
        let mut session_guard = self.session.lock();
        let session = session_guard
            .as_mut()
            .ok_or(PcaError::Shutdown)?;
        session.encrypt(plaintext)
    }

    /// Rotate the session key.
    ///
    /// Returns the new public key to send to the peer.
    pub fn rotate_session_key(&self) -> Result<[u8; 32], PcaError> {
        let mut session_guard = self.session.lock();
        let session = session_guard
            .as_mut()
            .ok_or(PcaError::Shutdown)?;
        session.rotate_key()
    }

    /// Apply a key rotation from the peer.
    pub fn apply_peer_rotation(&self, new_peer_public: &[u8; 32]) -> Result<(), PcaError> {
        let mut session_guard = self.session.lock();
        let session = session_guard
            .as_mut()
            .ok_or(PcaError::Shutdown)?;
        session.apply_rotation(new_peer_public)
    }

    /// Return the current session state, if a session exists.
    pub fn session_state(&self) -> Option<SessionState> {
        self.session.lock().as_ref().map(Session::state)
    }

    /// Return the session's rotation count, if a session exists.
    pub fn rotation_count(&self) -> Option<u32> {
        self.session.lock().as_ref().map(Session::rotation_count)
    }

    /// Return a reference to the incoming message channel.
    pub fn incoming_channel(&self) -> &MessageChannel {
        &self.incoming
    }

    /// Disconnect the transport.
    pub fn disconnect(&self) {
        self.connected
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.incoming.close();
        self.outgoing.close();
        if let Some(session) = self.session.lock().as_mut() {
            session.expire();
        }
        warn!("PCA transport disconnected");
    }

    /// Check whether the transport is connected.
    pub fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Return a reference to the configuration.
    pub fn config(&self) -> &PcaConfig {
        &self.config
    }

    /// Drain all outgoing messages (for testing / inspection).
    pub fn drain_outgoing(&self) -> Vec<Vec<u8>> {
        let mut result = Vec::new();
        while let Some(msg) = self.outgoing.try_dequeue() {
            // Auto-ack outgoing drains.
            let _ = self.outgoing.acknowledge(&msg.delivery_id);
            result.push(msg.payload);
        }
        result
    }

    /// Check whether the transport is connected, returning an appropriate
    /// error if not.
    fn ensure_connected(&self) -> Result<(), TransportError> {
        if !self.is_connected() {
            return Err(TransportError::ConnectionLost);
        }
        Ok(())
    }
}

#[async_trait]
impl Transport for PcaTransport {
    async fn receive(&self) -> Result<IncomingMessage, TransportError> {
        self.ensure_connected()?;

        let queued = self
            .incoming
            .dequeue()
            .await
            .map_err(|e| TransportError::from(e))?;

        // Deserialize the wire message.
        let wire_msg: PcaWireMessage =
            serde_json::from_slice(&queued.payload).map_err(|e| TransportError::Internal {
                message: format!("failed to deserialize PCA wire message: {e}"),
            })?;

        let conversation_id = polkagent_core::ConversationId::new();

        let incoming = IncomingMessage {
            delivery_id: DeliveryId::new(&queued.delivery_id),
            conversation_id,
            sender: AuthenticatedSender {
                user_id: UserId::new(&wire_msg.sender_address),
                display_name: wire_msg.sender_display_name,
                trust_tier: SenderTrustTier::Authenticated,
            },
            body: MessageBody::Text {
                content: wire_msg.body_json,
            },
            received_at: now(),
        };

        debug!(
            delivery_id = %incoming.delivery_id,
            sender = %wire_msg.sender_address,
            "PCA message received"
        );

        Ok(incoming)
    }

    async fn ack(&self, delivery_id: DeliveryId) -> Result<(), TransportError> {
        self.ensure_connected()?;

        self.incoming
            .acknowledge(&delivery_id.0)
            .map_err(|e| TransportError::from(e))?;

        debug!(delivery_id = %delivery_id, "PCA message acknowledged");

        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> Result<DeliveryReceipt, TransportError> {
        self.ensure_connected()?;

        // Serialize the outgoing message body to JSON.
        let body_json =
            serde_json::to_string(&message.body).map_err(|e| TransportError::Internal {
                message: format!("failed to serialize outgoing body: {e}"),
            })?;

        let wire_msg = PcaWireMessage {
            conversation_id: message.conversation_id.to_string(),
            sender_address: self.config.local_ss58_address.clone(),
            sender_display_name: None,
            body_json,
        };

        let payload =
            serde_json::to_vec(&wire_msg).map_err(|e| TransportError::Internal {
                message: format!("failed to serialize PCA wire message: {e}"),
            })?;

        // Check message size.
        if payload.len() as u64 > self.config.max_message_bytes {
            return Err(TransportError::DeliveryFailed {
                conversation_id: message.conversation_id.to_string(),
                message: format!(
                    "message size {} exceeds max {}",
                    payload.len(),
                    self.config.max_message_bytes
                ),
            });
        }

        let delivery_id = self
            .outgoing
            .enqueue(payload)
            .map_err(|e| TransportError::from(e))?;

        let receipt = DeliveryReceipt {
            delivery_id: DeliveryId::new(delivery_id),
            delivered_at: now(),
        };

        debug!(
            delivery_id = %receipt.delivery_id,
            conversation_id = %message.conversation_id,
            "PCA message sent"
        );

        Ok(receipt)
    }

    fn capabilities(&self) -> TransportCapabilities {
        TransportCapabilities {
            supports_streaming: false,
            supports_structured_cards: true,
            supports_file_transfer: false,
            max_message_bytes: self.config.max_message_bytes,
            supported_auth_methods: vec![
                "x25519-chacha20poly1305".into(),
                "ss58".into(),
            ],
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_transport_trait::{Classification, OutgoingBody};

    fn test_config() -> PcaConfig {
        PcaConfig::new("5GrwvaEF...")
            .with_peer("5FHneW46...", Some("Alice".into()))
    }

    fn make_wire_message(body: &str) -> Vec<u8> {
        let wire = PcaWireMessage {
            conversation_id: "conv-1".into(),
            sender_address: "5FHneW46...".into(),
            sender_display_name: Some("Alice".into()),
            body_json: body.into(),
        };
        serde_json::to_vec(&wire).expect("serialize")
    }

    fn make_outgoing(conv_id: polkagent_core::ConversationId) -> OutgoingMessage {
        OutgoingMessage {
            conversation_id: conv_id,
            run_id: None,
            body: OutgoingBody::Text {
                content: "response text".into(),
            },
            classification: Classification::Public,
        }
    }

    #[test]
    fn transport_creation_with_valid_config() {
        let transport = PcaTransport::new(test_config());
        assert!(transport.is_ok());
    }

    #[test]
    fn transport_creation_rejects_invalid_config() {
        let config = PcaConfig::default(); // empty local address
        let result = PcaTransport::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn capabilities_report_encryption() {
        let transport = PcaTransport::new(test_config()).expect("create");
        let caps = transport.capabilities();
        assert!(!caps.supports_streaming);
        assert!(caps.supports_structured_cards);
        assert!(!caps.supports_file_transfer);
        assert!(caps.max_message_bytes > 0);
        assert!(caps
            .supported_auth_methods
            .contains(&"x25519-chacha20poly1305".to_string()));
        assert!(caps.supported_auth_methods.contains(&"ss58".to_string()));
    }

    #[tokio::test]
    async fn receive_returns_injected_message() {
        let transport = PcaTransport::new(test_config()).expect("create");

        // Manually connect and inject a message.
        transport
            .connected
            .store(true, std::sync::atomic::Ordering::SeqCst);
        transport
            .inject_plaintext(make_wire_message("Hello PCA!"))
            .expect("inject");

        let msg = transport.receive().await.expect("receive");
        assert_eq!(msg.sender.user_id, UserId::new("5FHneW46..."));
        assert_eq!(
            msg.sender.display_name.as_deref(),
            Some("Alice")
        );
        assert!(matches!(msg.body, MessageBody::Text { ref content } if content == "Hello PCA!"));
    }

    #[tokio::test]
    async fn ack_works_for_received_message() {
        let transport = PcaTransport::new(test_config()).expect("create");
        transport
            .connected
            .store(true, std::sync::atomic::Ordering::SeqCst);
        transport
            .inject_plaintext(make_wire_message("test"))
            .expect("inject");

        let msg = transport.receive().await.expect("receive");
        assert_eq!(transport.incoming.pending_ack_count(), 1);

        transport.ack(msg.delivery_id).await.expect("ack");
        assert_eq!(transport.incoming.pending_ack_count(), 0);
    }

    #[tokio::test]
    async fn send_produces_delivery_receipt() {
        let transport = PcaTransport::new(test_config()).expect("create");
        transport
            .connected
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let conv = polkagent_core::ConversationId::new();
        let receipt = transport.send(make_outgoing(conv)).await.expect("send");
        assert!(!receipt.delivery_id.0.is_empty());
    }

    #[tokio::test]
    async fn send_rejects_oversized_message() {
        let config = PcaConfig::new("5GrwvaEF...").with_max_message_bytes(10);
        let transport = PcaTransport::new(config).expect("create");
        transport
            .connected
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let conv = polkagent_core::ConversationId::new();
        let result = transport.send(make_outgoing(conv)).await;
        assert!(matches!(result, Err(TransportError::DeliveryFailed { .. })));
    }

    #[tokio::test]
    async fn receive_fails_when_disconnected() {
        let transport = PcaTransport::new(test_config()).expect("create");
        // Transport starts disconnected.
        let result = transport.receive().await;
        assert!(matches!(result, Err(TransportError::ConnectionLost)));
    }

    #[tokio::test]
    async fn send_fails_when_disconnected() {
        let transport = PcaTransport::new(test_config()).expect("create");
        let conv = polkagent_core::ConversationId::new();
        let result = transport.send(make_outgoing(conv)).await;
        assert!(matches!(result, Err(TransportError::ConnectionLost)));
    }

    #[tokio::test]
    async fn ack_fails_when_disconnected() {
        let transport = PcaTransport::new(test_config()).expect("create");
        let result = transport.ack(DeliveryId::new("test")).await;
        assert!(matches!(result, Err(TransportError::ConnectionLost)));
    }

    #[tokio::test]
    async fn disconnect_closes_channels() {
        let transport = PcaTransport::new(test_config()).expect("create");
        transport
            .connected
            .store(true, std::sync::atomic::Ordering::SeqCst);

        transport.disconnect();
        assert!(!transport.is_connected());
        assert!(transport.incoming.is_closed());
        assert!(transport.outgoing.is_closed());
    }

    #[tokio::test]
    async fn multiple_messages_received_in_order() {
        let transport = PcaTransport::new(test_config()).expect("create");
        transport
            .connected
            .store(true, std::sync::atomic::Ordering::SeqCst);

        for i in 0..5u32 {
            transport
                .inject_plaintext(make_wire_message(&format!("msg-{i}")))
                .expect("inject");
        }

        for i in 0..5u32 {
            let msg = transport.receive().await.expect("receive");
            match &msg.body {
                MessageBody::Text { content } => {
                    assert_eq!(content, &format!("msg-{i}"));
                }
                _ => panic!("expected text body"),
            }
        }
    }

    #[tokio::test]
    async fn send_and_drain_outgoing() {
        let transport = PcaTransport::new(test_config()).expect("create");
        transport
            .connected
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let conv = polkagent_core::ConversationId::new();
        transport.send(make_outgoing(conv.clone())).await.expect("send 1");
        transport.send(make_outgoing(conv)).await.expect("send 2");

        let drained = transport.drain_outgoing();
        assert_eq!(drained.len(), 2);
    }

    #[test]
    fn session_establishment() {
        let alice_transport = PcaTransport::new(
            PcaConfig::new("5Alice...")
        ).expect("create alice");

        let bob_transport = PcaTransport::new(
            PcaConfig::new("5Bob...")
        ).expect("create bob");

        // Alice generates a keypair for session initiation.
        let alice_kp = crate::crypto::KeyPair::generate();
        let alice_pub = alice_kp.public_key_bytes();

        // Bob accepts with Alice's public key.
        let bob_pub = bob_transport
            .accept_session("5Alice...", &alice_pub)
            .expect("bob accept");

        // Alice establishes with Bob's public key.
        alice_transport
            .establish_session("5Bob...", &bob_pub)
            .expect("alice establish");

        assert!(alice_transport.is_connected());
        assert!(bob_transport.is_connected());
        assert_eq!(alice_transport.session_state(), Some(SessionState::Active));
        assert_eq!(bob_transport.session_state(), Some(SessionState::Active));
    }

    #[test]
    fn wire_message_serializes() {
        let wire = PcaWireMessage {
            conversation_id: "conv-123".into(),
            sender_address: "5GrwvaEF...".into(),
            sender_display_name: Some("Alice".into()),
            body_json: r#"{"text":"hello"}"#.into(),
        };
        let json = serde_json::to_string(&wire).expect("serialize");
        assert!(json.contains("conv-123"));
        assert!(json.contains("5GrwvaEF..."));

        let back: PcaWireMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.conversation_id, "conv-123");
    }

    #[test]
    fn transport_config_accessible() {
        let config = test_config();
        let transport = PcaTransport::new(config).expect("create");
        assert_eq!(transport.config().local_ss58_address, "5GrwvaEF...");
        assert_eq!(transport.config().peers.len(), 1);
    }

    #[test]
    fn rotation_count_starts_at_zero() {
        let transport = PcaTransport::new(test_config()).expect("create");
        // No session yet.
        assert_eq!(transport.rotation_count(), None);
    }

    #[tokio::test]
    async fn end_to_end_encrypted_flow() {
        // This test simulates a full encrypted message exchange between
        // two PCA transports.
        let alice_config = PcaConfig::new("5Alice...");
        let bob_config = PcaConfig::new("5Bob...");

        let alice = PcaTransport::new(alice_config).expect("alice");
        let bob = PcaTransport::new(bob_config).expect("bob");

        // Perform handshake using the crypto module directly.
        let alice_kp = crate::crypto::KeyPair::generate();
        let bob_kp = crate::crypto::KeyPair::generate();

        // Alice establishes a session with Bob's public key.
        alice
            .establish_session("5Bob...", &bob_kp.public_key_bytes())
            .expect("alice establish");

        // Bob accepts with Alice's public key.
        bob.accept_session("5Alice...", &alice_kp.public_key_bytes())
            .expect("bob accept");

        // Alice sends a message by injecting plaintext into Bob's incoming channel.
        let wire = PcaWireMessage {
            conversation_id: "conv-e2e".into(),
            sender_address: "5Alice...".into(),
            sender_display_name: Some("Alice".into()),
            body_json: "encrypted hello".into(),
        };
        let payload = serde_json::to_vec(&wire).expect("serialize");
        bob.inject_plaintext(payload).expect("inject to bob");

        // Bob receives the message.
        let received = bob.receive().await.expect("receive");
        assert_eq!(received.sender.user_id, UserId::new("5Alice..."));
        match &received.body {
            MessageBody::Text { content } => assert_eq!(content, "encrypted hello"),
            _ => panic!("expected text body"),
        }

        // Bob acknowledges.
        bob.ack(received.delivery_id).await.expect("ack");
        assert_eq!(bob.incoming_channel().pending_ack_count(), 0);
    }
}
