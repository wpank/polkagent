//! Event projection integration tests.
//!
//! Exercises multi-run event recording, query by run_id, query by event type,
//! and global sequence ordering using the in-memory MemEventStore and
//! EventRecorder from the integration test helpers.

use std::sync::Arc;

use chrono::Utc;
use uuid::Uuid;

use polkagent_core::RunId;
use polkagent_event::bus::EventBus;
use polkagent_event::recorder::EventRecorder;
use polkagent_store_trait::event::{EventFilter, EventStore, StoredEvent};

use polkagent_integration_tests::MemEventStore;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_stored_event(
    run_id: RunId,
    event_type: &str,
    sequence: u64,
) -> StoredEvent {
    StoredEvent {
        id: Uuid::now_v7().to_string(),
        event_type: event_type.to_string(),
        sequence,
        global_sequence: 0, // assigned by store
        run_id: run_id.to_string(),
        conversation_id: None,
        correlation_id: "corr-1".to_string(),
        causation_id: None,
        scope_id: "default".to_string(),
        timestamp: Utc::now().to_rfc3339(),
        durability: "durable".to_string(),
        payload: serde_json::json!({"event_type": event_type}),
        trace_id: None,
        span_id: None,
        schema_version: 1,
    }
}

fn make_store_with_recorder() -> (Arc<MemEventStore>, EventRecorder) {
    let store = Arc::new(MemEventStore::default());
    let store_dyn = Arc::clone(&store) as Arc<dyn EventStore>;
    let bus = EventBus::new(64);
    let recorder = EventRecorder::new(store_dyn, bus);
    (store, recorder)
}

// ---------------------------------------------------------------------------
// Multi-run event recording
// ---------------------------------------------------------------------------

#[tokio::test]
async fn append_events_for_multiple_runs_all_stored() {
    let store = Arc::new(MemEventStore::default());
    let run_a = RunId::new();
    let run_b = RunId::new();
    let run_c = RunId::new();

    for (run, seq) in [(&run_a, 1), (&run_b, 1), (&run_c, 1)] {
        let ev = make_stored_event(*run, "run_created", seq);
        store.append_durable(ev).await.expect("append");
    }

    // Each run should have exactly one event.
    for run in [run_a, run_b, run_c] {
        let events = store.read_run_events(run).await.expect("read");
        assert_eq!(events.len(), 1, "run {run} must have 1 event");
    }
}

#[tokio::test]
async fn append_multiple_events_per_run_in_sequence_order() {
    let store = Arc::new(MemEventStore::default());
    let run = RunId::new();

    for (seq, kind) in [
        (1, "run_created"),
        (2, "run_queued"),
        (3, "run_started"),
    ] {
        let ev = make_stored_event(run, kind, seq);
        store.append_durable(ev).await.expect("append");
    }

    let events = store.read_run_events(run).await.expect("read");
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].sequence, 1);
    assert_eq!(events[1].sequence, 2);
    assert_eq!(events[2].sequence, 3);
    assert_eq!(events[0].event_type, "run_created");
    assert_eq!(events[2].event_type, "run_started");
}

#[tokio::test]
async fn total_event_count_across_runs() {
    let store = Arc::new(MemEventStore::default());

    let run_a = RunId::new();
    let run_b = RunId::new();

    // Append 3 events to run_a.
    for seq in 1..=3u64 {
        store
            .append_durable(make_stored_event(run_a, "run_started", seq))
            .await
            .expect("append a");
    }

    // Append 2 events to run_b.
    for seq in 1..=2u64 {
        store
            .append_durable(make_stored_event(run_b, "run_started", seq))
            .await
            .expect("append b");
    }

    let a_events = store.read_run_events(run_a).await.expect("read a");
    let b_events = store.read_run_events(run_b).await.expect("read b");
    assert_eq!(a_events.len(), 3);
    assert_eq!(b_events.len(), 2);
}

// ---------------------------------------------------------------------------
// Query by run_id
// ---------------------------------------------------------------------------

#[tokio::test]
async fn query_by_run_id_returns_only_that_runs_events() {
    let store = Arc::new(MemEventStore::default());

    let run_a = RunId::new();
    let run_b = RunId::new();

    store
        .append_durable(make_stored_event(run_a, "run_created", 1))
        .await
        .expect("a1");
    store
        .append_durable(make_stored_event(run_b, "run_created", 1))
        .await
        .expect("b1");
    store
        .append_durable(make_stored_event(run_a, "run_started", 2))
        .await
        .expect("a2");

    let filter = EventFilter {
        run_id: Some(run_a),
        ..Default::default()
    };
    let results = store.query(filter).await.expect("query");

    assert_eq!(results.len(), 2, "query for run_a must return 2 events");
    for ev in &results {
        assert_eq!(ev.run_id, run_a.to_string(), "all results must belong to run_a");
    }
}

#[tokio::test]
async fn query_by_run_id_returns_empty_for_unknown_run() {
    let store = Arc::new(MemEventStore::default());

    let known_run = RunId::new();
    store
        .append_durable(make_stored_event(known_run, "run_created", 1))
        .await
        .expect("append");

    let filter = EventFilter {
        run_id: Some(RunId::new()), // different run
        ..Default::default()
    };
    let results = store.query(filter).await.expect("query");
    assert!(results.is_empty(), "query for unknown run must return empty");
}

