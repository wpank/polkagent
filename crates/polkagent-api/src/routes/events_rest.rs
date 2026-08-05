//! REST event query endpoints (complementing the WebSocket stream in `events.rs`).
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `GET` | `/events` | [`list_events`] |
//! | `GET` | `/events/:id` | [`get_event`] |

use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    Json,
};
use polkagent_store_trait::event::EventStoreError;
use tracing::{instrument, warn};

use crate::{
    dto::{CursorInfo, EventResponse, ListEventsQuery, ListEventsResponse, PageMeta, API_VERSION},
    error::ApiError,
    state::AppState,
};

// ---------------------------------------------------------------------------
// GET /events
// ---------------------------------------------------------------------------

/// List durable events with optional filters.
///
/// Query parameters:
/// - `run_id`: restrict to a specific run.
/// - `event_type`: restrict to a specific event type string.
/// - `since`: return events with `global_sequence >= since`.
/// - `limit`: page size (default 50, max 200).
///
/// Returns 501 if no event store is configured.
#[instrument(skip(state))]
pub async fn list_events(
    State(state): State<AppState>,
    Query(query): Query<ListEventsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let event_store = state
        .event_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("event store not configured".to_owned()))?;

    let limit = query.limit.unwrap_or(50).min(200) as usize;

    let filter = polkagent_store_trait::event::EventFilter {
        run_id: query.run_id,
        event_types: query.event_type.into_iter().collect(),
        since_global_sequence: query.since,
        // Fetch one extra to detect has_more.
        limit: Some(limit + 1),
        ..Default::default()
    };

    let events = event_store
        .query(filter)
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    let has_more = events.len() > limit;
    let page: Vec<_> = events.into_iter().take(limit).collect();

    let next_cursor = if has_more {
        page.last().map(|e| e.global_sequence.to_string())
    } else {
        None
    };

    let page_size = page.len();
    let data: Vec<serde_json::Value> = page
        .into_iter()
        .map(|evt| serde_json::to_value(&evt).unwrap_or(serde_json::Value::Null))
        .collect();

    Ok(Json(ListEventsResponse {
        version: API_VERSION.to_owned(),
        data,
        cursor: CursorInfo {
            next: next_cursor,
            has_more,
        },
        meta: PageMeta { page_size },
    }))
}

// ---------------------------------------------------------------------------
// GET /events/:id
// ---------------------------------------------------------------------------

/// Get a single event by its ID.
///
/// This uses the event store's exact ID lookup contract. Returns 501 if no
/// event store is configured.
/// Returns 404 if no event with the given ID exists.
#[instrument(skip(state), fields(event_id = %id))]
pub async fn get_event(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let event_store = state
        .event_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("event store not configured".to_owned()))?;

    let event = match event_store.get_event_by_id(&id).await {
        Ok(event) => event,
        Err(EventStoreError::NotFound(_)) => {
            return Err(ApiError::NotFound(format!("event {id}")));
        }
        Err(error) => {
            let error_kind = match &error {
                EventStoreError::DuplicateTerminalEvent { .. } => "duplicate_terminal_event",
                EventStoreError::NonMonotonicSequence { .. } => "non_monotonic_sequence",
                EventStoreError::NotFound(_) => "not_found",
                EventStoreError::Conflict(_) => "conflict",
                EventStoreError::Backend(_) => "backend",
                EventStoreError::Serialisation(_) => "serialisation",
            };
            warn!(error_kind, "durable event ID lookup failed");
            return Err(ApiError::InternalError("event lookup failed".to_owned()));
        }
    };

    let data = serde_json::to_value(&event)
        .map_err(|_| ApiError::InternalError("event response serialization failed".to_owned()))?;

    Ok(Json(EventResponse {
        version: API_VERSION.to_owned(),
        data,
    }))
}
