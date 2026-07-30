//! High-level facade over the memory store.
//!
//! [`MemoryService`] provides a convenient API for agent code to interact with
//! the memory subsystem without worrying about low-level store details like
//! timestamps, IDs, or provenance wiring.

use std::sync::Arc;

use chrono::Utc;
use tracing::debug;

use polkagent_core::ids::AgentId;

use crate::error::MemoryResult;
use crate::store::MemoryStore;
use crate::types::{
    Episode, EpisodeId, MemoryEntry, MemoryId, MemoryProvenance, MemoryQuery, MemoryType,
};

/// High-level memory operations for agents.
///
/// Wraps a [`MemoryStore`] implementation and provides ergonomic methods
/// for the most common memory operations.
pub struct MemoryService {
    store: Arc<dyn MemoryStore>,
}

impl MemoryService {
    /// Create a new service wrapping the given store.
    pub fn new(store: Arc<dyn MemoryStore>) -> Self {
        Self { store }
    }

    /// Store a new memory and return its identifier.
    ///
    /// Timestamps are set to the current time. Relevance starts at 1.0.
    pub async fn remember(
        &self,
        agent_id: AgentId,
        content: &str,
        memory_type: MemoryType,
        provenance: Option<MemoryProvenance>,
    ) -> MemoryResult<MemoryId> {
        let now = Utc::now();
        let entry = MemoryEntry {
            id: MemoryId::new(),
            agent_id,
            episode_id: None,
            memory_type,
            content: content.to_string(),
            embedding: None,
            metadata: serde_json::json!({}),
            provenance,
            created_at: now,
            accessed_at: now,
            access_count: 0,
            relevance_score: 1.0,
        };

        debug!(
            memory_id = %entry.id,
            agent_id = %agent_id,
            memory_type = %memory_type,
            "storing memory"
        );

        self.store.store_memory(&entry).await
    }

    /// Search for relevant memories matching the query text.
    ///
    /// Returns results ordered by relevance score (highest first).
    pub async fn recall(
        &self,
        agent_id: AgentId,
        query: &str,
        limit: usize,
    ) -> MemoryResult<Vec<MemoryEntry>> {
        debug!(agent_id = %agent_id, query = query, limit = limit, "recalling memories");

        let q = MemoryQuery {
            agent_id,
            query_text: query.to_string(),
            memory_types: None,
            limit,
            min_relevance: None,
            since: None,
            episode_id: None,
        };

        self.store.search(&q).await
    }

    /// Start a new episode (conversation session).
    pub async fn start_episode(
        &self,
        agent_id: AgentId,
        title: &str,
    ) -> MemoryResult<EpisodeId> {
        let episode = Episode {
            id: EpisodeId::new(),
            agent_id,
            title: title.to_string(),
            summary: None,
            started_at: Utc::now(),
            ended_at: None,
            turn_count: 0,
            metadata: serde_json::json!({}),
        };

        debug!(
            episode_id = %episode.id,
            agent_id = %agent_id,
            title = title,
            "starting episode"
        );

        self.store.create_episode(&episode).await
    }

    /// End an episode with a summary.
    pub async fn end_episode(
        &self,
        id: EpisodeId,
        summary: &str,
    ) -> MemoryResult<()> {
        debug!(episode_id = %id, "ending episode");
        self.store.end_episode(id, summary).await
    }

    /// Delete a memory (forget it).
    pub async fn forget(&self, id: MemoryId) -> MemoryResult<()> {
        debug!(memory_id = %id, "forgetting memory");
        self.store.delete_memory(id).await
    }

    /// Generate a simple summary of all memories in an episode.
    ///
    /// For now this concatenates all memory content with newlines. A future
    /// version will use LLM summarisation.
    pub async fn summarize_episode(
        &self,
        episode_id: EpisodeId,
    ) -> MemoryResult<String> {
        // Retrieve the episode to get the agent_id.
        let episode = self.store.get_episode(episode_id).await?;

        let query = MemoryQuery {
            agent_id: episode.agent_id,
            query_text: String::new(),
            memory_types: None,
            limit: 1000,
            min_relevance: None,
            since: None,
            episode_id: Some(episode_id),
        };

        let memories = self.store.search(&query).await?;

        let summary = memories
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        Ok(summary)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::SqliteMemoryStore;

    fn make_service() -> MemoryService {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        MemoryService::new(Arc::new(store))
    }

    #[tokio::test]
    async fn remember_and_recall() {
        let svc = make_service();
        let agent = AgentId::new();

        svc.remember(agent, "Rust is fast", MemoryType::Semantic, None)
            .await
            .unwrap();
        svc.remember(agent, "Python is flexible", MemoryType::Semantic, None)
            .await
            .unwrap();

        let results = svc.recall(agent, "Rust", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("Rust"));
    }

    #[tokio::test]
    async fn forget_removes_memory() {
        let svc = make_service();
        let agent = AgentId::new();

        let id = svc
            .remember(agent, "secret", MemoryType::Episodic, None)
            .await
            .unwrap();

        svc.forget(id).await.unwrap();

        let results = svc.recall(agent, "secret", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn episode_start_and_end() {
        let svc = make_service();
        let agent = AgentId::new();

        let ep_id = svc.start_episode(agent, "Test session").await.unwrap();
        svc.end_episode(ep_id, "It went well").await.unwrap();

        // Verify via the store directly -- the episode should have a summary.
        let ep = svc.store.get_episode(ep_id).await.unwrap();
        assert_eq!(ep.summary.as_deref(), Some("It went well"));
        assert!(ep.ended_at.is_some());
    }

    #[tokio::test]
    async fn summarize_episode_concatenates_memories() {
        let svc = make_service();
        let agent = AgentId::new();

        let ep_id = svc.start_episode(agent, "Summary test").await.unwrap();

        // Store memories linked to the episode.
        let now = Utc::now();
        for content in &["First point", "Second point", "Third point"] {
            let entry = MemoryEntry {
                id: MemoryId::new(),
                agent_id: agent,
                episode_id: Some(ep_id),
                memory_type: MemoryType::Episodic,
                content: (*content).to_string(),
                embedding: None,
                metadata: serde_json::json!({}),
                provenance: None,
                created_at: now,
                accessed_at: now,
                access_count: 0,
                relevance_score: 1.0,
            };
            svc.store.store_memory(&entry).await.unwrap();
        }

        let summary = svc.summarize_episode(ep_id).await.unwrap();
        assert!(summary.contains("First point"));
        assert!(summary.contains("Second point"));
        assert!(summary.contains("Third point"));
    }

    #[tokio::test]
    async fn remember_with_provenance() {
        let svc = make_service();
        let agent = AgentId::new();

        let prov = MemoryProvenance {
            source_run_id: Some("run-99".into()),
            source_turn: Some(1),
            extraction_method: "user_input".into(),
            confidence: 1.0,
            verified: true,
        };

        let id = svc
            .remember(agent, "User said hello", MemoryType::Episodic, Some(prov))
            .await
            .unwrap();

        let entry = svc.store.get_memory(id).await.unwrap();
        let p = entry.provenance.unwrap();
        assert_eq!(p.extraction_method, "user_input");
        assert!(p.verified);
    }
}
