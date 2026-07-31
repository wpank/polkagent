//! Transport port trait for the Polkagent platform.
//!
//! This crate defines the [`Transport`] trait — the narrow adapter boundary
//! between the Polkagent kernel and a concrete message channel (Polkadot
//! encrypted chat, WebSocket, HTTP API, CLI stdin/stdout, webhook ingress).
//!
//! # Contract
//!
//! - [`Transport::receive`] blocks until a message arrives or the transport
//!   is closed.
//! - [`Transport::ack`] must be called **exactly once** per [`IncomingMessage`]
//!   after the message is durably admitted to the system. Failure to call
//!   `ack` causes the transport to re-deliver the message.
//! - [`Transport::send`] does not guarantee end-to-end delivery; delivery
//!   tracking is a separate concern.
//! - Implementations must not add business logic. All admission validation,
//!   policy evaluation, and content inspection happen in the application layer.
//! - Implementations must be `Send + Sync + 'static`.

#[cfg(feature = "test-contracts")]
pub mod contracts;

#[cfg(feature = "test-contracts")]
pub mod conformance;

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use polkagent_core::{ConversationId, RunId, Timestamp};

// ---------------------------------------------------------------------------
// Incoming message types
// ---------------------------------------------------------------------------

/// A stable delivery identifier used to acknowledge receipt of an
/// [`IncomingMessage`].
///
/// The transport assigns this identifier. It must be unique within the
/// transport's delivery sequence so that at-least-once re-delivery can be
/// detected and de-duplicated by the application layer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeliveryId(pub String);

impl DeliveryId {
    /// Construct a `DeliveryId` from a string.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for DeliveryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An identifier for a user or service account that sent an incoming message.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserId(pub String);

impl UserId {
    /// Construct a `UserId` from a string.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The trust tier of an incoming message sender.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SenderTrustTier {
    /// A verified, authenticated user session.
    Authenticated,
    /// An external webhook or callback source (lower trust).
    External,
}

/// An authenticated sender attached to each incoming message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthenticatedSender {
    /// Stable user identifier assigned by the transport's authentication layer.
    pub user_id: UserId,
    /// Display name for the sender, if the transport provides one.
    pub display_name: Option<String>,
    /// Trust tier of the channel this message arrived on.
    pub trust_tier: SenderTrustTier,
}

/// The typed body of an incoming message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MessageBody {
    /// A plain text message.
    Text { content: String },
    /// A file attachment.
    File {
        /// Original filename.
        name: String,
        /// MIME type.
        mime_type: String,
        /// Raw file bytes.
        body: Vec<u8>,
    },
    /// A structured command (e.g. slash command, API payload).
    StructuredCommand {
        /// The command type discriminant.
        command_type: String,
        /// JSON-serialized command payload.
        payload_json: String,
    },
}

/// A message received from an external actor through a transport channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessage {
    /// Stable delivery identifier used for acknowledgement.
    pub delivery_id: DeliveryId,
    /// The conversation this message belongs to.
    pub conversation_id: ConversationId,
    /// The authenticated sender.
    pub sender: AuthenticatedSender,
    /// The message content.
    pub body: MessageBody,
    /// Transport-assigned receive timestamp (before any application processing).
    pub received_at: Timestamp,
}

// ---------------------------------------------------------------------------
// Outgoing message types
// ---------------------------------------------------------------------------

/// Data classification for outgoing message content.
///
/// The transport enforces that messages sent on a public-facing channel
/// do not contain data classified above [`Classification::Internal`].
/// Higher classification levels require out-of-band delivery paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    /// May appear in public projections and log exports.
    Public,
    /// Visible within the organization; excluded from public-facing channels.
    Internal,
    /// Visible to authorized users; excluded from public and internal channels.
    Private,
    /// Requires additional access controls.
    Sensitive,
}

/// The typed body of an outgoing message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutgoingBody {
    /// A plain text response.
    Text { content: String },
    /// A structured rich card (e.g. an approval prompt, a chain action summary).
    StructuredCard {
        /// Card type discriminant.
        card_type: String,
        /// JSON-serialized card payload.
        payload_json: String,
    },
    /// An approval request sent to a human reviewer.
    ApprovalRequest {
        /// JSON-serialized approval payload.
        payload_json: String,
    },
}

/// A message to deliver to an external recipient through a transport channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutgoingMessage {
    /// The conversation this message belongs to.
    pub conversation_id: ConversationId,
    /// The run that produced this message, if applicable.
    pub run_id: Option<RunId>,
    /// The message content.
    pub body: OutgoingBody,
    /// Content classification. The transport may reject delivery on channels
    /// that cannot enforce the required classification boundary.
    pub classification: Classification,
}

/// A delivery receipt returned by [`Transport::send`].
#[derive(Debug, Clone)]
pub struct DeliveryReceipt {
    /// Transport-assigned delivery identifier.
    pub delivery_id: DeliveryId,
    /// Timestamp when the transport accepted the message for delivery.
    pub delivered_at: Timestamp,
}

// ---------------------------------------------------------------------------
// Capabilities
// ---------------------------------------------------------------------------

/// Capabilities declared by a transport implementation.
#[derive(Debug, Clone)]
pub struct TransportCapabilities {
    /// Whether this transport can stream partial responses.
    pub supports_streaming: bool,
    /// Whether this transport can deliver structured rich cards.
    pub supports_structured_cards: bool,
    /// Whether this transport supports file attachments.
    pub supports_file_transfer: bool,
    /// Maximum message size in bytes. `0` means no enforced limit.
    pub max_message_bytes: u64,
    /// Authentication methods supported by this transport.
    pub supported_auth_methods: Vec<String>,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that a [`Transport`] implementation may return.
#[derive(Debug, Error)]
pub enum TransportError {
    /// The sender could not be authenticated.
    #[error("authentication failed: {message}")]
    Authentication {
        /// Human-readable description.
        message: String,
    },

