//! Enhanced memory lifecycle integration tests.
//!
//! Exercises store/search/update-relevance/delete, admission control for
//! duplicate entries, tenant isolation, classification filtering with
//! search_with_classification, and retention sweeps using RetentionSweeper
//! — all wired through the `polkagent-memory` crate boundary.

use std::sync::Arc;

use chrono::{Duration, Utc};

use polkagent_core::AgentId;
use polkagent_memory::{
    Classification, MemoryEntry, MemoryId, MemoryService, MemoryStore,
    MemoryType, RetentionPolicy, RetentionSweeper, SqliteMemoryStore,
};
use polkagent_memory::types::{MemoryQuery};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_store() -> Arc<SqliteMemoryStore> {
    Arc::new(SqliteMemoryStore::open_in_memory().expect("in-memory SQLite store"))
}

fn make_service() -> (MemoryService, Arc<SqliteMemoryStore>) {
    let store = make_store();
    let svc = MemoryService::new(store.clone() as Arc<dyn MemoryStore>);
    (svc, store)
}

fn make_entry(
    agent_id: AgentId,
    content: &str,
    relevance: f64,
    classification: Classification,
) -> MemoryEntry {
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
        relevance_score: relevance,
        confidence: 1.0,
        classification,
    }
}

fn make_dated_entry(agent_id: AgentId, content: &str, days_ago: i64) -> MemoryEntry {
    let created = Utc::now() - Duration::days(days_ago);
    MemoryEntry {
        id: MemoryId::new(),
        agent_id,
        episode_id: None,
        memory_type: MemoryType::Semantic,
        content: content.to_string(),
        embedding: None,
        metadata: serde_json::json!({}),
        provenance: None,
        created_at: created,
        accessed_at: created,
        access_count: 0,
        relevance_score: 0.9,
        confidence: 1.0,
        classification: Classification::Internal,
    }
}

// ---------------------------------------------------------------------------
// IT-MEM-01: Store → search → update relevance → delete lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn store_recall_update_relevance_delete_lifecycle() {
    let (svc, store) = make_service();
    let agent = AgentId::new();

    // 1. Store
    let id = svc
        .remember(
            agent,
            "Polkadot is a sharded protocol",
            MemoryType::Semantic,
            None,
        )
        .await
        .expect("store");

    // 2. Search – memory must be findable
    let before = svc.recall(agent, "sharded protocol", 10).await.expect("recall before update");
    assert_eq!(before.len(), 1, "memory must be found after store");
    assert!(before[0].content.contains("sharded protocol"));

    // 3. Update relevance via the store directly
    store.update_relevance(id, 0.1).await.expect("update relevance");

    // 4. The memory is still present after relevance update
    let after = svc.recall(agent, "sharded protocol", 10).await.expect("recall after update");
    assert_eq!(after.len(), 1, "memory must still be returned after relevance update");

    // 5. Delete (forget)
    svc.forget(id).await.expect("forget");

    let after_delete = svc.recall(agent, "sharded protocol", 10).await.expect("recall after delete");
    assert!(after_delete.is_empty(), "deleted memory must not appear in search");
}

#[tokio::test]
async fn remember_returns_stable_id() {
    let (svc, _) = make_service();
    let agent = AgentId::new();

    let id1 = svc
        .remember(agent, "stable id test memory", MemoryType::Semantic, None)
        .await
        .expect("store");

    let id2 = svc
        .remember(agent, "second distinct memory", MemoryType::Semantic, None)
        .await
        .expect("store 2");

    assert_ne!(id1, id2, "distinct memories must have different IDs");
}

#[tokio::test]
async fn recall_returns_entries_containing_query_term() {
    let (svc, _) = make_service();
    let agent = AgentId::new();

    svc.remember(agent, "Rust is a systems language", MemoryType::Semantic, None)
        .await
        .expect("store 1");
    svc.remember(agent, "Rust is also great for WebAssembly", MemoryType::Semantic, None)
        .await
        .expect("store 2");
    svc.remember(agent, "Python is a scripting language", MemoryType::Semantic, None)
        .await
        .expect("store 3");

    let results = svc.recall(agent, "Rust", 10).await.expect("recall");
    // All returned results should contain "Rust"
    for r in &results {
        assert!(r.content.to_lowercase().contains("rust"), "result '{}' doesn't contain 'Rust'", r.content);
    }
    // Python entry must not appear
    assert!(results.iter().all(|r| !r.content.contains("Python")));
}