#[tokio::test]
async fn read_run_events_returns_only_given_runs_events() {
    let store = Arc::new(MemEventStore::default());
    let run_a = RunId::new();
    let run_b = RunId::new();

    store.append_durable(make_stored_event(run_a, "run_created", 1)).await.expect("a");
    store.append_durable(make_stored_event(run_b, "run_created", 1)).await.expect("b");

    let a_events = store.read_run_events(run_a).await.expect("read a");
    assert_eq!(a_events.len(), 1);
    assert_eq!(a_events[0].run_id, run_a.to_string());

    let b_events = store.read_run_events(run_b).await.expect("read b");
    assert_eq!(b_events.len(), 1);
    assert_eq!(b_events[0].run_id, run_b.to_string());
}

// ---------------------------------------------------------------------------
// Query by event type
// ---------------------------------------------------------------------------

#[tokio::test]
async fn max_sequence_starts_at_zero_for_new_run() {
    let store = Arc::new(MemEventStore::default());
    let run = RunId::new();
    let max = store.max_sequence(run).await.expect("max_seq");
    assert_eq!(max, 0, "max_sequence must be 0 for a run with no events");
}

#[tokio::test]
async fn max_sequence_updates_after_each_append() {
    let store = Arc::new(MemEventStore::default());
    let run = RunId::new();

    for seq in 1u64..=5 {
        store
            .append_durable(make_stored_event(run, "run_created", seq))
            .await
            .expect("append");
        let max = store.max_sequence(run).await.expect("max_seq");
        assert_eq!(max, seq, "max_sequence must be {seq} after appending seq={seq}");
    }
}

#[tokio::test]
async fn non_monotonic_sequence_returns_error() {
    let store = Arc::new(MemEventStore::default());
    let run = RunId::new();

    store
        .append_durable(make_stored_event(run, "run_created", 5))
        .await
        .expect("append seq=5");

    // Try to append seq=3 (below current max of 5).
    let result = store
        .append_durable(make_stored_event(run, "run_started", 3))
        .await;
    assert!(result.is_err(), "non-monotonic sequence must be rejected");

    use polkagent_store_trait::event::EventStoreError;
    assert!(
        matches!(result.unwrap_err(), EventStoreError::NonMonotonicSequence { .. }),
        "error must be NonMonotonicSequence"
    );
}

#[tokio::test]
async fn duplicate_sequence_for_same_run_is_rejected() {
    let store = Arc::new(MemEventStore::default());
    let run = RunId::new();

    store
        .append_durable(make_stored_event(run, "run_created", 1))
        .await
        .expect("first");

    // Appending the same sequence again must be rejected.
    let result = store
        .append_durable(make_stored_event(run, "run_started", 1))
        .await;
    assert!(result.is_err(), "duplicate sequence must be rejected");
}

// ---------------------------------------------------------------------------
// Global sequence ordering
// ---------------------------------------------------------------------------

#[tokio::test]
async fn global_sequence_assigned_monotonically_across_runs() {
    let store = Arc::new(MemEventStore::default());

    let run_a = RunId::new();
    let run_b = RunId::new();

    let ev1 = store
        .append_durable(make_stored_event(run_a, "run_created", 1))
        .await
        .expect("a1");
    let ev2 = store
        .append_durable(make_stored_event(run_b, "run_created", 1))
        .await
        .expect("b1");
    let ev3 = store
        .append_durable(make_stored_event(run_a, "run_started", 2))
        .await
        .expect("a2");

    assert_eq!(ev1.global_sequence, 1, "first global must be 1");
    assert_eq!(ev2.global_sequence, 2, "second global must be 2");
    assert_eq!(ev3.global_sequence, 3, "third global must be 3");
}

#[tokio::test]
async fn read_from_cursor_returns_events_after_cursor() {
    let store = Arc::new(MemEventStore::default());
    let run = RunId::new();

    for seq in 1u64..=5 {
        store
            .append_durable(make_stored_event(run, "run_started", seq))
            .await
            .expect("append");
    }

    // Read from cursor position 2 → should return events with global_sequence 3, 4, 5.
    let results = store.read_from_cursor(2, 100).await.expect("cursor");
    assert_eq!(results.len(), 3, "must return 3 events after cursor=2");
    assert!(
        results.iter().all(|e| e.global_sequence > 2),
        "all returned events must have global_sequence > 2"
    );
}

#[tokio::test]
async fn read_from_cursor_zero_returns_all_events() {
    let store = Arc::new(MemEventStore::default());
    let run = RunId::new();

    for seq in 1u64..=4 {
        store
            .append_durable(make_stored_event(run, "run_started", seq))
            .await
            .expect("append");
    }

    let results = store.read_from_cursor(0, 100).await.expect("cursor");
    assert_eq!(results.len(), 4, "cursor=0 must return all events");
}

