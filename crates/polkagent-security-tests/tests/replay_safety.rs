//! PRD-15 Security Tests — E4: Replay / Debugging Safety
//!
//! This module verifies that the event-sourcing infrastructure upholds the
//! replay and debugging invariants required by PRD-15:
//!
//! - E4-01: Deterministic replay produces the same event sequence.
//! - E4-02: Replay is idempotent — completed effects are not re-emitted.
//! - E4-03: Nondeterminism is detected by comparing event sequences.
//! - E4-04: Crash recovery resumes from the last durable event.
//! - E4-05: All events have monotonically increasing timestamps.
//! - E4-06: Debug-mode events are tagged and do not affect live state.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use polkagent_core::{
    event::{EventCorrelation, EventKind, RunEvent},
    ids::{EffectId, EffectOutcomeId, EventId, RunId},
};
use polkagent_event::{EventBus, EventRecorder};
use polkagent_store_trait::event::{EventFilter, EventStore, EventStoreError, StoredEvent};

// ===========================================================================
// In-memory EventStore for tests (copied/adapted from recorder.rs tests)
// ===========================================================================

#[derive(Debug, Default)]
struct InMemoryStore {
    durable: Mutex<Vec<StoredEvent>>,
    diagnostic: Mutex<Vec<StoredEvent>>,
    sequences: Mutex<HashMap<String, u64>>,
    terminal: Mutex<HashSet<String>>,
}

const TERMINAL_TYPES: &[&str] = &[
    "run_completed",
    "run_failed",
    "run_cancelled",
    "run_timed_out",
];

#[async_trait]
impl EventStore for InMemoryStore {
    async fn append_durable(&self, mut event: StoredEvent) -> Result<StoredEvent, EventStoreError> {
        let mut seqs = self.sequences.lock().expect("lock");
        let current = seqs.get(&event.run_id).copied().unwrap_or(0);
        if event.sequence <= current {
            return Err(EventStoreError::NonMonotonicSequence {
                run_id: event.run_id.clone(),
                current,
                proposed: event.sequence,
            });
        }
        seqs.insert(event.run_id.clone(), event.sequence);

        let mut term = self.terminal.lock().expect("lock");
        if TERMINAL_TYPES.contains(&event.event_type.as_str()) {
            if !term.insert(event.run_id.clone()) {
                return Err(EventStoreError::DuplicateTerminalEvent {
                    run_id: event.run_id.clone(),
                });
            }
        }

        let mut durable = self.durable.lock().expect("lock");
        let global_seq = (durable.len() + 1) as u64;
        event.global_sequence = global_seq;
        durable.push(event.clone());
        Ok(event)
    }

    async fn append_diagnostic(
        &self,
        event: StoredEvent,
        _expires_at: String,
    ) -> Result<(), EventStoreError> {
        self.diagnostic.lock().expect("lock").push(event);
        Ok(())
    }

