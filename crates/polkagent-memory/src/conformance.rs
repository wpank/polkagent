//! Shared conformance test suite for [`MemoryStore`] implementations.
//!
//! This module defines the canonical set of behavioural tests that **every**
//! adapter implementing [`MemoryStore`] must pass (PRD-15).  Tests are plain
//! `async fn`s so any adapter crate can call them from its own `tests/`
//! directory without macro magic.
//!
//! # Usage
//!
//! ```rust,ignore
//! // In your adapter crate: tests/conformance.rs
//! use polkagent_memory::conformance;
//! use my_adapter::MyMemoryStore;
//!
//! #[tokio::test]
//! async fn test_store_and_retrieve() {
//!     let store = MyMemoryStore::new_for_tests();
//!     let agent_id = polkagent_core::ids::AgentId::new();
//!     conformance::test_store_and_retrieve(&store, agent_id).await;
//! }
//! ```

use chrono::Utc;
use polkagent_core::ids::AgentId;

use crate::error::MemoryError;
use crate::store::MemoryStore;
use crate::types::{Episode, EpisodeId, MemoryEntry, MemoryId, MemoryProvenance, MemoryQuery, MemoryType};

// ---------------------------------------------------------------------------
// Builder helpers
// ---------------------------------------------------------------------------

/// Build a minimal [`MemoryEntry`] for the given agent.
#[must_use]
pub fn make_memory_entry(agent_id: AgentId, content: impl Into<String>) -> MemoryEntry {
    let now = Utc::now();
    MemoryEntry {
        id: MemoryId::new(),
        agent_id,
        episode_id: None,
        memory_type: MemoryType::Semantic,
        content: content.into(),
        embedding: None,
        metadata: serde_json::Value::Null,
        provenance: Some(MemoryProvenance {
            source_run_id: None,
            source_turn: None,
            extraction_method: "conformance_test".into(),
            confidence: 1.0,
            verified: true,
        }),
        created_at: now,
        accessed_at: now,
        access_count: 0,
        relevance_score: 1.0,
        confidence: 1.0,
        classification: crate::classification::Classification::Internal,
    }
}

/// Build a minimal [`Episode`] for the given agent.
#[must_use]
pub fn make_episode(agent_id: AgentId, title: impl Into<String>) -> Episode {
    Episode {
        id: EpisodeId::new(),
        agent_id,
        title: title.into(),
        summary: None,
        started_at: Utc::now(),
        ended_at: None,
        turn_count: 0,
        metadata: serde_json::Value::Null,
    }
}

// ---------------------------------------------------------------------------
// MemoryStore conformance tests
// ---------------------------------------------------------------------------

/// Conformance: `store_memory()` then `get_memory()` returns the same data.
///
/// Verifies the basic CRUD contract for memory entries.
pub async fn test_store_and_retrieve(store: &dyn MemoryStore, agent_id: AgentId) {
    let entry = make_memory_entry(agent_id, "The user prefers dark mode.");
    let memory_id = entry.id;

    let returned_id = store
        .store_memory(&entry)
        .await
        .expect("store_memory() must not fail for a valid entry");

    assert_eq!(
        returned_id, memory_id,
        "store_memory() must return the same MemoryId that was in the entry"
    );

    let retrieved = store
        .get_memory(memory_id)
        .await
        .expect("get_memory() must succeed after store_memory()");

    assert_eq!(
        retrieved.id, memory_id,
        "get_memory() must return the stored MemoryId"
    );
    assert_eq!(
        retrieved.agent_id, agent_id,
        "get_memory() must return the stored agent_id"
    );
    assert_eq!(
        retrieved.content, "The user prefers dark mode.",
        "get_memory() must return the stored content"
    );
    assert_eq!(
        retrieved.memory_type,
        MemoryType::Semantic,
        "get_memory() must return the stored memory_type"
    );
}

