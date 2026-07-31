//! Export and import of agent memory archives.
//!
//! A [`MemoryArchive`] is a JSON-serialisable snapshot of all memories and
//! episodes belonging to a single agent. Archives can be produced with
//! [`export_archive`] and restored with [`import_archive`].
//!
//! # Format
//!
//! Archives are serialised as JSON objects. The version field allows future
//! schema evolution to be detected.
//!
//! # Duplicate handling
//!
//! During import, entries whose [`MemoryId`] or [`EpisodeId`] already exist
//! in the store are silently skipped and counted in
//! [`ImportResult::skipped_count`]. This makes imports idempotent.

use chrono::{DateTime, Utc};
use polkagent_core::ids::AgentId;
use serde::{Deserialize, Serialize};

use crate::error::{MemoryError, MemoryResult};
use crate::store::MemoryStore;
use crate::types::{Episode, MemoryEntry, MemoryQuery};

// ---------------------------------------------------------------------------
// Archive version
// ---------------------------------------------------------------------------

/// The current archive format version. Increment when the schema changes.
pub const ARCHIVE_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// MemoryArchive
// ---------------------------------------------------------------------------

/// A complete, portable snapshot of an agent's memories and episodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryArchive {
    /// The agent whose data is captured in this archive.
    pub agent_id: AgentId,
    /// All memory entries belonging to the agent at export time.
    pub entries: Vec<MemoryEntry>,
    /// All episodes belonging to the agent at export time.
    pub episodes: Vec<Episode>,
    /// When the archive was produced.
    pub exported_at: DateTime<Utc>,
    /// Archive format version.
    pub version: u32,
}

// ---------------------------------------------------------------------------
// ImportResult
// ---------------------------------------------------------------------------

