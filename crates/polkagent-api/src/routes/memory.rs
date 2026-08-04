//! Memory search and statistics endpoints.
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `POST` | `/memory/query` | [`query_memory`] |
//! | `GET` | `/memory/stats` | [`memory_stats`] |
//! | `POST` | `/memory/forget` | [`forget_memory`] |
//! | `GET` | `/memory/entries/:entry_id` | [`get_memory_entry`] |
//!
//! All endpoints return 501 Not Implemented when no memory store is configured
//! via [`AppState::memory_store`]. When a store is configured the handlers
//! delegate to its methods.

use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Json,
};
use tracing::instrument;

use crate::{
    dto::{
        MemoryEntryResponse, MemoryForgetRequest, MemoryForgetResponse, MemoryQueryRequest,
        MemoryQueryResponse, MemoryResult, MemoryStatsResponse, API_VERSION,
    },
    error::ApiError,
    state::AppState,
};

// ---------------------------------------------------------------------------
// POST /memory/query
// ---------------------------------------------------------------------------

/// Search memories by query string.
///
/// When a memory store is configured, delegates to its search interface.
/// Returns 501 Not Implemented when no memory store is configured.
#[instrument(skip(state, body), fields(query = %body.query))]
pub async fn query_memory(
    State(state): State<AppState>,
    Json(body): Json<MemoryQueryRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let store = state
        .memory_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("memory store not configured".to_owned()))?;

    let results = store
        .search(&body.query, body.limit as usize, body.namespace.as_deref())
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    Ok(Json(MemoryQueryResponse {
        version: API_VERSION.to_owned(),
        data: results,
    }))
}

// ---------------------------------------------------------------------------
// GET /memory/stats
// ---------------------------------------------------------------------------

/// Get memory usage statistics.
///
/// Returns 501 Not Implemented when no memory store is configured.
#[instrument(skip(state))]
pub async fn memory_stats(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let store = state
        .memory_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("memory store not configured".to_owned()))?;

    let stats = store
        .stats()
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    Ok(Json(MemoryStatsResponse {
        version: API_VERSION.to_owned(),
        total_memories: stats.total_memories,
        total_bytes: stats.total_bytes,
        namespaces: stats.namespaces,
    }))
}

// ---------------------------------------------------------------------------
// POST /memory/forget
// ---------------------------------------------------------------------------

/// Delete specific memory entries by ID.
///
/// Returns 501 Not Implemented when no memory store is configured.
#[instrument(skip(state, body))]
pub async fn forget_memory(
    State(state): State<AppState>,
    Json(body): Json<MemoryForgetRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let store = state
        .memory_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("memory store not configured".to_owned()))?;

    let deleted = store
        .delete_entries(&body.entry_ids)
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    Ok(Json(MemoryForgetResponse {
        version: API_VERSION.to_owned(),
        deleted,
    }))
}

// ---------------------------------------------------------------------------
// GET /memory/entries/:entry_id
// ---------------------------------------------------------------------------

/// Get a single memory entry by its ID.
///
/// Returns 501 Not Implemented when no memory store is configured.
/// Returns 404 if the entry does not exist.
#[instrument(skip(state), fields(entry_id = %entry_id))]
pub async fn get_memory_entry(
    State(state): State<AppState>,
    Path(entry_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let store = state
        .memory_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("memory store not configured".to_owned()))?;

    let entry = store
        .get_entry(&entry_id)
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound(format!("memory entry '{entry_id}'")))?;

    Ok(Json(MemoryEntryResponse {
        version: API_VERSION.to_owned(),
        data: entry,
    }))
}

// ---------------------------------------------------------------------------
// MemoryStore trait
// ---------------------------------------------------------------------------

/// Stats returned by [`MemoryStore::stats`].
pub struct MemoryStats {
    /// Total number of stored memory entries.
    pub total_memories: u64,
    /// Total bytes across all entries.
    pub total_bytes: u64,
    /// Number of distinct namespaces.
    pub namespaces: u32,
}

/// Trait for pluggable memory storage backends.
///
/// All methods are async to support both in-memory and network-backed stores.
/// Implementations must be `Send + Sync` so that `Arc<dyn MemoryStore>` can
/// be shared across Axum handlers.
#[async_trait::async_trait]
pub trait MemoryStore: Send + Sync {
    /// Search for memory entries matching a query.
    ///
    /// - `query`: free-text search string.
    /// - `limit`: maximum number of results to return.
    /// - `namespace`: optional namespace filter.
    async fn search(
        &self,
        query: &str,
        limit: usize,
        namespace: Option<&str>,
    ) -> Result<Vec<MemoryResult>, String>;

    /// Get aggregated statistics about the stored memories.
    async fn stats(&self) -> Result<MemoryStats, String>;

    /// Delete a set of memory entries by ID.
    ///
    /// Returns the number of entries actually deleted.
    async fn delete_entries(&self, entry_ids: &[String]) -> Result<u32, String>;

    /// Retrieve a single memory entry by ID.
    async fn get_entry(&self, entry_id: &str) -> Result<Option<MemoryResult>, String>;
}