/// Conformance: `get_memory()` on a non-existent ID returns an error.
pub async fn test_get_memory_not_found(store: &dyn MemoryStore) {
    let bogus_id = MemoryId::new();
    let result = store.get_memory(bogus_id).await;

    assert!(
        result.is_err(),
        "get_memory() must return an error for a non-existent MemoryId; got Ok(_)"
    );

    // The error should be NotFound.
    match result {
        Err(MemoryError::NotFound(_)) => {}
        Err(other) => {
            panic!(
                "get_memory() on missing id should return MemoryError::NotFound; got: {other}"
            );
        }
        Ok(_) => unreachable!(),
    }
}

/// Conformance: `search()` returns memories whose content matches the query.
///
/// This test stores two memories with different content and verifies that a
/// targeted search returns the relevant one.
pub async fn test_search_returns_relevant(store: &dyn MemoryStore, agent_id: AgentId) {
    // Store two semantically distinct memories.
    let entry_a = make_memory_entry(agent_id, "The user loves Rust programming.");
    let entry_b = make_memory_entry(agent_id, "The user dislikes loud music.");

    store.store_memory(&entry_a).await.expect("store a");
    store.store_memory(&entry_b).await.expect("store b");

    // Search for content related to "Rust".
    let query = MemoryQuery {
        agent_id,
        query_text: "Rust programming".into(),
        memory_types: None,
        limit: 10,
        min_relevance: None,
        since: None,
        episode_id: None,
    };

    let results = store
        .search(&query)
        .await
        .expect("search() must not fail for a valid query");

    assert!(
        !results.is_empty(),
        "search('Rust programming') must return at least one result; got empty"
    );

    // The Rust-related memory should appear in the results.
    let has_rust = results
        .iter()
        .any(|m| m.content.contains("Rust"));

    assert!(
        has_rust,
        "search('Rust programming') must return the memory about Rust; \
         got: {:?}",
        results.iter().map(|m| &m.content).collect::<Vec<_>>()
    );
}

/// Conformance: `search()` respects the `limit` field.
pub async fn test_search_respects_limit(store: &dyn MemoryStore, agent_id: AgentId) {
    // Store 5 memories with similar content.
    for i in 0..5_u32 {
        let entry = make_memory_entry(agent_id, format!("Memory entry number {i} about widgets."));
        store.store_memory(&entry).await.expect("store");
    }

    let query = MemoryQuery {
        agent_id,
        query_text: "widgets".into(),
        memory_types: None,
        limit: 2,
        min_relevance: None,
        since: None,
        episode_id: None,
    };

    let results = store
        .search(&query)
        .await
        .expect("search() with limit=2 must not fail");

    assert!(
        results.len() <= 2,
        "search() must respect limit=2; got {} results",
        results.len()
    );
}

/// Conformance: `delete_memory()` removes the entry from the store.
pub async fn test_delete_memory(store: &dyn MemoryStore, agent_id: AgentId) {
    let entry = make_memory_entry(agent_id, "A memory that will be deleted.");
    let memory_id = entry.id;

    store.store_memory(&entry).await.expect("store");

    // Verify it exists.
    store
        .get_memory(memory_id)
        .await
        .expect("get_memory() must succeed before delete");

    // Delete it.
    store
        .delete_memory(memory_id)
        .await
        .expect("delete_memory() must not fail for an existing memory");

    // Verify it no longer exists.
    let result = store.get_memory(memory_id).await;
    assert!(
        result.is_err(),
        "get_memory() must fail after delete_memory(); got Ok(_)"
    );
}

/// Conformance: `update_relevance()` changes the stored relevance score.
pub async fn test_update_relevance(store: &dyn MemoryStore, agent_id: AgentId) {
    let mut entry = make_memory_entry(agent_id, "Memory with adjustable relevance.");
    entry.relevance_score = 1.0;
    let memory_id = entry.id;

    store.store_memory(&entry).await.expect("store");

    store
        .update_relevance(memory_id, 0.5)
        .await
        .expect("update_relevance() must not fail for an existing memory");

    let updated = store
        .get_memory(memory_id)
        .await
        .expect("get_memory() after update_relevance()");

    assert!(
        (updated.relevance_score - 0.5).abs() < 1e-6,
        "get_memory() must reflect the updated relevance_score; \
         expected 0.5, got {}",
        updated.relevance_score,
    );
}

