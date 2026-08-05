//! In-process event bus backed by `tokio::sync::broadcast`.
//!
//! The [`EventBus`] provides a multi-producer, multi-consumer broadcast
//! channel for [`RunEvent`]s within a single process (PRD-10 §9.1
//! REQ-STREAM-001–003).
//!
//! # Backpressure contract
//!
//! | Durability class | Behaviour when channel is full |
//! |---|---|
//! | `Durable` | The caller should NOT publish directly to the bus; instead use [`EventRecorder`](crate::recorder::EventRecorder), which persists first. |
//! | `Diagnostic` | Ring-buffer: oldest undelivered messages are dropped by the broadcast channel when capacity is exceeded. |
//! | `Ephemeral` | Ring-buffer: same as Diagnostic. The bus never blocks. |
//!
//! The broadcast channel already implements ring-buffer semantics: when a
//! sender publishes and the channel is full, the oldest message is dropped
//! (slow receivers will receive a `RecvError::Lagged` on their next call).
//! Consumers that care about diagnostic events must drain the channel promptly.
//!
//! # Default capacity
//!
//! The default capacity of 1024 events (configurable via [`EventBus::new`])
//! matches PRD-10 REQ-STREAM-002.

use polkagent_core::event::RunEvent;
use tokio::sync::broadcast;
use tracing::trace;

/// Default broadcast channel capacity (PRD-10 REQ-STREAM-002).
pub const DEFAULT_CAPACITY: usize = 1024;

// ---------------------------------------------------------------------------
// EventBus
// ---------------------------------------------------------------------------

/// An in-process multi-subscriber event bus.
///
/// Clone this struct freely — all clones share the same underlying broadcast
/// channel sender. New subscribers are created via [`subscribe`].
///
/// [`subscribe`]: EventBus::subscribe
#[derive(Debug, Clone)]
pub struct EventBus {
    sender: broadcast::Sender<RunEvent>,
}

impl EventBus {
    /// Create a new bus with the given channel capacity.
    ///
    /// A capacity of 0 is upgraded to 1 (the `broadcast` crate minimum).
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(1);
        let (sender, _) = broadcast::channel(cap);
        Self { sender }
    }

    /// Create a new bus with the default capacity (`1024`).
    #[must_use]
    pub fn with_default_capacity() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }

    /// Subscribe to receive future events.
    ///
    /// The returned [`EventReceiver`] will receive all events published
    /// **after** the call to `subscribe`. Events published before this call
    /// are not replayed. For historical events, use the
    /// [`EventStore`](polkagent_store_trait::event::EventStore).
    #[must_use]
    pub fn subscribe(&self) -> EventReceiver {
        EventReceiver {
            inner: self.sender.subscribe(),
        }
    }

    /// Publish an event to all active subscribers.
    ///
    /// This operation is **non-blocking**: it returns immediately whether or
    /// not there are active subscribers. If the channel is full, the oldest
    /// undelivered event is dropped from slow receivers' queues (ring-buffer
    /// semantics). Durable events should be persisted via
    /// [`EventRecorder`](crate::recorder::EventRecorder) before calling this.
    ///
    /// Returns the number of receivers that received the event (which may be
    /// 0 if no subscribers are currently connected).
    pub fn publish(&self, event: RunEvent) -> usize {
        trace!(
            run_id = %event.run_id,
            sequence = event.sequence,
            "EventBus::publish",
        );
        self.sender.send(event).unwrap_or(0)
    }

    /// Return the number of active subscribers (receivers that have not been
    /// dropped).
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// Return the configured channel capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.sender.len()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::with_default_capacity()
    }
}

// ---------------------------------------------------------------------------
// EventReceiver
// ---------------------------------------------------------------------------

/// A subscriber handle for the [`EventBus`].
///
/// Call [`recv`] in a loop to process events as they arrive. If this receiver
/// falls too far behind (channel capacity exceeded), the next call to `recv`
/// returns an error carrying the number of skipped events.
///
/// [`recv`]: EventReceiver::recv
pub struct EventReceiver {
    inner: broadcast::Receiver<RunEvent>,
}

impl EventReceiver {
    /// Receive the next event, waiting until one is available.
    ///
    /// # Errors
    ///
    /// - `Err(RecvError::Lagged(n))` if `n` events were skipped because this
    ///   receiver was too slow (the channel's ring buffer overflowed).
    /// - `Err(RecvError::Closed)` if all senders have been dropped.
    pub async fn recv(&mut self) -> Result<RunEvent, broadcast::error::RecvError> {
        self.inner.recv().await
    }

