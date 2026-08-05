//! Abstract storage trait for the memory subsystem.
//!
//! The [`MemoryStore`] trait defines the contract that any backing store must
//! implement. The default (and currently only) implementation is the `SQLite`
//! store in [`crate::sqlite`].

use async_trait::async_trait;
use polkagent_core::ids::AgentId;

use crate::classification::Classification;
use crate::error::MemoryResult;
use crate::types::{Episode, EpisodeId, MemoryEntry, MemoryId, MemoryQuery};

/// Storage backend for agent memories and episodes.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Persist a new memory entry and return its identifier.
    async fn store_memory(&self, entry: &MemoryEntry) -> MemoryResult<MemoryId>;

    /// Retrieve a single memory by its identifier.
    async fn get_memory(&self, id: MemoryId) -> MemoryResult<MemoryEntry>;

    /// Retrieve a single memory without updating access metadata.
    ///
    /// Read-only projections use this method so observation does not change
    /// `accessed_at` or `access_count`. Interactive retrieval paths should use
    /// [`Self::get_memory`] when access tracking is part of their semantics.
    async fn peek_memory(&self, id: MemoryId) -> MemoryResult<MemoryEntry>;

    /// Search memories matching the given query.
    ///
    /// Uses FTS5 full-text search when available, falling back to SQL `LIKE`
    /// for databases without FTS support.
    async fn search(&self, query: &MemoryQuery) -> MemoryResult<Vec<MemoryEntry>>;

    /// Search memories matching the given query, filtered to entries whose
    /// classification level does not exceed `max_classification`.
    ///
    /// Entries above the maximum classification are silently excluded; no error
    /// is returned.
    async fn search_with_classification(
        &self,
        query: &MemoryQuery,
        max_classification: Classification,
    ) -> MemoryResult<Vec<MemoryEntry>>;

    /// Return a paginated list of all memory entries for an agent, ordered by
    /// creation time (oldest first).
    ///
    /// `limit` controls the page size; `offset` the number of entries to skip.
    /// This is used primarily by the export pipeline to stream all entries
    /// without loading them all at once.
    async fn list_entries(
        &self,
        agent_id: &AgentId,
        limit: usize,
        offset: usize,
    ) -> MemoryResult<Vec<MemoryEntry>>;

    /// Update the relevance score for a memory.
    async fn update_relevance(&self, id: MemoryId, score: f64) -> MemoryResult<()>;

    /// Delete a memory entry by its identifier.
    async fn delete_memory(&self, id: MemoryId) -> MemoryResult<()>;

    /// Delete all memories whose provenance references the given artifact id.
    ///
    /// Removes from both the primary `memories` table (which cascades to
    /// `memory_provenance`) and from the FTS5 index atomically within a
    /// single transaction.
    ///
    /// Returns the number of entries removed.
    async fn forget(&self, artifact_id: &str) -> MemoryResult<usize>;

    /// Return the total number of memory entries belonging to `agent_id`.
    async fn count_entries(&self, agent_id: &AgentId) -> MemoryResult<usize>;

    /// Delete all memories for `agent_id` that are older than `max_age`.
    ///
    /// Returns the number of entries deleted.
    async fn delete_by_age(
        &self,
        agent_id: &AgentId,
        max_age: chrono::Duration,
    ) -> MemoryResult<usize>;

    /// Create a new episode and return its identifier.
    async fn create_episode(&self, episode: &Episode) -> MemoryResult<EpisodeId>;

    /// Retrieve an episode by its identifier.
    async fn get_episode(&self, id: EpisodeId) -> MemoryResult<Episode>;

    /// Mark an episode as ended with an optional summary.
    async fn end_episode(&self, id: EpisodeId, summary: &str) -> MemoryResult<()>;

    /// List episodes for an agent, ordered by most recent first.
    async fn list_episodes(&self, agent_id: AgentId, limit: usize) -> MemoryResult<Vec<Episode>>;
}