/// Conformance: full episode lifecycle — create, retrieve, end.
pub async fn test_episode_lifecycle(store: &dyn MemoryStore, agent_id: AgentId) {
    let episode = make_episode(agent_id, "Conformance test episode");
    let episode_id = episode.id;

    // Create.
    let returned_id = store
        .create_episode(&episode)
        .await
        .expect("create_episode() must not fail for a valid episode");

    assert_eq!(
        returned_id, episode_id,
        "create_episode() must return the EpisodeId from the provided episode"
    );

    // Retrieve.
    let retrieved = store
        .get_episode(episode_id)
        .await
        .expect("get_episode() must succeed after create_episode()");

    assert_eq!(retrieved.id, episode_id);
    assert_eq!(retrieved.agent_id, agent_id);
    assert_eq!(retrieved.title, "Conformance test episode");
    assert!(retrieved.ended_at.is_none(), "episode must not be ended yet");

    // End.
    store
        .end_episode(episode_id, "Conformance episode ended successfully.")
        .await
        .expect("end_episode() must not fail for an active episode");

    let ended = store
        .get_episode(episode_id)
        .await
        .expect("get_episode() after end_episode()");

    assert!(
        ended.ended_at.is_some(),
        "ended_at must be set after end_episode()"
    );
    assert_eq!(
        ended.summary.as_deref(),
        Some("Conformance episode ended successfully."),
        "summary must be stored by end_episode()"
    );
}

/// Conformance: `list_episodes()` returns episodes for the given agent in
/// most-recent-first order.
pub async fn test_list_episodes_ordered(store: &dyn MemoryStore, agent_id: AgentId) {
    // Create three episodes.
    for i in 0..3_u32 {
        let episode = make_episode(agent_id, format!("Episode {i}"));
        store.create_episode(&episode).await.expect("create_episode");
    }

    let episodes = store
        .list_episodes(agent_id, 10)
        .await
        .expect("list_episodes() must not fail");

    assert!(
        episodes.len() >= 3,
        "list_episodes() must return at least 3 episodes; got {}",
        episodes.len()
    );

    // Verify most-recent-first ordering.
    for window in episodes.windows(2) {
        assert!(
            window[0].started_at >= window[1].started_at,
            "list_episodes() must return episodes in most-recent-first order; \
             got {:?} before {:?}",
            window[0].started_at,
            window[1].started_at,
        );
    }
}

/// Conformance: `list_episodes()` respects the `limit` parameter.
pub async fn test_list_episodes_respects_limit(store: &dyn MemoryStore, agent_id: AgentId) {
    for i in 0..5_u32 {
        let ep = make_episode(agent_id, format!("Limit test episode {i}"));
        store.create_episode(&ep).await.expect("create");
    }

    let limited = store
        .list_episodes(agent_id, 2)
        .await
        .expect("list_episodes(limit=2) must not fail");

    assert!(
        limited.len() <= 2,
        "list_episodes() must respect limit=2; got {} episodes",
        limited.len()
    );
}

/// Conformance: memories can be associated with an episode.
pub async fn test_memory_with_episode(store: &dyn MemoryStore, agent_id: AgentId) {
    // Create an episode first.
    let episode = make_episode(agent_id, "Episode for memory association");
    let episode_id = episode.id;
    store.create_episode(&episode).await.expect("create_episode");

    // Store a memory linked to that episode.
    let mut entry = make_memory_entry(agent_id, "Memory inside episode.");
    entry.episode_id = Some(episode_id);
    let memory_id = entry.id;

    store.store_memory(&entry).await.expect("store_memory");

    // Retrieve and verify the episode_id.
    let retrieved = store
        .get_memory(memory_id)
        .await
        .expect("get_memory()");

    assert_eq!(
        retrieved.episode_id,
        Some(episode_id),
        "retrieved memory must carry the associated episode_id"
    );
}