#[tokio::test]
async fn update_relevance_changes_score_in_store() {
    let (svc, store) = make_service();
    let agent = AgentId::new();

    let id = svc
        .remember(agent, "relevance test memory", MemoryType::Semantic, None)
        .await
        .expect("store");

    // Default relevance is 1.0
    let entry = store.get_memory(id).await.expect("get memory");
    assert!((entry.relevance_score - 1.0).abs() < f64::EPSILON);

    // Update to 0.3
    store.update_relevance(id, 0.3).await.expect("update relevance");
    let updated = store.get_memory(id).await.expect("get updated");
    assert!((updated.relevance_score - 0.3).abs() < f64::EPSILON);
}

#[tokio::test]
async fn forget_nonexistent_memory_does_not_panic() {
    let (svc, _) = make_service();
    let unknown = MemoryId::new();
    // Either succeeds silently or returns a not-found error; must not panic
    let _ = svc.forget(unknown).await;
}

// ---------------------------------------------------------------------------
// IT-MEM-02: Admission control — duplicate entries
// ---------------------------------------------------------------------------

#[tokio::test]
async fn store_same_content_twice_is_accepted_or_rejected_consistently() {
    let (svc, _) = make_service();
    let agent = AgentId::new();
    let content = "Unique content about Polkadot governance";

    let _id1 = svc
        .remember(agent, content, MemoryType::Semantic, None)
        .await
        .expect("first store");

    let result = svc.remember(agent, content, MemoryType::Semantic, None).await;

    match result {
        Ok(_) => {
            // Accepted: verify that searching returns entries
            let results = svc.recall(agent, content, 10).await.expect("recall");
            assert!(!results.is_empty());
        }
        Err(_) => {
            // Rejected: the first entry is still searchable
            let results = svc.recall(agent, content, 10).await.expect("recall");
            assert_eq!(results.len(), 1, "original entry should still be present");
        }
    }
}

#[tokio::test]
async fn similar_but_different_content_stored_separately() {
    let (svc, store) = make_service();
    let agent = AgentId::new();

    svc.remember(agent, "Polkadot uses NPoS consensus", MemoryType::Semantic, None)
        .await
        .expect("store 1");
    svc.remember(agent, "Polkadot uses NPoS consensus mechanism for validators", MemoryType::Semantic, None)
        .await
        .expect("store 2");

    let count = store.count_entries(&agent).await.expect("count");
    assert_eq!(count, 2, "two distinct entries must be stored");
}

#[tokio::test]
async fn duplicate_check_scoped_to_agent() {
    let (svc, store) = make_service();
    let agent_a = AgentId::new();
    let agent_b = AgentId::new();
    let content = "Shared knowledge across agents";

    svc.remember(agent_a, content, MemoryType::Semantic, None)
        .await
        .expect("store for A");
    svc.remember(agent_b, content, MemoryType::Semantic, None)
        .await
        .expect("store for B");

    // Each agent should have their own entry
    let count_a = store.count_entries(&agent_a).await.expect("count A");
    let count_b = store.count_entries(&agent_b).await.expect("count B");
    assert_eq!(count_a, 1, "agent A should have 1 entry");
    assert_eq!(count_b, 1, "agent B should have 1 entry");
}

// ---------------------------------------------------------------------------
// IT-MEM-03: Tenant isolation — agent A can't see agent B's memories
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_a_cannot_see_agent_b_memories() {
    let (svc, _) = make_service();
    let agent_a = AgentId::new();
    let agent_b = AgentId::new();

    svc.remember(
        agent_b,
        "Agent B private data about Kusama staking yields",
        MemoryType::Semantic,
        None,
    )
    .await
    .expect("store for B");

    let results = svc.recall(agent_a, "Kusama staking yields", 10).await.expect("recall A");
    assert!(results.is_empty(), "agent A must not see agent B's memories");
}

#[tokio::test]
async fn deleting_one_agent_memory_does_not_affect_another() {
    let (svc, _) = make_service();
    let agent_a = AgentId::new();
    let agent_b = AgentId::new();

    let id_a = svc
        .remember(agent_a, "Shared topic: parachain auctions", MemoryType::Semantic, None)
        .await
        .expect("store A");

    svc.remember(agent_b, "Shared topic: parachain auctions", MemoryType::Semantic, None)
        .await
        .expect("store B");

    svc.forget(id_a).await.expect("forget A");

    let results_b = svc.recall(agent_b, "parachain auctions", 10).await.expect("recall B");
    assert!(!results_b.is_empty(), "deleting A's memory must not affect B's");
}

