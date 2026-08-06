//! WebSocket event streaming endpoint (PRD-14 real-time run events).
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `GET` | `/events/stream` | [`event_stream`] (WebSocket upgrade) |
//!
//! # Protocol
//!
//! 1. Client opens a WebSocket connection to `/api/v1alpha1/events/stream`.
//! 2. Server subscribes to the [`EventBus`] before replaying durable events
//!    from the configured [`EventStore`].
//! 3. Durable events are replayed in bounded pages after `after_sequence`,
//!    then the stream follows the live bus without a replay/follow race.
//! 4. Optional query parameters allow filtering:
//!    - `run_id` — only forward events for the specified run.
//!    - `kinds` — comma-separated list of event kind names to include
//!      (e.g. `run_created,turn_started`).
//! 5. Durable frames add `global_sequence`, which clients persist and pass as
//!    `after_sequence` when reconnecting. Existing `RunEvent` fields remain
//!    unchanged.
//! 6. Server sends WebSocket Ping frames every 30 seconds for keepalive.
//! 7. On client disconnect or bus closure the task exits cleanly. If durable
//!    replay or lag recovery fails, the server closes with status 1011 and a
//!    generic reason rather than silently continuing or exposing backend text.
//!
//! [`EventBus`]: polkagent_event::EventBus
//! [`EventStore`]: polkagent_store_trait::event::EventStore
//! [`RunEvent`]: polkagent_core::event::RunEvent

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::rejection::QueryRejection;
use axum::extract::ws::{close_code, CloseFrame, Message, WebSocket};
use axum::{
    extract::{Query, State, WebSocketUpgrade},
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use polkagent_core::{
    event::{Durability, EventCorrelation, EventKind, RunEvent},
    EventId, RunId,
};
use polkagent_event::types::EventType;
use polkagent_store_trait::event::{EventStore, StoredEvent};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::{debug, trace, warn};

use crate::{error::ApiError, state::AppState};

// ---------------------------------------------------------------------------
// Query parameters
// ---------------------------------------------------------------------------

/// Query parameters for the `/events/stream` WebSocket endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct EventStreamQuery {
    /// Resume after this durable global sequence. Defaults to zero, which
    /// replays all retained durable events before following the live bus.
    pub after_sequence: Option<u64>,
    /// Filter events to only those belonging to this run.
    pub run_id: Option<RunId>,
    /// Comma-separated list of event kind names to include.
    ///
    /// When present, only events whose [`EventType`] matches one of the
    /// listed names are forwarded. Unknown names are silently ignored.
    pub kinds: Option<String>,
}

/// One event frame sent over the run-event WebSocket.
///
/// The flattened event preserves the existing `RunEvent` JSON shape.
/// Durable events add the global store checkpoint used by reconnect and lag
/// recovery. Best-effort live events omit it because they are not replayable.
#[derive(Debug, Clone, Serialize)]
struct EventStreamFrame {
    #[serde(flatten)]
    event: RunEvent,
    #[serde(skip_serializing_if = "Option::is_none")]
    global_sequence: Option<u64>,
}

/// Parsed filter derived from [`EventStreamQuery`].
#[derive(Debug, Clone)]
struct StreamFilter {
    run_id: Option<RunId>,
    kinds: Option<HashSet<String>>,
}

impl StreamFilter {
    fn from_query(q: &EventStreamQuery) -> Self {
        let kinds = q.kinds.as_ref().map(|s| {
            s.split(',')
                .map(|k| k.trim().to_owned())
                .filter(|k| !k.is_empty())
                .collect::<HashSet<String>>()
        });
        Self {
            run_id: q.run_id,
            kinds,
        }
    }

    /// Returns `true` if the event passes all configured filters.
    fn matches(&self, event: &polkagent_core::event::RunEvent) -> bool {
        // Run-ID filter
        if let Some(ref wanted) = self.run_id {
            if &event.run_id != wanted {
                return false;
            }
        }
        // Kind filter
        if let Some(ref wanted_kinds) = self.kinds {
            let event_type = EventType::from_kind(&event.kind);
            match event_type {
                Some(et) => {
                    if !wanted_kinds.contains(et.as_str()) {
                        return false;
                    }
                }
                None => {
                    // Uncatalogued event kind — exclude when filter is active.
                    return false;
                }
            }
        }
        true
    }

