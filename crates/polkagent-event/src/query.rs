//! Convenience query helpers over an [`EventStore`].
//!
//! These functions are thin wrappers that avoid callers having to construct
//! [`polkagent_store_trait::event::EventFilter`] and parse the results manually.
//!
//! # Available helpers
//!
//! | Function | Description |
//! |---|---|
//! | [`events_for_run`] | Fetch all durable events for a run, optionally from a cursor |
//! | [`latest_state`] | Derive the current [`RunState`] for a run by replaying its events |

use polkagent_core::{
    event::{Durability, EventCorrelation, EventKind, RunEvent},
    run::RunState,
    RunId,
};
use polkagent_store_trait::event::{EventStore, StoredEvent};

use crate::error::EventError;
use crate::projection::{Projection, RunStatusProjection};

// ---------------------------------------------------------------------------
// events_for_run
// ---------------------------------------------------------------------------

/// Fetch all durable events for `run_id` from the store.
///
/// If `since` is `Some(cursor)`, only events with `global_sequence > cursor`
/// are returned (useful for incremental catch-up). If `since` is `None`, all
/// events for the run are returned.
///
/// Results are ordered by `sequence` (per-run monotonic).
///
/// # Errors
///
/// Returns [`EventError::Store`] if the store cannot be queried.
pub async fn events_for_run(
    store: &dyn EventStore,
    run_id: RunId,
    since: Option<u64>,
) -> Result<Vec<StoredEvent>, EventError> {
    let events = if since.is_some() {
        // Use the cursor-based read for incremental fetching.
        let cursor = since.unwrap_or(0);
        let all = store
            .read_from_cursor(cursor, usize::MAX)
            .await
            .map_err(EventError::Store)?;

        // Filter to the specific run.
        all.into_iter()
            .filter(|e| e.run_id == run_id.to_string())
            .collect()
    } else {
        // Read all events for the run directly.
        store
            .read_run_events(run_id)
            .await
            .map_err(EventError::Store)?
    };

    Ok(events)
}

// ---------------------------------------------------------------------------
// latest_state
// ---------------------------------------------------------------------------

/// Derive the current [`RunState`] for `run_id` by replaying its events.
///
/// Returns `None` if the run has no events yet (i.e., it does not exist in
/// the event store).
///
/// This function replays the full event history for the run by applying each
/// event to a [`RunStatusProjection`]. For frequent queries it is more
/// efficient to maintain a live projection and query its state directly.
///
/// # Errors
///
/// Returns [`EventError::Store`] if the store cannot be queried.
/// Returns [`EventError::Serialisation`] if an event payload cannot be
/// deserialised.
pub async fn latest_state(
    store: &dyn EventStore,
    run_id: RunId,
) -> Result<Option<RunState>, EventError> {
    let stored_events = store
        .read_run_events(run_id)
        .await
        .map_err(EventError::Store)?;

    if stored_events.is_empty() {
        return Ok(None);
    }

    // Replay events into a RunStatusProjection.
    let mut proj = RunStatusProjection::new();

    for stored in &stored_events {
        // Deserialise the EventKind from the stored payload.
        let kind: EventKind =
            serde_json::from_value(stored.payload.clone()).map_err(EventError::Serialisation)?;

        let event_id: polkagent_core::EventId = stored.id.parse().map_err(|_| {
            EventError::Store(polkagent_store_trait::event::EventStoreError::NotFound(
                format!("invalid event_id: {}", stored.id),
            ))
        })?;

        let stored_run_id: RunId = stored.run_id.parse().map_err(|_| {
            EventError::Store(polkagent_store_trait::event::EventStoreError::NotFound(
                format!("invalid run_id: {}", stored.run_id),
            ))
        })?;

        let event = RunEvent {
            id: event_id,
            run_id: stored_run_id,
            sequence: stored.sequence,
            kind,
            durability: Durability::Durable,
            correlation: EventCorrelation {
                run_id: stored_run_id,
                ..Default::default()
            },
            causation_id: None,
            timestamp: chrono::DateTime::parse_from_rfc3339(&stored.timestamp)
                .map_or_else(|_| chrono::Utc::now(), |dt| dt.with_timezone(&chrono::Utc)),
        };

        proj.apply(&event)
            .map_err(|source| EventError::ProjectionApply {
                name: proj.name().to_owned(),
                source,
            })?;
    }

    Ok(proj.state(&run_id).cloned())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Query tests use expect/unwrap while exercising the in-memory fixture and
// replay contract, with messages identifying each failed boundary.
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "unit-test query assertions intentionally panic with focused diagnostics"
)]
mod tests {
    use super::*;
    use polkagent_core::{
        event::EventKind,
        ids::{EventId, RunId},
    };
    use polkagent_store_trait::event::{EventFilter, EventStore, EventStoreError, StoredEvent};
    use std::collections::HashMap;
    use std::sync::Mutex;

    // ── In-memory store (same as in recorder tests) ───────────────────────

    #[derive(Debug, Default)]
    struct MemStore {
        durable: Mutex<Vec<StoredEvent>>,
        sequences: Mutex<HashMap<String, u64>>,
        terminal: Mutex<std::collections::HashSet<String>>,
    }

