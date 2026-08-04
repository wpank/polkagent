//! PCA C0 outbound lane with single-slot submission and durable queue.
//!
//! The outbound lane implements a per-peer single-slot submission constraint:
//! at most one message may be outstanding (awaiting acknowledgement) per peer at
//! any time. If a new reply arrives while a previous one is still pending, the
//! payloads are merged under a new request ID (superset extension).
//!
//! The lane is backed by a bounded queue and supports persistence via
//! serializable state snapshots.

use std::collections::VecDeque;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::error::PcaError;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for an outbound lane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaneConfig {
    /// Maximum number of messages that may be buffered in the queue.
    pub max_queue_depth: usize,
    /// How long a submitted message may remain unacknowledged before the
    /// lane considers it timed out.
    #[serde(with = "duration_secs")]
    pub submission_timeout: Duration,
}

impl Default for LaneConfig {
    fn default() -> Self {
        Self {
            max_queue_depth: 64,
            submission_timeout: Duration::from_secs(30),
        }
    }
}

// ---------------------------------------------------------------------------
// Slot state
// ---------------------------------------------------------------------------

/// The state of the single outbound submission slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutboundSlot {
    /// No message is currently submitted.
    Empty,
    /// A message has been submitted and is awaiting peer acknowledgement.
    Pending {
        /// The request ID under which this message was submitted.
        request_id: String,
        /// The serialized payload.
        payload: Vec<u8>,
        /// When the message was submitted (milliseconds since UNIX epoch).
        submitted_at_ms: u64,
    },
    /// The peer has acknowledged the most recent submission.
    Acked,
}

// ---------------------------------------------------------------------------
// Queue entry
// ---------------------------------------------------------------------------

/// A message waiting in the outbound queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedOutbound {
    /// Unique identifier for this queued message.
    pub id: String,
    /// The serialized payload.
    pub payload: Vec<u8>,
    /// When the message was enqueued (milliseconds since UNIX epoch).
    pub enqueued_at_ms: u64,
}

// ---------------------------------------------------------------------------
// Persisted state
// ---------------------------------------------------------------------------

/// Serializable snapshot of the outbound lane state, used for persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundLaneState {
    /// The current slot state.
    pub slot: OutboundSlot,
    /// Messages waiting in the queue.
    pub queue: Vec<QueuedOutbound>,
}

// ---------------------------------------------------------------------------
// OutboundLane
// ---------------------------------------------------------------------------

/// A per-peer outbound message lane with single-slot submission semantics.
///
/// At most one message is outstanding (in `Pending` state) at a time. New
/// messages are buffered in a bounded queue until the slot becomes available.
///
/// If a new reply arrives while the slot is `Pending`, the payloads are merged
/// (superset extension) under a fresh request ID.
pub struct OutboundLane {
    config: LaneConfig,
    slot: OutboundSlot,
    queue: VecDeque<QueuedOutbound>,
}

impl OutboundLane {
    /// Create a new outbound lane with the given configuration.
    pub fn new(config: LaneConfig) -> Self {
        Self {
            config,
            slot: OutboundSlot::Empty,
            queue: VecDeque::new(),
        }
    }

    /// Restore an outbound lane from a persisted state snapshot.
    pub fn restore(config: LaneConfig, state: OutboundLaneState) -> Result<Self, PcaError> {
        if state.queue.len() > config.max_queue_depth {
            return Err(PcaError::ChannelError {
                reason: format!(
                    "persisted queue length {} exceeds max_queue_depth {}",
                    state.queue.len(),
                    config.max_queue_depth,
                ),
            });
        }
        Ok(Self {
            config,
            slot: state.slot,
            queue: VecDeque::from(state.queue),
        })
    }

    /// Take a snapshot of the current lane state for persistence.
    pub fn snapshot(&self) -> OutboundLaneState {
        OutboundLaneState {
            slot: self.slot.clone(),
            queue: self.queue.iter().cloned().collect(),
        }
    }

    /// Return a reference to the current lane configuration.
    pub fn config(&self) -> &LaneConfig {
        &self.config
    }

    /// Return a reference to the current slot state.
    pub fn slot(&self) -> &OutboundSlot {
        &self.slot
    }

