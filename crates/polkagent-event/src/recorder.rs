//! [`EventRecorder`]: append events to an [`EventStore`], enforce ordering
//! invariants, then broadcast on the in-process [`EventBus`].
//!
//! # Contract
//!
//! 1. **Persist before broadcast.** Durable events are written to the store
//!    before they are published on the bus.  Ephemeral events skip the store.
//!
//! 2. **Monotonic sequences.** The recorder assigns the per-run `sequence`
//!    number.  It fetches the current maximum from the store and increments it.
//!    A concurrent writer that violates ordering is rejected by the store's
//!    `UNIQUE(run_id, sequence)` constraint.
//!
//! 3. **Single terminal event.** Before writing a terminal event
//!    (`RunCompleted`, `RunFailed`, `RunCancelled`, `RunTimedOut`) the recorder
//!    checks the store and rejects the write with
//!    [`EventError::DuplicateTerminalEvent`] if one already exists.

use std::sync::Arc;

use chrono::Utc;
use polkagent_core::{
    event::{EventKind, RunEvent},
    DurabilityClass,
};
use polkagent_store_trait::event::{EventStore, EventStoreError, StoredEvent};
use serde_json;
use tracing::{debug, error, instrument};

use crate::{bus::EventBus, error::EventError, types::event_type_of};

// ---------------------------------------------------------------------------
// EventRecorder
// ---------------------------------------------------------------------------

/// Records [`RunEvent`]s to an [`EventStore`] and broadcasts them on an
/// [`EventBus`].
///
/// The recorder is cheaply cloneable (Arc-backed internally) so it can be
/// shared across Tokio tasks without wrapping in another `Arc`.
#[derive(Clone)]
pub struct EventRecorder {
    store: Arc<dyn EventStore>,
    bus: EventBus,
}

impl std::fmt::Debug for EventRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventRecorder")
            .field("bus_subscribers", &self.bus.subscriber_count())
            .finish_non_exhaustive()
    }
}

impl EventRecorder {
    /// Create a new recorder backed by the given store and bus.
    #[must_use]
    pub fn new(store: Arc<dyn EventStore>, bus: EventBus) -> Self {
        Self { store, bus }
    }