#[tokio::test]
async fn multiple_agents_can_coexist() {
    let (svc, _) = make_service();
    let agents: Vec<AgentId> = (0..5).map(|_| AgentId::new()).collect();

    for (i, &agent) in agents.iter().enumerate() {
        svc.remember(
            agent,
            &format!("Agent {i} unique memory about Polkadot"),
            MemoryType::Semantic,
            None,
        )
        .await
        .expect("store");
    }

    // Each agent sees only its own memory when querying by specific content
    for (i, &agent) in agents.iter().enumerate() {
        let results = svc
            .recall(agent, &format!("Agent {i} unique memory"), 10)
            .await
            .expect("recall");
        assert_eq!(results.len(), 1, "agent {i} should see exactly 1 memory");
        assert!(results[0].content.contains(&format!("Agent {i}")));
    }
}

// ---------------------------------------------------------------------------
// IT-MEM-04: Classification filter — restricted entries filtered from results
// ---------------------------------------------------------------------------

#[tokio::test]
async fn internal_classification_visible_in_normal_recall() {
    let (svc, _) = make_service();
    let agent = AgentId::new();

    svc.remember(agent, "Internal Polkadot configuration", MemoryType::Semantic, None)
        .await
        .expect("store");

    let results = svc.recall(agent, "Polkadot configuration", 10).await.expect("recall");
    assert!(!results.is_empty(), "internal memories must be visible to their owner");
}

#[tokio::test]
async fn search_with_classification_filters_confidential_entries() {
    let (_, store) = make_service();
    let agent = AgentId::new();

    let public_entry = make_entry(agent, "Public block height info", 1.0, Classification::Public);
    let confidential_entry = make_entry(agent, "Confidential validator key", 1.0, Classification::Confidential);

    store.store_memory(&public_entry).await.expect("store public");
    store.store_memory(&confidential_entry).await.expect("store confidential");

    let query = MemoryQuery {
        agent_id: Some(agent),
        query_text: String::new(), // match all
        memory_types: None,
        limit: 100,
        min_relevance: None,
        since: None,
        episode_id: None,
    };

    // Search with max_classification = Internal: only Public is visible
    let visible = store
        .search_with_classification(&query, Classification::Internal)
        .await
        .expect("search with classification");

    // Public entries should appear; Confidential entries should be excluded
    for entry in &visible {
        assert!(
            entry.classification <= Classification::Internal,
            "result with classification {:?} exceeded filter level",
            entry.classification
        );
    }
}

#[tokio::test]
async fn search_with_classification_confidential_max_shows_all() {
    let (_, store) = make_service();
    let agent = AgentId::new();

    let public = make_entry(agent, "Public validator count", 1.0, Classification::Public);
    let internal = make_entry(agent, "Internal config setting", 1.0, Classification::Internal);
    let confidential = make_entry(agent, "Confidential key material", 1.0, Classification::Confidential);

    store.store_memory(&public).await.expect("store public");
    store.store_memory(&internal).await.expect("store internal");
    store.store_memory(&confidential).await.expect("store confidential");

    let query = MemoryQuery {
        agent_id: Some(agent),
        query_text: String::new(),
        memory_types: None,
        limit: 100,
        min_relevance: None,
        since: None,
        episode_id: None,
    };

    // With Confidential max — all entries should be visible
    let all = store
        .search_with_classification(&query, Classification::Confidential)
        .await
        .expect("search with max classification");

    assert_eq!(all.len(), 3, "all three entries must be visible with Confidential max");
}

#[tokio::test]
async fn public_classification_entries_visible_to_store() {
    let (_, store) = make_service();
    let agent = AgentId::new();

    let entry = make_entry(agent, "Public Polkadot block height info", 1.0, Classification::Public);
    store.store_memory(&entry).await.expect("store public");

    let query = MemoryQuery {
        agent_id: Some(agent),
        query_text: "block height".to_string(),
        memory_types: None,
        limit: 10,
        min_relevance: None,
        since: None,
        episode_id: None,
    };

    let results = store.search(&query).await.expect("search");
    assert!(!results.is_empty(), "public memories must be visible");
}