    /// Return the number of messages waiting in the queue.
    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    /// Return `true` if the slot is `Empty` or `Acked` (i.e. available for
    /// a new submission).
    pub fn slot_is_available(&self) -> bool {
        matches!(self.slot, OutboundSlot::Empty | OutboundSlot::Acked)
    }

    // -----------------------------------------------------------------------
    // Enqueue
    // -----------------------------------------------------------------------

    /// Enqueue a message for outbound delivery.
    ///
    /// If the slot is available (Empty or Acked), the message is submitted
    /// immediately. Otherwise it is buffered in the queue.
    ///
    /// Returns the request ID if the message was submitted, or the queue entry
    /// ID if it was buffered.
    pub fn enqueue(&mut self, payload: Vec<u8>, now_ms: u64) -> Result<String, PcaError> {
        if self.slot_is_available() {
            let request_id = Self::generate_request_id();
            self.slot = OutboundSlot::Pending {
                request_id: request_id.clone(),
                payload,
                submitted_at_ms: now_ms,
            };
            debug!(request_id = %request_id, "outbound message submitted directly");
            Ok(request_id)
        } else {
            self.enqueue_to_buffer(payload, now_ms)
        }
    }

    /// Enqueue a reply while the slot is pending (superset extension).
    ///
    /// If the slot is `Pending`, the existing payload and the new payload are
    /// merged under a fresh request ID. The merge strategy concatenates both
    /// payloads with a newline separator.
    ///
    /// If the slot is not `Pending`, this behaves like a regular `enqueue`.
    pub fn enqueue_reply(&mut self, payload: Vec<u8>, now_ms: u64) -> Result<String, PcaError> {
        match &self.slot {
            OutboundSlot::Pending {
                payload: existing, ..
            } => {
                let merged = Self::merge_payloads(existing, &payload);
                let new_request_id = Self::generate_request_id();
                debug!(
                    new_request_id = %new_request_id,
                    "superset extension: merged pending payload with new reply"
                );
                self.slot = OutboundSlot::Pending {
                    request_id: new_request_id.clone(),
                    payload: merged,
                    submitted_at_ms: now_ms,
                };
                Ok(new_request_id)
            }
            _ => self.enqueue(payload, now_ms),
        }
    }

    // -----------------------------------------------------------------------
    // ACK handling
    // -----------------------------------------------------------------------

    /// Handle a peer acknowledgement for the given request ID.
    ///
    /// If the request ID matches the current pending submission, the slot
    /// transitions to `Acked` and the next queued message (if any) is
    /// promoted to `Pending`.
    ///
    /// Stale request IDs (from superseded submissions) are silently ignored.
    ///
    /// Returns `true` if the ACK was accepted, `false` if it was stale.
    pub fn handle_peer_ack(&mut self, request_id: &str, now_ms: u64) -> bool {
        match &self.slot {
            OutboundSlot::Pending {
                request_id: pending_id,
                ..
            } => {
                if pending_id != request_id {
                    warn!(
                        stale_id = %request_id,
                        current_id = %pending_id,
                        "ignoring stale ACK"
                    );
                    return false;
                }

                debug!(request_id = %request_id, "peer ACK accepted");
                self.slot = OutboundSlot::Acked;
                self.try_promote_next(now_ms);
                true
            }
            _ => {
                warn!(
                    request_id = %request_id,
                    slot = ?self.slot,
                    "ACK received but slot is not Pending"
                );
                false
            }
        }
    }

    // -----------------------------------------------------------------------
    // Timeout
    // -----------------------------------------------------------------------

    /// Check whether the current pending submission has timed out.
    ///
    /// Returns `Some(request_id)` if timed out, `None` otherwise.
    pub fn check_timeout(&self, now_ms: u64) -> Option<String> {
        if let OutboundSlot::Pending {
            request_id,
            submitted_at_ms,
            ..
        } = &self.slot
        {
            let elapsed = Duration::from_millis(now_ms.saturating_sub(*submitted_at_ms));
            if elapsed >= self.config.submission_timeout {
                return Some(request_id.clone());
            }
        }
        None
    }