    const LOCAL_TERMINAL: &[&str] = &[
        "run_completed",
        "run_failed",
        "run_cancelled",
        "run_timed_out",
    ];

    #[async_trait::async_trait]
    impl EventStore for MemStore {
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
            if LOCAL_TERMINAL.contains(&event.event_type.as_str())
                && !term.insert(event.run_id.clone())
            {
                return Err(EventStoreError::DuplicateTerminalEvent {
                    run_id: event.run_id.clone(),
                });
            }

            let mut durable = self.durable.lock().expect("lock");
            event.global_sequence = (durable.len() + 1) as u64;
            durable.push(event.clone());
            Ok(event)
        }

        async fn append_diagnostic(
            &self,
            _event: StoredEvent,
            _expires_at: String,
        ) -> Result<(), EventStoreError> {
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

    // ── Helper: write an event directly to the store ──────────────────────

    async fn write_event(store: &MemStore, run_id: &RunId, kind_str: &str, seq: u64) {
        let event = StoredEvent {
            id: EventId::new().to_string(),
            event_type: kind_str.to_string(),
            sequence: seq,
            global_sequence: 0,
            run_id: run_id.to_string(),
            conversation_id: None,
            correlation_id: EventId::new().to_string(),
            causation_id: None,
            scope_id: String::new(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            durability: "durable".to_string(),
            payload: serde_json::json!({ "RunCreated": {} }),
            trace_id: None,
            span_id: None,
            schema_version: 1,
        };
        let _stored = store
            .append_durable(StoredEvent {
                event_type: kind_str.to_string(),
                ..event
            })
            .await
            .expect("write_event");
    }

    // ── events_for_run tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn events_for_run_returns_empty_for_unknown_run() {
        let store = MemStore::default();
        let run_id = RunId::new();
        let events = events_for_run(&store, run_id, None).await.expect("query");
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn events_for_run_returns_all_events_without_cursor() {
        let store = MemStore::default();
        let run_id = RunId::new();

        write_event(&store, &run_id, "run_created", 1).await;
        write_event(&store, &run_id, "run_started", 2).await;

        let events = events_for_run(&store, run_id, None).await.expect("query");
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn events_for_run_with_cursor_returns_events_after_cursor() {
        let store = MemStore::default();
        let run_id = RunId::new();

        write_event(&store, &run_id, "run_created", 1).await;
        write_event(&store, &run_id, "run_started", 2).await;

        // cursor=0 means "all events since global_sequence > 0"
        let events = events_for_run(&store, run_id, Some(0))
            .await
            .expect("query");
        assert_eq!(events.len(), 2);

        // cursor=1 means "events with global_sequence > 1"
        let events = events_for_run(&store, run_id, Some(1))
            .await
            .expect("query");
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn events_for_run_excludes_other_runs() {
        let store = MemStore::default();
        let run_a = RunId::new();
        let run_b = RunId::new();

        write_event(&store, &run_a, "run_created", 1).await;
        write_event(&store, &run_b, "run_created", 1).await;

        let events = events_for_run(&store, run_a, None).await.expect("query");
        assert_eq!(events.len(), 1);
    }

    // ── latest_state tests ────────────────────────────────────────────────

    #[tokio::test]
    async fn latest_state_returns_none_for_unknown_run() {
        let store = MemStore::default();
        let run_id = RunId::new();
        let state = latest_state(&store, run_id).await.expect("query");
        assert!(state.is_none());
    }

    #[tokio::test]
    async fn latest_state_reflects_last_event() {
        let store = MemStore::default();
        let run_id = RunId::new();

        // Write events with valid EventKind JSON payloads.
        let created_payload = serde_json::to_value(EventKind::RunCreated).expect("serialize");
        let started_payload = serde_json::to_value(EventKind::RunStarted).expect("serialize");

        let event_id_1 = EventId::new();
        store
            .append_durable(StoredEvent {
                id: event_id_1.to_string(),
                event_type: "run_created".to_string(),
                sequence: 1,
                global_sequence: 0,
                run_id: run_id.to_string(),
                conversation_id: None,
                correlation_id: event_id_1.to_string(),
                causation_id: None,
                scope_id: String::new(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                durability: "durable".to_string(),
                payload: created_payload,
                trace_id: None,
                span_id: None,
                schema_version: 1,
            })
            .await
            .expect("write created");

        let state = latest_state(&store, run_id).await.expect("query");
        assert_eq!(state, Some(RunState::Created));

        let event_id_2 = EventId::new();
        store
            .append_durable(StoredEvent {
                id: event_id_2.to_string(),
                event_type: "run_started".to_string(),
                sequence: 2,
                global_sequence: 0,
                run_id: run_id.to_string(),
                conversation_id: None,
                correlation_id: event_id_2.to_string(),
                causation_id: None,
                scope_id: String::new(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                durability: "durable".to_string(),
                payload: started_payload,
                trace_id: None,
                span_id: None,
                schema_version: 1,
            })
            .await
            .expect("write started");

        let state = latest_state(&store, run_id).await.expect("query");
        assert_eq!(state, Some(RunState::Running));
    }
}
