//! Reliable ordered message channel over encrypted sessions.
//!
//! [`MessageChannel`] provides an in-memory message queue that guarantees
//! ordered delivery with at-least-once semantics. Messages are held in the
//! queue until explicitly acknowledged, at which point they are removed.
//! Unacknowledged messages remain available for re-delivery.

use std::collections::{BTreeMap, VecDeque};

use parking_lot::Mutex;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::error::PcaError;
use crate::session::{EncryptedEnvelope, Session};

/// A message in the delivery queue.
#[derive(Debug, Clone)]
pub struct QueuedMessage {
    /// Unique delivery identifier for acknowledgement.
    pub delivery_id: String,
    /// The plaintext payload (already decrypted).
    pub payload: Vec<u8>,
    /// Monotonic sequence number for ordering.
    pub sequence: u64,
    /// Whether this message has been delivered to the consumer.
    pub delivered: bool,
}

/// A reliable ordered message channel backed by an in-memory queue.
///
/// Messages are enqueued (typically after decryption) and dequeued in order.
/// Each message receives a unique delivery ID. Messages remain in the pending
/// set until acknowledged.
pub struct MessageChannel {
    /// Messages waiting to be delivered to the consumer.
    incoming: Mutex<VecDeque<QueuedMessage>>,
    /// Messages that have been delivered but not yet acknowledged.
    /// Keyed by `delivery_id` for efficient lookup.
    pending_ack: Mutex<BTreeMap<String, QueuedMessage>>,
    /// Next sequence number for incoming messages.
    next_sequence: Mutex<u64>,
    /// Maximum number of unacknowledged messages allowed.
    max_in_flight: usize,
    /// Notification channel for new messages.
    notify: tokio::sync::Notify,
    /// Whether the channel is closed.
    closed: std::sync::atomic::AtomicBool,
}