// ---------------------------------------------------------------------------
// IT-MEM-05: Retention sweep — old entries deleted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retention_sweep_deletes_old_entries() {
    let store = make_store();
    let agent = AgentId::new();

    // Store 2 old entries and 1 recent
    let old1 = make_dated_entry(agent, "Old memory about Polkadot from the past", 400);
    let old2 = make_dated_entry(agent, "Another old memory about governance", 400);
    let recent = make_entry(agent, "Recent Polkadot memory", 1.0, Classification::Internal);

    store.store_memory(&old1).await.expect("store old1");
    store.store_memory(&old2).await.expect("store old2");
    store.store_memory(&recent).await.expect("store recent");

    assert_eq!(store.count_entries(&agent).await.expect("count before"), 3);

    let policy = RetentionPolicy {
        max_entries_per_agent: 1000,
        max_age_days: 365,
        min_relevance: 0.0,
        sweep_interval_secs: 3600,
    };
    let sweeper = RetentionSweeper::new(store.clone() as Arc<dyn MemoryStore>, policy, agent);
    let result = sweeper.sweep().await.expect("sweep");

    assert_eq!(result.by_age, 2, "two old entries should be deleted by age");
    assert_eq!(result.deleted_count, 2);

    let after = store.count_entries(&agent).await.expect("count after");
    assert_eq!(after, 1, "only the recent entry should survive");
}

#[tokio::test]
async fn retention_sweep_deletes_low_relevance_entries() {
    let store = make_store();
    let agent = AgentId::new();

    let high = make_entry(agent, "High relevance Polkadot fact", 0.9, Classification::Internal);
    let low1 = make_entry(agent, "Low relevance irrelevant info", 0.05, Classification::Internal);
    let low2 = make_entry(agent, "Very low relevance stale note", 0.03, Classification::Internal);

    store.store_memory(&high).await.expect("store high");
    store.store_memory(&low1).await.expect("store low1");
    store.store_memory(&low2).await.expect("store low2");

    let policy = RetentionPolicy {
        max_entries_per_agent: 1000,
        max_age_days: 365,
        min_relevance: 0.1,
        sweep_interval_secs: 3600,
    };
    let sweeper = RetentionSweeper::new(store.clone() as Arc<dyn MemoryStore>, policy, agent);
    let result = sweeper.sweep().await.expect("sweep");

    assert_eq!(result.by_relevance, 2, "two low-relevance entries should be swept");
    assert_eq!(store.count_entries(&agent).await.expect("count"), 1);
}

#[tokio::test]
async fn retention_sweep_enforces_count_limit() {
    let store = make_store();
    let agent = AgentId::new();

    for i in 0..10 {
        let entry = make_entry(agent, &format!("Memory {i} about Polkadot", ), 0.9, Classification::Internal);
        store.store_memory(&entry).await.expect("store");
    }

    let policy = RetentionPolicy {
        max_entries_per_agent: 5,
        max_age_days: 365,
        min_relevance: 0.0,
        sweep_interval_secs: 3600,
    };
    let sweeper = RetentionSweeper::new(store.clone() as Arc<dyn MemoryStore>, policy, agent);
    let result = sweeper.sweep().await.expect("sweep");

    assert_eq!(result.by_count_limit, 5, "5 entries should be deleted to cap at 5");
    assert_eq!(store.count_entries(&agent).await.expect("count"), 5);
}

#[tokio::test]
async fn retention_sweep_no_deletions_within_policy() {
    let store = make_store();
    let agent = AgentId::new();

    let e1 = make_entry(agent, "Polkadot fact one", 0.8, Classification::Internal);
    let e2 = make_entry(agent, "Polkadot fact two", 0.7, Classification::Internal);
    store.store_memory(&e1).await.expect("store");
    store.store_memory(&e2).await.expect("store");

    let policy = RetentionPolicy {
        max_entries_per_agent: 100,
        max_age_days: 365,
        min_relevance: 0.0,
        sweep_interval_secs: 3600,
    };
    let sweeper = RetentionSweeper::new(store.clone() as Arc<dyn MemoryStore>, policy, agent);
    let result = sweeper.sweep().await.expect("sweep");

    assert_eq!(result.deleted_count, 0, "no entries should be deleted when within policy");
    assert_eq!(store.count_entries(&agent).await.expect("count"), 2);
}

#[tokio::test]
async fn retention_sweep_combined_age_relevance_and_count() {
    let store = make_store();
    let agent = AgentId::new();

    // 1 old entry + 1 low-relevance + 5 normal — then cap at 3
    let old = make_dated_entry(agent, "Old archived Polkadot history", 10);
    let low_rel = make_entry(agent, "Low relevance stale data", 0.01, Classification::Internal);
    for i in 0..5 {
        let e = make_entry(agent, &format!("Normal Polkadot memory {i}"), 0.9, Classification::Internal);
        store.store_memory(&e).await.expect("store normal");
    }
    store.store_memory(&old).await.expect("store old");
    store.store_memory(&low_rel).await.expect("store low_rel");

    let policy = RetentionPolicy {
        max_entries_per_agent: 3,
        max_age_days: 5,
        min_relevance: 0.05,
        sweep_interval_secs: 3600,
    };
    let sweeper = RetentionSweeper::new(store.clone() as Arc<dyn MemoryStore>, policy, agent);
    let result = sweeper.sweep().await.expect("sweep");

    assert_eq!(result.by_age, 1, "one entry deleted by age");
    assert_eq!(result.by_relevance, 1, "one entry deleted by relevance");
    assert_eq!(result.by_count_limit, 2, "two more deleted to cap at 3");
    assert_eq!(store.count_entries(&agent).await.expect("count"), 3);
}