    /// The message could not be delivered to the conversation.
    #[error("delivery failed for conversation {conversation_id}: {message}")]
    DeliveryFailed {
        /// The target conversation.
        conversation_id: String,
        /// Human-readable description.
        message: String,
    },

    /// The transport connection has been lost.
    #[error("transport connection lost")]
    ConnectionLost,

    /// The transport's rate limit has been exceeded.
    #[error("transport rate limit exceeded; retry after {retry_after:?}")]
    RateLimit {
        /// How long to wait before retrying.
        retry_after: Option<Duration>,
    },

    /// An unexpected internal error.
    #[error("transport internal error: {message}")]
    Internal {
        /// Human-readable description.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Trait definition
// ---------------------------------------------------------------------------

/// The narrow transport port.
///
/// One implementation exists per channel type (Polkadot encrypted chat,
/// WebSocket, HTTP API, CLI stdin/stdout, webhook ingress). The kernel
/// depends only on this trait.
///
/// # Contract
///
/// - Implementations must be `Send + Sync + 'static`.
/// - [`receive`] blocks until a message arrives or the transport is closed.
///   A closed transport returns [`TransportError::ConnectionLost`].
/// - [`ack`] must be called **exactly once** per [`IncomingMessage`] after
///   the message has been durably admitted. Not calling `ack` causes the
///   transport to re-deliver.
/// - [`send`] does not guarantee end-to-end delivery; it returns a receipt
///   as soon as the transport accepts the message for delivery.
/// - Implementations must contain no business logic. Content inspection,
///   policy evaluation, and admission control happen in the application layer.
///
/// [`receive`]: Transport::receive
/// [`ack`]: Transport::ack
/// [`send`]: Transport::send
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    /// Wait for the next incoming message.
    ///
    /// Blocks until a message is available or the transport is closed.
    /// Returns [`TransportError::ConnectionLost`] when the transport is closed.
    async fn receive(&self) -> Result<IncomingMessage, TransportError>;

    /// Acknowledge that a message has been durably admitted.
    ///
    /// Must be called exactly once per message, after the message has been
    /// committed to durable storage by the application layer.
    async fn ack(&self, delivery_id: DeliveryId) -> Result<(), TransportError>;

    /// Send an outgoing message.
    ///
    /// Returns a [`DeliveryReceipt`] when the transport accepts the message.
    /// The receipt does not imply end-to-end delivery.
    async fn send(&self, message: OutgoingMessage) -> Result<DeliveryReceipt, TransportError>;

    /// Return the capabilities of this transport.
    fn capabilities(&self) -> TransportCapabilities;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_id_display() {
        let id = DeliveryId::new("msg-abc-123");
        assert_eq!(format!("{id}"), "msg-abc-123");
    }

    #[test]
    fn user_id_display() {
        let id = UserId::new("user-42");
        assert_eq!(format!("{id}"), "user-42");
    }

    #[test]
    fn sender_trust_tier_serde() {
        let tier = SenderTrustTier::Authenticated;
        let json = serde_json::to_string(&tier).expect("serialize");
        assert_eq!(json, r#""authenticated""#);
        let back: SenderTrustTier = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, tier);
    }

    #[test]
    fn incoming_message_serializes() {
        let msg = IncomingMessage {
            delivery_id: DeliveryId::new("d-1"),
            conversation_id: polkagent_core::ConversationId::new(),
            sender: AuthenticatedSender {
                user_id: UserId::new("u-1"),
                display_name: Some("Alice".into()),
                trust_tier: SenderTrustTier::Authenticated,
            },
            body: MessageBody::Text { content: "Hello!".into() },
            received_at: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        assert!(json.contains("Hello!"));
        assert!(json.contains("Alice"));
    }

    #[test]
    fn outgoing_message_serializes() {
        let msg = OutgoingMessage {
            conversation_id: polkagent_core::ConversationId::new(),
            run_id: None,
            body: OutgoingBody::Text { content: "Response text".into() },
            classification: Classification::Internal,
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        assert!(json.contains("Response text"));
        assert!(json.contains("internal"));
    }

    #[test]
    fn classification_ordering() {
        assert!(Classification::Public < Classification::Internal);
        assert!(Classification::Internal < Classification::Private);
        assert!(Classification::Private < Classification::Sensitive);
    }

    #[test]
    fn transport_error_authentication_message() {
        let e = TransportError::Authentication { message: "expired token".into() };
        assert!(format!("{e}").contains("expired token"));
    }

    #[test]
    fn transport_error_rate_limit_with_retry() {
        let e = TransportError::RateLimit { retry_after: Some(Duration::from_secs(5)) };
        let msg = format!("{e}");
        assert!(msg.contains("rate limit") || msg.contains("Rate limit"));
    }

    #[test]
    fn transport_error_connection_lost() {
        let e = TransportError::ConnectionLost;
        let msg = format!("{e}");
        assert!(msg.contains("connection") || msg.contains("lost"));
    }

    /// Compile-time check: `Transport` can be used as a `dyn` trait object.
    #[allow(dead_code)]
    fn _transport_is_object_safe(_t: &dyn Transport) {}
}