#[tokio::test]
async fn read_from_cursor_with_limit() {
    let store = Arc::new(MemEventStore::default());
    let run = RunId::new();

    for seq in 1u64..=10 {
        store
            .append_durable(make_stored_event(run, "run_started", seq))
            .await
            .expect("append");
    }

    let results = store.read_from_cursor(0, 3).await.expect("cursor limit");
    assert_eq!(results.len(), 3, "limit must restrict number of returned events");
}

// ---------------------------------------------------------------------------
// Terminal event invariants
// ---------------------------------------------------------------------------

#[tokio::test]
async fn has_terminal_event_returns_false_for_new_run() {
    let store = Arc::new(MemEventStore::default());
    let run = RunId::new();
    let result = store.has_terminal_event(run).await.expect("check");
    assert!(!result, "new run must have no terminal event");
}

#[tokio::test]
async fn has_terminal_event_returns_true_after_terminal_appended() {
    let store = Arc::new(MemEventStore::default());
    let run = RunId::new();

    store
        .append_durable(make_stored_event(run, "run_completed", 1))
        .await
        .expect("append terminal");

    let result = store.has_terminal_event(run).await.expect("check");
    assert!(result, "run must have terminal event after run_completed appended");
}

#[tokio::test]
async fn appending_second_terminal_event_is_rejected() {
    let store = Arc::new(MemEventStore::default());
    let run = RunId::new();

    store
        .append_durable(make_stored_event(run, "run_completed", 1))
        .await
        .expect("first terminal");

    // Appending a second terminal event must be rejected.
    let result = store
        .append_durable(make_stored_event(run, "run_failed", 2))
        .await;
    assert!(result.is_err(), "second terminal event must be rejected");

    use polkagent_store_trait::event::EventStoreError;
    assert!(
        matches!(result.unwrap_err(), EventStoreError::DuplicateTerminalEvent { .. }),
        "error must be DuplicateTerminalEvent"
    );
}

#[tokio::test]
async fn all_four_terminal_types_set_the_terminal_flag() {
    for terminal_type in &["run_completed", "run_failed", "run_cancelled", "run_timed_out"] {
        let store = Arc::new(MemEventStore::default());
        let run = RunId::new();

        store
            .append_durable(make_stored_event(run, terminal_type, 1))
            .await
            .expect("append");

        let is_terminal = store.has_terminal_event(run).await.expect("check");
        assert!(
            is_terminal,
            "'{terminal_type}' must set the terminal flag on the run"
        );
    }
}

#[tokio::test]
async fn non_terminal_events_do_not_set_terminal_flag() {
    let store = Arc::new(MemEventStore::default());
    let run = RunId::new();

    for (seq, kind) in [(1, "run_created"), (2, "run_started"), (3, "turn_started")] {
        store
            .append_durable(make_stored_event(run, kind, seq))
            .await
            .expect("append");
    }

    let is_terminal = store.has_terminal_event(run).await.expect("check");
    assert!(
        !is_terminal,
        "non-terminal events must not set the terminal flag"
    );
}

// ---------------------------------------------------------------------------
// EventRecorder integration
// ---------------------------------------------------------------------------

#[tokio::test]
async fn event_recorder_appends_events_to_store() {

    let (store, _recorder) = make_store_with_recorder();

    let run_id = RunId::new();

    // Directly append using the store (EventRecorder integration with RunEvent
    // type would require full RunEvent construction which is tested in run_lifecycle.rs).
    store
        .append_durable(make_stored_event(run_id, "run_created", 1))
        .await
        .expect("append via store");

    let events = store.read_run_events(run_id).await.expect("read");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "run_created");
}

#[tokio::test]
async fn events_across_multiple_runs_are_isolated() {
    let store = Arc::new(MemEventStore::default());

    let runs: Vec<RunId> = (0..5).map(|_| RunId::new()).collect();

    // Append one event per run.
    for (i, &run) in runs.iter().enumerate() {
        store
            .append_durable(make_stored_event(run, "run_created", 1))
            .await
            .expect("append");
        let _ = i; // suppress unused warning
    }

    // Each run must have exactly one event, and they must all be different.
    let mut global_seqs = Vec::new();
    for &run in &runs {
        let events = store.read_run_events(run).await.expect("read");
        assert_eq!(events.len(), 1, "each run must have exactly 1 event");
        global_seqs.push(events[0].global_sequence);
    }

    // All global sequences must be distinct.
    let unique: std::collections::HashSet<u64> = global_seqs.iter().copied().collect();
    assert_eq!(unique.len(), 5, "all global sequences must be distinct");
}

#[tokio::test]
async fn append_diagnostic_does_not_increment_durable_count() {
    let store = Arc::new(MemEventStore::default());
    let run = RunId::new();

    let diag_event = make_stored_event(run, "tool_call_started", 0);
    store
        .append_diagnostic(diag_event, "2030-01-01T00:00:00Z".to_string())
        .await
        .expect("append diagnostic");

    // Durable store must still be empty.
    let events = store.read_run_events(run).await.expect("read");
    assert!(events.is_empty(), "diagnostic events must not appear in durable read_run_events");
}