    /// Force-clear a timed-out submission, moving the slot to `Empty` and
    /// promoting the next queued message if available.
    ///
    /// Returns the payload of the cleared submission, or `None` if the slot
    /// was not in the expected `Pending` state with the given request ID.
    pub fn clear_timeout(&mut self, request_id: &str, now_ms: u64) -> Option<Vec<u8>> {
        match &self.slot {
            OutboundSlot::Pending {
                request_id: pending_id,
                payload,
                ..
            } if pending_id == request_id => {
                let payload = payload.clone();
                warn!(request_id = %request_id, "clearing timed-out submission");
                self.slot = OutboundSlot::Empty;
                self.try_promote_next(now_ms);
                Some(payload)
            }
            _ => None,
        }
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Buffer a message in the queue.
    fn enqueue_to_buffer(&mut self, payload: Vec<u8>, now_ms: u64) -> Result<String, PcaError> {
        if self.queue.len() >= self.config.max_queue_depth {
            return Err(PcaError::ChannelError {
                reason: format!(
                    "outbound queue full ({} / {})",
                    self.queue.len(),
                    self.config.max_queue_depth,
                ),
            });
        }

        let id = Self::generate_request_id();
        self.queue.push_back(QueuedOutbound {
            id: id.clone(),
            payload,
            enqueued_at_ms: now_ms,
        });

        debug!(id = %id, queue_len = self.queue.len(), "message buffered in outbound queue");
        Ok(id)
    }

    /// Promote the next queued message to the submission slot.
    fn try_promote_next(&mut self, now_ms: u64) {
        if let Some(next) = self.queue.pop_front() {
            let request_id = Self::generate_request_id();
            debug!(
                request_id = %request_id,
                queued_id = %next.id,
                "promoting queued message to submission slot"
            );
            self.slot = OutboundSlot::Pending {
                request_id,
                payload: next.payload,
                submitted_at_ms: now_ms,
            };
        }
    }

    /// Generate a unique request ID.
    fn generate_request_id() -> String {
        format!("req-{}", Uuid::now_v7())
    }

    /// Merge two payloads using newline-delimited concatenation.
    fn merge_payloads(existing: &[u8], new: &[u8]) -> Vec<u8> {
        let mut merged = Vec::with_capacity(existing.len() + 1 + new.len());
        merged.extend_from_slice(existing);
        merged.push(b'\n');
        merged.extend_from_slice(new);
        merged
    }
}

// ---------------------------------------------------------------------------
// Serde helper
// ---------------------------------------------------------------------------

mod duration_secs {
    use std::time::Duration;

    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(duration.as_secs())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = u64::deserialize(deserializer)?;
        Ok(Duration::from_secs(secs))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> LaneConfig {
        LaneConfig {
            max_queue_depth: 4,
            submission_timeout: Duration::from_secs(5),
        }
    }

    fn now_ms() -> u64 {
        1_000_000
    }

    // -- Construction and defaults --

    #[test]
    fn new_lane_starts_empty() {
        let lane = OutboundLane::new(default_config());
        assert_eq!(lane.slot(), &OutboundSlot::Empty);
        assert_eq!(lane.queue_len(), 0);
        assert!(lane.slot_is_available());
    }

    #[test]
    fn default_lane_config_has_sane_values() {
        let config = LaneConfig::default();
        assert_eq!(config.max_queue_depth, 64);
        assert_eq!(config.submission_timeout, Duration::from_secs(30));
    }

    // -- Direct submission --

    #[test]
    fn enqueue_submits_directly_when_slot_empty() {
        let mut lane = OutboundLane::new(default_config());
        let id = lane.enqueue(b"hello".to_vec(), now_ms()).expect("enqueue");
        assert!(id.starts_with("req-"));
        assert!(!lane.slot_is_available());
        assert_eq!(lane.queue_len(), 0);

        match lane.slot() {
            OutboundSlot::Pending {
                request_id,
                payload,
                submitted_at_ms,
            } => {
                assert_eq!(request_id, &id);
                assert_eq!(payload, b"hello");
                assert_eq!(*submitted_at_ms, now_ms());
            }
            other => panic!("expected Pending, got {other:?}"),
        }
    }

