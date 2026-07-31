//! Memory search and statistics endpoints.
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `POST` | `/memory/query` | [`query_memory`] |
//! | `GET` | `/memory/stats` | [`memory_stats`] |
//!
//! **Stub:** Both endpoints return 501 Not Implemented until a memory store
//! backend is integrated.

use axum::{
    extract::State,
    response::IntoResponse,
    Json,
};
use tracing::instrument;

use crate::{
    dto::{MemoryQueryRequest, MemoryQueryResponse, MemoryStatsResponse},
    error::ApiError,
    state::AppState,
};

// ---------------------------------------------------------------------------
// POST /memory/query
// ---------------------------------------------------------------------------

/// Search memories by query string.
///
/// **Stub:** Returns 501 Not Implemented. Will be wired to the memory
/// store once it is available.
#[instrument(skip(_state, body), fields(query = %body.query))]
pub async fn query_memory(
    State(_state): State<AppState>,
    Json(body): Json<MemoryQueryRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // When a memory store is available, this will delegate to it.
    // For now, return 501.
    let _ = body;
    Err::<Json<MemoryQueryResponse>, _>(ApiError::NotImplemented(
        "memory query is not yet implemented".to_owned(),
    ))
}

// ---------------------------------------------------------------------------
// GET /memory/stats
// ---------------------------------------------------------------------------

/// Get memory usage statistics.
///
/// **Stub:** Returns 501 Not Implemented. Will be wired to the memory
/// store once it is available.
#[instrument(skip(_state))]
pub async fn memory_stats(
    State(_state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    Err::<Json<MemoryStatsResponse>, _>(ApiError::NotImplemented(
        "memory stats is not yet implemented".to_owned(),
    ))
}
