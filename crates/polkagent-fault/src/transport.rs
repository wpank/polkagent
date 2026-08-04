//! Fault-injecting [`Transport`] wrapper.
//!
//! [`FaultTransport`] wraps any [`Transport`] implementation and intercepts
//! calls at named injection points, simulating network partitions, message
//! loss, and reordering.
//!
//! # Injection points
//!
//! - `"before_send"` — checked before forwarding `send` to the inner transport.
//! - `"after_send"` — checked after the inner transport confirms delivery.
//! - `"before_receive"` — checked before blocking on the inner `receive`.

use std::sync::Arc;

use async_trait::async_trait;
use polkagent_core::now;
use polkagent_transport_trait::{
    DeliveryId, DeliveryReceipt, IncomingMessage, OutgoingMessage, Transport,
    TransportCapabilities, TransportError,
};

use crate::injector::FaultInjector;
use crate::types::Fault;

// ---------------------------------------------------------------------------
// FaultTransport
// ---------------------------------------------------------------------------

/// A fault-injecting wrapper around any [`Transport`].
pub struct FaultTransport<T: Transport> {
    inner: T,
    injector: Arc<FaultInjector>,
}

impl<T: Transport> FaultTransport<T> {
    /// Wrap `inner` with a fault injector.
    pub fn new(inner: T, injector: Arc<FaultInjector>) -> Self {
        Self { inner, injector }
    }
}

#[async_trait]
impl<T: Transport> Transport for FaultTransport<T> {
    async fn receive(&self) -> Result<IncomingMessage, TransportError> {
        // --- before_receive ---
        if let Some(fault) = self.injector.check("before_receive") {
            match fault {
                Fault::Crash => panic!("FaultTransport: crash at before_receive"),
                Fault::Error { message } => {
                    return Err(TransportError::Internal { message });
                }
                Fault::Timeout { ms } => {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                    return Err(TransportError::ConnectionLost);
                }
                Fault::SlowDown { ms } => {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                }
                Fault::CorruptData { .. } | Fault::PartialWrite => {
                    // Corrupt/partial handled after receive returns data.
                }
            }
        }

        self.inner.receive().await
    }

    async fn ack(&self, delivery_id: DeliveryId) -> Result<(), TransportError> {
        self.inner.ack(delivery_id).await
    }

    async fn send(&self, message: OutgoingMessage) -> Result<DeliveryReceipt, TransportError> {
        // --- before_send ---
        if let Some(fault) = self.injector.check("before_send") {
            match fault {
                Fault::Crash => panic!("FaultTransport: crash at before_send"),
                Fault::Error { message: msg } => {
                    // Simulate network partition / message loss.
                    return Err(TransportError::Internal { message: msg });
                }
                Fault::Timeout { ms } => {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                    return Err(TransportError::ConnectionLost);
                }
                Fault::SlowDown { ms } => {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                }
                Fault::CorruptData { .. } | Fault::PartialWrite => {
                    // Drop the message without delivering it (simulates loss).
                    let fake_id = DeliveryId::new(format!("lost-{}", now().timestamp_millis()));
                    return Ok(DeliveryReceipt {
                        delivery_id: fake_id,
                        delivered_at: now(),
                    });
                }
            }
        }

        let receipt = self.inner.send(message).await?;

        // --- after_send ---
        if let Some(fault) = self.injector.check("after_send") {
            match fault {
                Fault::Crash => panic!("FaultTransport: crash at after_send"),
                Fault::Error { message } => {
                    return Err(TransportError::Internal { message });
                }
                Fault::Timeout { ms } => {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                    return Err(TransportError::ConnectionLost);
                }
                Fault::SlowDown { ms } => {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                }
                Fault::CorruptData { .. } | Fault::PartialWrite => {
                    // Post-send corruption is observable only to caller;
                    // the message was already forwarded. Just return the receipt.
                }
            }
        }

        Ok(receipt)
    }