    #[test]
    fn enqueue_submits_directly_when_slot_acked() {
        let mut lane = OutboundLane::new(default_config());
        let id1 = lane.enqueue(b"msg1".to_vec(), now_ms()).expect("enqueue");
        lane.handle_peer_ack(&id1, now_ms());
        // Queue is empty, so slot transitions to Acked without promotion.
        // The next enqueue should submit directly.
        // Actually, handle_peer_ack transitions to Acked, then tries to promote.
        // Since queue is empty, slot stays Acked.
        assert!(lane.slot_is_available());

        let id2 = lane.enqueue(b"msg2".to_vec(), now_ms()).expect("enqueue");
        assert!(!lane.slot_is_available());
        match lane.slot() {
            OutboundSlot::Pending { request_id, .. } => {
                assert_eq!(request_id, &id2);
            }
            other => panic!("expected Pending, got {other:?}"),
        }
    }

    // -- Queue buffering --

    #[test]
    fn enqueue_buffers_when_slot_pending() {
        let mut lane = OutboundLane::new(default_config());
        let _id1 = lane.enqueue(b"msg1".to_vec(), now_ms()).expect("first");
        let id2 = lane.enqueue(b"msg2".to_vec(), now_ms()).expect("second");
        assert!(id2.starts_with("req-"));
        assert_eq!(lane.queue_len(), 1);
    }

    #[test]
    fn queue_depth_enforced() {
        let config = LaneConfig {
            max_queue_depth: 2,
            ..default_config()
        };
        let mut lane = OutboundLane::new(config);
        lane.enqueue(b"slot".to_vec(), now_ms()).expect("slot");
        lane.enqueue(b"q1".to_vec(), now_ms()).expect("q1");
        lane.enqueue(b"q2".to_vec(), now_ms()).expect("q2");

        let result = lane.enqueue(b"q3".to_vec(), now_ms());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("outbound queue full"));
    }

    // -- Superset extension --

    #[test]
    fn enqueue_reply_merges_when_pending() {
        let mut lane = OutboundLane::new(default_config());
        let id1 = lane.enqueue(b"original".to_vec(), now_ms()).expect("first");
        let id2 = lane
            .enqueue_reply(b"updated".to_vec(), now_ms() + 100)
            .expect("reply");

        // Should have generated a new request_id.
        assert_ne!(id1, id2);
        assert_eq!(lane.queue_len(), 0);

        match lane.slot() {
            OutboundSlot::Pending {
                request_id,
                payload,
                submitted_at_ms,
            } => {
                assert_eq!(request_id, &id2);
                assert_eq!(payload, b"original\nupdated");
                assert_eq!(*submitted_at_ms, now_ms() + 100);
            }
            other => panic!("expected Pending, got {other:?}"),
        }
    }

    #[test]
    fn enqueue_reply_falls_through_when_slot_empty() {
        let mut lane = OutboundLane::new(default_config());
        let id = lane
            .enqueue_reply(b"reply".to_vec(), now_ms())
            .expect("reply on empty");

        match lane.slot() {
            OutboundSlot::Pending {
                request_id,
                payload,
                ..
            } => {
                assert_eq!(request_id, &id);
                assert_eq!(payload, b"reply");
            }
            other => panic!("expected Pending, got {other:?}"),
        }
    }

    #[test]
    fn multiple_superset_extensions_accumulate() {
        let mut lane = OutboundLane::new(default_config());
        lane.enqueue(b"a".to_vec(), now_ms()).expect("first");
        lane.enqueue_reply(b"b".to_vec(), now_ms()).expect("second");
        let id3 = lane.enqueue_reply(b"c".to_vec(), now_ms()).expect("third");

        match lane.slot() {
            OutboundSlot::Pending {
                request_id,
                payload,
                ..
            } => {
                assert_eq!(request_id, &id3);
                assert_eq!(payload, b"a\nb\nc");
            }
            other => panic!("expected Pending, got {other:?}"),
        }
    }

    // -- ACK handling --

    #[test]
    fn handle_peer_ack_accepts_matching_request_id() {
        let mut lane = OutboundLane::new(default_config());
        let id = lane.enqueue(b"msg".to_vec(), now_ms()).expect("enqueue");
        let accepted = lane.handle_peer_ack(&id, now_ms());
        assert!(accepted);
        assert!(lane.slot_is_available());
    }

    #[test]
    fn handle_peer_ack_rejects_stale_request_id() {
        let mut lane = OutboundLane::new(default_config());
        let _id = lane.enqueue(b"msg".to_vec(), now_ms()).expect("enqueue");
        let accepted = lane.handle_peer_ack("req-stale-id", now_ms());
        assert!(!accepted);
        assert!(!lane.slot_is_available());
    }

