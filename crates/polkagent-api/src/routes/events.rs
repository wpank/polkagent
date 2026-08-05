//! WebSocket event streaming endpoint (PRD-14 real-time run events).
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `GET` | `/events/stream` | [`event_stream`] (WebSocket upgrade) |
//!
//! # Protocol
//!
//! 1. Client opens a WebSocket connection to `/api/v1alpha1/events/stream`.
//! 2. Server subscribes to the [`EventBus`] and forwards every
//!    [`RunEvent`] as a JSON text frame.
//! 3. Optional query parameters allow filtering:
//!    - `run_id` — only forward events for the specified run.
//!    - `kinds` — comma-separated list of event kind names to include
//!      (e.g. `run_created,turn_started`).
//! 4. Server sends WebSocket Ping frames every 30 seconds for keepalive.
//! 5. On client disconnect or bus closure the task exits cleanly.
//!
//! [`EventBus`]: polkagent_event::EventBus
//! [`RunEvent`]: polkagent_core::event::RunEvent

use axum::extract::ws::{Message, WebSocket};
use axum::{
    extract::{Query, State, WebSocketUpgrade},
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use polkagent_core::RunId;
use polkagent_event::types::EventType;
use serde::Deserialize;
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{debug, trace, warn};

use crate::state::AppState;

// ---------------------------------------------------------------------------
// Query parameters
// ---------------------------------------------------------------------------

/// Query parameters for the `/events/stream` WebSocket endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct EventStreamQuery {
    /// Filter events to only those belonging to this run.
    pub run_id: Option<RunId>,
    /// Comma-separated list of event kind names to include.
    ///
    /// When present, only events whose [`EventType`] matches one of the
    /// listed names are forwarded. Unknown names are silently ignored.
    pub kinds: Option<String>,
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
/// Subscribes to the in-process [`polkagent_event::EventBus`] and forwards matching events as
/// JSON text frames. The connection remains open until the client disconnects,
/// the bus is closed, or the server shuts down.
pub async fn event_stream(
    State(state): State<AppState>,
    Query(query): Query<EventStreamQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let filter = StreamFilter::from_query(&query);
    let receiver = state.event_bus.subscribe();

    debug!(
        run_id = ?filter.run_id,
        kinds = ?filter.kinds,
        "WebSocket event stream upgrade accepted"
    );

    ws.on_upgrade(move |socket| handle_socket(socket, receiver, filter))
}

/// Drive the WebSocket connection: read from the `EventBus`, write JSON frames
/// to the client, and send periodic pings.
async fn handle_socket(
    socket: WebSocket,
    mut receiver: polkagent_event::EventReceiver,
    filter: StreamFilter,
) {
    let (mut sender, mut rx_ws) = socket.split();
    let mut ping_interval = tokio::time::interval(PING_INTERVAL);
    // The first tick fires immediately; consume it so the first real ping
    // is PING_INTERVAL from now.
    ping_interval.tick().await;

    loop {
        tokio::select! {
            // ── Event from bus ──────────────────────────────────────────
            result = receiver.recv() => {
                match result {
                    Ok(event) => {
                        if !filter.matches(&event) {
                            continue;
                        }
                        match serde_json::to_string(&event) {
                            Ok(json) => {
                                if sender.send(Message::Text(json.into())).await.is_err() {
                                    // Client disconnected.
                                    debug!("WebSocket client disconnected (send failed)");
                                    break;
                                }
                            }
                            Err(err) => {
                                warn!(error = %err, "failed to serialize event to JSON");
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "WebSocket event receiver lagged");
                        // Continue receiving from the new tail.
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("EventBus closed; terminating WebSocket stream");
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
            run_id: None,
            kinds: None,
        };
        let filter = StreamFilter::from_query(&query);
        assert!(filter.kinds.is_none());
    }
}