    /// Record an event to the store and broadcast it on the bus.
    ///
    /// For **durable** events (`Durability::Durable`) the recorder:
    /// 1. Checks the duplicate-terminal-event invariant.
    /// 2. Fetches the current maximum sequence for the run.
    /// 3. Assigns `sequence = max + 1`.
    /// 4. Writes the event to the store (the store enforces uniqueness).
    /// 5. Publishes the event on the bus.
    ///
    /// For **ephemeral** and **diagnostic** events the event is only published
    /// on the bus (no store write).
    ///
    /// # Errors
    ///
    /// - [`EventError::DuplicateTerminalEvent`] if the event is terminal and
    ///   the run already has a terminal event.
    /// - [`EventError::NonMonotonicSequence`] if the store rejects the
    ///   sequence (concurrent writer race).
    /// - [`EventError::Store`] for any other store error.
    /// - [`EventError::Serialisation`] if the event payload cannot be
    ///   serialised.
    #[instrument(
        skip(self, event),
        fields(
            run_id = %event.run_id,
            kind   = ?event.kind,
        )
    )]
    pub async fn record(&self, mut event: RunEvent) -> Result<RunEvent, EventError> {
        if event.correlation.run_id != event.run_id {
            return Err(EventError::Store(EventStoreError::Serialisation(
                "event correlation.run_id does not match event.run_id".to_owned(),
            )));
        }
        let durability = durability_of(&event.kind);

        match durability {
            DurabilityClass::Durable => {
                self.record_durable(&mut event).await?;
            }
            DurabilityClass::Diagnostic => {
                // Diagnostic events: write to store with expiry, then broadcast.
                self.record_diagnostic(&mut event).await?;
            }
            DurabilityClass::Ephemeral => {
                // Ephemeral events: skip the store, publish only.
                debug!("publishing ephemeral event (no store write)");
            }
        }

        // Broadcast on bus regardless of durability.
        self.bus.publish(event.clone());

        Ok(event)
    }

    // ── private helpers ───────────────────────────────────────────────────

    async fn record_durable(&self, event: &mut RunEvent) -> Result<(), EventError> {
        let run_id = event.run_id;
        let is_terminal = is_terminal_kind(&event.kind);

        // Invariant: at most one terminal event per run (PRD-10 REQ-EVT-004).
        if is_terminal {
            let has_terminal = self.store.has_terminal_event(run_id).await?;
            if has_terminal {
                error!(%run_id, "duplicate terminal event rejected");
                return Err(EventError::DuplicateTerminalEvent { run_id });
            }
        }

        // Assign the next monotonic sequence number.
        let max_seq = self.store.max_sequence(run_id).await?;
        let next_seq = max_seq + 1;
        event.sequence = next_seq;

        // Serialise the kind payload.
        let payload = serde_json::to_value(&event.kind).map_err(EventError::Serialisation)?;

        let stored = StoredEvent {
            id: event.id.to_string(),
            event_type: event_type_of(&event.kind),
            sequence: next_seq,
            global_sequence: 0, // assigned by store
            run_id: event.run_id.to_string(),
            turn_id: event
                .correlation
                .turn_id
                .as_ref()
                .map(std::string::ToString::to_string),
            step_id: event
                .correlation
                .step_id
                .as_ref()
                .map(std::string::ToString::to_string),
            effect_intent_id: event
                .correlation
                .effect_intent_id
                .as_ref()
                .map(std::string::ToString::to_string),
            effect_attempt_id: event
                .correlation
                .effect_attempt_id
                .as_ref()
                .map(std::string::ToString::to_string),
            conversation_id: None,
            correlation_id: event.id.to_string(), // use event id as default
            causation_id: event
                .causation_id
                .as_ref()
                .map(std::string::ToString::to_string),
            scope_id: String::new(), // populated by caller context when available
            timestamp: event.timestamp.to_rfc3339(),
            durability: "durable".to_string(),
            payload,
            trace_id: None,
            span_id: None,
            schema_version: 1,
        };

        // Write to store (store enforces UNIQUE(run_id, sequence)).
        let returned = self
            .store
            .append_durable(stored)
            .await
            .map_err(|e| match e {
                EventStoreError::NonMonotonicSequence {
                    run_id: _rid,
                    current,
                    proposed,
                } => EventError::NonMonotonicSequence {
                    run_id: event.run_id,
                    current,
                    proposed,
                },
                other => EventError::Store(other),
            })?;

        // Update the global_sequence from the store's assignment.
        event.sequence = returned.sequence;
        debug!(
            sequence = event.sequence,
            global_sequence = returned.global_sequence,
            "durable event persisted"
        );

        Ok(())
    }

    async fn record_diagnostic(&self, event: &mut RunEvent) -> Result<(), EventError> {
        // Diagnostic rows share the run_events(run_id, sequence) uniqueness
        // domain with durable lifecycle rows. Assign the next sequence here
        // just as we do for durable events; leaving caller-provided `0`
        // causes the second diagnostic event for a run to collide in SQLite.
        event.sequence = self.store.max_sequence(event.run_id).await? + 1;
        let payload = serde_json::to_value(&event.kind).map_err(EventError::Serialisation)?;

        let stored = StoredEvent {
            id: event.id.to_string(),
            event_type: event_type_of(&event.kind),
            sequence: event.sequence,
            global_sequence: 0,
            run_id: event.run_id.to_string(),
            turn_id: event
                .correlation
                .turn_id
                .as_ref()
                .map(std::string::ToString::to_string),
            step_id: event
                .correlation
                .step_id
                .as_ref()
                .map(std::string::ToString::to_string),
            effect_intent_id: event
                .correlation
                .effect_intent_id
                .as_ref()
                .map(std::string::ToString::to_string),
            effect_attempt_id: event
                .correlation
                .effect_attempt_id
                .as_ref()
                .map(std::string::ToString::to_string),
            conversation_id: None,
            correlation_id: event.id.to_string(),
            causation_id: event
                .causation_id
                .as_ref()
                .map(std::string::ToString::to_string),
            scope_id: String::new(),
            timestamp: event.timestamp.to_rfc3339(),
            durability: "diagnostic".to_string(),
            payload,
            trace_id: None,
            span_id: None,
            schema_version: 1,
        };

        // Diagnostic events expire after 7 days by default.
        let expires_at = (Utc::now() + chrono::Duration::days(7)).to_rfc3339();

        self.store
            .append_diagnostic(stored, expires_at)
            .await
            .map_err(EventError::Store)?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Determine the durability class for an `EventKind`.
fn durability_of(kind: &EventKind) -> DurabilityClass {
    // Map from the core EventKind to our DurabilityClass.
    // By default everything is Durable unless it's a known streaming kind.
    match kind {
        EventKind::StreamingToken { .. } | EventKind::ProgressUpdate { .. } => {
            DurabilityClass::Ephemeral
        }

        EventKind::ToolCallStarted { .. }
        | EventKind::ToolCallCompleted { .. }
        | EventKind::DiagnosticLog { .. } => DurabilityClass::Diagnostic,

        _ => DurabilityClass::Durable,
    }
}

/// Returns `true` if the `EventKind` is a terminal run lifecycle event.
fn is_terminal_kind(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::RunCompleted { .. }
            | EventKind::RunFailed { .. }
            | EventKind::RunCancelled { .. }
            | EventKind::RunTimedOut
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Recorder tests use expect/unwrap while asserting persistence, ordering, and
// broadcast invariants against an in-memory store.
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "unit-test recorder assertions intentionally panic with focused diagnostics"
)]
mod tests {
    use super::*;
    use crate::bus::EventBus;
    use crate::error::EventError;
    use crate::types::TERMINAL_EVENT_TYPES;
    use polkagent_core::{
        event::{EventCorrelation, EventKind, RunEvent},
        ids::{EffectAttemptId, EffectId, EventId, RunId, StepId, TurnId},
    };
    use polkagent_store_trait::event::{EventFilter, EventStore, EventStoreError, StoredEvent};
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tokio;

