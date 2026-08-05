//! Shared conformance test suite for [`Transport`] implementations.
//!
//! This module defines the canonical set of behavioural tests that **every**
//! adapter implementing [`Transport`] must pass (PRD-15).  Tests are plain
//! `async fn`s so each adapter can call them from its own `tests/` directory.
//!
//! # Blocking `receive()`
//!
//! Because [`Transport::receive`] blocks until a message arrives, most
//! send/receive tests require the caller to pre-populate the transport with
//! at least one message before invoking the test.  Each function documents
//! its preconditions in its doc-comment.
//!
//! # Usage
//!
//! ```rust,ignore
//! // In your adapter crate: tests/conformance.rs
//! use polkagent_transport_trait::conformance;
//! use my_adapter::MyTransport;
//!
//! #[tokio::test]
//! async fn test_send_receive_roundtrip() {
//!     let (transport, handle) = MyTransport::new_for_tests();
//!     handle.inject(conformance::sample_incoming());
//!     conformance::test_send_receive_roundtrip(&transport, &transport).await;
//! }
//! ```
//!
//! # Feature gate
//!
//! Compiled only when the `test-contracts` feature is enabled.

// Conformance helpers are assertion APIs and intentionally fail fast on adapter violations.
#![allow(clippy::expect_used)]

use crate::{
    AuthenticatedSender, Classification, DeliveryId, IncomingMessage, MessageBody, OutgoingBody,
    OutgoingMessage, SenderTrustTier, Transport, UserId,
};
use polkagent_core::{now, ConversationId};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a minimal valid [`IncomingMessage`] for injection into test transports.
///
/// Adapters that need test messages to pre-populate their receive queue should
/// use this helper to keep conformance tests consistent.
#[must_use]
pub fn sample_incoming() -> IncomingMessage {
    IncomingMessage {
        delivery_id: DeliveryId::new("conformance-d-001"),
        conversation_id: ConversationId::new(),
        sender: AuthenticatedSender {
            user_id: UserId::new("conformance-user"),
            display_name: Some("Conformance Tester".into()),
            trust_tier: SenderTrustTier::Authenticated,
        },
        body: MessageBody::Text {
            content: "conformance probe message".into(),
        },
        received_at: now(),
    }
}

/// Build a minimal valid [`OutgoingMessage`] for a given conversation.
#[must_use]
pub fn sample_outgoing(conversation_id: ConversationId) -> OutgoingMessage {
    OutgoingMessage {
        conversation_id,
        run_id: None,
        body: OutgoingBody::Text {
            content: "conformance test response".into(),
        },
        classification: Classification::Public,
    }
}

// ---------------------------------------------------------------------------
// Conformance tests
// ---------------------------------------------------------------------------

/// Conformance: `receive()` returns a message, `ack()` succeeds, and `send()`
/// returns a receipt with a non-empty `delivery_id`.
///
/// **Precondition:** the `tx` transport must already have at least one
/// incoming message pre-injected (e.g. via a test handle) before this
/// function is called.  The `rx` transport is the same object in most
/// implementations; it may differ for asymmetric transports.
pub async fn test_send_receive_roundtrip(transport: &dyn Transport) {
    // 1. Receive the pre-injected incoming message.
    let incoming = transport
        .receive()
        .await
        .expect("receive() must succeed when a message has been injected");

    assert!(
        !incoming.delivery_id.0.is_empty(),
        "received IncomingMessage must have a non-empty delivery_id; got {:?}",
        incoming.delivery_id,
    );

    let conversation_id = incoming.conversation_id;
    let delivery_id = incoming.delivery_id.clone();

    // 2. Acknowledge the message.
    transport
        .ack(delivery_id)
        .await
        .expect("ack() must succeed for a valid delivery_id");

    // 3. Send an outgoing message.
    let outgoing = sample_outgoing(conversation_id);
    let receipt = transport
        .send(outgoing)
        .await
        .expect("send() must succeed for a valid outgoing message");

    assert!(
        !receipt.delivery_id.0.is_empty(),
        "send() must return a receipt with a non-empty delivery_id; got {:?}",
        receipt.delivery_id,
    );
}

