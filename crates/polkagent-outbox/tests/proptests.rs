//! Property-based tests for `polkagent-outbox`.
//!
//! These tests use `proptest` to verify invariants of the durable outbox,
//! deduplication log, and exponential backoff across randomly generated inputs.

use std::collections::HashSet;
use std::time::Duration;

use proptest::prelude::*;
use serde_json::json;

use polkagent_outbox::dedup::DeduplicationLog;
use polkagent_outbox::outbox::{DurableOutbox, OutboxConfig};
use polkagent_outbox::retry::ExponentialBackoff;
use polkagent_outbox::OutboxMessage;

// =========================================================================
// Helpers
// =========================================================================

/// Build an outbox with a generous lease so items stay claimed.
fn outbox_held() -> DurableOutbox {
    DurableOutbox::with_config(OutboxConfig {
        lease_duration: chrono::Duration::seconds(300),
    })
}

/// Build an outbox with a zero-length lease so items are reclaimable instantly.
fn outbox_instant_expire() -> DurableOutbox {
    DurableOutbox::with_config(OutboxConfig {
        lease_duration: chrono::Duration::seconds(0),
    })
}

/// Create a simple outbox message.
fn msg(partition: &str, idem: &str) -> OutboxMessage {
    OutboxMessage::new(partition, idem, json!({"test": true}), 10)
}

// =========================================================================
// 1. Outbox preserves message ordering (FIFO within a partition)
// =========================================================================

proptest! {
    /// Messages enqueued to the same partition are claimed in insertion order.
    #[test]
    fn outbox_preserves_fifo_ordering(count in 2..20usize) {
        let mut ob = outbox_held();
        let mut enqueued_ids = Vec::new();

        for i in 0..count {
            let id = ob.enqueue(msg("fifo-partition", &format!("key-{i}")))
                .expect("enqueue");
            enqueued_ids.push(id);
        }

        let mut claimed_ids = Vec::new();
        for _ in 0..count {
            // Use instant-expire trick: claim, record id, nack to release
            // so we can claim the next. But to test pure FIFO, claim+ack.
            let item = ob.claim_next("consumer").expect("claim").expect("some");
            claimed_ids.push(item.id);
            ob.acknowledge(item.id, "consumer").expect("ack");
        }

        prop_assert_eq!(
            enqueued_ids, claimed_ids,
            "messages must come out in insertion order"
        );
    }
}

// =========================================================================
// 2. Deduplication log rejects duplicate IDs
// =========================================================================

proptest! {
    /// After recording (consumer, key), `is_duplicate` returns true for the
    /// same pair and false for any different key.
    #[test]
    fn dedup_rejects_duplicate_ids(
        consumer in "[a-z]{1,10}",
        key in "[a-z]{1,10}",
        other_key in "[a-z]{1,10}",
    ) {
        let mut log = DeduplicationLog::with_default_window();
        log.record_processed(&consumer, &key).expect("record");
        prop_assert!(
            log.is_duplicate(&consumer, &key),
            "same (consumer, key) must be a duplicate"
        );

        if key != other_key {
            prop_assert!(
                !log.is_duplicate(&consumer, &other_key),
                "different key must not be a duplicate"
            );
        }
    }

    /// Recording the same (consumer, key) pair multiple times is idempotent:
    /// the live entry count does not increase.
    #[test]
    fn dedup_record_is_idempotent(
        consumer in "[a-z]{1,8}",
        key in "[a-z]{1,8}",
        repeats in 2..10usize,
    ) {
        let mut log = DeduplicationLog::with_default_window();
        for _ in 0..repeats {
            log.record_processed(&consumer, &key).expect("record");
        }
        prop_assert_eq!(log.live_entry_count(), 1, "idempotent recording must not grow");
    }
}

// =========================================================================
// 3. Exponential backoff never exceeds max delay
// =========================================================================

proptest! {
    /// For any attempt number, the computed delay never exceeds `max_delay`.
    #[test]
    fn backoff_never_exceeds_max(
        base_ms in 1u64..10_000,
        max_ms in 1u64..1_000_000,
        max_attempts in 1u32..20,
        attempt in 0u32..100,
    ) {
        let base = Duration::from_millis(base_ms);
        let max = Duration::from_millis(max_ms);
        let policy = ExponentialBackoff::new(base, max, max_attempts);
        let delay = policy.next_delay(attempt);

        prop_assert!(
            delay <= max,
            "delay {:?} exceeded max {:?} at attempt {}",
            delay, max, attempt
        );
    }

    /// A zero base delay always produces a zero delay.
    #[test]
    fn backoff_zero_base_always_zero(
        max_ms in 1u64..1_000_000,
        attempt in 0u32..100,
    ) {
        let policy = ExponentialBackoff::new(
            Duration::ZERO,
            Duration::from_millis(max_ms),
            10,
        );
        let delay = policy.next_delay(attempt);
        prop_assert_eq!(delay, Duration::ZERO);
    }
}

// =========================================================================
// 4. Retry count increments correctly
// =========================================================================

proptest! {
    /// Each claim increments `attempt_count` by exactly 1.
    #[test]
    fn retry_count_increments(claims in 1..8u32) {
        let mut ob = outbox_instant_expire();
        let id = ob.enqueue(OutboxMessage::new("p", "k", json!(null), 100))
            .expect("enqueue");

        for expected in 1..=claims {
            // Small sleep to ensure expired lease
            std::thread::sleep(std::time::Duration::from_millis(2));
            let item = ob.claim_next("c").expect("claim").expect("some");
            prop_assert_eq!(
                item.attempt_count, expected,
                "attempt_count should be {} on claim #{}",
                expected, expected
            );
            ob.nack(id).expect("nack");
        }
    }
}

// =========================================================================
// 5. Messages are never lost (everything inserted is eventually delivered)
// =========================================================================

proptest! {
    /// Every message enqueued to the outbox can be claimed (no messages are lost).
    #[test]
    fn messages_never_lost(count in 1..15usize) {
        let mut ob = outbox_held();
        let mut enqueued: HashSet<_> = HashSet::new();

        for i in 0..count {
            let id = ob.enqueue(msg("partition", &format!("msg-{i}")))
                .expect("enqueue");
            enqueued.insert(id);
        }

        let mut delivered = HashSet::new();
        for _ in 0..count {
            let item = ob.claim_next("worker").expect("claim").expect("some");
            delivered.insert(item.id);
            ob.acknowledge(item.id, "worker").expect("ack");
        }

        prop_assert_eq!(
            enqueued, delivered,
            "all enqueued messages must be delivered"
        );

        // Queue must be empty.
        prop_assert_eq!(ob.live_count(), 0, "queue must be empty after draining");
    }
}