    // ── In-memory EventStore for tests ────────────────────────────────────

    #[derive(Debug, Default)]
    struct InMemoryStore {
        durable: Mutex<Vec<StoredEvent>>,
        diagnostic: Mutex<Vec<StoredEvent>>,
        /// Tracks (`run_id` -> max sequence).
        sequences: Mutex<HashMap<String, u64>>,
        /// Tracks `run_ids` that have terminal events.
        terminal: Mutex<std::collections::HashSet<String>>,
    }

    #[async_trait::async_trait]
    impl EventStore for InMemoryStore {
        async fn append_durable(
            &self,
            mut event: StoredEvent,
        ) -> Result<StoredEvent, EventStoreError> {
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
            let is_terminal = TERMINAL_EVENT_TYPES.contains(&event.event_type.as_str());
            if is_terminal && !term.insert(event.run_id.clone()) {
                return Err(EventStoreError::DuplicateTerminalEvent {
                    run_id: event.run_id.clone(),
                });
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
            self.sequences
                .lock()
                .expect("lock")
                .insert(event.run_id.clone(), event.sequence);
            let mut diag = self.diagnostic.lock().expect("lock");
            diag.push(event);
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

        async fn read_run_events(
            &self,
            run_id: RunId,
        ) -> Result<Vec<StoredEvent>, EventStoreError> {
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
                        .is_none_or(|rid| e.run_id == rid.to_string())
                })
                .cloned()
                .collect())
        }

        async fn max_sequence(&self, run_id: RunId) -> Result<u64, EventStoreError> {
            let seqs = self.sequences.lock().expect("lock");
            Ok(seqs.get(&run_id.to_string()).copied().unwrap_or(0))
        }