    #[test]
    fn handle_peer_ack_rejects_when_slot_empty() {
        let mut lane = OutboundLane::new(default_config());
        let accepted = lane.handle_peer_ack("req-whatever", now_ms());
        assert!(!accepted);
    }

    #[test]
    fn handle_peer_ack_rejects_when_slot_acked() {
        let mut lane = OutboundLane::new(default_config());
        let id = lane.enqueue(b"msg".to_vec(), now_ms()).expect("enqueue");
        lane.handle_peer_ack(&id, now_ms());
        // Slot is now Acked, another ACK should fail.
        let accepted = lane.handle_peer_ack(&id, now_ms());
        assert!(!accepted);
    }

    #[test]
    fn ack_promotes_next_queued_message() {
        let mut lane = OutboundLane::new(default_config());
        let id1 = lane.enqueue(b"msg1".to_vec(), now_ms()).expect("first");
        lane.enqueue(b"msg2".to_vec(), now_ms()).expect("second");
        assert_eq!(lane.queue_len(), 1);

        lane.handle_peer_ack(&id1, now_ms() + 1000);
        // The queued message should have been promoted.
        assert_eq!(lane.queue_len(), 0);
        assert!(!lane.slot_is_available());

        match lane.slot() {
            OutboundSlot::Pending { payload, .. } => {
                assert_eq!(payload, b"msg2");
            }
            other => panic!("expected Pending with msg2, got {other:?}"),
        }
    }

    #[test]
    fn ack_after_superset_extension_uses_new_id() {
        let mut lane = OutboundLane::new(default_config());
        let id1 = lane.enqueue(b"orig".to_vec(), now_ms()).expect("first");
        let id2 = lane
            .enqueue_reply(b"ext".to_vec(), now_ms())
            .expect("reply");

        // Old ID should be stale.
        assert!(!lane.handle_peer_ack(&id1, now_ms()));
        // New ID should work.
        assert!(lane.handle_peer_ack(&id2, now_ms()));
    }

    // -- Timeout --

    #[test]
    fn check_timeout_returns_none_when_not_timed_out() {
        let mut lane = OutboundLane::new(default_config());
        lane.enqueue(b"msg".to_vec(), now_ms()).expect("enqueue");
        assert!(lane.check_timeout(now_ms() + 1000).is_none());
    }

    #[test]
    fn check_timeout_returns_request_id_when_timed_out() {
        let mut lane = OutboundLane::new(default_config());
        let id = lane.enqueue(b"msg".to_vec(), now_ms()).expect("enqueue");
        // Timeout is 5 seconds.
        let result = lane.check_timeout(now_ms() + 6000);
        assert_eq!(result, Some(id));
    }

    #[test]
    fn check_timeout_returns_none_when_slot_empty() {
        let lane = OutboundLane::new(default_config());
        assert!(lane.check_timeout(now_ms() + 100_000).is_none());
    }

    #[test]
    fn clear_timeout_removes_pending_and_promotes() {
        let mut lane = OutboundLane::new(default_config());
        let id1 = lane.enqueue(b"msg1".to_vec(), now_ms()).expect("first");
        lane.enqueue(b"msg2".to_vec(), now_ms()).expect("second");

        let cleared = lane.clear_timeout(&id1, now_ms() + 6000);
        assert_eq!(cleared, Some(b"msg1".to_vec()));
        assert!(!lane.slot_is_available());
        assert_eq!(lane.queue_len(), 0);

        match lane.slot() {
            OutboundSlot::Pending { payload, .. } => {
                assert_eq!(payload, b"msg2");
            }
            other => panic!("expected promoted msg2, got {other:?}"),
        }
    }

    #[test]
    fn clear_timeout_returns_none_for_wrong_id() {
        let mut lane = OutboundLane::new(default_config());
        lane.enqueue(b"msg".to_vec(), now_ms()).expect("enqueue");
        let cleared = lane.clear_timeout("req-wrong", now_ms());
        assert!(cleared.is_none());
    }

    #[test]
    fn clear_timeout_returns_none_when_empty() {
        let mut lane = OutboundLane::new(default_config());
        let cleared = lane.clear_timeout("req-whatever", now_ms());
        assert!(cleared.is_none());
    }

    // -- Persistence --