    /// Apply the same filter before deserializing a durable store record.
    fn matches_stored(&self, event: &StoredEvent) -> bool {
        if self
            .run_id
            .is_some_and(|wanted| event.run_id != wanted.to_string())
        {
            return false;
        }
        self.kinds
            .as_ref()
            .is_none_or(|wanted| wanted.contains(&event.event_type))
    }
}

// ---------------------------------------------------------------------------
// Durable replay and follow
// ---------------------------------------------------------------------------

/// Maximum number of durable records retained by one replay read.
pub(super) const REPLAY_PAGE_SIZE: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StreamFailure {
    Backend,
    InvalidProjection,
    Closed,
}

impl StreamFailure {
    fn code(self) -> &'static str {
        match self {
            Self::Backend => "event_store_backend",
            Self::InvalidProjection => "invalid_event_projection",
            Self::Closed => "event_bus_closed",
        }
    }

    fn close_reason(self) -> &'static str {
        match self {
            Self::Backend => "durable event recovery unavailable",
            Self::InvalidProjection => "durable event recovery invalid",
            Self::Closed => "event stream closed",
        }
    }
}

/// Replay durable records after a global checkpoint, then follow the live bus.
///
/// The bus is attached before this value is constructed. Live durable events
/// are treated as wake-ups: the authoritative frame is always read back from
/// the store, which supplies the global sequence needed for exact dedupe.
struct DurableEventFollower {
    store: Arc<dyn EventStore>,
    receiver: polkagent_event::EventReceiver,
    filter: StreamFilter,
    durable_checkpoint: u64,
    pending: VecDeque<EventStreamFrame>,
    replay_required: bool,
}

impl DurableEventFollower {
    fn new(
        store: Arc<dyn EventStore>,
        receiver: polkagent_event::EventReceiver,
        filter: StreamFilter,
        after_sequence: u64,
    ) -> Self {
        Self {
            store,
            receiver,
            filter,
            durable_checkpoint: after_sequence,
            pending: VecDeque::new(),
            replay_required: true,
        }
    }

