//! Contract and conformance tests for [`FakeTransport`].
//!
//! These tests verify that the fake transport implementation conforms to the
//! [`Transport`] trait contract and the shared PRD-15 conformance suite defined
//! in `polkagent-transport-trait`.

use polkagent_core::{now, ConversationId};
use polkagent_transport_fake::FakeTransport;
use polkagent_transport_trait::{
    conformance, contracts, AuthenticatedSender, DeliveryId, IncomingMessage, MessageBody,
    SenderTrustTier, UserId,
};

/// Create a test incoming message for injection.
fn make_test_message() -> IncomingMessage {
    IncomingMessage {
        delivery_id: DeliveryId::new("contract-test-d-001"),
        conversation_id: ConversationId::new(),
        sender: AuthenticatedSender {
            user_id: UserId::new("contract-test-user"),
            display_name: Some("Contract Tester".into()),
            trust_tier: SenderTrustTier::Authenticated,
        },
        body: MessageBody::Text {
            content: "contract test message".into(),
        },
        received_at: now(),
    }
}

// ---------------------------------------------------------------------------
// Legacy contracts (kept for backwards compatibility)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn send_and_ack() {
    let (transport, handle) = FakeTransport::new();
    handle.inject_message(make_test_message());
    contracts::test_send_and_ack(&transport).await;
}

#[tokio::test]
async fn capabilities_returns() {
    let (transport, _handle) = FakeTransport::new();
    contracts::test_capabilities_returns(&transport).await;
}

// ---------------------------------------------------------------------------
// PRD-15 conformance suite
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conformance_send_receive_roundtrip() {
    let (transport, handle) = FakeTransport::new();
    handle.inject_message(make_test_message());
    conformance::test_send_receive_roundtrip(&transport).await;
}

#[tokio::test]
async fn conformance_message_ordering() {
    let (transport, handle) = FakeTransport::new();
    // Inject two messages with distinct delivery IDs.
    let mut m1 = make_test_message();
    m1.delivery_id = DeliveryId::new("conformance-order-1");
    let mut m2 = make_test_message();
    m2.delivery_id = DeliveryId::new("conformance-order-2");
    handle.inject_message(m1);
    handle.inject_message(m2);
    conformance::test_message_ordering(&transport).await;
}

#[tokio::test]
async fn conformance_connection_lifecycle() {
    let (transport, _handle) = FakeTransport::new();
    conformance::test_connection_lifecycle(&transport).await;
}

#[tokio::test]
async fn conformance_reconnection_after_disconnect() {
    let (transport, _handle) = FakeTransport::new();
    conformance::test_reconnection_after_disconnect(&transport).await;
}

#[tokio::test]
async fn conformance_ack_unknown_delivery_id() {
    let (transport, _handle) = FakeTransport::new();
    conformance::test_ack_unknown_delivery_id(&transport).await;
}

#[tokio::test]
async fn conformance_send_receipt_populated() {
    let (transport, _handle) = FakeTransport::new();
    conformance::test_send_receipt_populated(&transport).await;
}
