//! Deterministic fake [`Transport`] adapter for testing.
//!
//! This crate provides [`FakeTransport`] and its companion
//! [`FakeTransportHandle`] — together they implement the [`Transport`] trait
//! using in-process `tokio::sync::mpsc` channels.
//!
//! The split design mirrors real transport semantics:
//! - The [`FakeTransport`] is the production-facing side. It is given to the
//!   system under test and implements [`Transport`].
//! - The [`FakeTransportHandle`] is the test-facing side. It lets tests inject
//!   incoming messages and inspect all outgoing messages.
//!
//! # Quick-start example
//!
//! ```rust
//! use polkagent_transport_fake::{FakeTransport, FakeTransportHandle};
//! use polkagent_transport_trait::Transport;
//!
//! # tokio_test::block_on(async {
//! let (transport, handle) = FakeTransport::new();
//!
//! // Inject a message as if it arrived from the network.
//! use polkagent_transport_trait::{
//!     AuthenticatedSender, DeliveryId, IncomingMessage, MessageBody,
//!     SenderTrustTier, UserId,
//! };
//! use polkagent_core::{ConversationId, now};
//!
//! let msg = IncomingMessage {
//!     delivery_id: DeliveryId::new("d-001"),
//!     conversation_id: ConversationId::new(),
//!     sender: AuthenticatedSender {
//!         user_id: UserId::new("user-alice"),
//!         display_name: Some("Alice".into()),
//!         trust_tier: SenderTrustTier::Authenticated,
//!     },
//!     body: MessageBody::Text { content: "hello agent".into() },
//!     received_at: now(),
//! };
//! handle.inject_message(msg);
//!
//! // The system under test can receive it.
//! let received = transport.receive().await.expect("should receive");
//! assert_eq!(received.delivery_id, DeliveryId::new("d-001"));
//!
//! // Inspect outgoing messages sent by the system under test.
//! use polkagent_transport_trait::{Classification, OutgoingBody, OutgoingMessage};
//! let out = OutgoingMessage {
//!     conversation_id: received.conversation_id,
//!     run_id: None,
//!     body: OutgoingBody::Text { content: "hello user".into() },
//!     classification: Classification::Public,
//! };
//! transport.send(out).await.expect("send ok");
//! let sent = handle.drain_sent();
//! assert_eq!(sent.len(), 1);
//! # });
//! ```

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use polkagent_core::now;
use polkagent_transport_trait::{
    DeliveryId, DeliveryReceipt, IncomingMessage, OutgoingMessage, Transport,
    TransportCapabilities, TransportError,
};

// ---------------------------------------------------------------------------
// FakeTransportHandle
// ---------------------------------------------------------------------------

/// The test-facing handle for a [`FakeTransport`].
///
/// Use this to inject messages into the transport and inspect sent messages.
pub struct FakeTransportHandle {
    /// Channel sender for injecting incoming messages.
    incoming_tx: tokio::sync::mpsc::UnboundedSender<IncomingMessage>,
    /// Accumulator of all outgoing messages sent through the transport.
    sent: std::sync::Arc<Mutex<Vec<OutgoingMessage>>>,
}

impl FakeTransportHandle {
    /// Inject an incoming message as if it arrived from the network.
    ///
    /// The next call to [`Transport::receive`] on the paired [`FakeTransport`]
    /// will return this message.
    ///
    /// # Panics
    ///
    /// Panics if the paired [`FakeTransport`] has been dropped.
    pub fn inject_message(&self, msg: IncomingMessage) {
        self.incoming_tx.send(msg).expect("FakeTransport receiver dropped");
    }

    /// Return and clear all outgoing messages that have been sent through the
    /// paired [`FakeTransport`] since the last call to `drain_sent`.
    ///
    /// Messages are returned in the order they were sent.
    pub fn drain_sent(&self) -> Vec<OutgoingMessage> {
        let mut guard = self.sent.lock().expect("mutex poisoned");
        std::mem::take(&mut *guard)
    }

    /// Return the number of outgoing messages accumulated since the last
    /// [`drain_sent`] call, without removing them.
    ///
    /// [`drain_sent`]: FakeTransportHandle::drain_sent
    #[must_use]
    pub fn sent_count(&self) -> usize {
        self.sent.lock().expect("mutex poisoned").len()
    }
}

// ---------------------------------------------------------------------------
// FakeTransport
// ---------------------------------------------------------------------------