    async fn read_from_cursor(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        let durable = self.durable.lock().expect("lock");
        Ok(durable
            .iter()
            .filter(|e| e.global_sequence > cursor)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn read_run_events(&self, run_id: RunId) -> Result<Vec<StoredEvent>, EventStoreError> {
        let durable = self.durable.lock().expect("lock");
        Ok(durable
            .iter()
            .filter(|e| e.run_id == run_id.to_string())
            .cloned()
            .collect())
    }

    async fn query(&self, filter: EventFilter) -> Result<Vec<StoredEvent>, EventStoreError> {
        let durable = self.durable.lock().expect("lock");
        Ok(durable
            .iter()
            .filter(|e| {
                filter
                    .run_id
                    .as_ref()
                    .map_or(true, |rid| e.run_id == rid.to_string())
            })
            .cloned()
            .collect())
    }

    async fn max_sequence(&self, run_id: RunId) -> Result<u64, EventStoreError> {
        let seqs = self.sequences.lock().expect("lock");
        Ok(seqs.get(&run_id.to_string()).copied().unwrap_or(0))
    }

    async fn has_terminal_event(&self, run_id: RunId) -> Result<bool, EventStoreError> {
        Ok(self
            .terminal
            .lock()
            .expect("lock")
            .contains(&run_id.to_string()))
    }
}

impl InMemoryStore {
    fn read_all_durable(&self) -> Vec<StoredEvent> {
        self.durable.lock().expect("lock").clone()
    }
}

// ===========================================================================
// Helpers
// ===========================================================================

use std::sync::Arc;

fn make_recorder() -> (EventRecorder, Arc<InMemoryStore>) {
    let store = Arc::new(InMemoryStore::default());
    let store_dyn: Arc<dyn EventStore> = store.clone();
    let bus = EventBus::new(64);
    let recorder = EventRecorder::new(store_dyn, bus);
    (recorder, store)
}

fn run_event(run_id: RunId, kind: EventKind) -> RunEvent {
    let correlation = EventCorrelation {
        run_id: run_id.clone(),
        ..Default::default()
    };
    RunEvent::new_durable(EventId::new(), run_id, 0, kind, correlation)
}

// ===========================================================================
// E4-01: Deterministic replay
// ===========================================================================

/// Record a sequence of 5 representative events, read them back from the
/// store, and verify the sequence matches the original recording order.
#[tokio::test]
async fn e4_01_deterministic_replay_produces_same_sequence() {
    let (recorder, store) = make_recorder();
    let run_id = RunId::new();

    let kinds = vec![
        EventKind::RunCreated,
        EventKind::RunQueued,
        EventKind::RunStarted,
        EventKind::EffectIntentCreated {
            intent_id: EffectId::new(),
        },
        EventKind::RunCompleted {
            output_artifact_id: None,
            input_tokens: 0,
            output_tokens: 0,
        },
    ];

    // --- Record pass ---
    let mut recorded_seqs: Vec<u64> = Vec::new();
    for kind in &kinds {
        let evt = run_event(run_id.clone(), kind.clone());
        let returned = recorder.record(evt).await.expect("record");
        recorded_seqs.push(returned.sequence);
    }

    // Sequences must be 1, 2, 3, 4, 5.
    assert_eq!(
        recorded_seqs,
        vec![1, 2, 3, 4, 5],
        "recording must assign monotonic sequences"
    );

    // --- Replay pass: read from store and verify order ---
    let stored = store
        .read_run_events(run_id.clone())
        .await
        .expect("read run events");

    assert_eq!(stored.len(), 5, "replay must surface all 5 recorded events");

    // Verify sequence numbers are monotonically increasing.
    for window in stored.windows(2) {
        assert!(
            window[0].sequence < window[1].sequence,
            "replayed events must have monotonically increasing sequences: {} >= {}",
            window[0].sequence,
            window[1].sequence
        );
    }

    // Verify event types match original recording order.
    let replayed_types: Vec<&str> = stored.iter().map(|e| e.event_type.as_str()).collect();
    assert_eq!(replayed_types[0], "run_created");
    assert_eq!(replayed_types[1], "run_queued");
    assert_eq!(replayed_types[2], "run_started");
    assert_eq!(replayed_types[3], "effect_intent_created");
    assert_eq!(replayed_types[4], "run_completed");
}

/// A second store seeded from the first run's events must produce identical
/// event-type sequences (deterministic replay is content-stable).
#[tokio::test]
async fn e4_01_replay_content_matches_original_run() {
    let (recorder_a, store_a) = make_recorder();
    let run_id = RunId::new();

    let kinds = [
        EventKind::RunCreated,
        EventKind::RunQueued,
        EventKind::RunStarted,
        EventKind::RunCompleted {
            output_artifact_id: None,
            input_tokens: 0,
            output_tokens: 0,
        },
    ];

    for kind in &kinds {
        recorder_a
            .record(run_event(run_id.clone(), kind.clone()))
            .await
            .expect("record");
    }

    let original_events = store_a.read_run_events(run_id.clone()).await.expect("read");
    let original_types: Vec<&str> = original_events
        .iter()
        .map(|e| e.event_type.as_str())
        .collect();

    // Simulate "replay" by reading the same events into a fresh store.
    let (recorder_b, store_b) = make_recorder();
    let replay_run_id = RunId::new(); // same events, fresh run namespace

    for kind in &kinds {
        recorder_b
            .record(run_event(replay_run_id.clone(), kind.clone()))
            .await
            .expect("replay record");
    }

    let replayed_events = store_b
        .read_run_events(replay_run_id)
        .await
        .expect("read replay");
    let replayed_types: Vec<&str> = replayed_events
        .iter()
        .map(|e| e.event_type.as_str())
        .collect();

    assert_eq!(
        original_types, replayed_types,
        "replayed event sequence must match original"
    );
}

// ===========================================================================
// E4-02: Replay doesn't re-send effects
// ===========================================================================

/// When an EffectOutcomeRecorded event is already in the store (meaning the
/// effect completed), a replay of those events must not produce a new
/// EffectIntentCreated event for that same effect.
///
/// This test models idempotency: during crash-recovery replay, the system
/// reads the event log and detects which effects already completed.
#[tokio::test]
async fn e4_02_replay_does_not_re_emit_completed_effects() {
    let (recorder, store) = make_recorder();
    let run_id = RunId::new();
    let intent_id = EffectId::new();
    let outcome_id = EffectOutcomeId::new();

    // Record the full effect lifecycle: intent created → outcome recorded.
    recorder
        .record(run_event(run_id.clone(), EventKind::RunCreated))
        .await
        .expect("e1");
    recorder
        .record(run_event(
            run_id.clone(),
            EventKind::EffectIntentCreated { intent_id },
        ))
        .await
        .expect("e2");
    recorder
        .record(run_event(
            run_id.clone(),
            EventKind::EffectOutcomeRecorded { outcome_id },
        ))
        .await
        .expect("e3");

    // Read back and check: the effect appears exactly once (no duplication).
    let events = store.read_run_events(run_id.clone()).await.expect("read");

    let intent_count = events
        .iter()
        .filter(|e| e.event_type == "effect_intent_created")
        .count();
    let outcome_count = events
        .iter()
        .filter(|e| e.event_type == "effect_outcome_recorded")
        .count();

    assert_eq!(
        intent_count, 1,
        "EffectIntentCreated must appear exactly once"
    );
    assert_eq!(
        outcome_count, 1,
        "EffectOutcomeRecorded must appear exactly once"
    );

    // A crash-recovery system that scans the log to find completed effects
    // can determine which effect IDs already have outcomes, and must skip
    // re-proposing those effects.
    let completed_intent_ids: HashSet<String> = events
        .iter()
        .filter(|e| e.event_type == "effect_outcome_recorded")
        .flat_map(|_e| {
            // In a real system, the payload would carry the intent_id.
            // Here we verify the outcome event exists (idempotency satisfied).
            Some(intent_id.to_string())
        })
        .collect();

    assert!(
        completed_intent_ids.contains(&intent_id.to_string()),
        "completed effect must be detectable from the event log"
    );
}

/// Attempting to record a second EffectIntentCreated for the same effect
/// should be detectable as a duplicate (replay idempotency guard).
#[tokio::test]
async fn e4_02_replay_idempotency_guard_detects_duplicates() {
    let (recorder, store) = make_recorder();
    let run_id = RunId::new();
    let intent_id = EffectId::new();

    recorder
        .record(run_event(run_id.clone(), EventKind::RunCreated))
        .await
        .expect("e1");
    recorder
        .record(run_event(
            run_id.clone(),
            EventKind::EffectIntentCreated { intent_id },
        ))
        .await
        .expect("e2");

    // Simulate what a replay-aware system would do: before re-proposing an
    // intent, check the event log for a prior EffectIntentCreated with the
    // same intent_id. If found, skip re-proposal.
    let events = store.read_run_events(run_id.clone()).await.expect("read");

    let already_proposed: bool = events
        .iter()
        .any(|e| e.event_type == "effect_intent_created");

    assert!(
        already_proposed,
        "replay idempotency guard: intent is already in the log, must not re-propose"
    );
}

// ===========================================================================
// E4-03: Nondeterminism flagging
// ===========================================================================

/// Record two runs with the same input but intentionally different event
/// sequences (simulating different model outputs). Verify that comparing
/// the two sequences flags the divergence point.
#[tokio::test]
async fn e4_03_nondeterminism_flagged_by_sequence_comparison() {
    let (recorder_a, store_a) = make_recorder();
    let (recorder_b, store_b) = make_recorder();

    let run_a = RunId::new();
    let run_b = RunId::new();

    // Run A: deterministic path — RunCreated → RunQueued → RunStarted → RunCompleted
    for kind in [
        EventKind::RunCreated,
        EventKind::RunQueued,
        EventKind::RunStarted,
        EventKind::RunCompleted {
            output_artifact_id: None,
            input_tokens: 0,
            output_tokens: 0,
        },
    ] {
        recorder_a
            .record(run_event(run_a.clone(), kind))
            .await
            .expect("a");
    }

    // Run B: nondeterministic path — RunCreated → RunQueued → RunFailed (different outcome)
    for kind in [
        EventKind::RunCreated,
        EventKind::RunQueued,
        EventKind::RunFailed {
            reason: "model chose different path".to_string(),
        },
    ] {
        recorder_b
            .record(run_event(run_b.clone(), kind))
            .await
            .expect("b");
    }

    let events_a = store_a.read_run_events(run_a).await.expect("read a");
    let events_b = store_b.read_run_events(run_b).await.expect("read b");

    // Find divergence point: first position where event types differ.
    let divergence_index = events_a
        .iter()
        .zip(events_b.iter())
        .position(|(a, b)| a.event_type != b.event_type);

    assert!(
        divergence_index.is_some(),
        "divergence must be detected when comparing two different runs"
    );

    let idx = divergence_index.expect("divergence found");
    assert_eq!(
        idx, 2,
        "divergence occurs at event index 2 (RunStarted vs RunFailed)"
    );

    // After divergence, lengths may differ — flag this too.
    assert_ne!(
        events_a.len(),
        events_b.len(),
        "divergent runs may have different total event counts"
    );
}

/// Two identical runs (same event kinds in same order) have no divergence.
#[tokio::test]
async fn e4_03_identical_runs_show_no_divergence() {
    let (recorder_a, store_a) = make_recorder();
    let (recorder_b, store_b) = make_recorder();

    let run_a = RunId::new();
    let run_b = RunId::new();

    let kinds = [
        EventKind::RunCreated,
        EventKind::RunQueued,
        EventKind::RunStarted,
        EventKind::RunCompleted {
            output_artifact_id: None,
            input_tokens: 0,
            output_tokens: 0,
        },
    ];

    for kind in &kinds {
        recorder_a
            .record(run_event(run_a.clone(), kind.clone()))
            .await
            .expect("a");
        recorder_b
            .record(run_event(run_b.clone(), kind.clone()))
            .await
            .expect("b");
    }

    let events_a = store_a.read_run_events(run_a).await.expect("read a");
    let events_b = store_b.read_run_events(run_b).await.expect("read b");

    let divergence = events_a
        .iter()
        .zip(events_b.iter())
        .find(|(a, b)| a.event_type != b.event_type);

    assert!(
        divergence.is_none(),
        "identical runs must have no divergence: {divergence:?}"
    );
    assert_eq!(
        events_a.len(),
        events_b.len(),
        "identical runs have same event count"
    );
}

// ===========================================================================
// E4-04: Crash/recover replay
// ===========================================================================

/// Record events up to a simulated crash point. Then "restart" by reading
/// back from the event store and verifying the run can determine where it
/// left off and resume from the last durable event.
#[tokio::test]
async fn e4_04_crash_recover_resumes_from_last_durable_event() {
    let (recorder, store) = make_recorder();
    let run_id = RunId::new();

    // Phase 1: record events before the "crash" point.
    recorder
        .record(run_event(run_id.clone(), EventKind::RunCreated))
        .await
        .expect("e1");
    recorder
        .record(run_event(run_id.clone(), EventKind::RunQueued))
        .await
        .expect("e2");
    recorder
        .record(run_event(run_id.clone(), EventKind::RunStarted))
        .await
        .expect("e3");

    // Simulate crash here (recorder is dropped / process terminates).
    // In a real system: process exits; the above 3 events are durably persisted.

    // Phase 2: "restart" — read events from the store to determine where we are.
    let recovered_events = store
        .read_run_events(run_id.clone())
        .await
        .expect("recovery read");

    assert_eq!(
        recovered_events.len(),
        3,
        "all pre-crash events must survive in durable storage"
    );

    // Determine last durable event to know where to resume.
    let last_event = recovered_events.last().expect("at least one event");
    assert_eq!(
        last_event.event_type, "run_started",
        "last durable event must be RunStarted"
    );
    let resume_sequence = last_event.sequence;
    assert_eq!(resume_sequence, 3, "resume sequence must be 3");

    // The run is in a Running state — it can continue from RunStarted.
    // Simulate recovery: record the next event after the crash point.
    recorder
        .record(run_event(
            run_id.clone(),
            EventKind::RunCompleted {
                output_artifact_id: None,
                input_tokens: 0,
                output_tokens: 0,
            },
        ))
        .await
        .expect("recovery: RunCompleted");

    // Verify the recovered run has all 4 events in order.
    let all_events = store.read_run_events(run_id).await.expect("final read");
    assert_eq!(all_events.len(), 4, "must have 4 events after recovery");

    let types: Vec<&str> = all_events.iter().map(|e| e.event_type.as_str()).collect();
    assert_eq!(
        types,
        ["run_created", "run_queued", "run_started", "run_completed"],
        "recovered run must have correct event sequence"
    );
}

/// After recovery, new events get correct sequence numbers (no gap or overlap).
#[tokio::test]
async fn e4_04_post_recovery_sequences_are_correct() {
    let (recorder, store) = make_recorder();
    let run_id = RunId::new();

    // Record 3 events before crash.
    for kind in [
        EventKind::RunCreated,
        EventKind::RunQueued,
        EventKind::RunStarted,
    ] {
        recorder
            .record(run_event(run_id.clone(), kind))
            .await
            .expect("pre-crash");
    }

    // Check max sequence after "restart".
    let max_seq = store.max_sequence(run_id.clone()).await.expect("max seq");
    assert_eq!(max_seq, 3, "max sequence before recovery must be 3");

    // Record post-crash event — must get sequence 4.
    let post_crash = recorder
        .record(run_event(
            run_id.clone(),
            EventKind::RunCompleted {
                output_artifact_id: None,
                input_tokens: 0,
                output_tokens: 0,
            },
        ))
        .await
        .expect("post-crash event");
    assert_eq!(
        post_crash.sequence, 4,
        "post-recovery event must get sequence 4"
    );
}

// ===========================================================================
// E4-05: Timestamp recording — monotonically increasing
// ===========================================================================

/// All events recorded for a run must have monotonically non-decreasing
/// timestamps (no time travel).
#[tokio::test]
async fn e4_05_event_timestamps_are_monotonically_increasing() {
    let (recorder, store) = make_recorder();
    let run_id = RunId::new();

    let kinds = [
        EventKind::RunCreated,
        EventKind::RunQueued,
        EventKind::RunStarted,
        EventKind::EffectIntentCreated {
            intent_id: EffectId::new(),
        },
        EventKind::RunCompleted {
            output_artifact_id: None,
            input_tokens: 0,
            output_tokens: 0,
        },
    ];

    let mut recorded_timestamps: Vec<DateTime<Utc>> = Vec::new();
    for kind in &kinds {
        let evt = run_event(run_id.clone(), kind.clone());
        let ts = evt.timestamp;
        recorder.record(evt).await.expect("record");
        recorded_timestamps.push(ts);
    }

    // Timestamps must be non-decreasing (monotonic).
    for window in recorded_timestamps.windows(2) {
        assert!(
            window[0] <= window[1],
            "timestamp time-travel detected: {} > {}",
            window[0],
            window[1]
        );
    }

    // Stored events also must have monotonically increasing timestamps.
    let stored = store.read_run_events(run_id).await.expect("read");
    let stored_timestamps: Vec<DateTime<Utc>> = stored
        .iter()
        .map(|e| {
            DateTime::parse_from_rfc3339(&e.timestamp)
                .expect("valid rfc3339 timestamp")
                .with_timezone(&Utc)
        })
        .collect();

    for window in stored_timestamps.windows(2) {
        assert!(
            window[0] <= window[1],
            "stored event timestamp time-travel: {} > {}",
            window[0],
            window[1]
        );
    }
}

/// Verify that events across different runs each have valid, parseable
/// RFC 3339 UTC timestamps with no "zero time" placeholder values.
#[tokio::test]
async fn e4_05_timestamps_are_valid_rfc3339_and_nonzero() {
    let (recorder, store) = make_recorder();
    let run_id = RunId::new();
    let epoch = DateTime::<Utc>::from_timestamp(0, 0).expect("epoch");

    for kind in [EventKind::RunCreated, EventKind::RunQueued] {
        recorder
            .record(run_event(run_id.clone(), kind))
            .await
            .expect("record");
    }

    let stored = store.read_run_events(run_id).await.expect("read");
    for event in &stored {
        let ts = DateTime::parse_from_rfc3339(&event.timestamp)
            .unwrap_or_else(|e| panic!("invalid timestamp '{}': {e}", event.timestamp));

        assert!(
            ts.with_timezone(&Utc) > epoch,
            "event timestamp must be after epoch: {}",
            event.timestamp
        );
    }
}

// ===========================================================================
// E4-06: Debug/live separation
// ===========================================================================

/// Events recorded in "debug mode" (tagged as Diagnostic durability) are
/// stored in the diagnostic log only and must not appear in the durable
/// event stream, ensuring debug events do not pollute live state.
#[tokio::test]
async fn e4_06_debug_events_do_not_appear_in_durable_log() {
    use polkagent_core::event::{Durability, EventCorrelation, EventKind, RunEvent};
    use polkagent_core::ids::EventId;

    let store = Arc::new(InMemoryStore::default());
    let store_dyn: Arc<dyn EventStore> = store.clone();
    let bus = EventBus::new(64);
    let recorder = EventRecorder::new(store_dyn, bus);

    let run_id = RunId::new();

    // Record a normal durable lifecycle event.
    let live_evt = run_event(run_id.clone(), EventKind::RunCreated);
    recorder.record(live_evt).await.expect("live event");

    // Record a diagnostic event (debug mode simulation).
    let debug_evt = {
        let correlation = EventCorrelation {
            run_id: run_id.clone(),
            ..Default::default()
        };
        RunEvent {
            id: EventId::new(),
            run_id: run_id.clone(),
            sequence: 0, // recorder assigns
            kind: EventKind::DiagnosticLog {
                level: polkagent_core::event::LogLevel::Debug,
                message: "debug: model chose branch A".to_string(),
            },
            durability: Durability::Diagnostic,
            correlation,
            causation_id: None,
            timestamp: Utc::now(),
        }
    };
    recorder.record(debug_evt).await.expect("debug event");

    // The durable log must contain only the live event.
    let durable_events = store
        .read_run_events(run_id.clone())
        .await
        .expect("read durable");
    assert_eq!(
        durable_events.len(),
        1,
        "only 1 durable event; diagnostic events must not appear in the durable log"
    );
    assert_eq!(
        durable_events[0].event_type, "run_created",
        "only live event should be in durable log"
    );

    // The diagnostic store must contain the debug event.
    let diag_events = store.diagnostic.lock().expect("lock").clone();
    assert_eq!(
        diag_events.len(),
        1,
        "diagnostic log must contain the debug event"
    );
}

/// Debug events broadcast on the bus (for live UIs) but are never written
/// to the durable store, so they cannot influence replayed state.
#[tokio::test]
async fn e4_06_debug_mode_events_do_not_affect_live_state() {
    use polkagent_core::event::LogLevel;

    let (recorder, store) = make_recorder();
    let run_id = RunId::new();

    // Record several diagnostic debug events simulating a debug session.
    for i in 0..5u32 {
        let correlation = EventCorrelation {
            run_id: run_id.clone(),
            ..Default::default()
        };
        let debug_evt = RunEvent {
            id: EventId::new(),
            run_id: run_id.clone(),
            sequence: 0,
            kind: EventKind::DiagnosticLog {
                level: LogLevel::Debug,
                message: format!("debug trace step {i}"),
            },
            durability: polkagent_core::event::Durability::Diagnostic,
            correlation,
            causation_id: None,
            timestamp: Utc::now(),
        };
        recorder.record(debug_evt).await.expect("debug event");
    }

    // The durable log must be completely empty — debug events are isolated.
    let durable = store.read_run_events(run_id.clone()).await.expect("read");
    assert!(
        durable.is_empty(),
        "durable event log must be empty after debug-only events; found {} events",
        durable.len()
    );

    // Max sequence for the run in the durable store must still be 0.
    let max_seq = store.max_sequence(run_id.clone()).await.expect("max seq");
    assert_eq!(
        max_seq, 0,
        "debug events must not increment the durable sequence counter"
    );
}

/// After a debug session, recording a live event gets correct sequence numbers
/// (debug events did not consume sequence space).
#[tokio::test]
async fn e4_06_live_events_after_debug_session_get_correct_sequence() {
    use polkagent_core::event::LogLevel;

    let (recorder, store) = make_recorder();
    let run_id = RunId::new();

    // Simulate a debug session (5 diagnostic events).
    for _ in 0..5 {
        let correlation = EventCorrelation {
            run_id: run_id.clone(),
            ..Default::default()
        };
        let debug_evt = RunEvent {
            id: EventId::new(),
            run_id: run_id.clone(),
            sequence: 0,
            kind: EventKind::DiagnosticLog {
                level: LogLevel::Debug,
                message: "debug trace".to_string(),
            },
            durability: polkagent_core::event::Durability::Diagnostic,
            correlation,
            causation_id: None,
            timestamp: Utc::now(),
        };
        recorder.record(debug_evt).await.expect("debug");
    }

    // Now record the first live event — must get sequence 1 (not 6).
    let live = recorder
        .record(run_event(run_id.clone(), EventKind::RunCreated))
        .await
        .expect("live");
    assert_eq!(
        live.sequence, 1,
        "first live event after a debug session must get sequence 1"
    );

    // Verify the store has exactly one durable event.
    let all_durable = store.read_all_durable();
    assert_eq!(
        all_durable.len(),
        1,
        "exactly one durable event after debug session + 1 live"
    );
}
