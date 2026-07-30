//! Contract tests for [`FakeTransport`] via the reusable contract test suite.
//!
//! These tests verify that the fake transport implementation conforms to the
//! [`Transport`] trait contract defined in `polkagent-transport-trait`.

use polkagent_core::{ConversationId, now};
use polkagent_transport_fake::FakeTransport;
use polkagent_transport_trait::contracts;
use polkagent_transport_trait::{
    AuthenticatedSender, DeliveryId, IncomingMessage, MessageBody, SenderTrustTier, UserId,
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

#[tokio::test]
async fn send_and_ack() {
    let (transport, handle) = FakeTransport::new();
    // Precondition: inject a message before calling the contract test.
    handle.inject_message(make_test_message());
    contracts::test_send_and_ack(&transport).await;
}

#[tokio::test]
async fn capabilities_returns() {
    let (transport, _handle) = FakeTransport::new();
    contracts::test_capabilities_returns(&transport).await;
}