/// A deterministic fake implementation of [`Transport`].
///
/// Created via [`FakeTransport::new`], which returns both the transport itself
/// and a [`FakeTransportHandle`] for test control.
///
/// # Simulating conditions
///
/// - **Delay:** call [`FakeTransport::set_send_delay`] to inject latency on
///   [`send`] calls.
/// - **Errors:** call [`FakeTransport::set_send_error`] to make the next
///   `send` call fail with a specific error.
/// - **Disconnection:** call [`FakeTransport::disconnect`] to make
///   [`receive`] return [`TransportError::ConnectionLost`].
///
/// [`send`]: Transport::send
/// [`receive`]: Transport::receive
pub struct FakeTransport {
    /// Channel receiver for incoming messages injected by the handle.
    ///
    /// Uses `tokio::sync::Mutex` because `receive` holds this lock across
    /// an `.await` point (the `recv().await` call). `std::sync::Mutex` is
    /// not `Send` across await boundaries.
    incoming_rx: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<IncomingMessage>>,
    /// Shared accumulator for outgoing messages.
    sent: std::sync::Arc<Mutex<Vec<OutgoingMessage>>>,
    /// Whether the transport is disconnected.
    disconnected: AtomicBool,
    /// Artificial delay applied to every `send` call, in milliseconds.
    send_delay_ms: AtomicU64,
    /// Pending error to inject on the next `send` call.
    next_send_error: Mutex<Option<SendErrorKind>>,
    /// Total number of `ack` calls received.
    ack_count: AtomicU64,
}

/// Copyable description of a transport error to inject on the next send.
enum SendErrorKind {
    Authentication { message: String },
    DeliveryFailed { conversation_id: String, message: String },
    ConnectionLost,
    RateLimit { retry_after: Option<Duration> },
    Internal { message: String },
}

impl SendErrorKind {
    fn to_error(&self) -> TransportError {
        match self {
            Self::Authentication { message } => {
                TransportError::Authentication { message: message.clone() }
            }
            Self::DeliveryFailed { conversation_id, message } => TransportError::DeliveryFailed {
                conversation_id: conversation_id.clone(),
                message: message.clone(),
            },
            Self::ConnectionLost => TransportError::ConnectionLost,
            Self::RateLimit { retry_after } => TransportError::RateLimit { retry_after: *retry_after },
            Self::Internal { message } => TransportError::Internal { message: message.clone() },
        }
    }
}

impl FakeTransport {
    /// Create a new `FakeTransport` / [`FakeTransportHandle`] pair.
    ///
    /// The transport and its handle share an in-process channel. Neither
    /// makes any network calls.
    #[must_use]
    pub fn new() -> (Self, FakeTransportHandle) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let sent = std::sync::Arc::new(Mutex::new(Vec::new()));
        let transport = Self {
            incoming_rx: tokio::sync::Mutex::new(rx),
            sent: std::sync::Arc::clone(&sent),
            disconnected: AtomicBool::new(false),
            send_delay_ms: AtomicU64::new(0),
            next_send_error: Mutex::new(None),
            ack_count: AtomicU64::new(0),
        };
        let handle = FakeTransportHandle { incoming_tx: tx, sent };
        (transport, handle)
    }

    // -----------------------------------------------------------------------
    // Test control
    // -----------------------------------------------------------------------

    /// Set an artificial delay for every subsequent [`send`] call.
    ///
    /// [`send`]: Transport::send
    pub fn set_send_delay(&self, delay: Duration) {
        self.send_delay_ms.store(delay.as_millis() as u64, Ordering::SeqCst);
    }

    /// Inject an error to be returned by the next [`send`] call.
    ///
    /// After one send call consumes the error, subsequent sends succeed unless
    /// another error is injected.
    ///
    /// [`send`]: Transport::send
    pub fn set_next_send_error(&self, error: TransportError) {
        let kind = match error {
            TransportError::Authentication { message } => {
                SendErrorKind::Authentication { message }
            }
            TransportError::DeliveryFailed { conversation_id, message } => {
                SendErrorKind::DeliveryFailed { conversation_id, message }
            }
            TransportError::ConnectionLost => SendErrorKind::ConnectionLost,
            TransportError::RateLimit { retry_after } => SendErrorKind::RateLimit { retry_after },
            TransportError::Internal { message } => SendErrorKind::Internal { message },
        };
        let mut guard = self.next_send_error.lock().expect("mutex poisoned");
        *guard = Some(kind);
    }

    /// Simulate a disconnection.
    ///
    /// After this call, [`receive`] will immediately return
    /// [`TransportError::ConnectionLost`] instead of blocking.
    ///
    /// [`receive`]: Transport::receive
    pub fn disconnect(&self) {
        self.disconnected.store(true, Ordering::SeqCst);
    }

    /// Reconnect the transport after a [`disconnect`].
    ///
    /// [`disconnect`]: FakeTransport::disconnect
    pub fn reconnect(&self) {
        self.disconnected.store(false, Ordering::SeqCst);
    }

    /// Return how many [`ack`] calls have been made.
    ///
    /// [`ack`]: Transport::ack
    #[must_use]
    pub fn ack_count(&self) -> u64 {
        self.ack_count.load(Ordering::SeqCst)
    }

    /// Take the pending send error, if any.
    ///
    /// Factored into a non-async helper so that the `std::sync::MutexGuard`
    /// never appears inside the async state machine generated by
    /// `#[async_trait]`. A `MutexGuard` is `!Send`, and including one in the
    /// future — even if it is dropped before an `.await` — can cause
    /// `future cannot be sent between threads safely` errors.
    fn take_next_send_error(&self) -> Option<SendErrorKind> {
        self.next_send_error.lock().expect("mutex poisoned").take()
    }

    /// Push an outgoing message into the sent accumulator.
    ///
    /// Same rationale as [`take_next_send_error`]: keeps the
    /// `std::sync::MutexGuard` out of the async state machine.
    fn push_sent(&self, message: OutgoingMessage) {
        self.sent.lock().expect("mutex poisoned").push(message);
    }
}

