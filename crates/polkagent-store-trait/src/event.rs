//! Event store trait and supporting types.
//!
//! The [`EventStore`] trait is the durable-storage contract for run events.
//! It enforces the ordering invariants from PRD-10 §8.3:
//!
//! - Per-run `sequence` numbers are monotonically increasing with no gaps.
//! - `global_sequence` provides a total order across all runs in a scope.
//! - At most one terminal event (`RunCompleted`, `RunFailed`, `RunCancelled`,
//!   `RunTimedOut`) is permitted per run.
//!
//! Concrete implementations live in `polkagent-store-sqlite` and future
//! cloud backends.

use async_trait::async_trait;
use polkagent_core::RunId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const EVENT_ID_LOOKUP_PAGE_SIZE: usize = 1_000;

fn invalid_event_page(message: &'static str) -> EventStoreError {
    EventStoreError::Backend(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message,
    )))
}

// ---------------------------------------------------------------------------
// EventStoreError
// ---------------------------------------------------------------------------

/// Errors returned by [`EventStore`] operations.
#[derive(Debug, Error)]
pub enum EventStoreError {
    /// A terminal event already exists for this run.
    ///
    /// PRD-10 REQ-EVT-004: at most one terminal event per run.
    #[error("run {run_id} already has a terminal event")]
    DuplicateTerminalEvent { run_id: String },

    /// The proposed sequence number is not monotonically greater than the
    /// current maximum for this run.
    ///
    /// PRD-10 REQ-EVT-001: per-run sequence must be strictly increasing.
    #[error("sequence {proposed} is not monotonically greater than current max {current} for run {run_id}")]
    NonMonotonicSequence {
        run_id: String,
        current: u64,
        proposed: u64,
    },

    /// A record that was expected to exist was not found.
    #[error("record not found: {0}")]
    NotFound(String),

    /// A write constraint was violated (e.g., duplicate event ID).
    #[error("constraint violation: {0}")]
    Conflict(String),

    /// The underlying storage layer returned an error.
    #[error("backend error: {0}")]
    Backend(#[from] Box<dyn std::error::Error + Send + Sync>),

    /// Serialisation or deserialisation of the event payload failed.
    #[error("serialisation error: {0}")]
    Serialisation(String),
}

// ---------------------------------------------------------------------------
// StoredEvent  (lightweight wire type, no dependency on polkagent-event)
// ---------------------------------------------------------------------------

/// A minimal event record as stored in (and read from) the authority store.
///
/// This type intentionally avoids importing the richer `RunEvent` type from
/// `polkagent-event` so that `polkagent-store-trait` stays free of that
/// dependency. The `polkagent-event` crate converts between `RunEvent` and
/// `StoredEvent`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    /// Globally unique event identifier (UUID v7).
    pub id: String,
    /// Canonical event type name (e.g., `"RunStarted"`).
    pub event_type: String,
    /// Per-run monotonic sequence number (starts at 1).
    pub sequence: u64,
    /// Global monotonic sequence number across all runs in the scope.
    /// Assigned by the store; `0` means "not yet assigned".
    pub global_sequence: u64,
    /// The run this event belongs to.
    pub run_id: String,
    /// The turn correlated with this event, if any.
    #[serde(default)]
    pub turn_id: Option<String>,
    /// The step correlated with this event, if any.
    #[serde(default)]
    pub step_id: Option<String>,
    /// The effect intent correlated with this event, if any.
    #[serde(default)]
    pub effect_intent_id: Option<String>,
    /// The effect attempt correlated with this event, if any.
    #[serde(default)]
    pub effect_attempt_id: Option<String>,
    /// The conversation this event belongs to, if any.
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// Correlation ID linking related events across run boundaries.
    pub correlation_id: String,
    /// The event that directly caused this event, if any.
    #[serde(default)]
    pub causation_id: Option<String>,
    /// Workspace/tenant scope identifier.
    pub scope_id: String,
    /// ISO 8601 UTC timestamp when the event occurred.
    pub timestamp: String,
    /// Durability class: `"durable"`, `"diagnostic"`, or `"ephemeral"`.
    pub durability: String,
    /// JSON-encoded event payload.
    pub payload: serde_json::Value,
    /// W3C trace ID (32 hex chars), if tracing is active.
    #[serde(default)]
    pub trace_id: Option<String>,
    /// W3C span ID (16 hex chars), if tracing is active.
    #[serde(default)]
    pub span_id: Option<String>,
    /// Schema version of this event's payload.
    pub schema_version: u32,
}