        async fn has_terminal_event(&self, run_id: RunId) -> Result<bool, EventStoreError> {
            let term = self.terminal.lock().expect("lock");
            Ok(term.contains(&run_id.to_string()))
        }
    }

    // ── Helper ────────────────────────────────────────────────────────────

    fn make_durable_recorder() -> (EventRecorder, Arc<InMemoryStore>) {
        let store = Arc::new(InMemoryStore::default());
        let store_dyn: Arc<dyn EventStore> = store.clone();
        let bus = EventBus::new(64);
        let recorder = EventRecorder::new(store_dyn, bus);
        (recorder, store)
    }

    fn run_event(run_id: RunId, kind: EventKind) -> RunEvent {
        let correlation = EventCorrelation {
            run_id,
            ..Default::default()
        };
        RunEvent::new_durable(
            EventId::new(),
            run_id,
            0, /* pre-assign */
            kind,
            correlation,
        )
    }

    // ── Tests ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn records_durable_event_with_monotonic_sequence() {
        let (recorder, store) = make_durable_recorder();
        let run_id = RunId::new();

        let e1 = run_event(run_id, EventKind::RunCreated);
        let returned = recorder.record(e1).await.expect("record e1");
        assert_eq!(returned.sequence, 1);

        let e2 = run_event(run_id, EventKind::RunQueued);
        let returned2 = recorder.record(e2).await.expect("record e2");
        assert_eq!(returned2.sequence, 2);

        // Verify the store has two events.
        let events = store
            .read_run_events(run_id)
            .await
            .expect("read run events");
        assert_eq!(events.len(), 2);
        assert!(events[0].sequence < events[1].sequence);
    }

    #[tokio::test]
    async fn recorder_persists_complete_correlation_and_causation_metadata() {
        let (recorder, store) = make_durable_recorder();
        let run_id = RunId::new();
        let turn_id = TurnId::new();
        let step_id = StepId::new();
        let effect_intent_id = EffectId::new();
        let effect_attempt_id = EffectAttemptId::new();
        let causation_id = EventId::new();
        let mut event = RunEvent::new_durable(
            EventId::new(),
            run_id,
            0,
            EventKind::RunStarted,
            EventCorrelation {
                run_id,
                turn_id: Some(turn_id),
                step_id: Some(step_id),
                effect_intent_id: Some(effect_intent_id),
                effect_attempt_id: Some(effect_attempt_id),
            },
        );
        event.causation_id = Some(causation_id);

        recorder
            .record(event)
            .await
            .expect("record correlated event");
        let stored = store
            .read_run_events(run_id)
            .await
            .expect("read correlated event");
        assert_eq!(stored.len(), 1);
        assert_eq!(
            stored[0].turn_id.as_deref(),
            Some(turn_id.to_string().as_str())
        );
        assert_eq!(
            stored[0].step_id.as_deref(),
            Some(step_id.to_string().as_str())
        );
        assert_eq!(
            stored[0].effect_intent_id.as_deref(),
            Some(effect_intent_id.to_string().as_str())
        );
        assert_eq!(
            stored[0].effect_attempt_id.as_deref(),
            Some(effect_attempt_id.to_string().as_str())
        );
        assert_eq!(
            stored[0].causation_id.as_deref(),
            Some(causation_id.to_string().as_str())
        );
    }

    #[tokio::test]
    async fn recorder_rejects_mismatched_correlation_run() {
        let (recorder, store) = make_durable_recorder();
        let run_id = RunId::new();
        let event = RunEvent::new_durable(
            EventId::new(),
            run_id,
            0,
            EventKind::RunCreated,
            EventCorrelation {
                run_id: RunId::new(),
                ..Default::default()
            },
        );
        let error = recorder
            .record(event)
            .await
            .expect_err("mismatched correlation run must fail");
        assert!(matches!(
            error,
            EventError::Store(EventStoreError::Serialisation(_))
        ));
        assert!(store.durable.lock().expect("durable store").is_empty());
    }

    #[tokio::test]
    async fn rejects_duplicate_terminal_event() {
        let (recorder, _) = make_durable_recorder();
        let run_id = RunId::new();

        // First terminal event should succeed.
        let e1 = run_event(
            run_id,
            EventKind::RunCompleted {
                output_artifact_id: None,
                input_tokens: 0,
                output_tokens: 0,
            },
        );
        recorder.record(e1).await.expect("first terminal ok");

        // Second terminal event must be rejected.
        let e2 = run_event(
            run_id,
            EventKind::RunFailed {
                reason: "oops".into(),
            },
        );
        let result = recorder.record(e2).await;
        assert!(
            matches!(result, Err(EventError::DuplicateTerminalEvent { .. })),
            "expected DuplicateTerminalEvent, got {result:?}"
        );
    }

    #[tokio::test]
    async fn broadcasts_event_on_bus() {
        let store = Arc::new(InMemoryStore::default()) as Arc<dyn EventStore>;
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let recorder = EventRecorder::new(store, bus);

        let run_id = RunId::new();
        let event = run_event(run_id, EventKind::RunCreated);
        let event_id = event.id;

        recorder.record(event).await.expect("record");

        let received = rx.recv().await.expect("receive");
        assert_eq!(received.id, event_id);
    }

    #[tokio::test]
    async fn ephemeral_event_not_written_to_store() {
        let store = Arc::new(InMemoryStore::default());
        let store_dyn: Arc<dyn EventStore> = store.clone();
        let bus = EventBus::new(16);
        let recorder = EventRecorder::new(store_dyn, bus);

        let run_id = RunId::new();
        let event = run_event(
            run_id,
            EventKind::StreamingToken {
                text: "hello".into(),
            },
        );

        recorder.record(event).await.expect("record ephemeral");

        let events = store
            .read_run_events(run_id)
            .await
            .expect("read run events");
        assert!(events.is_empty(), "ephemeral events must not be stored");
    }

    #[tokio::test]
    async fn diagnostic_events_share_the_monotonic_run_sequence() {
        let store = Arc::new(InMemoryStore::default());
        let store_dyn: Arc<dyn EventStore> = store.clone();
        let recorder = EventRecorder::new(store_dyn, EventBus::new(16));
        let run_id = RunId::new();

        let started = recorder
            .record(run_event(
                run_id,
                EventKind::ToolCallStarted {
                    tool_name: "test.echo".to_owned(),
                },
            ))
            .await
            .expect("record started diagnostic");
        let completed = recorder
            .record(run_event(
                run_id,
                EventKind::ToolCallCompleted {
                    tool_name: "test.echo".to_owned(),
                },
            ))
            .await
            .expect("record completed diagnostic");

        assert_eq!(started.sequence, 1);
        assert_eq!(completed.sequence, 2);
        let diagnostic = store.diagnostic.lock().expect("diagnostic lock");
        assert_eq!(diagnostic.len(), 2);
        assert_eq!(diagnostic[0].sequence, 1);
        assert_eq!(diagnostic[1].sequence, 2);
    }

    #[tokio::test]
    async fn event_ordering_across_multiple_runs() {
        let (recorder, store) = make_durable_recorder();

        let run_a = RunId::new();
        let run_b = RunId::new();

        recorder
            .record(run_event(run_a, EventKind::RunCreated))
            .await
            .expect("a1");
        recorder
            .record(run_event(run_b, EventKind::RunCreated))
            .await
            .expect("b1");
        recorder
            .record(run_event(run_a, EventKind::RunQueued))
            .await
            .expect("a2");

        let a_events = store.read_run_events(run_a).await.expect("read a");
        let b_events = store.read_run_events(run_b).await.expect("read b");

        assert_eq!(a_events.len(), 2);
        assert_eq!(b_events.len(), 1);
        // Per-run sequences are independent.
        assert_eq!(a_events[0].sequence, 1);
        assert_eq!(a_events[1].sequence, 2);
        assert_eq!(b_events[0].sequence, 1);
    }
}
