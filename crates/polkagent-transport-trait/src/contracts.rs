//! Reusable contract tests for [`Transport`] implementations.
//!
//! This module provides test functions that verify any implementation of
//! [`Transport`] conforms to the expected behaviour defined in the trait
//! contract. Each function accepts a reference to an implementor and exercises
//! one aspect of the contract.
//!
//! Because [`Transport::receive`] blocks until a message arrives, the
//! send/ack contract test requires the caller to pre-populate the transport
//! with at least one message before calling the test function.
//!
//! # Usage
//!
//! In your adapter crate's integration test:
//!
//! ```rust,ignore
//! use polkagent_transport_trait::contracts;
//!
//! #[tokio::test]
//! async fn send_and_ack() {
//!     let (transport, handle) = FakeTransport::new();
//!     handle.inject_message(some_incoming_message());
//!     contracts::test_send_and_ack(&transport).await;
//! }
//! ```
//!
//! # Feature gate
//!
//! This module is only available when the `test-contracts` feature is enabled.

use crate::{Classification, OutgoingBody, OutgoingMessage, Transport, TransportCapabilities};
use polkagent_core::ConversationId;

/// Build a minimal [`OutgoingMessage`] for contract tests.
#[must_use]
pub fn minimal_outgoing(conversation_id: ConversationId) -> OutgoingMessage {
    OutgoingMessage {
        conversation_id,
        run_id: None,
        body: OutgoingBody::Text {
            content: "contract test response".into(),
        },
        classification: Classification::Public,
    }
}

/// Contract: `send()` succeeds and messages are delivered.
///
/// This test verifies that:
/// 1. A previously injected incoming message can be received via `receive()`.
/// 2. The received message can be acknowledged via `ack()`.
/// 3. An outgoing message can be sent via `send()` and a delivery receipt
///    is returned with a non-empty `delivery_id`.
///
/// **Precondition:** The caller must inject at least one incoming message
/// into the transport before calling this function (e.g., via a test handle).
pub async fn test_send_and_ack(transport: &dyn Transport) {
    // 1. Receive a pre-injected message.
    let incoming = transport
        .receive()
        .await
        .expect("receive() must succeed when a message has been injected");

    // Verify the delivery_id is non-empty.
    assert!(
        !incoming.delivery_id.0.is_empty(),
        "received message must have a non-empty delivery_id"
    );

    // 2. Acknowledge the message.
    let delivery_id = incoming.delivery_id.clone();
    transport
        .ack(delivery_id)
        .await
        .expect("ack() must succeed for a valid delivery_id");

    // 3. Send an outgoing message.
    let outgoing = minimal_outgoing(incoming.conversation_id);
    let receipt = transport
        .send(outgoing)
        .await
        .expect("send() must succeed for a valid outgoing message");

    assert!(
        !receipt.delivery_id.0.is_empty(),
        "send() must return a receipt with a non-empty delivery_id"
    );
}

/// Contract: `capabilities()` returns a valid struct.
///
/// Every transport must report its capabilities. This test verifies that
/// the returned [`TransportCapabilities`] struct has coherent values:
/// - `max_message_bytes` is set (may be 0 for "no limit").
/// - `supported_auth_methods` is a valid (possibly empty) list.
pub async fn test_capabilities_returns(transport: &dyn Transport) {
    let caps: TransportCapabilities = transport.capabilities();

    // We cannot assert specific values since they are implementation-specific,
    // but we verify the struct is accessible and its fields are well-formed.
    // max_message_bytes is a u64 so any value is technically valid.
    // supported_auth_methods must be a valid Vec (not panic on access).
    let _streaming = caps.supports_streaming;
    let _cards = caps.supports_structured_cards;
    let _files = caps.supports_file_transfer;
    let _max = caps.max_message_bytes;
    let _auth_count = caps.supported_auth_methods.len();

    // If max_message_bytes is non-zero, it should be a reasonable value.
    // We don't enforce a specific range, just that it didn't overflow or
    // return garbage.
    if caps.max_message_bytes > 0 {
        assert!(
            caps.max_message_bytes <= 1024 * 1024 * 1024, // 1 GiB sanity cap
            "max_message_bytes seems unreasonably large: {}",
            caps.max_message_bytes
        );
    }
}
