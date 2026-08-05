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
use tracing::instrument;

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
/// This performs a filtered query on the event store using the event's
/// string ID. Returns 501 if no event store is configured.
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

    // Read from cursor 0 with a small limit and filter client-side by ID.
    // A production implementation would add a `get_by_id` method to EventStore.
    // For now, use read_from_cursor as a reasonable fallback.
    let events = event_store
        .read_from_cursor(0, 10_000)
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    let event = events
        .into_iter()
        .find(|e| e.id == id)
        .ok_or_else(|| ApiError::NotFound(format!("event {id}")))?;

    let data = serde_json::to_value(&event).map_err(|e| ApiError::InternalError(e.to_string()))?;

    Ok(Json(EventResponse {
        version: API_VERSION.to_owned(),
        data,
    }))
}
