//! Durable outbox with at-least-once delivery and FIFO partitioned ordering.
//!
//! # Design
//!
//! [`DurableOutbox`] uses an in-memory store (two `HashMap`s) — a `SQLite`
//! backend will be slotted in without changing the public API. The core
//! invariants are:
//!
//! * **Durability** — a message is written before `enqueue` returns; it
//!   survives any subsequent panic within the same process (`SQLite`: across
//!   restarts).
//! * **At-least-once** — a message stays in the queue until a consumer calls
//!   `acknowledge`. If the lease expires before that the message becomes
//!   reclaimable again.
//! * **FIFO within a partition** — `claim_next` always returns the
//!   lowest-sequence unclaimed (or lease-expired) item within the winning
//!   partition.
//! * **Lease-based claim** — only the consumer holding the current lease may
//!   acknowledge. Other consumers requesting the same item get `None` until the
//!   lease expires.
//! * **Dead-lettering** — when `attempt_count` exceeds `max_retries` the item
//!   is moved to the dead-letter set and removed from the live queue.
//!
//! # Concurrency
//!
//! The outbox is protected by a `tokio::sync::Mutex`. All async methods hold
//! the lock only for the duration of the in-memory operation (sub-microsecond),
//! so lock contention is not a concern at the scale where an in-memory store
//! is appropriate.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Duration, Utc};
use thiserror::Error;
use tracing::{debug, warn};

use crate::message::{OutboxId, OutboxItem, OutboxMessage};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by [`DurableOutbox`] operations.
#[derive(Debug, Error)]
pub enum OutboxError {
    /// No item with the given [`OutboxId`] exists in the live queue.
    #[error("outbox item not found: {0}")]
    NotFound(OutboxId),

    /// The caller is not the consumer that currently holds the lease.
    #[error("consumer '{caller}' does not hold the lease for item {id}; lease held by '{holder}'")]
    LeaseConflict {
        id: OutboxId,
        caller: String,
        holder: String,
    },

    /// The item was dead-lettered and can no longer be acknowledged or nacked.
    #[error("item {0} is dead-lettered and cannot be modified")]
    DeadLettered(OutboxId),

    /// The caller-supplied consumer identifier was empty.
    #[error("consumer_id must not be empty")]
    EmptyConsumerId,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for [`DurableOutbox`].
#[derive(Debug, Clone)]
pub struct OutboxConfig {
    /// How long a claimed item remains reserved for its consumer.
    ///
    /// If the consumer does not call `acknowledge` or `nack` within this
    /// window the item becomes reclaimable by any consumer.
    pub lease_duration: Duration,
}

impl Default for OutboxConfig {
    fn default() -> Self {
        Self {
            lease_duration: Duration::seconds(30),
        }
    }
}

// ---------------------------------------------------------------------------
// Internal partition state
// ---------------------------------------------------------------------------

/// Per-partition ordered queue (sequence → [`OutboxId`]).
type PartitionQueue = BTreeMap<u64, OutboxId>;

// ---------------------------------------------------------------------------
// DurableOutbox
// ---------------------------------------------------------------------------

/// An in-memory durable outbox.
///
/// Wrap in `Arc<tokio::sync::Mutex<DurableOutbox>>` for shared async access.
pub struct DurableOutbox {
    config: OutboxConfig,
    /// Live items indexed by their [`OutboxId`].
    items: HashMap<OutboxId, OutboxItem>,
    /// Partition → ordered queue of live item ids (by sequence).
    partitions: HashMap<String, PartitionQueue>,
    /// Next sequence number per partition.
    partition_seq: HashMap<String, u64>,
    /// Dead-lettered items (removed from `items` and `partitions`).
    dead_letter: HashMap<OutboxId, OutboxItem>,
}

impl DurableOutbox {
    /// Create a new outbox with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(OutboxConfig::default())
    }