    async fn next(&mut self) -> Result<EventStreamFrame, StreamFailure> {
        loop {
            if let Some(frame) = self.pending.pop_front() {
                return Ok(frame);
            }

            if self.replay_required {
                self.load_replay_page().await?;
                if !self.pending.is_empty() {
                    continue;
                }
                if self.replay_required {
                    // A full page containing only filtered events may still
                    // have a later match, so keep scanning without skipping.
                    continue;
                }
            }

            match self.receiver.recv().await {
                Ok(event) if event.durability == Durability::Durable => {
                    // The durable store is authoritative. Reading after the
                    // last consumed checkpoint also deduplicates events that
                    // raced with the initial replay.
                    self.replay_required = true;
                }
                Ok(event) => {
                    if self.filter.matches(&event) {
                        return Ok(EventStreamFrame {
                            event,
                            global_sequence: None,
                        });
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(
                        skipped,
                        durable_checkpoint = self.durable_checkpoint,
                        "WebSocket event receiver lagged; replaying durable events"
                    );
                    self.replay_required = true;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(StreamFailure::Closed);
                }
            }
        }
    }

    async fn load_replay_page(&mut self) -> Result<(), StreamFailure> {
        let page = self
            .store
            .read_from_cursor(self.durable_checkpoint, REPLAY_PAGE_SIZE)
            .await
            .map_err(|_| StreamFailure::Backend)?;
        if page.len() > REPLAY_PAGE_SIZE {
            return Err(StreamFailure::InvalidProjection);
        }

        self.replay_required = page.len() == REPLAY_PAGE_SIZE;
        for stored in page {
            if stored.global_sequence <= self.durable_checkpoint {
                return Err(StreamFailure::InvalidProjection);
            }
            self.durable_checkpoint = stored.global_sequence;
            if !self.filter.matches_stored(&stored) {
                continue;
            }
            self.pending.push_back(EventStreamFrame {
                global_sequence: Some(stored.global_sequence),
                event: stored_event_to_run_event(stored)?,
            });
        }
        Ok(())
    }
}

pub(super) fn stored_event_to_run_event(stored: StoredEvent) -> Result<RunEvent, StreamFailure> {
    let id = stored
        .id
        .parse::<EventId>()
        .map_err(|_| StreamFailure::InvalidProjection)?;
    let run_id = stored
        .run_id
        .parse::<RunId>()
        .map_err(|_| StreamFailure::InvalidProjection)?;
    let causation_id = stored
        .causation_id
        .as_deref()
        .map(str::parse::<EventId>)
        .transpose()
        .map_err(|_| StreamFailure::InvalidProjection)?;
    let timestamp = stored
        .timestamp
        .parse()
        .map_err(|_| StreamFailure::InvalidProjection)?;
    let kind = serde_json::from_value::<EventKind>(stored.payload)
        .map_err(|_| StreamFailure::InvalidProjection)?;
    if EventType::from_kind(&kind)
        .is_some_and(|event_type| event_type.as_str() != stored.event_type)
    {
        return Err(StreamFailure::InvalidProjection);
    }

    Ok(RunEvent {
        id,
        run_id,
        sequence: stored.sequence,
        kind,
        durability: Durability::Durable,
        correlation: EventCorrelation {
            run_id,
            ..Default::default()
        },
        causation_id,
        timestamp,
    })
}

// ---------------------------------------------------------------------------
// Keepalive interval
// ---------------------------------------------------------------------------

/// Interval between WebSocket Ping frames.
const PING_INTERVAL: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `GET /api/v1alpha1/events/stream` — WebSocket upgrade for real-time event
/// streaming.
///
/// Attaches to the in-process [`polkagent_event::EventBus`], replays matching
/// durable records from the injected store, and follows live events as JSON
/// text frames. The connection remains open until the client disconnects, the
/// bus is closed, recovery fails, or the server shuts down.
pub async fn event_stream(
    State(state): State<AppState>,
    query: Result<Query<EventStreamQuery>, QueryRejection>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, ApiError> {
    let Query(query) = query.map_err(|_| {
        ApiError::ValidationError(
            "event stream query parameters must use valid UUID and non-negative decimal values"
                .to_owned(),
        )
    })?;
    let store = state
        .event_store
        .clone()
        .ok_or_else(|| ApiError::NotImplemented("event store not configured".to_owned()))?;
    let filter = StreamFilter::from_query(&query);
    // Subscribe before replay so every commit published during replay remains
    // observable and can trigger a durable read after the replay checkpoint.
    let receiver = state.event_bus.subscribe();
    let after_sequence = query.after_sequence.unwrap_or(0);

    debug!(
        after_sequence,
        run_id = ?filter.run_id,
        kinds = ?filter.kinds,
        "WebSocket event stream upgrade accepted"
    );

    Ok(ws.on_upgrade(move |socket| {
        handle_socket(
            socket,
            DurableEventFollower::new(store, receiver, filter, after_sequence),
        )
    }))
}

/// Drive the WebSocket connection: read from the `EventBus`, write JSON frames
/// to the client, and send periodic pings.
async fn handle_socket(socket: WebSocket, mut follower: DurableEventFollower) {
    let (mut sender, mut rx_ws) = socket.split();
    let mut ping_interval = tokio::time::interval(PING_INTERVAL);
    // The first tick fires immediately; consume it so the first real ping
    // is PING_INTERVAL from now.
    ping_interval.tick().await;

    loop {
        tokio::select! {
            // ── Event from bus ──────────────────────────────────────────
            result = follower.next() => {
                match result {
                    Ok(frame) => {
                        if let Ok(json) = serde_json::to_string(&frame) {
                            if sender.send(Message::Text(json.into())).await.is_err() {
                                // Client disconnected.
                                debug!("WebSocket client disconnected (send failed)");
                                break;
                            }
                        } else {
                            let error = StreamFailure::InvalidProjection;
                            warn!(
                                error_code = error.code(),
                                "failed to serialize WebSocket event frame"
                            );
                            let _ = sender
                                .send(Message::Close(Some(CloseFrame {
                                    code: close_code::ERROR,
                                    reason: error.close_reason().into(),
                                })))
                                .await;
                            break;
                        }
                    }
                    Err(StreamFailure::Closed) => {
                        debug!("EventBus closed; terminating WebSocket stream");
                        break;
                    }
                    Err(error) => {
                        warn!(
                            error_code = error.code(),
                            durable_checkpoint = follower.durable_checkpoint,
                            "WebSocket durable event recovery failed"
                        );
                        let _ = sender
                            .send(Message::Close(Some(CloseFrame {
                                code: close_code::ERROR,
                                reason: error.close_reason().into(),
                            })))
                            .await;
                        break;
                    }
                }
            }

            // ── Keepalive ping ──────────────────────────────────────────
            _ = ping_interval.tick() => {
                if sender.send(Message::Ping(vec![].into())).await.is_err() {
                    debug!("WebSocket client disconnected (ping failed)");
                    break;
                }
                trace!("sent WebSocket Ping");
            }

            // ── Incoming client message (or close) ─────────────────────
            msg = rx_ws.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => {
                        debug!("WebSocket client sent Close or disconnected");
                        break;
                    }
                    Some(Ok(Message::Pong(_))) => {
                        trace!("received WebSocket Pong");
                    }
                    Some(Ok(_)) => {
                        // Ignore text/binary messages from the client.
                    }
                    Some(Err(err)) => {
                        warn!(error = %err, "WebSocket receive error");
                        break;
                    }
                }
            }
        }
    }

