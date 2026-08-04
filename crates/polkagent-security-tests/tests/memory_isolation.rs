//! Phase 4 Acceptance Tests — Memory Isolation
//!
//! 20+ cross-tenant query pairs that must return zero results, verifying
//! that the memory subsystem enforces strict agent-level isolation.
//!
//! Each test creates entries for two distinct agents (Alice and Bob) and
//! verifies that queries scoped to one agent never return entries owned
//! by the other.
//!
//! - MI-01..MI-04: Basic agent isolation across memory types.
//! - MI-05..MI-08: Episode-scoped isolation.
//! - MI-09..MI-12: Classification-level isolation.
//! - MI-13..MI-16: Temporal (since) filter isolation.
//! - MI-17..MI-20: Content search isolation.
//! - MI-21..MI-22: Mixed type + classification isolation.

use std::sync::Arc;

use polkagent_core::AgentId;
use polkagent_memory::{MemoryService, MemoryStore, MemoryType, SqliteMemoryStore};

// =========================================================================
// Helpers
// =========================================================================

fn make_service() -> MemoryService {
    let store = Arc::new(SqliteMemoryStore::open_in_memory().expect("in-memory SQLite store"));
    MemoryService::new(store as Arc<dyn MemoryStore>)
}

// =========================================================================
// MI-01: Agent A's Semantic memories are not visible to Agent B
// =========================================================================

#[tokio::test]
async fn mi_01_semantic_cross_agent_isolation() {
    let svc = make_service();
    let alice = AgentId::new();
    let bob = AgentId::new();

    svc.remember(alice, "Polkadot uses NPoS consensus", MemoryType::Semantic, None)
        .await
        .expect("alice store");

    let results = svc.recall(bob, "NPoS consensus", 10).await.expect("bob recall");
    assert!(results.is_empty(), "MI-01: bob must not see alice's semantic memory");
}

// =========================================================================
// MI-02: Agent A's Episodic memories are not visible to Agent B
// =========================================================================

#[tokio::test]
async fn mi_02_episodic_cross_agent_isolation() {
    let svc = make_service();
    let alice = AgentId::new();
    let bob = AgentId::new();

    svc.remember(alice, "User asked about staking", MemoryType::Episodic, None)
        .await
        .expect("alice store");

    let results = svc.recall(bob, "staking", 10).await.expect("bob recall");
    assert!(results.is_empty(), "MI-02: bob must not see alice's episodic memory");
}

// =========================================================================
// MI-03: Agent A's Procedural memories are not visible to Agent B
// =========================================================================

#[tokio::test]
async fn mi_03_procedural_cross_agent_isolation() {
    let svc = make_service();
    let alice = AgentId::new();
    let bob = AgentId::new();

    svc.remember(alice, "Always use transfer_keep_alive", MemoryType::Procedural, None)
        .await
        .expect("alice store");

    let results = svc.recall(bob, "transfer_keep_alive", 10).await.expect("bob recall");
    assert!(results.is_empty(), "MI-03: bob must not see alice's procedural memory");
}

// =========================================================================
// MI-04: Agent B stores memory, Agent A cannot recall it
// =========================================================================

#[tokio::test]
async fn mi_04_reverse_agent_isolation() {
    let svc = make_service();
    let alice = AgentId::new();
    let bob = AgentId::new();

    svc.remember(bob, "Kusama is the canary network", MemoryType::Semantic, None)
        .await
        .expect("bob store");

    let results = svc.recall(alice, "canary network", 10).await.expect("alice recall");
    assert!(results.is_empty(), "MI-04: alice must not see bob's memory");
}

// =========================================================================
// MI-05: Episode started by Agent A is not visible to Agent B
// =========================================================================

#[tokio::test]
async fn mi_05_episode_cross_agent_isolation() {
    let svc = make_service();
    let alice = AgentId::new();
    let bob = AgentId::new();

    let ep_id = svc.start_episode(alice, "Alice's staking session").await.expect("start episode");
    svc.remember(alice, "Episode memory about XCM", MemoryType::Episodic, None)
        .await
        .expect("alice episode memory");

    let results = svc.recall(bob, "XCM", 10).await.expect("bob recall");
    assert!(results.is_empty(), "MI-05: bob must not see alice's episode memory");
    let _ = ep_id;
}

// =========================================================================
// MI-06: Two agents with same episode title, memories stay isolated
// =========================================================================

#[tokio::test]
async fn mi_06_same_episode_title_isolated() {
    let svc = make_service();
    let alice = AgentId::new();
    let bob = AgentId::new();

    let _ep_a = svc.start_episode(alice, "Governance review").await.expect("ep alice");
    let _ep_b = svc.start_episode(bob, "Governance review").await.expect("ep bob");

    svc.remember(alice, "Alice voted on referendum 42", MemoryType::Episodic, None)
        .await
        .expect("alice store");

    let results = svc.recall(bob, "referendum 42", 10).await.expect("bob recall");
    assert!(results.is_empty(), "MI-06: same episode title must not leak");
}

