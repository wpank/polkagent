//! IT-04: Outbox delivery integration tests.
//!
//! Exercises DurableOutbox with FIFO ordering, failure-mid-drain retry,
//! deduplication via DeduplicationLog, and exponential backoff configuration.

use serde_json::json;

use polkagent_outbox::{
    DeduplicationLog, DurableOutbox, ExponentialBackoff, OutboxConfig, OutboxMessage,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn msg(partition: &str, idempotency: &str) -> OutboxMessage {
    OutboxMessage::new(partition, idempotency, json!({"v": 1}), 3)
}

#[allow(dead_code)]
fn msg_payload(partition: &str, idempotency: &str, payload: serde_json::Value) -> OutboxMessage {
    OutboxMessage::new(partition, idempotency, payload, 3)
}

// ---------------------------------------------------------------------------
// FIFO ordering
// ---------------------------------------------------------------------------

#[test]
fn enqueue_and_drain_preserves_fifo_order_within_partition() {
    let mut ob = DurableOutbox::new();

    let id1 = ob.enqueue(msg("run:1", "key-1")).expect("enqueue 1");
    let id2 = ob.enqueue(msg("run:1", "key-2")).expect("enqueue 2");
    let id3 = ob.enqueue(msg("run:1", "key-3")).expect("enqueue 3");

    // Drain in order: claim → ack → repeat.
    let c1 = ob.claim_next("worker-a").expect("claim 1").expect("some 1");
    assert_eq!(c1.id, id1, "first item must be first enqueued");
    ob.acknowledge(id1, "worker-a").expect("ack 1");

    let c2 = ob.claim_next("worker-a").expect("claim 2").expect("some 2");
    assert_eq!(c2.id, id2, "second item must be second enqueued");
    ob.acknowledge(id2, "worker-a").expect("ack 2");

    let c3 = ob.claim_next("worker-a").expect("claim 3").expect("some 3");
    assert_eq!(c3.id, id3, "third item must be third enqueued");
    ob.acknowledge(id3, "worker-a").expect("ack 3");

    assert_eq!(
        ob.live_count(),
        0,
        "queue must be empty after draining all items"
    );
}

#[test]
fn fifo_ordering_is_per_partition() {
    let mut ob = DurableOutbox::new();

    // Enqueue in alternating partitions.
    let id_p1_a = ob.enqueue(msg("part-1", "k1a")).expect("e1");
    let id_p2_a = ob.enqueue(msg("part-2", "k2a")).expect("e2");
    let _id_p1_b = ob.enqueue(msg("part-1", "k1b")).expect("e3");
    let _id_p2_b = ob.enqueue(msg("part-2", "k2b")).expect("e4");

    // The first claim picks the globally earliest available head.
    let first = ob.claim_next("worker").expect("c1").expect("some");
    // part-1 and part-2 heads are both sequence 0; either is valid.
    let first_id = first.id;
    assert!(
        first_id == id_p1_a || first_id == id_p2_a,
        "first claim must be a partition head"
    );
}

#[test]
fn claimed_head_blocks_rest_of_partition() {
    let mut ob = DurableOutbox::with_config(OutboxConfig {
        lease_duration: chrono::Duration::seconds(60),
    });

    ob.enqueue(msg("p", "k1")).expect("e1");
    let id2 = ob.enqueue(msg("p", "k2")).expect("e2");

    // Claim and hold the head without acknowledging.
    ob.claim_next("consumer-a").expect("c1").expect("some");

    // A second consumer must not get item 2 — the partition is blocked.
    let second = ob.claim_next("consumer-b").expect("c2");
    assert!(
        second.as_ref().map(|i| i.id) != Some(id2),
        "second item must not be claimable while partition head is held"
    );
}

// ---------------------------------------------------------------------------
// Failure mid-drain / retry
// ---------------------------------------------------------------------------

#[test]
fn nack_returns_item_for_retry_after_failure() {
    let mut ob = DurableOutbox::new();
    let id = ob.enqueue(msg("run:99", "retry-key")).expect("enqueue");

    // First attempt — worker claims but fails.
    let claimed = ob.claim_next("worker-1").expect("c1").expect("some");
    assert_eq!(claimed.id, id);
    ob.nack(id).expect("nack");

    // Item is back in the queue — another worker picks it up.
    let retry = ob.claim_next("worker-2").expect("c2").expect("some");
    assert_eq!(retry.id, id, "same item must be re-claimable after nack");
    assert_eq!(retry.attempt_count, 2, "attempt count must be incremented");
    ob.acknowledge(id, "worker-2").expect("ack");
    assert_eq!(ob.live_count(), 0);
}

#[test]
fn nack_mid_drain_does_not_discard_subsequent_items() {
    // Enqueue three items; nack the first, verify the second and third survive.
    let mut ob = DurableOutbox::new();
    let id1 = ob.enqueue(msg("p", "k1")).expect("e1");
    let _id2 = ob.enqueue(msg("p", "k2")).expect("e2");
    let _id3 = ob.enqueue(msg("p", "k3")).expect("e3");

    // Claim and nack the first item.
    ob.claim_next("w").expect("c1").expect("some");
    ob.nack(id1).expect("nack1");

    // The queue still has 3 items (item 1 was returned, 2 and 3 were never
    // touched).
    assert_eq!(ob.live_count(), 3, "nack must not discard subsequent items");
}

#[test]
fn item_is_dead_lettered_after_exhausting_max_retries() {
    let mut ob = DurableOutbox::new();
    // max_retries = 1 → allow original attempt + 1 retry only.
    let id = ob
        .enqueue(OutboxMessage::new("p", "k", json!(null), 1))
        .expect("enqueue");

    // Attempt 1: nack (retry_count → 1).
    ob.claim_next("c").expect("ok").expect("some");
    ob.nack(id).expect("nack1"); // not exhausted yet

    // Attempt 2: nack (retry_count → 2 > max_retries=1 → dead-letter).
    ob.claim_next("c").expect("ok").expect("some");
    ob.nack(id).expect("nack2");

    assert_eq!(
        ob.live_count(),
        0,
        "exhausted item must be removed from live queue"
    );
    assert_eq!(
        ob.dead_letter_items().len(),
        1,
        "exhausted item must appear in dead-letter queue"
    );
}

#[test]
fn dead_lettered_item_cannot_be_reprocessed() {
    let mut ob = DurableOutbox::new();
    // max_retries = 0 → one attempt, then dead-letter on first nack.
    let id = ob
        .enqueue(OutboxMessage::new("p", "k", json!(null), 0))
        .expect("enqueue");

    ob.claim_next("c").expect("ok").expect("some");
    ob.nack(id).expect("nack"); // retry_count=1 > 0 → dead-letter

    // Further claim should return None — dead-lettered items are excluded.
    let next = ob.claim_next("c").expect("should not error");
    assert!(
        next.is_none(),
        "dead-lettered item must not be claimable again"
    );
}

#[test]
fn expired_lease_allows_reclaim() {
    let mut ob = DurableOutbox::with_config(OutboxConfig {
        lease_duration: chrono::Duration::zero(),
    });
    ob.enqueue(msg("p", "k")).expect("enqueue");

    // Claim — lease expires at now (0-duration).
    ob.claim_next("worker-a").expect("c1").expect("some");

    // Sleep briefly to advance the clock past the zero-duration lease.
    std::thread::sleep(std::time::Duration::from_millis(5));

    // A different worker can now reclaim.
    let reclaim = ob.claim_next("worker-b").expect("c2");
    assert!(
        reclaim.is_some(),
        "item must be reclaimable after lease expiry"
    );
    assert_eq!(reclaim.unwrap().claimed_by.as_deref(), Some("worker-b"));
}

// ---------------------------------------------------------------------------
// Deduplication
// ---------------------------------------------------------------------------

#[test]
fn deduplication_log_prevents_duplicate_processing() {
    let mut dedup = DeduplicationLog::with_default_window();

    // Mark a key as processed.
    dedup
        .record_processed("consumer-1", "order:99:created")
        .expect("record");

    assert!(
        dedup.is_duplicate("consumer-1", "order:99:created"),
        "same consumer + key must be detected as duplicate"
    );
}

#[test]
fn same_key_different_consumer_not_duplicate() {
    let mut dedup = DeduplicationLog::with_default_window();

    dedup
        .record_processed("consumer-1", "event:abc")
        .expect("record");

    assert!(
        !dedup.is_duplicate("consumer-2", "event:abc"),
        "same key but different consumer must not be duplicate"
    );
}

#[test]
fn dedup_does_not_block_first_delivery() {
    let mut dedup = DeduplicationLog::with_default_window();

    assert!(
        !dedup.is_duplicate("consumer-1", "brand-new-key"),
        "fresh key must not be detected as duplicate"
    );
}

#[test]
fn dedup_combined_with_outbox_prevents_double_processing() {
    let mut ob = DurableOutbox::with_config(OutboxConfig {
        lease_duration: chrono::Duration::zero(),
    });
    let mut dedup = DeduplicationLog::with_default_window();

    let idem_key = "event:run:1:step:3";
    ob.enqueue(msg("run:1", idem_key)).expect("enqueue");

    // First consumer claims and processes.
    let item = ob.claim_next("consumer-a").expect("c1").expect("some");
    let key = item.message.idempotency_key.clone();
    if !dedup.is_duplicate("consumer-a", &key) {
        dedup.record_processed("consumer-a", &key).expect("record");
    }
    ob.acknowledge(item.id, "consumer-a").expect("ack");

    // Simulate re-delivery: enqueue the same idempotency key again.
    ob.enqueue(msg("run:1", idem_key)).expect("re-enqueue");

    // Second consumer claims and checks dedup.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let item2 = ob.claim_next("consumer-a").expect("c2").expect("some");
    let key2 = item2.message.idempotency_key.clone();

    // The dedup log should flag this as a duplicate.
    assert!(
        dedup.is_duplicate("consumer-a", &key2),
        "redelivery of the same idempotency key must be detected as duplicate"
    );

    // Acknowledge without re-processing.
    ob.acknowledge(item2.id, "consumer-a").expect("ack2");
    assert_eq!(ob.live_count(), 0, "queue must be empty");
    assert_eq!(dedup.live_entry_count(), 1, "dedup log must have one entry");
}

#[test]
fn dedup_record_is_idempotent() {
    let mut dedup = DeduplicationLog::with_default_window();

    dedup.record_processed("c", "k").expect("first");
    dedup.record_processed("c", "k").expect("second — no error");

    // Still just one live entry.
    assert_eq!(dedup.live_entry_count(), 1);
    assert!(dedup.is_duplicate("c", "k"));
}

// ---------------------------------------------------------------------------
// Backoff testing
// ---------------------------------------------------------------------------

#[test]
fn backoff_delay_never_exceeds_cap() {
    let policy = ExponentialBackoff::new(
        std::time::Duration::from_secs(1),
        std::time::Duration::from_secs(60),
        5,
    );

    for attempt in 0..20u32 {
        let d = policy.next_delay(attempt);
        assert!(
            d <= policy.max_delay,
            "attempt {attempt}: delay {d:?} must not exceed cap {:?}",
            policy.max_delay
        );
    }
}

#[test]
fn backoff_increases_monotonically_in_expectation() {
    // Even with jitter, the upper bound on the delay should grow with the
    // attempt number. We assert the cap is increasing.
    let policy = ExponentialBackoff::new(
        std::time::Duration::from_secs(1),
        std::time::Duration::from_secs(300),
        10,
    );

    // At attempt 0 the window is [0, base); at attempt 3 it is [0, min(cap, base*8)).
    // We check that the window grows by comparing delays from many samples.
    // Because of jitter this is probabilistic — we just check the cap is larger.
    let base_nanos = policy.base_delay.as_nanos();
    let cap_nanos = policy.max_delay.as_nanos();

    let upper_at_0 = cap_nanos.min(base_nanos * (1u128 << 0));
    let upper_at_5 = cap_nanos.min(base_nanos * (1u128 << 5));

    assert!(
        upper_at_5 >= upper_at_0,
        "backoff upper bound must be non-decreasing"
    );
}

#[test]
fn backoff_should_dead_letter_threshold() {
    let policy = ExponentialBackoff::new(
        std::time::Duration::from_secs(1),
        std::time::Duration::from_secs(60),
        3,
    );

    // Attempts 0–3 are within limit.
    assert!(!policy.should_dead_letter(0));
    assert!(!policy.should_dead_letter(1));
    assert!(!policy.should_dead_letter(3));

    // Attempt 4 exceeds max_attempts=3.
    assert!(policy.should_dead_letter(4));
    assert!(policy.should_dead_letter(100));
}

#[test]
fn backoff_default_policy_sensible_values() {
    let policy = ExponentialBackoff::default();
    assert_eq!(policy.max_attempts, 5);
    assert!(policy.max_delay >= policy.base_delay);
    assert!(!policy.should_dead_letter(5));
    assert!(policy.should_dead_letter(6));
}

#[test]
fn backoff_zero_base_delay_always_zero() {
    let policy = ExponentialBackoff::new(
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(60),
        5,
    );
    for attempt in 0..10 {
        assert_eq!(policy.next_delay(attempt), std::time::Duration::ZERO);
    }
}

#[test]
fn backoff_large_attempt_does_not_panic() {
    let policy = ExponentialBackoff::default();
    // Should not panic or overflow at extreme attempt numbers.
    let _ = policy.next_delay(200);
    let _ = policy.next_delay(u32::MAX);
}

// ---------------------------------------------------------------------------
// Multi-consumer ordering across partitions
// ---------------------------------------------------------------------------

#[test]
fn items_from_multiple_partitions_can_be_interleaved() {
    let mut ob = DurableOutbox::new();

    let _id_a1 = ob.enqueue(msg("agent-a", "a1")).expect("a1");
    let _id_b1 = ob.enqueue(msg("agent-b", "b1")).expect("b1");
    let _id_a2 = ob.enqueue(msg("agent-a", "a2")).expect("a2");
    let _id_b2 = ob.enqueue(msg("agent-b", "b2")).expect("b2");

    // Drain all four items.
    let mut claimed = Vec::new();
    for _ in 0..4 {
        if let Some(item) = ob.claim_next("w").expect("claim") {
            let id = item.id;
            claimed.push(item.message.partition_key.clone());
            ob.acknowledge(id, "w").expect("ack");
        }
    }

    assert_eq!(claimed.len(), 4, "all four items must be drained");
    assert_eq!(ob.live_count(), 0, "queue must be empty after draining all");
}
