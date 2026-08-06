//! Abstract storage trait for the memory subsystem.
//!
//! The [`MemoryStore`] trait defines the contract that any backing store must
//! implement. The default (and currently only) implementation is the `SQLite`
//! store in [`crate::sqlite`].

use async_trait::async_trait;
use polkagent_core::ids::AgentId;

use crate::classification::Classification;
use crate::error::{MemoryError, MemoryResult};
use crate::types::{Episode, EpisodeId, MemoryEntry, MemoryId, MemoryQuery};

/// Exact aggregate statistics for one memory store.
///
/// `total_bytes` is the sum of the UTF-8 byte lengths of every stored memory
/// entry's `content` field. It intentionally does not claim `SQLite` file size,
/// allocated pages, index overhead, embeddings, or metadata bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryStoreStats {
    /// Total number of stored memory entries.
    pub total_memories: u64,
    /// Sum of stored memory-content UTF-8 bytes.
    pub total_bytes: u64,
    /// Number of distinct stored memory types.
    pub namespaces: u32,
}

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

    /// Return exact aggregate statistics without changing access metadata.
    ///
    /// Backends must override this method only when they can provide all
    /// fields truthfully from their authoritative store.
    async fn stats(&self) -> MemoryResult<MemoryStoreStats> {
        Err(MemoryError::InvalidOperation(
            "memory statistics are not supported by this store".to_owned(),
        ))
    }

    /// Atomically delete the requested identifiers from the authoritative
    /// store and return the number of distinct existing entries removed.
    ///
    /// Unknown and duplicate identifiers are idempotent and do not increment
    /// the returned count. Backends must override this method only when they
    /// can guarantee that an error leaves every requested entry unchanged.
    async fn delete_memories(&self, _ids: &[MemoryId]) -> MemoryResult<usize> {
        Err(MemoryError::InvalidOperation(
            "atomic memory batch deletion is not supported by this store".to_owned(),
        ))
    }

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