// ---------------------------------------------------------------------------
// EventFilter
// ---------------------------------------------------------------------------

/// Filters for querying events from the store (PRD-10 §8.5 REQ-EVT-020).
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    /// Restrict to events for a specific run.
    pub run_id: Option<RunId>,
    /// Restrict to events for a specific conversation.
    pub conversation_id: Option<String>,
    /// Restrict to events within a specific scope.
    pub scope_id: Option<String>,
    /// Restrict to events of these types.
    pub event_types: Vec<String>,
    /// Return only events with `global_sequence >= since`.
    pub since_global_sequence: Option<u64>,
    /// Maximum number of events to return (default: unlimited).
    pub limit: Option<usize>,
    /// If `true`, include diagnostic events in addition to durable ones.
    pub include_diagnostic: bool,
}

// ---------------------------------------------------------------------------
// EventStore trait
// ---------------------------------------------------------------------------

/// Durable storage contract for run events.
///
/// Implementations must enforce:
///
/// 1. **Monotonic per-run sequences** (PRD-10 REQ-EVT-001): each call to
///    `append_durable` with `sequence = N` must fail with
///    [`EventStoreError::NonMonotonicSequence`] if any prior event for the
///    same run has `sequence >= N`.
///
/// 2. **Single terminal event** (PRD-10 REQ-EVT-004): appending a second
///    terminal event (`RunCompleted`, `RunFailed`, `RunCancelled`,
///    `RunTimedOut`) for a run that already has one must fail with
///    [`EventStoreError::DuplicateTerminalEvent`].
///
/// 3. **Ordered global sequence** (PRD-10 REQ-EVT-002): `global_sequence`
///    values are assigned by the store and are strictly increasing across all
///    runs. Callers must not supply a `global_sequence` value; the store
///    assigns it and returns the completed [`StoredEvent`].
///
/// 4. **Durable visibility** (PRD-10 REQ-EVT-003): a durable event is
///    visible to readers only after the transaction that created it commits.
#[async_trait]
pub trait EventStore: Send + Sync {
    /// Append a durable event to the store.
    ///
    /// The store assigns `global_sequence` and returns the completed record.
    /// The caller must set `sequence` to the next monotonic value for the run.
    ///
    /// # Errors
    ///
    /// - [`EventStoreError::DuplicateTerminalEvent`] if the event is terminal
    ///   and the run already has a terminal event.
    /// - [`EventStoreError::NonMonotonicSequence`] if `sequence` is not
    ///   strictly greater than the current maximum for the run.
    /// - [`EventStoreError::Conflict`] if an event with the same ID exists.
    async fn append_durable(&self, event: StoredEvent) -> Result<StoredEvent, EventStoreError>;

    /// Append a diagnostic event to the store.
    ///
    /// Diagnostic events have a shorter retention period and are stored in a
    /// separate table. The store does not enforce terminal-event or sequence
    /// invariants for diagnostic events.
    ///
    /// `expires_at` is an ISO 8601 UTC timestamp after which the event may
    /// be deleted by the retention enforcement job.
    async fn append_diagnostic(
        &self,
        event: StoredEvent,
        expires_at: String,
    ) -> Result<(), EventStoreError>;