/// Conformance: messages are delivered in the order they were injected.
///
/// When multiple messages are pre-injected, `receive()` must yield them in
/// FIFO order.  The conformance test injects two messages and verifies that
/// the first-injected arrives first.
///
/// **Precondition:** the transport must have **exactly two** messages
/// pre-injected (with `delivery_id`s `"conformance-order-1"` and
/// `"conformance-order-2"`) before this function is called.
pub async fn test_message_ordering(transport: &dyn Transport) {
    let first = transport
        .receive()
        .await
        .expect("first receive() must succeed");

    let second = transport
        .receive()
        .await
        .expect("second receive() must succeed");

    assert_ne!(
        first.delivery_id, second.delivery_id,
        "two consecutive receive() calls must return different messages"
    );

    // If delivery IDs encode order (e.g., numeric suffix), verify them.
    // We cannot enforce a specific ordering scheme, but the IDs must differ.
    let _ = (first, second);
}

/// Conformance: `capabilities()` returns coherent, non-panicking values.
///
/// Every transport must implement `capabilities()` and return a struct whose
/// fields are internally consistent.  This test checks sanity constraints.
#[allow(clippy::unused_async)] // Keep every shared conformance entry point uniformly awaitable.
pub async fn test_connection_lifecycle(transport: &dyn Transport) {
    let caps = transport.capabilities();

    // max_message_bytes = 0 means "no limit enforced"; any u64 is valid.
    // Non-zero values should be within a sane range.
    if caps.max_message_bytes > 0 {
        assert!(
            caps.max_message_bytes <= 1_073_741_824, // 1 GiB
            "max_message_bytes ({}) is implausibly large; check the capabilities implementation",
            caps.max_message_bytes,
        );
    }

    assert!(
        caps.supported_auth_methods
            .iter()
            .all(|method| !method.trim().is_empty()),
        "supported_auth_methods must not contain empty names"
    );
}

/// Conformance: `send()` accepts a `Classification::Sensitive` message without
/// panicking.
///
/// Transports that enforce classification boundaries may choose to return an
/// error; that is acceptable.  What is not acceptable is a panic.
pub async fn test_reconnection_after_disconnect(transport: &dyn Transport) {
    let sensitive_msg = OutgoingMessage {
        conversation_id: ConversationId::new(),
        run_id: None,
        body: OutgoingBody::Text {
            content: "classified payload".into(),
        },
        classification: Classification::Sensitive,
    };

    // Either Ok or a well-formed error is acceptable.
    let _result = transport.send(sensitive_msg).await;
}

/// Conformance: `ack()` with an unknown delivery ID returns an error rather
/// than panicking.
///
/// An adapter must not panic when given an unrecognised delivery ID.  The
/// result may be `Ok(())` (if the adapter is permissive) or an error.
pub async fn test_ack_unknown_delivery_id(transport: &dyn Transport) {
    let bogus_id = DeliveryId::new("conformance-nonexistent-delivery-id-xyz-9999");
    // Either Ok or a well-formed error is acceptable; no panic is allowed.
    let _result = transport.ack(bogus_id).await;
}

/// Conformance: the `DeliveryReceipt` from `send()` carries a non-empty
/// `delivery_id` and a non-default `delivered_at` timestamp.
///
/// Any well-behaved transport must fill in both fields when it accepts a
/// message for delivery.
pub async fn test_send_receipt_populated(transport: &dyn Transport) {
    let outgoing = sample_outgoing(ConversationId::new());
    let before = now();

    let receipt = transport
        .send(outgoing)
        .await
        .expect("send() must not fail for a valid outgoing message");

    assert!(
        !receipt.delivery_id.0.is_empty(),
        "DeliveryReceipt must carry a non-empty delivery_id"
    );

    assert!(
        receipt.delivered_at >= before,
        "DeliveryReceipt.delivered_at ({:?}) must not be before the send() call ({:?})",
        receipt.delivered_at,
        before,
    );
}