    #[test]
    fn snapshot_captures_slot_and_queue() {
        let mut lane = OutboundLane::new(default_config());
        lane.enqueue(b"slot-msg".to_vec(), now_ms()).expect("slot");
        lane.enqueue(b"q-msg".to_vec(), now_ms()).expect("queue");

        let state = lane.snapshot();
        assert!(matches!(state.slot, OutboundSlot::Pending { .. }));
        assert_eq!(state.queue.len(), 1);
        assert_eq!(state.queue[0].payload, b"q-msg");
    }

    #[test]
    fn snapshot_roundtrips_through_json() {
        let mut lane = OutboundLane::new(default_config());
        lane.enqueue(b"msg".to_vec(), now_ms()).expect("enqueue");

        let state = lane.snapshot();
        let json = serde_json::to_string(&state).expect("serialize");
        let back: OutboundLaneState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.slot, state.slot);
        assert_eq!(back.queue.len(), state.queue.len());
    }

    #[test]
    fn restore_reconstructs_lane_state() {
        let mut lane = OutboundLane::new(default_config());
        let id = lane.enqueue(b"pending".to_vec(), now_ms()).expect("slot");
        lane.enqueue(b"queued".to_vec(), now_ms()).expect("queue");

        let state = lane.snapshot();
        let restored = OutboundLane::restore(default_config(), state).expect("restore");

        assert!(!restored.slot_is_available());
        assert_eq!(restored.queue_len(), 1);

        match restored.slot() {
            OutboundSlot::Pending {
                request_id,
                payload,
                ..
            } => {
                assert_eq!(request_id, &id);
                assert_eq!(payload, b"pending");
            }
            other => panic!("expected Pending, got {other:?}"),
        }
    }

    #[test]
    fn restore_rejects_oversized_queue() {
        let state = OutboundLaneState {
            slot: OutboundSlot::Empty,
            queue: (0..5)
                .map(|i| QueuedOutbound {
                    id: format!("q-{i}"),
                    payload: vec![i as u8],
                    enqueued_at_ms: now_ms(),
                })
                .collect(),
        };
        let config = LaneConfig {
            max_queue_depth: 2,
            ..default_config()
        };
        let result = OutboundLane::restore(config, state);
        assert!(result.is_err());
    }

    #[test]
    fn restore_empty_state() {
        let state = OutboundLaneState {
            slot: OutboundSlot::Empty,
            queue: Vec::new(),
        };
        let lane = OutboundLane::restore(default_config(), state).expect("restore");
        assert!(lane.slot_is_available());
        assert_eq!(lane.queue_len(), 0);
    }

    // -- Config serialization --

    #[test]
    fn lane_config_roundtrips_through_json() {
        let config = LaneConfig {
            max_queue_depth: 32,
            submission_timeout: Duration::from_secs(10),
        };
        let json = serde_json::to_string(&config).expect("serialize");
        let back: LaneConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.max_queue_depth, 32);
        assert_eq!(back.submission_timeout, Duration::from_secs(10));
    }

    // -- Full lifecycle --

    #[test]
    fn full_lifecycle_enqueue_ack_promote() {
        let mut lane = OutboundLane::new(LaneConfig {
            max_queue_depth: 8,
            submission_timeout: Duration::from_secs(30),
        });

        // Submit 3 messages: first goes to slot, rest to queue.
        let id1 = lane.enqueue(b"m1".to_vec(), 1000).expect("m1");
        lane.enqueue(b"m2".to_vec(), 1001).expect("m2");
        lane.enqueue(b"m3".to_vec(), 1002).expect("m3");
        assert_eq!(lane.queue_len(), 2);

        // ACK first -> promotes second.
        assert!(lane.handle_peer_ack(&id1, 2000));
        assert_eq!(lane.queue_len(), 1);
        let id2 = match lane.slot() {
            OutboundSlot::Pending {
                request_id,
                payload,
                ..
            } => {
                assert_eq!(payload, b"m2");
                request_id.clone()
            }
            other => panic!("expected m2 Pending, got {other:?}"),
        };

        // ACK second -> promotes third.
        assert!(lane.handle_peer_ack(&id2, 3000));
        assert_eq!(lane.queue_len(), 0);
        let id3 = match lane.slot() {
            OutboundSlot::Pending {
                request_id,
                payload,
                ..
            } => {
                assert_eq!(payload, b"m3");
                request_id.clone()
            }
            other => panic!("expected m3 Pending, got {other:?}"),
        };

        // ACK third -> slot becomes Acked, no more queued.
        assert!(lane.handle_peer_ack(&id3, 4000));
        assert!(lane.slot_is_available());
        assert_eq!(lane.queue_len(), 0);
    }

    #[test]
    fn superset_then_ack_lifecycle() {
        let mut lane = OutboundLane::new(default_config());

        let _id1 = lane.enqueue(b"v1".to_vec(), 1000).expect("v1");
        let id2 = lane.enqueue_reply(b"v2".to_vec(), 1100).expect("v2 merge");
        let id3 = lane.enqueue_reply(b"v3".to_vec(), 1200).expect("v3 merge");

        // Only the latest ID is valid.
        assert!(!lane.handle_peer_ack(&id2, 2000));
        assert!(lane.handle_peer_ack(&id3, 2000));
        assert!(lane.slot_is_available());
    }

    #[test]
    fn timeout_and_retry_lifecycle() {
        let config = LaneConfig {
            max_queue_depth: 4,
            submission_timeout: Duration::from_secs(2),
        };
        let mut lane = OutboundLane::new(config);

        let id1 = lane.enqueue(b"msg".to_vec(), 1000).expect("enqueue");

        // Not timed out yet.
        assert!(lane.check_timeout(2000).is_none());

        // Timed out.
        assert_eq!(lane.check_timeout(4000), Some(id1.clone()));

        // Clear the timeout and re-enqueue.
        let cleared = lane.clear_timeout(&id1, 4000);
        assert_eq!(cleared, Some(b"msg".to_vec()));
        assert!(lane.slot_is_available());

        // Re-enqueue the message.
        let id2 = lane.enqueue(b"msg".to_vec(), 4000).expect("retry");
        assert_ne!(id1, id2);
        assert!(!lane.slot_is_available());
    }

    #[test]
    fn snapshot_restore_then_continue() {
        let mut lane = OutboundLane::new(default_config());
        let id = lane.enqueue(b"persist-me".to_vec(), 1000).expect("enqueue");
        lane.enqueue(b"queued".to_vec(), 1001).expect("queue");

        // Snapshot and restore.
        let state = lane.snapshot();
        let json = serde_json::to_string(&state).expect("ser");
        let state2: OutboundLaneState = serde_json::from_str(&json).expect("de");
        let mut restored = OutboundLane::restore(default_config(), state2).expect("restore");

        // Continue from restored state: ACK should work.
        assert!(restored.handle_peer_ack(&id, 2000));
        assert_eq!(restored.queue_len(), 0);
        match restored.slot() {
            OutboundSlot::Pending { payload, .. } => {
                assert_eq!(payload, b"queued");
            }
            other => panic!("expected promoted queued, got {other:?}"),
        }
    }

    #[test]
    fn request_ids_are_unique() {
        let mut lane = OutboundLane::new(LaneConfig {
            max_queue_depth: 100,
            ..default_config()
        });
        let mut ids = std::collections::HashSet::new();
        // First goes to slot.
        let id = lane.enqueue(b"s".to_vec(), now_ms()).expect("slot");
        assert!(ids.insert(id));
        // Rest go to queue.
        for i in 0..20u32 {
            let id = lane
                .enqueue(i.to_le_bytes().to_vec(), now_ms())
                .expect("enqueue");
            assert!(ids.insert(id), "duplicate request id");
        }
    }

    #[test]
    fn outbound_slot_equality() {
        let a = OutboundSlot::Empty;
        let b = OutboundSlot::Empty;
        assert_eq!(a, b);

        let c = OutboundSlot::Acked;
        assert_ne!(a, c);

        let d = OutboundSlot::Pending {
            request_id: "r1".into(),
            payload: vec![1],
            submitted_at_ms: 100,
        };
        let e = OutboundSlot::Pending {
            request_id: "r1".into(),
            payload: vec![1],
            submitted_at_ms: 100,
        };
        assert_eq!(d, e);

        let f = OutboundSlot::Pending {
            request_id: "r2".into(),
            payload: vec![1],
            submitted_at_ms: 100,
        };
        assert_ne!(d, f);
    }
}