    /// Read durable events from a cursor position.
    ///
    /// Returns events with `global_sequence > cursor`, up to `limit` results,
    /// ordered by `global_sequence` ascending. Used for cursor-based replay
    /// and catch-up streaming (PRD-10 §8.6, REQ-EVT-033).
    async fn read_from_cursor(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError>;

    /// Load one durable event by its globally unique string ID.
    ///
    /// Adapters should override this with an indexed point lookup when their
    /// schema supports one. The object-safe default scans bounded cursor pages
    /// until it finds the event or reaches an empty page; it never applies a
    /// fixed history cap. Malformed pages fail closed instead of looping or
    /// silently skipping history.
    async fn get_event_by_id(&self, id: &str) -> Result<StoredEvent, EventStoreError> {
        let mut cursor = 0;
        loop {
            let page = self
                .read_from_cursor(cursor, EVENT_ID_LOOKUP_PAGE_SIZE)
                .await?;
            if page.is_empty() {
                return Err(EventStoreError::NotFound(format!("event {id}")));
            }
            if page.len() > EVENT_ID_LOOKUP_PAGE_SIZE {
                return Err(invalid_event_page(
                    "event cursor store exceeded the requested page size",
                ));
            }

            let mut next_cursor = cursor;
            for event in &page {
                if event.global_sequence <= next_cursor {
                    return Err(invalid_event_page(
                        "event cursor store did not advance global sequence",
                    ));
                }
                next_cursor = event.global_sequence;
            }
            if let Some(event) = page.into_iter().find(|event| event.id == id) {
                return Ok(event);
            }
            cursor = next_cursor;
        }
    }

    /// Read all durable events for a specific run, ordered by `sequence`.
    ///
    /// Used for run-scoped replay and projection rebuild.
    async fn read_run_events(&self, run_id: RunId) -> Result<Vec<StoredEvent>, EventStoreError>;

    /// Query durable events matching the given filter.
    ///
    /// Results are ordered by `global_sequence` ascending.
    async fn query(&self, filter: EventFilter) -> Result<Vec<StoredEvent>, EventStoreError>;

    /// Return the current maximum `sequence` for a run, or `0` if the run
    /// has no events yet. Used to calculate the next sequence number.
    async fn max_sequence(&self, run_id: RunId) -> Result<u64, EventStoreError>;

    /// Return `true` if the run already has a terminal event.
    ///
    /// Used by the event recorder before writing a new terminal
    /// event (PRD-10 REQ-EVT-004).
    async fn has_terminal_event(&self, run_id: RunId) -> Result<bool, EventStoreError>;
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "event lookup contract tests fail at explicit fixture and cursor boundaries"
)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Clone, Copy)]
    enum CursorBehavior {
        Normal,
        NonAdvancing,
        Oversized,
    }

    struct CursorStore {
        events: Vec<StoredEvent>,
        reads: Mutex<Vec<(u64, usize)>>,
        behavior: CursorBehavior,
    }

    impl CursorStore {
        fn normal(events: Vec<StoredEvent>) -> Self {
            Self {
                events,
                reads: Mutex::new(Vec::new()),
                behavior: CursorBehavior::Normal,
            }
        }

        fn malformed(behavior: CursorBehavior) -> Self {
            Self {
                events: Vec::new(),
                reads: Mutex::new(Vec::new()),
                behavior,
            }
        }

        fn reads(&self) -> Vec<(u64, usize)> {
            self.reads.lock().expect("read log lock").clone()
        }
    }

    fn stored_event(global_sequence: u64, id: String) -> StoredEvent {
        StoredEvent {
            id,
            event_type: "test_event".to_owned(),
            sequence: global_sequence,
            global_sequence,
            run_id: "00000000-0000-0000-0000-000000000001".to_owned(),
            turn_id: None,
            step_id: None,
            effect_intent_id: None,
            effect_attempt_id: None,
            conversation_id: None,
            correlation_id: "lookup-contract".to_owned(),
            causation_id: None,
            scope_id: "test".to_owned(),
            timestamp: "2024-01-01T00:00:00Z".to_owned(),
            durability: "durable".to_owned(),
            payload: serde_json::Value::Null,
            trace_id: None,
            span_id: None,
            schema_version: 1,
        }
    }

    #[test]
    fn stored_event_legacy_json_defaults_component_correlation_to_none() {
        let json = serde_json::json!({
            "id": "00000000-0000-7000-8000-000000000001",
            "event_type": "run_created",
            "sequence": 1,
            "global_sequence": 1,
            "run_id": "00000000-0000-7000-8000-000000000002",
            "conversation_id": null,
            "correlation_id": "legacy-correlation",
            "causation_id": null,
            "scope_id": "",
            "timestamp": "2024-01-01T00:00:00Z",
            "durability": "durable",
            "payload": "run_created",
            "trace_id": null,
            "span_id": null,
            "schema_version": 1
        });
        let event: StoredEvent = serde_json::from_value(json).expect("legacy StoredEvent JSON");
        assert_eq!(event.turn_id, None);
        assert_eq!(event.step_id, None);
        assert_eq!(event.effect_intent_id, None);
        assert_eq!(event.effect_attempt_id, None);
    }

    fn unsupported() -> EventStoreError {
        EventStoreError::Backend(Box::new(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "not used by event lookup contract fixture",
        )))
    }

    #[async_trait]
    impl EventStore for CursorStore {
        async fn append_durable(
            &self,
            _event: StoredEvent,
        ) -> Result<StoredEvent, EventStoreError> {
            Err(unsupported())
        }

        async fn append_diagnostic(
            &self,
            _event: StoredEvent,
            _expires_at: String,
        ) -> Result<(), EventStoreError> {
            Err(unsupported())
        }

        async fn read_from_cursor(
            &self,
            cursor: u64,
            limit: usize,
        ) -> Result<Vec<StoredEvent>, EventStoreError> {
            self.reads
                .lock()
                .expect("read log lock")
                .push((cursor, limit));
            match self.behavior {
                CursorBehavior::Normal => Ok(self
                    .events
                    .iter()
                    .filter(|event| event.global_sequence > cursor)
                    .take(limit)
                    .cloned()
                    .collect()),
                CursorBehavior::NonAdvancing => {
                    Ok(vec![stored_event(cursor, "non-advancing".to_owned())])
                }
                CursorBehavior::Oversized => Ok((1..=EVENT_ID_LOOKUP_PAGE_SIZE + 1)
                    .map(|sequence| {
                        stored_event(
                            u64::try_from(sequence).expect("malformed sequence fits u64"),
                            format!("oversized-{sequence}"),
                        )
                    })
                    .collect()),
            }
        }

        async fn read_run_events(
            &self,
            _run_id: RunId,
        ) -> Result<Vec<StoredEvent>, EventStoreError> {
            Err(unsupported())
        }

        async fn query(&self, _filter: EventFilter) -> Result<Vec<StoredEvent>, EventStoreError> {
            Err(unsupported())
        }

        async fn max_sequence(&self, _run_id: RunId) -> Result<u64, EventStoreError> {
            Err(unsupported())
        }

        async fn has_terminal_event(&self, _run_id: RunId) -> Result<bool, EventStoreError> {
            Err(unsupported())
        }
    }

    #[tokio::test]
    async fn default_lookup_pages_past_ten_thousand_without_a_history_cap() {
        let earlier = 10_001_u64;
        let mut events = (1..=earlier)
            .map(|sequence| stored_event(sequence, format!("earlier-{sequence}")))
            .collect::<Vec<_>>();
        events.push(stored_event(earlier + 1, "target".to_owned()));
        let store = CursorStore::normal(events);
        let object_safe_store: &dyn EventStore = &store;

        let found = object_safe_store
            .get_event_by_id("target")
            .await
            .expect("find event after ten thousand earlier events");
        assert_eq!(found.global_sequence, earlier + 1);
        let reads = store.reads();
        assert_eq!(reads.len(), 11);
        assert!(reads
            .iter()
            .all(|(_, limit)| *limit == EVENT_ID_LOOKUP_PAGE_SIZE));
        assert_eq!(reads.last(), Some(&(10_000, EVENT_ID_LOOKUP_PAGE_SIZE)));
    }

    #[tokio::test]
    async fn default_lookup_scans_to_an_empty_page_before_returning_not_found() {
        let events = (1..=10_001_u64)
            .map(|sequence| stored_event(sequence, format!("event-{sequence}")))
            .collect();
        let store = CursorStore::normal(events);

        let error = store
            .get_event_by_id("missing")
            .await
            .expect_err("missing event must be typed not found");
        assert!(matches!(error, EventStoreError::NotFound(_)));
        assert_eq!(store.reads().len(), 12);
    }

    #[tokio::test]
    async fn default_lookup_rejects_non_advancing_and_oversized_pages() {
        for behavior in [CursorBehavior::NonAdvancing, CursorBehavior::Oversized] {
            let store = CursorStore::malformed(behavior);
            let error = store
                .get_event_by_id("missing")
                .await
                .expect_err("malformed cursor store must fail closed");
            assert!(matches!(error, EventStoreError::Backend(_)));
            assert_eq!(store.reads().len(), 1);
        }
    }
}