    /// Create a new outbox with explicit configuration.
    #[must_use]
    pub fn with_config(config: OutboxConfig) -> Self {
        Self {
            config,
            items: HashMap::new(),
            partitions: HashMap::new(),
            partition_seq: HashMap::new(),
            dead_letter: HashMap::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Producer API
    // -----------------------------------------------------------------------

    /// Persist a message in the outbox and return its assigned [`OutboxId`].
    ///
    /// The message is available for claiming immediately after this call
    /// returns. The caller is responsible for ensuring the [`OutboxMessage`]
    /// fields are well-formed.
    ///
    /// # Errors
    ///
    /// Currently infallible for the in-memory backend; errors will be added
    /// when the `SQLite` backend is integrated.
    pub fn enqueue(&mut self, message: OutboxMessage) -> Result<OutboxId, OutboxError> {
        let id = OutboxId::new();
        let partition_key = message.partition_key.clone();

        let seq = self.partition_seq.entry(partition_key.clone()).or_insert(0);
        let sequence = *seq;
        *seq += 1;

        let item = OutboxItem {
            id,
            message,
            claimed_by: None,
            claimed_until: None,
            attempt_count: 0,
            dead_lettered: false,
            sequence,
        };

        self.items.insert(id, item);
        self.partitions
            .entry(partition_key)
            .or_default()
            .insert(sequence, id);

        debug!(outbox_id = %id, "message enqueued");
        Ok(id)
    }

    // -----------------------------------------------------------------------
    // Consumer API
    // -----------------------------------------------------------------------

    /// Attempt to claim the next available item for `consumer_id`.
    ///
    /// "Available" means: not dead-lettered, and either unclaimed or with an
    /// expired lease. Among all available items the one with the lowest
    /// partition sequence number is returned (FIFO within a partition; across
    /// partitions any available head item may be chosen).
    ///
    /// Returns `Ok(None)` when the queue is empty or all items are currently
    /// claimed.
    ///
    /// # Errors
    ///
    /// Returns [`OutboxError::EmptyConsumerId`] when `consumer_id` is empty.
    pub fn claim_next(&mut self, consumer_id: &str) -> Result<Option<OutboxItem>, OutboxError> {
        if consumer_id.is_empty() {
            return Err(OutboxError::EmptyConsumerId);
        }

        let now = Utc::now();
        let lease_until = now + self.config.lease_duration;

        // Find the first claimable item across all partitions.
        // For each partition we inspect only its head (lowest sequence).
        // We pick the partition whose head item has the globally smallest
        // sequence — this provides deterministic ordering under concurrent
        // single-partition writers.
        let candidate_id = self.find_claimable_head(now);

        match candidate_id {
            None => Ok(None),
            Some(id) => {
                let Some(item) = self.items.get_mut(&id) else {
                    return Ok(None);
                };

                item.claimed_by = Some(consumer_id.to_owned());
                item.claimed_until = Some(lease_until);
                item.attempt_count += 1;

                debug!(
                    outbox_id = %id,
                    consumer_id = consumer_id,
                    lease_until = %lease_until,
                    attempt = item.attempt_count,
                    "item claimed"
                );

                Ok(Some(item.clone()))
            }
        }
    }

    /// Acknowledge successful processing of item `outbox_id` by `consumer_id`.
    ///
    /// The item is removed from the live queue. Only the consumer holding the
    /// current lease may acknowledge.
    ///
    /// # Errors
    ///
    /// * [`OutboxError::NotFound`] — no live item with this id.
    /// * [`OutboxError::LeaseConflict`] — caller does not hold the lease.
    /// * [`OutboxError::DeadLettered`] — item has already been dead-lettered.
    pub fn acknowledge(
        &mut self,
        outbox_id: OutboxId,
        consumer_id: &str,
    ) -> Result<(), OutboxError> {
        // Validate existence and lease in a scoped borrow, extracting the
        // data we need before the borrow ends.
        let (partition_key, sequence) = {
            if self.dead_letter.contains_key(&outbox_id) {
                return Err(OutboxError::DeadLettered(outbox_id));
            }
            let item = self
                .items
                .get(&outbox_id)
                .ok_or(OutboxError::NotFound(outbox_id))?;

            match &item.claimed_by {
                Some(holder) if holder == consumer_id => {}
                Some(holder) => {
                    return Err(OutboxError::LeaseConflict {
                        id: outbox_id,
                        caller: consumer_id.to_owned(),
                        holder: holder.clone(),
                    });
                }
                None => {
                    return Err(OutboxError::LeaseConflict {
                        id: outbox_id,
                        caller: consumer_id.to_owned(),
                        holder: String::new(),
                    });
                }
            }

            (item.message.partition_key.clone(), item.sequence)
        };

        self.items.remove(&outbox_id);
        if let Some(queue) = self.partitions.get_mut(&partition_key) {
            queue.remove(&sequence);
        }

        debug!(outbox_id = %outbox_id, consumer_id = consumer_id, "item acknowledged");
        Ok(())
    }

    /// Return item `outbox_id` to the queue without acknowledging it.
    ///
    /// The lease is released immediately so that any consumer may reclaim the
    /// item. The `message.retry_count` is incremented; if it then exceeds
    /// `message.max_retries` the item is moved to the dead-letter set.
    ///
    /// # Errors
    ///
    /// * [`OutboxError::NotFound`] — no live item with this id.
    /// * [`OutboxError::DeadLettered`] — item has already been dead-lettered.
    pub fn nack(&mut self, outbox_id: OutboxId) -> Result<(), OutboxError> {
        let should_dead_letter = {
            let item = self.get_live_item(outbox_id)?;
            item.message.retry_count += 1;
            item.claimed_by = None;
            item.claimed_until = None;
            item.message.is_exhausted()
        };

        if should_dead_letter {
            self.move_to_dead_letter(outbox_id);
        } else {
            debug!(outbox_id = %outbox_id, "item nacked — returned to queue");
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Inspection
    // -----------------------------------------------------------------------

    /// Return a snapshot of every dead-lettered item.
    pub fn dead_letter_items(&self) -> Vec<OutboxItem> {
        self.dead_letter.values().cloned().collect()
    }

    /// Return the number of live (non-dead-lettered) items in the queue.
    pub fn live_count(&self) -> usize {
        self.items.len()
    }

    /// Return a snapshot of all live items in the queue.
    ///
    /// The snapshot is not ordered; use `item.sequence` for ordering within a
    /// partition.
    pub fn live_items(&self) -> Vec<OutboxItem> {
        self.items.values().cloned().collect()
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Scan all partition heads and return the id of the claimable item with
    /// the globally smallest sequence number.
    ///
    /// An item is claimable if it is not dead-lettered and either unclaimed or
    /// its lease has expired.
    fn find_claimable_head(&self, now: DateTime<Utc>) -> Option<OutboxId> {
        let mut best: Option<(u64, OutboxId)> = None;

        for queue in self.partitions.values() {
            // Walk from the lowest sequence upward within this partition until
            // we find a claimable head item (earlier items might be actively
            // claimed, blocking the partition).
            for (&seq, &id) in queue {
                let Some(item) = self.items.get(&id) else {
                    continue;
                };

                if item.dead_lettered {
                    continue;
                }

                let claimable = match item.claimed_until {
                    None => true,
                    Some(until) => until <= now,
                };

                if claimable {
                    // This is the head of this partition. Compare globally.
                    match best {
                        None => best = Some((seq, id)),
                        Some((best_seq, _)) if seq < best_seq => {
                            best = Some((seq, id));
                        }
                        _ => {}
                    }
                }

                // Stop at the first item in this partition — later items are
                // blocked behind the head regardless of their claim status.
                break;
            }
        }

        best.map(|(_, id)| id)
    }

    /// Get a mutable reference to a live (non-dead-lettered) item or return an
    /// appropriate error.
    fn get_live_item(&mut self, id: OutboxId) -> Result<&mut OutboxItem, OutboxError> {
        if self.dead_letter.contains_key(&id) {
            return Err(OutboxError::DeadLettered(id));
        }
        self.items.get_mut(&id).ok_or(OutboxError::NotFound(id))
    }

    /// Remove an item from the live queue and insert it into the dead-letter
    /// set.
    fn move_to_dead_letter(&mut self, id: OutboxId) {
        if let Some(mut item) = self.items.remove(&id) {
            item.dead_lettered = true;
            let partition_key = item.message.partition_key.clone();
            let sequence = item.sequence;

            warn!(
                outbox_id = %id,
                attempt_count = item.attempt_count,
                partition_key = %partition_key,
                "item dead-lettered after exhausting retries"
            );

            if let Some(queue) = self.partitions.get_mut(&partition_key) {
                queue.remove(&sequence);
            }
            self.dead_letter.insert(id, item);
        }
    }
}

impl Default for DurableOutbox {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Test fixtures use explicit panic boundaries to identify broken invariants.
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "unit-test outbox assertions intentionally panic with focused diagnostics"
)]
mod tests {
    use super::*;
    use crate::message::OutboxMessage;
    use serde_json::json;

    fn msg(partition: &str, idempotency: &str) -> OutboxMessage {
        OutboxMessage::new(partition, idempotency, json!({"v": 1}), 2)
    }

    fn outbox_with_lease(secs: i64) -> DurableOutbox {
        DurableOutbox::with_config(OutboxConfig {
            lease_duration: Duration::seconds(secs),
        })
    }

    // -----------------------------------------------------------------------
    // Basic enqueue / claim / ack
    // -----------------------------------------------------------------------

    #[test]
    fn enqueue_and_claim_basic() {
        let mut ob = DurableOutbox::new();
        let id = ob.enqueue(msg("p1", "k1")).expect("enqueue");
        let item = ob.claim_next("consumer-a").expect("claim").expect("some");
        assert_eq!(item.id, id);
        assert_eq!(item.claimed_by.as_deref(), Some("consumer-a"));
    }

    #[test]
    fn acknowledge_removes_item() {
        let mut ob = DurableOutbox::new();
        let id = ob.enqueue(msg("p1", "k1")).expect("enqueue");
        ob.claim_next("consumer-a").expect("claim").expect("some");
        ob.acknowledge(id, "consumer-a").expect("ack");
        assert_eq!(ob.live_count(), 0);
    }

    #[test]
    fn claim_empty_queue_returns_none() {
        let mut ob = DurableOutbox::new();
        let result = ob.claim_next("consumer-a").expect("no error");
        assert!(result.is_none());
    }

    // -----------------------------------------------------------------------
    // FIFO ordering within a partition
    // -----------------------------------------------------------------------

    #[test]
    fn enqueue_claim_preserves_fifo_order_within_partition() {
        let mut ob = outbox_with_lease(30);
        let id1 = ob.enqueue(msg("part", "k1")).expect("1");
        let id2 = ob.enqueue(msg("part", "k2")).expect("2");
        let id3 = ob.enqueue(msg("part", "k3")).expect("3");

        // Each claim+ack must proceed in enqueue order.
        let c1 = ob.claim_next("c").expect("c1").expect("some");
        assert_eq!(c1.id, id1);
        ob.acknowledge(id1, "c").expect("ack1");

        let c2 = ob.claim_next("c").expect("c2").expect("some");
        assert_eq!(c2.id, id2);
        ob.acknowledge(id2, "c").expect("ack2");

        let c3 = ob.claim_next("c").expect("c3").expect("some");
        assert_eq!(c3.id, id3);
        ob.acknowledge(id3, "c").expect("ack3");
    }

    #[test]
    fn claimed_head_blocks_partition() {
        // If the head of a partition is claimed, no further item in that
        // partition should be returned (FIFO guarantee).
        let mut ob = outbox_with_lease(60);
        ob.enqueue(msg("part", "k1")).expect("e1");
        let id2 = ob.enqueue(msg("part", "k2")).expect("e2");

        // Claim and hold the head.
        ob.claim_next("consumer-a").expect("c1").expect("some");

        // No other item in the same partition should be available.
        let second = ob.claim_next("consumer-b").expect("c2");
        assert!(
            second.is_none() || second.map(|i| i.id) != Some(id2),
            "second item in blocked partition must not be claimed"
        );
    }

    // -----------------------------------------------------------------------
    // Lease expiry / reclaimability
    // -----------------------------------------------------------------------

    #[test]
    fn unclaimed_after_lease_expiry_is_reclaimable() {
        // Use a 0-second lease so the item expires instantly.
        let mut ob = outbox_with_lease(0);
        ob.enqueue(msg("part", "k1")).expect("enqueue");

        // Claim — lease_until = now + 0s = now (already expired).
        ob.claim_next("consumer-a").expect("c1").expect("some");

        // Sleep 1 ms to ensure now > lease_until.
        std::thread::sleep(std::time::Duration::from_millis(5));

        // A second consumer should be able to reclaim.
        let reclaim = ob.claim_next("consumer-b").expect("c2");
        assert!(
            reclaim.is_some(),
            "item should be reclaimable after lease expiry"
        );
        assert_eq!(reclaim.unwrap().claimed_by.as_deref(), Some("consumer-b"));
    }

    // -----------------------------------------------------------------------
    // Nack / retry / dead-letter
    // -----------------------------------------------------------------------

    #[test]
    fn nack_returns_item_to_queue() {
        let mut ob = DurableOutbox::new();
        let id = ob.enqueue(msg("p", "k")).expect("enqueue");
        ob.claim_next("c").expect("c1").expect("some");
        ob.nack(id).expect("nack");

        // Item should now be claimable again.
        let reclaim = ob.claim_next("c").expect("c2");
        assert!(reclaim.is_some());
    }

    #[test]
    fn dead_letter_after_max_retries_exceeded() {
        // max_retries = 1 means: first attempt + one retry = 2 total, dead-
        // letter on the third nack.
        let mut ob = DurableOutbox::new();
        let id = ob
            .enqueue(OutboxMessage::new("p", "k", json!(null), 1))
            .expect("enqueue");

        // Attempt 1: nack (retry_count -> 1, attempt_count = 1)
        ob.claim_next("c").expect("ok").expect("some");
        ob.nack(id).expect("nack1"); // retry_count=1, not exhausted yet

        // Attempt 2: nack (retry_count -> 2, attempt_count = 2, exhausted)
        ob.claim_next("c").expect("ok").expect("some");
        ob.nack(id).expect("nack2"); // retry_count=2 > max_retries=1 → dead-letter

        assert_eq!(ob.live_count(), 0, "item must be removed from live queue");
        assert_eq!(
            ob.dead_letter_items().len(),
            1,
            "item must be in dead-letter"
        );
    }

    #[test]
    fn dead_lettered_item_cannot_be_acknowledged() {
        let mut ob = DurableOutbox::new();
        let id = ob
            .enqueue(OutboxMessage::new("p", "k", json!(null), 0))
            .expect("enqueue");

        // max_retries=0: exhausted after the first nack (retry_count -> 1 > 0).
        ob.claim_next("c").expect("ok").expect("some");
        ob.nack(id).expect("nack");

        let err = ob.acknowledge(id, "c").unwrap_err();
        assert!(
            matches!(err, OutboxError::DeadLettered(_)),
            "expected DeadLettered, got {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Lease conflict
    // -----------------------------------------------------------------------

    #[test]
    fn wrong_consumer_cannot_acknowledge() {
        let mut ob = outbox_with_lease(60);
        let id = ob.enqueue(msg("p", "k")).expect("enqueue");
        ob.claim_next("consumer-a").expect("ok").expect("some");

        let err = ob.acknowledge(id, "consumer-b").unwrap_err();
        assert!(
            matches!(err, OutboxError::LeaseConflict { .. }),
            "expected LeaseConflict, got {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Concurrent consumers — same message not issued twice
    // -----------------------------------------------------------------------

    #[test]
    fn concurrent_consumers_do_not_get_same_message() {
        // Single-threaded simulation: consumer-a claims, consumer-b tries.
        let mut ob = outbox_with_lease(60);
        let id = ob.enqueue(msg("p", "k")).expect("enqueue");

        let item_a = ob.claim_next("consumer-a").expect("a").expect("some");
        assert_eq!(item_a.id, id);

        // consumer-b should get nothing — queue is empty / item is claimed.
        let item_b = ob.claim_next("consumer-b").expect("b");
        assert!(
            item_b.is_none(),
            "consumer-b must not receive the claimed item"
        );
    }

    // -----------------------------------------------------------------------
    // Validation
    // -----------------------------------------------------------------------

    #[test]
    fn empty_consumer_id_returns_error() {
        let mut ob = DurableOutbox::new();
        ob.enqueue(msg("p", "k")).expect("enqueue");
        let err = ob.claim_next("").unwrap_err();
        assert!(matches!(err, OutboxError::EmptyConsumerId));
    }

    #[test]
    fn acknowledge_unknown_id_returns_not_found() {
        let mut ob = DurableOutbox::new();
        let phantom = OutboxId::new();
        let err = ob.acknowledge(phantom, "c").unwrap_err();
        assert!(matches!(err, OutboxError::NotFound(_)));
    }

    #[test]
    fn attempt_count_increments_on_each_claim() {
        let mut ob = DurableOutbox::new();
        let id = ob
            .enqueue(OutboxMessage::new("p", "k", json!(null), 5))
            .expect("enqueue");

        for expected_attempt in 1..=3u32 {
            let item = ob.claim_next("c").expect("ok").expect("some");
            assert_eq!(item.attempt_count, expected_attempt);
            ob.nack(id).expect("nack");
        }
    }
}