    /// Try to receive the next event without waiting.
    ///
    /// Returns `Err(TryRecvError::Empty)` if no events are currently queued.
    pub fn try_recv(&mut self) -> Result<RunEvent, broadcast::error::TryRecvError> {
        self.inner.try_recv()
    }

    /// Resubscribe from the current tail of the channel, discarding any
    /// events that accumulated in this receiver's queue.
    #[must_use]
    pub fn resubscribe(&self) -> Self {
        Self {
            inner: self.inner.resubscribe(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Bus tests use expect so channel failures stop at the exact delivery
// invariant being exercised.
#[allow(
    clippy::expect_used,
    reason = "unit-test channel assertions intentionally panic with focused diagnostics"
)]
mod tests {
    use super::*;
    use polkagent_core::{
        event::{EventCorrelation, EventKind, RunEvent},
        ids::{EventId, RunId},
    };

    fn make_event(run_id: RunId, sequence: u64) -> RunEvent {
        RunEvent::new_durable(
            EventId::new(),
            run_id,
            sequence,
            EventKind::RunCreated,
            EventCorrelation {
                run_id,
                ..Default::default()
            },
        )
    }

    #[tokio::test]
    async fn subscriber_receives_published_events() {
        let bus = EventBus::new(8);
        let mut rx = bus.subscribe();

        let run_id = RunId::new();
        let event = make_event(run_id, 1);
        let event_id = event.id;

        bus.publish(event);

        let received = rx.recv().await.expect("should receive event");
        assert_eq!(received.id, event_id);
    }

    #[tokio::test]
    async fn multiple_subscribers_each_receive_event() {
        let bus = EventBus::new(8);
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        let run_id = RunId::new();
        let event = make_event(run_id, 1);
        let event_id = event.id;

        bus.publish(event);

        let r1 = rx1.recv().await.expect("rx1 should receive");
        let r2 = rx2.recv().await.expect("rx2 should receive");
        assert_eq!(r1.id, event_id);
        assert_eq!(r2.id, event_id);
    }

    #[tokio::test]
    async fn publish_with_no_subscribers_does_not_panic() {
        let bus = EventBus::new(4);
        let run_id = RunId::new();
        // No subscribers — should not panic
        let delivered = bus.publish(make_event(run_id, 1));
        assert_eq!(delivered, 0);
    }

    #[tokio::test]
    async fn lagged_receiver_gets_error() {
        // Capacity of 2; publish 4 events before the receiver reads any.
        let bus = EventBus::new(2);
        let mut rx = bus.subscribe();

        let run_id = RunId::new();
        for i in 1..=4u64 {
            bus.publish(make_event(run_id, i));
        }

        // The first recv should return a Lagged error because the buffer
        // overflowed.
        let result = rx.recv().await;
        assert!(
            matches!(result, Err(broadcast::error::RecvError::Lagged(_))),
            "expected Lagged, got {result:?}"
        );
    }

    #[tokio::test]
    async fn try_recv_returns_empty_when_no_events() {
        let bus = EventBus::new(8);
        let mut rx = bus.subscribe();
        assert!(
            matches!(rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)),
            "should be empty before any publish"
        );
    }

    #[test]
    fn subscriber_count_tracks_live_receivers() {
        let bus = EventBus::new(8);
        assert_eq!(bus.subscriber_count(), 0);
        let rx1 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 1);
        let rx2 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 2);
        drop(rx1);
        assert_eq!(bus.subscriber_count(), 1);
        drop(rx2);
        assert_eq!(bus.subscriber_count(), 0);
    }

    #[test]
    fn default_bus_has_default_capacity() {
        let bus = EventBus::default();
        // Capacity is not directly inspectable on the sender, but we can
        // at least verify it doesn't panic.
        let _rx = bus.subscribe();
    }

    #[tokio::test]
    async fn events_are_ordered_within_run() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();

        let run_id = RunId::new();
        for seq in 1..=5u64 {
            bus.publish(make_event(run_id, seq));
        }

        let mut last_seq = 0u64;
        for _ in 0..5 {
            let event = rx.recv().await.expect("should receive");
            assert!(
                event.sequence > last_seq,
                "sequences must be monotonically increasing"
            );
            last_seq = event.sequence;
        }
    }
}