/// Statistics returned after an import operation.
#[derive(Debug, Clone, Default)]
pub struct ImportResult {
    /// Number of entries and episodes successfully imported.
    pub imported_count: usize,
    /// Number of entries and episodes skipped because they already exist.
    pub skipped_count: usize,
    /// Non-fatal errors encountered for individual records.
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// export_archive
// ---------------------------------------------------------------------------

/// Export all memories and episodes for `agent_id` into a portable archive.
///
/// The archive captures a point-in-time snapshot. Any changes made to the
/// store after `export_archive` returns are not reflected in the archive.
pub async fn export_archive(
    store: &dyn MemoryStore,
    agent_id: &AgentId,
) -> MemoryResult<MemoryArchive> {
    // Fetch all memories for the agent (no text filter, no limit cap).
    let query = MemoryQuery {
        agent_id: *agent_id,
        query_text: String::new(),
        memory_types: None,
        limit: usize::MAX / 2, // effectively unlimited
        min_relevance: None,
        since: None,
        episode_id: None,
    };

    let entries = store.search(&query).await?;
    let episodes = store.list_episodes(*agent_id, usize::MAX / 2).await?;

    Ok(MemoryArchive {
        agent_id: *agent_id,
        entries,
        episodes,
        exported_at: Utc::now(),
        version: ARCHIVE_VERSION,
    })
}

// ---------------------------------------------------------------------------
// import_archive
// ---------------------------------------------------------------------------

/// Import a previously exported archive into the store.
///
/// Entries and episodes whose identifiers already exist in the store are
/// silently skipped (idempotent). Entries in the archive that belong to a
/// different `agent_id` than the one recorded in the archive header are
/// also silently skipped.
pub async fn import_archive(
    store: &dyn MemoryStore,
    archive: &MemoryArchive,
) -> MemoryResult<ImportResult> {
    let mut result = ImportResult::default();

    // --- Import episodes first (memories may reference them) ---
    for episode in &archive.episodes {
        match store.create_episode(episode).await {
            Ok(_) => {
                result.imported_count += 1;
            }
            Err(MemoryError::Sqlite(ref e))
                if e.to_string().contains("UNIQUE constraint failed") =>
            {
                result.skipped_count += 1;
            }
            Err(e) => {
                result.errors.push(format!("episode {}: {e}", episode.id));
            }
        }
    }

    // --- Import memory entries ---
    for entry in &archive.entries {
        // Only import entries that belong to the archive's agent.
        if entry.agent_id != archive.agent_id {
            result.skipped_count += 1;
            continue;
        }

        match store.store_memory(entry).await {
            Ok(_) => {
                result.imported_count += 1;
            }
            Err(MemoryError::Sqlite(ref e))
                if e.to_string().contains("UNIQUE constraint failed") =>
            {
                result.skipped_count += 1;
            }
            Err(e) => {
                result.errors.push(format!("memory {}: {e}", entry.id));
            }
        }
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use polkagent_core::ids::AgentId;
    use std::sync::Arc;

    use crate::classification::Classification;
    use crate::sqlite::SqliteMemoryStore;
    use crate::types::{EpisodeId, MemoryEntry, MemoryId, MemoryType};

    fn make_store() -> Arc<dyn MemoryStore> {
        Arc::new(SqliteMemoryStore::open_in_memory().unwrap())
    }

    fn make_entry(agent_id: AgentId, content: &str) -> MemoryEntry {
        let now = Utc::now();
        MemoryEntry {
            id: MemoryId::new(),
            agent_id,
            episode_id: None,
            memory_type: MemoryType::Semantic,
            content: content.to_string(),
            embedding: None,
            metadata: serde_json::json!({}),
            provenance: None,
            created_at: now,
            accessed_at: now,
            access_count: 0,
            relevance_score: 1.0,
            confidence: 1.0,
            classification: Classification::default(),
        }
    }

    fn make_episode(agent_id: AgentId, title: &str) -> Episode {
        Episode {
            id: EpisodeId::new(),
            agent_id,
            title: title.to_string(),
            summary: None,
            started_at: Utc::now(),
            ended_at: None,
            turn_count: 0,
            metadata: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn export_full_archive_contains_all_entries() {
        let store = make_store();
        let agent = AgentId::new();

        for content in &["fact A", "fact B", "fact C"] {
            store.store_memory(&make_entry(agent, content)).await.unwrap();
        }
        store.create_episode(&make_episode(agent, "chat 1")).await.unwrap();
        store.create_episode(&make_episode(agent, "chat 2")).await.unwrap();

        let archive = export_archive(store.as_ref(), &agent).await.unwrap();

        assert_eq!(archive.agent_id, agent);
        assert_eq!(archive.entries.len(), 3);
        assert_eq!(archive.episodes.len(), 2);
        assert_eq!(archive.version, ARCHIVE_VERSION);
    }

    #[tokio::test]
    async fn import_entries_restored_correctly() {
        let source_store = make_store();
        let agent = AgentId::new();

        store_memory_helper(&source_store, &make_entry(agent, "imported fact")).await;
        let archive = export_archive(source_store.as_ref(), &agent).await.unwrap();

        let dest_store = make_store();
        let result = import_archive(dest_store.as_ref(), &archive).await.unwrap();

        assert_eq!(result.imported_count, 1);
        assert_eq!(result.skipped_count, 0);
        assert!(result.errors.is_empty());

        // Verify the entry is actually in the destination store.
        let query = MemoryQuery {
            agent_id: agent,
            query_text: "imported".to_string(),
            memory_types: None,
            limit: 10,
            min_relevance: None,
            since: None,
            episode_id: None,
        };
        let results = dest_store.search(&query).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "imported fact");
    }

    #[tokio::test]
    async fn import_duplicate_skipped() {
        let store = make_store();
        let agent = AgentId::new();

        store.store_memory(&make_entry(agent, "existing fact")).await.unwrap();
        let archive = export_archive(store.as_ref(), &agent).await.unwrap();

        // Import into the same store — the entry already exists.
        let result = import_archive(store.as_ref(), &archive).await.unwrap();

        assert_eq!(result.skipped_count, 1);
        assert_eq!(result.imported_count, 0);
    }

    #[tokio::test]
    async fn import_export_round_trip() {
        let store = make_store();
        let agent = AgentId::new();

        // Populate source.
        for content in &["alpha", "beta", "gamma"] {
            store.store_memory(&make_entry(agent, content)).await.unwrap();
        }
        store.create_episode(&make_episode(agent, "session")).await.unwrap();

        // Export.
        let archive = export_archive(store.as_ref(), &agent).await.unwrap();

        // Import into a fresh store.
        let fresh = make_store();
        let result = import_archive(fresh.as_ref(), &archive).await.unwrap();

        // 3 entries + 1 episode = 4 imported.
        assert_eq!(result.imported_count, 4);
        assert_eq!(result.skipped_count, 0);
        assert!(result.errors.is_empty());

        // Verify counts.
        assert_eq!(fresh.count_entries(&agent).await.unwrap(), 3);

        let episodes = fresh.list_episodes(agent, 10).await.unwrap();
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].title, "session");
    }

    #[tokio::test]
    async fn archive_serde_round_trip() {
        let store = make_store();
        let agent = AgentId::new();

        store.store_memory(&make_entry(agent, "serialisable fact")).await.unwrap();
        let archive = export_archive(store.as_ref(), &agent).await.unwrap();

        let json = serde_json::to_string(&archive).unwrap();
        let back: MemoryArchive = serde_json::from_str(&json).unwrap();

        assert_eq!(back.agent_id, archive.agent_id);
        assert_eq!(back.entries.len(), 1);
        assert_eq!(back.entries[0].content, "serialisable fact");
        assert_eq!(back.version, ARCHIVE_VERSION);
    }

    // Helper to avoid borrowing issues.
    async fn store_memory_helper(store: &Arc<dyn MemoryStore>, entry: &MemoryEntry) {
        store.store_memory(entry).await.unwrap();
    }
}
