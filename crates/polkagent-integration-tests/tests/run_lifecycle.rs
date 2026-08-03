//! Full run lifecycle integration test.
//!
//! Creates an agent, drives a run through every lifecycle state, and verifies
//! that the event store has a complete, ordered record of all transitions.

use polkagent_core::{AgentId, RunState};
use polkagent_integration_tests::make_run_manager;
use polkagent_store_trait::event::{EventFilter, EventStore};

// ---------------------------------------------------------------------------
// Happy-path: Created -> Queued -> Running -> Completing -> Completed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_lifecycle_created_to_completed() {
    let (mgr, event_store) = make_run_manager();
    let agent_id = AgentId::new();

    // 1. Create
    let run_id = mgr.create_run(agent_id).await.expect("create_run");
    let state = mgr.get_state(run_id.clone()).await.expect("state");
    assert_eq!(state, RunState::Created);

    // 2. Enqueue (Created -> Queued)
    mgr.enqueue_run(run_id.clone()).await.expect("enqueue");
    let state = mgr.get_state(run_id.clone()).await.expect("state");
    assert_eq!(state, RunState::Queued);

    // 3. Start (Queued -> Running)
    mgr.start_run(run_id.clone()).await.expect("start");
    let state = mgr.get_state(run_id.clone()).await.expect("state");
    assert_eq!(state, RunState::Running);

    // 4. Completing (Running -> Completing)
    mgr.completing_run(run_id.clone()).await.expect("completing");
    let state = mgr.get_state(run_id.clone()).await.expect("state");
    assert_eq!(state, RunState::Completing);

    // 5. Completed (Completing -> Completed)
    mgr.complete_run(run_id.clone(), None, 0, 0)
        .await
        .expect("complete");
    let state = mgr.get_state(run_id.clone()).await.expect("state");
    assert_eq!(state, RunState::Completed);

    // Verify events recorded for each transition.
    let events = event_store
        .read_run_events(run_id.clone())
        .await
        .expect("read events");

    let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert_eq!(
        types,
        &[
            "run_created",
            "run_queued",
            "run_started",
            "run_completing",
            "run_completed",
        ],
        "expected all lifecycle events in order"
    );

    // Verify monotonic sequence numbers.
    for window in events.windows(2) {
        assert!(
            window[0].sequence < window[1].sequence,
            "event sequences must be strictly monotonic"
        );
    }
}

// ---------------------------------------------------------------------------
// Events are queryable via the filter API
// ---------------------------------------------------------------------------

#[tokio::test]
async fn events_are_queryable_via_event_filter() {
    let (mgr, event_store) = make_run_manager();
    let agent_id = AgentId::new();

    let run_id = mgr.create_run(agent_id).await.expect("create");
    mgr.enqueue_run(run_id.clone()).await.expect("enqueue");
    mgr.start_run(run_id.clone()).await.expect("start");

    // Query with run_id filter.
    let filter = EventFilter {
        run_id: Some(run_id.clone()),
        ..Default::default()
    };
    let results = event_store.query(filter).await.expect("query");
    assert_eq!(results.len(), 3, "3 events for this run");

    // Query without filter returns all events.
    let all = event_store
        .query(EventFilter::default())
        .await
        .expect("query all");
    assert!(
        all.len() >= 3,
        "unfiltered query should include at least the 3 events"
    );
}

// ---------------------------------------------------------------------------
// Events are readable via cursor-based API
// ---------------------------------------------------------------------------

#[tokio::test]
async fn events_readable_from_cursor() {
    let (mgr, event_store) = make_run_manager();
    let agent_id = AgentId::new();

    let run_id = mgr.create_run(agent_id).await.expect("create");
    mgr.enqueue_run(run_id.clone()).await.expect("enqueue");

    // Read from cursor 0 should return all events.
    let page1 = event_store
        .read_from_cursor(0, 100)
        .await
        .expect("cursor read");
    assert_eq!(page1.len(), 2);

    // Read from cursor after the first event should return only the second.
    let first_gseq = page1[0].global_sequence;
    let page2 = event_store
        .read_from_cursor(first_gseq, 100)
        .await
        .expect("cursor read 2");
    assert_eq!(page2.len(), 1);
    assert_eq!(page2[0].event_type, "run_queued");
}

// ---------------------------------------------------------------------------
// Terminal event invariant
// ---------------------------------------------------------------------------

#[tokio::test]
async fn terminal_run_cannot_be_cancelled() {
    let (mgr, _) = make_run_manager();
    let agent_id = AgentId::new();
    let run_id = mgr.create_run(agent_id).await.expect("create");

    mgr.enqueue_run(run_id.clone()).await.expect("enqueue");
    mgr.start_run(run_id.clone()).await.expect("start");
    mgr.completing_run(run_id.clone()).await.expect("completing");
    mgr.complete_run(run_id.clone(), None, 0, 0)
        .await
        .expect("complete");

    // Attempting to cancel a completed run must fail.
    let err = mgr
        .cancel_run(run_id.clone(), "too late")
        .await
        .expect_err("should fail");
    assert!(
        format!("{err:?}").contains("Transition"),
        "expected a transition error, got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Multiple independent runs do not interfere
// ---------------------------------------------------------------------------

#[tokio::test]
async fn two_runs_have_independent_event_sequences() {
    let (mgr, event_store) = make_run_manager();

    let run_a = mgr.create_run(AgentId::new()).await.expect("create a");
    let run_b = mgr.create_run(AgentId::new()).await.expect("create b");

    mgr.enqueue_run(run_a.clone()).await.expect("enqueue a");
    mgr.enqueue_run(run_b.clone()).await.expect("enqueue b");

    let a_events = event_store
        .read_run_events(run_a)
        .await
        .expect("read a");
    let b_events = event_store
        .read_run_events(run_b)
        .await
        .expect("read b");

    // Each run should have exactly 2 events with independent per-run sequences.
    assert_eq!(a_events.len(), 2);
    assert_eq!(b_events.len(), 2);
    assert_eq!(a_events[0].sequence, 1);
    assert_eq!(a_events[1].sequence, 2);
    assert_eq!(b_events[0].sequence, 1);
    assert_eq!(b_events[1].sequence, 2);
}