    debug!("WebSocket event stream handler exiting");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "unit tests intentionally fail fast when an expected parsed filter is absent"
)]
mod tests {
    use super::*;
    use polkagent_core::{
        event::{EventCorrelation, EventKind, RunEvent},
        ids::{EventId, RunId},
    };

    fn make_event(run_id: RunId, kind: EventKind) -> RunEvent {
        RunEvent::new_durable(
            EventId::new(),
            run_id,
            1,
            kind,
            EventCorrelation {
                run_id,
                ..Default::default()
            },
        )
    }

    #[test]
    fn filter_without_criteria_matches_all() {
        let filter = StreamFilter {
            run_id: None,
            kinds: None,
        };
        let run_id = RunId::new();
        let event = make_event(run_id, EventKind::RunCreated);
        assert!(filter.matches(&event));
    }

    #[test]
    fn filter_by_run_id_matches_correct_run() {
        let target = RunId::new();
        let other = RunId::new();
        let filter = StreamFilter {
            run_id: Some(target),
            kinds: None,
        };

        let event_match = make_event(target, EventKind::RunCreated);
        let event_miss = make_event(other, EventKind::RunCreated);

        assert!(filter.matches(&event_match));
        assert!(!filter.matches(&event_miss));
    }

    #[test]
    fn filter_by_kinds_matches_listed_kinds() {
        let run_id = RunId::new();
        let kinds: HashSet<String> = ["run_created", "run_started"]
            .iter()
            .map(ToString::to_string)
            .collect();
        let filter = StreamFilter {
            run_id: None,
            kinds: Some(kinds),
        };

        let event_match = make_event(run_id, EventKind::RunCreated);
        let event_miss = make_event(run_id, EventKind::RunQueued);

        assert!(filter.matches(&event_match));
        assert!(!filter.matches(&event_miss));
    }

    #[test]
    fn filter_combined_run_id_and_kinds() {
        let target = RunId::new();
        let other = RunId::new();
        let kinds: HashSet<String> = ["run_created"].iter().map(ToString::to_string).collect();
        let filter = StreamFilter {
            run_id: Some(target),
            kinds: Some(kinds),
        };

        // Correct run, correct kind
        assert!(filter.matches(&make_event(target, EventKind::RunCreated)));
        // Correct run, wrong kind
        assert!(!filter.matches(&make_event(target, EventKind::RunQueued)));
        // Wrong run, correct kind
        assert!(!filter.matches(&make_event(other, EventKind::RunCreated)));
    }

    #[test]
    fn from_query_parses_comma_separated_kinds() {
        let query = EventStreamQuery {
            after_sequence: None,
            run_id: None,
            kinds: Some("run_created, turn_started ,run_completed".to_owned()),
        };
        let filter = StreamFilter::from_query(&query);
        let kinds = filter.kinds.unwrap();
        assert_eq!(kinds.len(), 3);
        assert!(kinds.contains("run_created"));
        assert!(kinds.contains("turn_started"));
        assert!(kinds.contains("run_completed"));
    }

    #[test]
    fn from_query_no_kinds_means_no_filter() {
        let query = EventStreamQuery {
            after_sequence: None,
            run_id: None,
            kinds: None,
        };
        let filter = StreamFilter::from_query(&query);
        assert!(filter.kinds.is_none());
    }
}