#[async_trait]
impl Transport for FakeTransport {
    async fn receive(&self) -> Result<IncomingMessage, TransportError> {
        if self.disconnected.load(Ordering::SeqCst) {
            return Err(TransportError::ConnectionLost);
        }
        let msg = {
            // Use tokio::sync::Mutex so the guard can be held across `.await`.
            let mut rx = self.incoming_rx.lock().await;
            rx.recv().await
        };
        match msg {
            Some(m) => Ok(m),
            None => Err(TransportError::ConnectionLost),
        }
    }

    async fn ack(&self, _delivery_id: DeliveryId) -> Result<(), TransportError> {
        if self.disconnected.load(Ordering::SeqCst) {
            return Err(TransportError::ConnectionLost);
        }
        self.ack_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> Result<DeliveryReceipt, TransportError> {
        // Check for a pending injected error.
        if let Some(kind) = self.take_next_send_error() {
            return Err(kind.to_error());
        }

        if self.disconnected.load(Ordering::SeqCst) {
            return Err(TransportError::ConnectionLost);
        }

        // Apply artificial delay.
        let delay_ms = self.send_delay_ms.load(Ordering::SeqCst);
        if delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }

        let delivery_id = DeliveryId::new(format!("fake-sent-{}", now().timestamp_millis()));
        let delivered_at = now();

        self.push_sent(message);

        Ok(DeliveryReceipt { delivery_id, delivered_at })
    }

