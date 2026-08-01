//! High-level facade over the memory store.
//!
//! [`MemoryService`] provides a convenient API for agent code to interact with
//! the memory subsystem without worrying about low-level store details like
//! timestamps, IDs, or provenance wiring.

use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use tracing::debug;

use polkagent_core::ids::AgentId;

use crate::error::{MemoryError, MemoryResult};
use crate::export::{export_archive, import_archive, ImportResult};
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
            confidence: 1.0,
            classification: crate::classification::Classification::Internal,
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

    /// Export all memories and episodes for `agent_id` to a JSON file at `path`.
    ///
    /// Returns the total number of records written (entries + episodes).
    ///
    /// # Errors
    ///
    /// Returns an error if the store query fails, serialisation fails, or the
    /// file cannot be written.
    pub async fn export_agent_memory(
        &self,
        agent_id: &AgentId,
        path: &Path,
    ) -> MemoryResult<usize> {
        let archive = export_archive(self.store.as_ref(), agent_id).await?;
        let total = archive.entries.len() + archive.episodes.len();

        let json = serde_json::to_string_pretty(&archive)
            .map_err(MemoryError::Json)?;

        std::fs::write(path, json.as_bytes())
            .map_err(|e| MemoryError::InvalidOperation(format!("writing archive: {e}")))?;

        debug!(
            agent_id = %agent_id,
            path = %path.display(),
            records = total,
            "exported memory archive"
        );

        Ok(total)
    }

    /// Import memories and episodes from a JSON archive file at `path`.
    ///
    /// Entries whose identifiers already exist in the store are silently
    /// skipped (idempotent). Returns import statistics.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read, JSON parsing fails, or a
    /// store operation returns a non-duplicate error.
    pub async fn import_agent_memory(&self, path: &Path) -> MemoryResult<ImportResult> {
        let bytes = std::fs::read(path)
            .map_err(|e| MemoryError::InvalidOperation(format!("reading archive: {e}")))?;

        let archive: crate::export::MemoryArchive = serde_json::from_slice(&bytes)
            .map_err(MemoryError::Json)?;

        let result = import_archive(self.store.as_ref(), &archive).await?;

        debug!(
            agent_id = %archive.agent_id,
            path = %path.display(),
            imported = result.imported_count,
            skipped = result.skipped_count,
            "imported memory archive"
        );

        Ok(result)
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
                confidence: 1.0,
                classification: crate::classification::Classification::Internal,
            };
            svc.store.store_memory(&entry).await.unwrap();
        }

        let summary = svc.summarize_episode(ep_id).await.unwrap();
        assert!(summary.contains("First point"));
        assert!(summary.contains("Second point"));
        assert!(summary.contains("Third point"));
    }

    #[tokio::test]
    async fn export_agent_memory_creates_valid_json_file() {
        let svc = make_service();
        let agent = AgentId::new();

        svc.remember(agent, "fact one", MemoryType::Semantic, None)
            .await
            .unwrap();
        svc.remember(agent, "fact two", MemoryType::Episodic, None)
            .await
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive.json");

        let count = svc.export_agent_memory(&agent, &path).await.unwrap();
        assert_eq!(count, 2);
        assert!(path.exists());

        // Parse the file back and verify it is valid JSON.
        let bytes = std::fs::read(&path).unwrap();
        let archive: crate::export::MemoryArchive =
            serde_json::from_slice(&bytes).unwrap();

        assert_eq!(archive.agent_id, agent);
        assert_eq!(archive.entries.len(), 2);
        assert_eq!(archive.version, crate::export::ARCHIVE_VERSION);
    }

    #[tokio::test]
    async fn export_agent_memory_includes_episodes() {
        let svc = make_service();
        let agent = AgentId::new();

        svc.start_episode(agent, "session 1").await.unwrap();
        svc.start_episode(agent, "session 2").await.unwrap();
        svc.remember(agent, "memory in session", MemoryType::Episodic, None)
            .await
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive.json");

        // Count = entries + episodes = 1 + 2 = 3.
        let count = svc.export_agent_memory(&agent, &path).await.unwrap();
        assert_eq!(count, 3);

        let bytes = std::fs::read(&path).unwrap();
        let archive: crate::export::MemoryArchive =
            serde_json::from_slice(&bytes).unwrap();
        assert_eq!(archive.episodes.len(), 2);
        assert_eq!(archive.entries.len(), 1);
    }

    #[tokio::test]
    async fn import_agent_memory_restores_entries() {
        let source_svc = make_service();
        let dest_svc = make_service();
        let agent = AgentId::new();

        source_svc
            .remember(agent, "transferred fact", MemoryType::Semantic, None)
            .await
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive.json");

        source_svc.export_agent_memory(&agent, &path).await.unwrap();
        let result = dest_svc.import_agent_memory(&path).await.unwrap();

        assert_eq!(result.imported_count, 1);
        assert_eq!(result.skipped_count, 0);
        assert!(result.errors.is_empty());

        // Verify the memory is searchable in the destination service.
        let found = dest_svc.recall(agent, "transferred", 10).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].content, "transferred fact");
    }

    #[tokio::test]
    async fn export_import_round_trip_preserves_all_fields() {
        let source_svc = make_service();
        let dest_svc = make_service();
        let agent = AgentId::new();

        let prov = MemoryProvenance {
            source_run_id: Some("run-1".into()),
            source_turn: Some(7),
            extraction_method: "user_input".into(),
            confidence: 0.95,
            verified: true,
        };

        let id = source_svc
            .remember(agent, "preserve me", MemoryType::Procedural, Some(prov))
            .await
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive.json");

        source_svc.export_agent_memory(&agent, &path).await.unwrap();
        dest_svc.import_agent_memory(&path).await.unwrap();

        let entry = dest_svc.store.get_memory(id).await.unwrap();
        assert_eq!(entry.content, "preserve me");
        assert_eq!(entry.memory_type, MemoryType::Procedural);
        assert_eq!(entry.agent_id, agent);

        let p = entry.provenance.unwrap();
        assert_eq!(p.source_run_id.as_deref(), Some("run-1"));
        assert_eq!(p.source_turn, Some(7));
        assert_eq!(p.extraction_method, "user_input");
        assert!((p.confidence - 0.95).abs() < f64::EPSILON);
        assert!(p.verified);
    }

    #[tokio::test]
    async fn import_agent_memory_skips_duplicates() {
        let svc = make_service();
        let agent = AgentId::new();

        svc.remember(agent, "existing", MemoryType::Semantic, None)
            .await
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive.json");

        svc.export_agent_memory(&agent, &path).await.unwrap();

        // Import into the SAME store — should skip.
        let result = svc.import_agent_memory(&path).await.unwrap();
        assert_eq!(result.skipped_count, 1);
        assert_eq!(result.imported_count, 0);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn import_agent_memory_file_not_found_returns_error() {
        let svc = make_service();
        let missing = std::path::Path::new("/nonexistent/path/archive.json");
        let result = svc.import_agent_memory(missing).await;
        assert!(result.is_err());
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