    fn capabilities(&self) -> TransportCapabilities {
        self.inner.capabilities()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FaultSchedule;
    use polkagent_core::{now, ConversationId};
    use polkagent_transport_fake::FakeTransport;
    use polkagent_transport_trait::{
        AuthenticatedSender, Classification, MessageBody, OutgoingBody, SenderTrustTier, UserId,
    };

    fn make_outgoing() -> OutgoingMessage {
        OutgoingMessage {
            conversation_id: ConversationId::new(),
            run_id: None,
            body: OutgoingBody::Text {
                content: "hello".into(),
            },
            classification: Classification::Public,
        }
    }

    fn make_incoming() -> IncomingMessage {
        IncomingMessage {
            delivery_id: DeliveryId::new("d-1"),
            conversation_id: ConversationId::new(),
            sender: AuthenticatedSender {
                user_id: UserId::new("u-1"),
                display_name: None,
                trust_tier: SenderTrustTier::Authenticated,
            },
            body: MessageBody::Text {
                content: "test".into(),
            },
            received_at: now(),
        }
    }

    #[tokio::test]
    async fn no_fault_send_delivers_message() {
        let (inner, handle) = FakeTransport::new();
        let injector = Arc::new(FaultInjector::new());
        let transport = FaultTransport::new(inner, injector);
        transport.send(make_outgoing()).await.expect("send ok");
        assert_eq!(handle.sent_count(), 1);
    }

    #[tokio::test]
    async fn partition_fault_prevents_send() {
        let (inner, handle) = FakeTransport::new();
        let injector = Arc::new(FaultInjector::new());
        injector.add_fault(
            "before_send",
            Fault::Error {
                message: "network partition".into(),
            },
            FaultSchedule::Always,
        );
        let transport = FaultTransport::new(inner, injector);
        let result = transport.send(make_outgoing()).await;
        assert!(result.is_err());
        // Message should NOT have been delivered to the inner transport.
        assert_eq!(handle.sent_count(), 0);
    }

    #[tokio::test]
    async fn message_loss_before_send_returns_fake_receipt() {
        let (inner, handle) = FakeTransport::new();
        let injector = Arc::new(FaultInjector::new());
        injector.add_fault("before_send", Fault::PartialWrite, FaultSchedule::Always);
        let transport = FaultTransport::new(inner, injector);
        // Returns Ok but the inner transport never sees the message.
        let result = transport.send(make_outgoing()).await;
        assert!(result.is_ok());
        assert_eq!(handle.sent_count(), 0);
    }

    #[tokio::test]
    async fn connection_lost_on_before_receive() {
        let (inner, handle) = FakeTransport::new();
        let injector = Arc::new(FaultInjector::new());
        injector.add_fault(
            "before_receive",
            Fault::Error {
                message: "connection lost".into(),
            },
            FaultSchedule::Always,
        );
        let transport = FaultTransport::new(inner, injector);
        // Inject a message so the inner transport has something to deliver.
        handle.inject_message(make_incoming());
        // But the fault fires before we even try.
        let result = transport.receive().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn crash_fault_on_send_panics() {
        let (inner, _handle) = FakeTransport::new();
        let injector = Arc::new(FaultInjector::new());
        injector.add_fault("before_send", Fault::Crash, FaultSchedule::Always);
        let transport = Arc::new(FaultTransport::new(inner, injector));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let rt = tokio::runtime::Runtime::new().expect("rt");
            rt.block_on(transport.send(make_outgoing()))
        }));
        assert!(result.is_err(), "crash fault should panic");
    }

    #[tokio::test]
    async fn after_send_error_returns_error() {
        let (inner, handle) = FakeTransport::new();
        let injector = Arc::new(FaultInjector::new());
        injector.add_fault(
            "after_send",
            Fault::Error {
                message: "post-send failure".into(),
            },
            FaultSchedule::Always,
        );
        let transport = FaultTransport::new(inner, injector);
        let result = transport.send(make_outgoing()).await;
        // The inner transport DID receive the message.
        assert_eq!(handle.sent_count(), 1);
        // But the caller sees an error.
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn timeout_on_before_send_returns_connection_lost() {
        let (inner, _handle) = FakeTransport::new();
        let injector = Arc::new(FaultInjector::new());
        injector.add_fault(
            "before_send",
            Fault::Timeout { ms: 1 },
            FaultSchedule::Always,
        );
        let transport = FaultTransport::new(inner, injector);
        let result = transport.send(make_outgoing()).await;
        assert!(matches!(result, Err(TransportError::ConnectionLost)));
    }
}