#[tokio::test]
async fn retention_sweep_isolation_across_agents() {
    let store = make_store();
    let agent_a = AgentId::new();
    let agent_b = AgentId::new();

    // Agent A: 10 entries — policy caps at 5
    for i in 0..10 {
        let e = make_entry(agent_a, &format!("Agent A memory {i}"), 0.9, Classification::Internal);
        store.store_memory(&e).await.expect("store A");
    }

    // Agent B: 2 entries — should be untouched
    let b1 = make_entry(agent_b, "Agent B memory one", 0.9, Classification::Internal);
    let b2 = make_entry(agent_b, "Agent B memory two", 0.9, Classification::Internal);
    store.store_memory(&b1).await.expect("store B1");
    store.store_memory(&b2).await.expect("store B2");

    let policy = RetentionPolicy {
        max_entries_per_agent: 5,
        max_age_days: 365,
        min_relevance: 0.0,
        sweep_interval_secs: 3600,
    };

    // Sweep only for agent A
    let sweeper_a = RetentionSweeper::new(store.clone() as Arc<dyn MemoryStore>, policy, agent_a);
    sweeper_a.sweep().await.expect("sweep A");

    assert_eq!(store.count_entries(&agent_a).await.expect("count A"), 5, "A should have 5 entries");
    assert_eq!(store.count_entries(&agent_b).await.expect("count B"), 2, "B entries should be untouched");
}

// ---------------------------------------------------------------------------
// Extra: count_entries + delete_by_age
// ---------------------------------------------------------------------------

#[tokio::test]
async fn count_entries_starts_at_zero() {
    let (_, store) = make_service();
    let agent = AgentId::new();
    let count = store.count_entries(&agent).await.expect("count");
    assert_eq!(count, 0);
}

#[tokio::test]
async fn count_entries_increases_after_store() {
    let (svc, store) = make_service();
    let agent = AgentId::new();

    svc.remember(agent, "one", MemoryType::Semantic, None).await.expect("store 1");
    svc.remember(agent, "two", MemoryType::Semantic, None).await.expect("store 2");
    svc.remember(agent, "three", MemoryType::Semantic, None).await.expect("store 3");

    let count = store.count_entries(&agent).await.expect("count");
    assert_eq!(count, 3);
}

#[tokio::test]
async fn delete_by_age_removes_old_entries() {
    let (_, store) = make_service();
    let agent = AgentId::new();

    let old = make_dated_entry(agent, "Old Polkadot history", 400);
    let recent = make_entry(agent, "Recent Polkadot fact", 1.0, Classification::Internal);

    store.store_memory(&old).await.expect("store old");
    store.store_memory(&recent).await.expect("store recent");

    // Delete entries older than 365 days
    let deleted = store
        .delete_by_age(&agent, chrono::Duration::days(365))
        .await
        .expect("delete by age");

    assert_eq!(deleted, 1, "one entry should be deleted");
    assert_eq!(store.count_entries(&agent).await.expect("count"), 1, "only recent survives");
}

#[tokio::test]
async fn recall_with_multiple_types_returns_all_types() {
    let (svc, _) = make_service();
    let agent = AgentId::new();

    svc.remember(agent, "Polkadot semantic fact", MemoryType::Semantic, None).await.expect("store semantic");
    svc.remember(agent, "Polkadot procedural step", MemoryType::Procedural, None).await.expect("store procedural");
    svc.remember(agent, "Polkadot episodic event", MemoryType::Episodic, None).await.expect("store episodic");

    let results = svc.recall(agent, "Polkadot", 10).await.expect("recall");
    assert_eq!(results.len(), 3, "all memory types must be recalled");

    let types: Vec<_> = results.iter().map(|r| r.memory_type.clone()).collect();
    assert!(types.contains(&MemoryType::Semantic));
    assert!(types.contains(&MemoryType::Procedural));
    assert!(types.contains(&MemoryType::Episodic));
}