    fn capabilities(&self) -> TransportCapabilities {
        TransportCapabilities {
            supports_streaming: true,
            supports_structured_cards: true,
            supports_file_transfer: true,
            max_message_bytes: 1024 * 1024,
            supported_auth_methods: vec!["fake".into()],
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::now;
    use polkagent_transport_trait::{
        AuthenticatedSender, Classification, MessageBody, OutgoingBody, OutgoingMessage,
        SenderTrustTier, UserId,
    };

    use polkagent_core::ConversationId;

    fn make_message(id: &str) -> IncomingMessage {
        IncomingMessage {
            delivery_id: DeliveryId::new(id),
            conversation_id: ConversationId::new(),
            sender: AuthenticatedSender {
                user_id: UserId::new("test-user"),
                display_name: None,
                trust_tier: SenderTrustTier::Authenticated,
            },
            body: MessageBody::Text { content: "test message".into() },
            received_at: now(),
        }
    }

    fn make_outgoing(conv: ConversationId) -> OutgoingMessage {
        OutgoingMessage {
            conversation_id: conv,
            run_id: None,
            body: OutgoingBody::Text { content: "response".into() },
            classification: Classification::Public,
        }
    }

    #[tokio::test]
    async fn inject_message_is_received_by_transport() {
        let (transport, handle) = FakeTransport::new();
        handle.inject_message(make_message("d-001"));
        let received = transport.receive().await.expect("receive ok");
        assert_eq!(received.delivery_id, DeliveryId::new("d-001"));
    }

    #[tokio::test]
    async fn send_is_accumulated_in_handle() {
        let (transport, handle) = FakeTransport::new();
        let conv = ConversationId::new();
        transport.send(make_outgoing(conv.clone())).await.expect("send ok");
        transport.send(make_outgoing(conv)).await.expect("send ok");
        assert_eq!(handle.sent_count(), 2);
    }

    #[tokio::test]
    async fn drain_sent_clears_accumulator() {
        let (transport, handle) = FakeTransport::new();
        let conv = ConversationId::new();
        transport.send(make_outgoing(conv)).await.expect("send ok");
        let drained = handle.drain_sent();
        assert_eq!(drained.len(), 1);
        assert_eq!(handle.sent_count(), 0);
    }

    #[tokio::test]
    async fn ack_increments_counter() {
        let (transport, _handle) = FakeTransport::new();
        assert_eq!(transport.ack_count(), 0);
        transport.ack(DeliveryId::new("d-001")).await.expect("ack ok");
        transport.ack(DeliveryId::new("d-002")).await.expect("ack ok");
        assert_eq!(transport.ack_count(), 2);
    }

    #[tokio::test]
    async fn disconnect_makes_receive_fail() {
        let (transport, _handle) = FakeTransport::new();
        transport.disconnect();
        let result = transport.receive().await;
        assert!(matches!(result, Err(TransportError::ConnectionLost)));
    }

    #[tokio::test]
    async fn disconnect_makes_send_fail() {
        let (transport, _handle) = FakeTransport::new();
        transport.disconnect();
        let conv = ConversationId::new();
        let result = transport.send(make_outgoing(conv)).await;
        assert!(matches!(result, Err(TransportError::ConnectionLost)));
    }

    #[tokio::test]
    async fn disconnect_makes_ack_fail() {
        let (transport, _handle) = FakeTransport::new();
        transport.disconnect();
        let result = transport.ack(DeliveryId::new("d-001")).await;
        assert!(matches!(result, Err(TransportError::ConnectionLost)));
    }

    #[tokio::test]
    async fn reconnect_restores_transport() {
        let (transport, handle) = FakeTransport::new();
        transport.disconnect();
        transport.reconnect();
        handle.inject_message(make_message("d-after-reconnect"));
        let received = transport.receive().await.expect("receive after reconnect");
        assert_eq!(received.delivery_id, DeliveryId::new("d-after-reconnect"));
    }

    #[tokio::test]
    async fn set_send_delay_is_respected() {
        let (transport, _handle) = FakeTransport::new();
        transport.set_send_delay(Duration::from_millis(50));
        let conv = ConversationId::new();
        let start = std::time::Instant::now();
        transport.send(make_outgoing(conv)).await.expect("send ok");
        assert!(start.elapsed() >= Duration::from_millis(40));
    }

    #[tokio::test]
    async fn set_next_send_error_is_returned_once() {
        let (transport, handle) = FakeTransport::new();
        transport.set_next_send_error(TransportError::Internal {
            message: "injected error".into(),
        });
        let conv = ConversationId::new();
        // First send should fail.
        let err = transport.send(make_outgoing(conv.clone())).await.expect_err("must fail");
        assert!(matches!(err, TransportError::Internal { .. }));
        // Second send should succeed.
        transport.send(make_outgoing(conv)).await.expect("second send ok");
        assert_eq!(handle.sent_count(), 1);
    }

    #[tokio::test]
    async fn capabilities_are_reported() {
        let (transport, _handle) = FakeTransport::new();
        let caps = transport.capabilities();
        assert!(caps.supports_streaming);
        assert!(caps.supports_structured_cards);
        assert!(caps.supports_file_transfer);
        assert!(caps.max_message_bytes > 0);
    }

    #[tokio::test]
    async fn multiple_messages_are_received_in_order() {
        let (transport, handle) = FakeTransport::new();
        for i in 0..5u32 {
            handle.inject_message(make_message(&format!("d-{i}")));
        }
        for i in 0..5u32 {
            let msg = transport.receive().await.expect("receive ok");
            assert_eq!(msg.delivery_id, DeliveryId::new(format!("d-{i}")));
        }
    }

    #[tokio::test]
    async fn handle_dropped_closes_transport() {
        let (transport, handle) = FakeTransport::new();
        drop(handle);
        let result = transport.receive().await;
        assert!(matches!(result, Err(TransportError::ConnectionLost)));
    }

    #[tokio::test]
    async fn send_receipt_contains_delivery_id() {
        let (transport, _handle) = FakeTransport::new();
        let conv = ConversationId::new();
        let receipt = transport.send(make_outgoing(conv)).await.expect("send ok");
        assert!(!receipt.delivery_id.0.is_empty());
    }
}
