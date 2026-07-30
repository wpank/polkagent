//! Abstract storage trait for the memory subsystem.
//!
//! The [`MemoryStore`] trait defines the contract that any backing store must
//! implement. The default (and currently only) implementation is the `SQLite`
//! store in [`crate::sqlite`].

use async_trait::async_trait;
use polkagent_core::ids::AgentId;

use crate::error::MemoryResult;
use crate::types::{Episode, EpisodeId, MemoryEntry, MemoryId, MemoryQuery};

/// Storage backend for agent memories and episodes.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Persist a new memory entry and return its identifier.
    async fn store_memory(&self, entry: &MemoryEntry) -> MemoryResult<MemoryId>;

    /// Retrieve a single memory by its identifier.
    async fn get_memory(&self, id: MemoryId) -> MemoryResult<MemoryEntry>;

    /// Search memories matching the given query.
    ///
    /// Uses FTS5 full-text search when available, falling back to SQL `LIKE`
    /// for databases without FTS support.
    async fn search(&self, query: &MemoryQuery) -> MemoryResult<Vec<MemoryEntry>>;

    /// Update the relevance score for a memory.
    async fn update_relevance(&self, id: MemoryId, score: f64) -> MemoryResult<()>;

    /// Delete a memory entry by its identifier.
    async fn delete_memory(&self, id: MemoryId) -> MemoryResult<()>;

    /// Create a new episode and return its identifier.
    async fn create_episode(&self, episode: &Episode) -> MemoryResult<EpisodeId>;

    /// Retrieve an episode by its identifier.
    async fn get_episode(&self, id: EpisodeId) -> MemoryResult<Episode>;

    /// Mark an episode as ended with an optional summary.
    async fn end_episode(&self, id: EpisodeId, summary: &str) -> MemoryResult<()>;

    /// List episodes for an agent, ordered by most recent first.
    async fn list_episodes(&self, agent_id: AgentId, limit: usize) -> MemoryResult<Vec<Episode>>;
}