impl MessageChannel {
    /// Create a new message channel with the given capacity.
    pub fn new(max_in_flight: usize) -> Self {
        Self {
            incoming: Mutex::new(VecDeque::new()),
            pending_ack: Mutex::new(BTreeMap::new()),
            next_sequence: Mutex::new(0),
            max_in_flight,
            notify: tokio::sync::Notify::new(),
            closed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Enqueue a raw plaintext payload into the channel.
    ///
    /// Returns the delivery ID assigned to this message.
    pub fn enqueue(&self, payload: Vec<u8>) -> Result<String, PcaError> {
        if self.is_closed() {
            return Err(PcaError::ChannelError {
                reason: "channel is closed".into(),
            });
        }

        let pending_count = self.pending_ack.lock().len();
        let incoming_count = self.incoming.lock().len();
        if pending_count + incoming_count >= self.max_in_flight {
            return Err(PcaError::ChannelError {
                reason: format!(
                    "max in-flight limit reached ({} pending + {} queued >= {})",
                    pending_count, incoming_count, self.max_in_flight
                ),
            });
        }

        let delivery_id = format!("pca-{}", Uuid::now_v7());
        let sequence = {
            let mut seq = self.next_sequence.lock();
            let s = *seq;
            *seq += 1;
            s
        };

        let msg = QueuedMessage {
            delivery_id: delivery_id.clone(),
            payload,
            sequence,
            delivered: false,
        };

        self.incoming.lock().push_back(msg);
        self.notify.notify_one();

        debug!(delivery_id = %delivery_id, sequence, "message enqueued");

        Ok(delivery_id)
    }

    /// Enqueue an encrypted envelope, decrypting it via the given session.
    pub fn enqueue_encrypted(
        &self,
        session: &mut Session,
        envelope: &EncryptedEnvelope,
    ) -> Result<String, PcaError> {
        let plaintext = session.decrypt(envelope)?;
        self.enqueue(plaintext)
    }

    /// Dequeue the next available message.
    ///
    /// Returns `None` if no messages are available. The message is moved
    /// to the pending-ack set and will be re-delivered if not acknowledged.
    pub fn try_dequeue(&self) -> Option<QueuedMessage> {
        let mut incoming = self.incoming.lock();
        if let Some(mut msg) = incoming.pop_front() {
            msg.delivered = true;
            let delivery_id = msg.delivery_id.clone();
            self.pending_ack.lock().insert(delivery_id, msg.clone());
            Some(msg)
        } else {
            None
        }
    }

    /// Wait for the next message to become available.
    ///
    /// Blocks until a message is enqueued or the channel is closed.
    pub async fn dequeue(&self) -> Result<QueuedMessage, PcaError> {
        loop {
            if self.is_closed() {
                return Err(PcaError::Shutdown);
            }

            if let Some(msg) = self.try_dequeue() {
                return Ok(msg);
            }

            // Wait for notification of a new message.
            self.notify.notified().await;
        }
    }

    /// Acknowledge that a message has been processed.
    ///
    /// Removes the message from the pending-ack set. Returns an error if
    /// the delivery ID is not found.
    pub fn acknowledge(&self, delivery_id: &str) -> Result<(), PcaError> {
        let mut pending = self.pending_ack.lock();
        if pending.remove(delivery_id).is_some() {
            debug!(delivery_id = %delivery_id, "message acknowledged");
            Ok(())
        } else {
            warn!(delivery_id = %delivery_id, "ack for unknown delivery id");
            Err(PcaError::ChannelError {
                reason: format!("unknown delivery id: {delivery_id}"),
            })
        }
    }

    /// Return all unacknowledged messages to the incoming queue for re-delivery.
    ///
    /// Messages are re-queued in their original sequence order.
    pub fn redeliver_unacked(&self) -> usize {
        let mut pending = self.pending_ack.lock();
        let mut incoming = self.incoming.lock();

        let count = pending.len();
        if count == 0 {
            return 0;
        }

        // Take all pending messages (BTreeMap iterates in key order).
        let taken = std::mem::take(&mut *pending);
        let mut msgs: Vec<QueuedMessage> = taken.into_values().collect();
        msgs.sort_by_key(|m| m.sequence);

        // Push in reverse order so that after all push_front calls,
        // the lowest-sequence message ends up at the front of the queue.
        for mut msg in msgs.into_iter().rev() {
            msg.delivered = false;
            incoming.push_front(msg);
        }

        if count > 0 {
            self.notify.notify_one();
        }

        debug!(count, "redelivered unacked messages");
        count
    }

    /// Return the number of messages waiting to be delivered.
    pub fn queued_count(&self) -> usize {
        self.incoming.lock().len()
    }

    /// Return the number of messages pending acknowledgement.
    pub fn pending_ack_count(&self) -> usize {
        self.pending_ack.lock().len()
    }

    /// Close the channel, preventing further enqueues and waking any waiters.
    pub fn close(&self) {
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Check if the channel is closed.
    pub fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "channel tests intentionally fail fast when fixture operations violate expectations"
)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_and_dequeue() {
        let channel = MessageChannel::new(100);
        let id = channel.enqueue(b"hello".to_vec()).expect("enqueue");
        assert!(!id.is_empty());

        let msg = channel.try_dequeue().expect("dequeue");
        assert_eq!(msg.payload, b"hello");
        assert!(msg.delivered);
    }

    #[test]
    fn fifo_ordering() {
        let channel = MessageChannel::new(100);
        channel.enqueue(b"first".to_vec()).expect("enqueue");
        channel.enqueue(b"second".to_vec()).expect("enqueue");
        channel.enqueue(b"third".to_vec()).expect("enqueue");

        let m1 = channel.try_dequeue().expect("dequeue");
        let m2 = channel.try_dequeue().expect("dequeue");
        let m3 = channel.try_dequeue().expect("dequeue");

        assert_eq!(m1.payload, b"first");
        assert_eq!(m2.payload, b"second");
        assert_eq!(m3.payload, b"third");
        assert!(m1.sequence < m2.sequence);
        assert!(m2.sequence < m3.sequence);
    }

    #[test]
    fn try_dequeue_returns_none_when_empty() {
        let channel = MessageChannel::new(100);
        assert!(channel.try_dequeue().is_none());
    }

    #[test]
    fn acknowledge_removes_from_pending() {
        let channel = MessageChannel::new(100);
        channel.enqueue(b"test".to_vec()).expect("enqueue");

        let msg = channel.try_dequeue().expect("dequeue");
        assert_eq!(channel.pending_ack_count(), 1);

        channel.acknowledge(&msg.delivery_id).expect("ack");
        assert_eq!(channel.pending_ack_count(), 0);
    }

    #[test]
    fn ack_unknown_delivery_id_fails() {
        let channel = MessageChannel::new(100);
        let result = channel.acknowledge("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn max_in_flight_enforced() {
        let channel = MessageChannel::new(2);
        channel.enqueue(b"a".to_vec()).expect("enqueue 1");
        channel.enqueue(b"b".to_vec()).expect("enqueue 2");

        let result = channel.enqueue(b"c".to_vec());
        assert!(result.is_err());
    }

    #[test]
    fn ack_frees_capacity() {
        let channel = MessageChannel::new(2);
        channel.enqueue(b"a".to_vec()).expect("enqueue 1");
        channel.enqueue(b"b".to_vec()).expect("enqueue 2");

        // Dequeue and ack one to free capacity.
        let msg = channel.try_dequeue().expect("dequeue");
        channel.acknowledge(&msg.delivery_id).expect("ack");

        // Now we should be able to enqueue again.
        channel.enqueue(b"c".to_vec()).expect("enqueue 3");
    }

    #[test]
    fn redeliver_unacked_returns_messages() {
        let channel = MessageChannel::new(100);
        channel.enqueue(b"msg1".to_vec()).expect("enqueue");
        channel.enqueue(b"msg2".to_vec()).expect("enqueue");

        // Dequeue both.
        let _m1 = channel.try_dequeue().expect("dequeue");
        let _m2 = channel.try_dequeue().expect("dequeue");
        assert_eq!(channel.queued_count(), 0);
        assert_eq!(channel.pending_ack_count(), 2);

        // Redeliver.
        let count = channel.redeliver_unacked();
        assert_eq!(count, 2);
        assert_eq!(channel.queued_count(), 2);
        assert_eq!(channel.pending_ack_count(), 0);

        // Messages should be in the original order.
        let r1 = channel.try_dequeue().expect("dequeue");
        let r2 = channel.try_dequeue().expect("dequeue");
        assert!(r1.sequence < r2.sequence);
    }

    #[test]
    fn closed_channel_rejects_enqueue() {
        let channel = MessageChannel::new(100);
        channel.close();
        let result = channel.enqueue(b"test".to_vec());
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn dequeue_wakes_on_enqueue() {
        let channel = std::sync::Arc::new(MessageChannel::new(100));
        let ch = channel.clone();

        let handle = tokio::spawn(async move { ch.dequeue().await.expect("dequeue") });

        // Give the dequeue task time to start waiting.
        tokio::task::yield_now().await;

        channel.enqueue(b"wakeup".to_vec()).expect("enqueue");

        let msg = handle.await.expect("join");
        assert_eq!(msg.payload, b"wakeup");
    }

    #[tokio::test]
    async fn dequeue_returns_error_on_close() {
        let channel = std::sync::Arc::new(MessageChannel::new(100));
        let ch = channel.clone();

        let handle = tokio::spawn(async move { ch.dequeue().await });

        tokio::task::yield_now().await;

        channel.close();

        let result = handle.await.expect("join");
        assert!(result.is_err());
    }

    #[test]
    fn delivery_ids_are_unique() {
        let channel = MessageChannel::new(100);
        let mut ids = std::collections::HashSet::new();
        for _ in 0..50 {
            let id = channel.enqueue(b"data".to_vec()).expect("enqueue");
            assert!(ids.insert(id), "duplicate delivery id");
        }
    }

    #[test]
    fn sequence_numbers_are_monotonic() {
        let channel = MessageChannel::new(100);
        for _ in 0..10 {
            channel.enqueue(b"data".to_vec()).expect("enqueue");
        }

        let mut prev_seq = None;
        while let Some(msg) = channel.try_dequeue() {
            if let Some(prev) = prev_seq {
                assert!(msg.sequence > prev);
            }
            prev_seq = Some(msg.sequence);
        }
    }

    #[test]
    fn queued_and_pending_counts() {
        let channel = MessageChannel::new(100);
        assert_eq!(channel.queued_count(), 0);
        assert_eq!(channel.pending_ack_count(), 0);

        channel.enqueue(b"a".to_vec()).expect("enqueue");
        channel.enqueue(b"b".to_vec()).expect("enqueue");
        assert_eq!(channel.queued_count(), 2);

        let msg = channel.try_dequeue().expect("dequeue");
        assert_eq!(channel.queued_count(), 1);
        assert_eq!(channel.pending_ack_count(), 1);

        channel.acknowledge(&msg.delivery_id).expect("ack");
        assert_eq!(channel.pending_ack_count(), 0);
    }
}