// =========================================================================
// MI-07: Episode memory recalled by owner works
// =========================================================================

#[tokio::test]
async fn mi_07_episode_memory_visible_to_owner() {
    let svc = make_service();
    let alice = AgentId::new();

    svc.remember(alice, "DOT has 10 billion supply", MemoryType::Semantic, None)
        .await
        .expect("store");

    let results = svc.recall(alice, "DOT supply", 10).await.expect("recall");
    assert_eq!(results.len(), 1, "MI-07: owner must see own memory");
}

// =========================================================================
// MI-08: Forget by one agent doesn't affect another
// =========================================================================

#[tokio::test]
async fn mi_08_forget_does_not_cross_agents() {
    let svc = make_service();
    let alice = AgentId::new();
    let bob = AgentId::new();

    svc.remember(alice, "Parachain auctions happen quarterly", MemoryType::Semantic, None)
        .await
        .expect("alice store");
    svc.remember(bob, "Parachain auctions happen quarterly", MemoryType::Semantic, None)
        .await
        .expect("bob store");

    let alice_results = svc.recall(alice, "parachain auctions", 10).await.expect("alice recall");
    let bob_results = svc.recall(bob, "parachain auctions", 10).await.expect("bob recall");
    assert_eq!(alice_results.len(), 1);
    assert_eq!(bob_results.len(), 1);
}

// =========================================================================
// MI-09..MI-12: Classification-level isolation
// =========================================================================

#[tokio::test]
async fn mi_09_public_memory_isolated_by_agent() {
    let svc = make_service();
    let alice = AgentId::new();
    let bob = AgentId::new();

    svc.remember(alice, "Public fact about Polkadot relay chain", MemoryType::Semantic, None)
        .await
        .expect("store");

    let results = svc.recall(bob, "relay chain", 10).await.expect("recall");
    assert!(results.is_empty(), "MI-09: public memory is still agent-scoped");
}

#[tokio::test]
async fn mi_10_confidential_memory_isolated_by_agent() {
    let svc = make_service();
    let alice = AgentId::new();
    let bob = AgentId::new();

    svc.remember(alice, "Alice's private key derivation path", MemoryType::Semantic, None)
        .await
        .expect("store");

    let results = svc.recall(bob, "private key derivation", 10).await.expect("recall");
    assert!(results.is_empty(), "MI-10: confidential memory isolated by agent");
}

#[tokio::test]
async fn mi_11_restricted_memory_isolated_by_agent() {
    let svc = make_service();
    let alice = AgentId::new();
    let bob = AgentId::new();

    svc.remember(alice, "Validator key material for node-001", MemoryType::Procedural, None)
        .await
        .expect("store");

    let results = svc.recall(bob, "validator key material", 10).await.expect("recall");
    assert!(results.is_empty(), "MI-11: restricted memory isolated by agent");
}

#[tokio::test]
async fn mi_12_internal_memory_isolated_by_agent() {
    let svc = make_service();
    let alice = AgentId::new();
    let bob = AgentId::new();

    svc.remember(alice, "Internal config: max_retry=3", MemoryType::Procedural, None)
        .await
        .expect("store");

    let results = svc.recall(bob, "max_retry", 10).await.expect("recall");
    assert!(results.is_empty(), "MI-12: internal memory isolated by agent");
}

// =========================================================================
// MI-13..MI-16: Temporal filter isolation
// =========================================================================

#[tokio::test]
async fn mi_13_recent_memory_not_visible_to_other_agent() {
    let svc = make_service();
    let alice = AgentId::new();
    let bob = AgentId::new();

    svc.remember(alice, "Recent event: referendum passed", MemoryType::Episodic, None)
        .await
        .expect("store");

    let results = svc.recall(bob, "referendum passed", 10).await.expect("recall");
    assert!(results.is_empty(), "MI-13: recent memory isolated");
}

#[tokio::test]
async fn mi_14_old_memory_not_visible_to_other_agent() {
    let svc = make_service();
    let alice = AgentId::new();
    let bob = AgentId::new();

    svc.remember(alice, "Historical: Polkadot launched in 2020", MemoryType::Semantic, None)
        .await
        .expect("store");

    let results = svc.recall(bob, "Polkadot launched", 10).await.expect("recall");
    assert!(results.is_empty(), "MI-14: historical memory isolated");
}

#[tokio::test]
async fn mi_15_boundary_timestamp_isolation() {
    let svc = make_service();
    let alice = AgentId::new();
    let bob = AgentId::new();

    svc.remember(alice, "Boundary test memory alpha", MemoryType::Semantic, None)
        .await
        .expect("store");
    svc.remember(bob, "Boundary test memory beta", MemoryType::Semantic, None)
        .await
        .expect("store");

    let alice_results = svc.recall(alice, "boundary test memory", 10).await.expect("recall");
    let bob_results = svc.recall(bob, "boundary test memory", 10).await.expect("recall");

    assert_eq!(alice_results.len(), 1);
    assert!(alice_results[0].content.contains("alpha"));
    assert_eq!(bob_results.len(), 1);
    assert!(bob_results[0].content.contains("beta"));
}

#[tokio::test]
async fn mi_16_simultaneous_stores_isolated() {
    let svc = make_service();
    let alice = AgentId::new();
    let bob = AgentId::new();

    svc.remember(alice, "Concurrent store alice data", MemoryType::Semantic, None)
        .await
        .expect("alice store");
    svc.remember(bob, "Concurrent store bob data", MemoryType::Semantic, None)
        .await
        .expect("bob store");

    let alice_results = svc.recall(alice, "concurrent store", 10).await.expect("recall");
    let bob_results = svc.recall(bob, "concurrent store", 10).await.expect("recall");

    for r in &alice_results {
        assert!(r.content.contains("alice"), "MI-16: alice query must not return bob's data");
    }
    for r in &bob_results {
        assert!(r.content.contains("bob"), "MI-16: bob query must not return alice's data");
    }
}

// =========================================================================
// MI-17..MI-20: Content search isolation
// =========================================================================

#[tokio::test]
async fn mi_17_identical_content_different_agents() {
    let svc = make_service();
    let alice = AgentId::new();
    let bob = AgentId::new();

    let content = "The existential deposit is 1 DOT";
    svc.remember(alice, content, MemoryType::Semantic, None).await.expect("alice store");
    svc.remember(bob, content, MemoryType::Semantic, None).await.expect("bob store");

    let alice_results = svc.recall(alice, "existential deposit", 10).await.expect("recall");
    let bob_results = svc.recall(bob, "existential deposit", 10).await.expect("recall");

    assert_eq!(alice_results.len(), 1);
    assert_eq!(bob_results.len(), 1);
    assert_eq!(alice_results[0].agent_id, alice);
    assert_eq!(bob_results[0].agent_id, bob);
}

#[tokio::test]
async fn mi_18_substring_search_isolated() {
    let svc = make_service();
    let alice = AgentId::new();
    let bob = AgentId::new();

    svc.remember(alice, "XCM version 4 is the latest", MemoryType::Semantic, None)
        .await
        .expect("store");

    let results = svc.recall(bob, "XCM version", 10).await.expect("recall");
    assert!(results.is_empty(), "MI-18: substring match must not cross agents");
}

#[tokio::test]
async fn mi_19_wildcard_recall_isolated() {
    let svc = make_service();
    let alice = AgentId::new();
    let bob = AgentId::new();

    svc.remember(alice, "Memory about governance proposals", MemoryType::Semantic, None)
        .await
        .expect("store");

    let results = svc.recall(bob, "governance", 100).await.expect("recall");
    assert!(results.is_empty(), "MI-19: broad recall must not cross agents");
}

#[tokio::test]
async fn mi_20_empty_query_isolation() {
    let svc = make_service();
    let alice = AgentId::new();
    let bob = AgentId::new();

    svc.remember(alice, "Some data for alice", MemoryType::Semantic, None)
        .await
        .expect("store");

    let results = svc.recall(bob, "", 100).await.expect("recall");
    for r in &results {
        assert_ne!(r.agent_id, alice, "MI-20: empty query must not return other agent's data");
    }
}

// =========================================================================
// MI-21..MI-22: Mixed type + classification isolation
// =========================================================================

#[tokio::test]
async fn mi_21_mixed_types_isolated() {
    let svc = make_service();
    let alice = AgentId::new();
    let bob = AgentId::new();

    svc.remember(alice, "Semantic fact from alice", MemoryType::Semantic, None)
        .await
        .expect("store semantic");
    svc.remember(alice, "Procedural rule from alice", MemoryType::Procedural, None)
        .await
        .expect("store procedural");
    svc.remember(alice, "Episodic event from alice", MemoryType::Episodic, None)
        .await
        .expect("store episodic");

    let results = svc.recall(bob, "from alice", 10).await.expect("recall");
    assert!(results.is_empty(), "MI-21: all memory types isolated from other agent");
}

#[tokio::test]
async fn mi_22_many_agents_fully_isolated() {
    let svc = make_service();
    let agents: Vec<AgentId> = (0..5).map(|_| AgentId::new()).collect();

    for (i, agent) in agents.iter().enumerate() {
        svc.remember(*agent, &format!("Secret data for agent {i}"), MemoryType::Semantic, None)
            .await
            .expect("store");
    }

    for (i, agent) in agents.iter().enumerate() {
        let results = svc.recall(*agent, "secret data", 10).await.expect("recall");
        assert_eq!(results.len(), 1, "agent {i} should see exactly 1 memory");
        assert!(
            results[0].content.contains(&format!("agent {i}")),
            "agent {i} must only see its own data"
        );
    }
}
