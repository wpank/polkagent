//! Retention policy enforcement for the memory store.
//!
//! A [`RetentionPolicy`] defines the maximum age, minimum relevance, and
//! maximum number of entries allowed per agent. [`RetentionSweeper`] wraps
//! any [`MemoryStore`] and applies the policy when [`sweep`](RetentionSweeper::sweep)
//! is called.
//!
//! # Sweep strategy
//!
//! 1. Delete all entries older than `max_age_days` (`by_age`).
//! 2. Delete all remaining entries whose `relevance_score < min_relevance` (`by_relevance`).
//! 3. If the remaining count still exceeds `max_entries_per_agent`, delete the
//!    oldest entries until the limit is satisfied (`by_count_limit`).

use std::sync::Arc;

use polkagent_core::ids::AgentId;

use crate::error::MemoryResult;
use crate::store::MemoryStore;
use crate::types::MemoryQuery;

// ---------------------------------------------------------------------------
// RetentionPolicy
// ---------------------------------------------------------------------------

/// Configuration that governs which memory entries should be removed.
#[derive(Debug, Clone)]
pub struct RetentionPolicy {
    /// Maximum number of entries allowed for a single agent. When the count
    /// exceeds this value the oldest entries are deleted.
    pub max_entries_per_agent: usize,
    /// Maximum age of an entry in days. Entries older than this are deleted.
    pub max_age_days: u64,
    /// Minimum relevance score. Entries below this are deleted.
    pub min_relevance: f64,
    /// How often (in seconds) an automatic sweep should be requested.
    ///
    /// This field is informational — [`RetentionSweeper::sweep`] does not
    /// run on a timer internally; callers are responsible for scheduling.
    pub sweep_interval_secs: u64,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            max_entries_per_agent: 10_000,
            max_age_days: 90,
            min_relevance: 0.1,
            sweep_interval_secs: 3600,
        }
    }
}

// ---------------------------------------------------------------------------
// SweepResult
// ---------------------------------------------------------------------------

/// Statistics returned by a completed retention sweep.
#[derive(Debug, Clone, Default)]
pub struct SweepResult {
    /// Total number of entries deleted during the sweep.
    pub deleted_count: usize,
    /// Entries deleted because they exceeded `max_age_days`.
    pub by_age: usize,
    /// Entries deleted because their `relevance_score` was below `min_relevance`.
    pub by_relevance: usize,
    /// Entries deleted to bring the agent's count within `max_entries_per_agent`.
    pub by_count_limit: usize,
}

// ---------------------------------------------------------------------------
// RetentionSweeper
// ---------------------------------------------------------------------------

/// Applies a [`RetentionPolicy`] to all entries belonging to a specific agent.
pub struct RetentionSweeper {
    store: Arc<dyn MemoryStore>,
    policy: RetentionPolicy,
    agent_id: AgentId,
}

impl RetentionSweeper {
    /// Create a new sweeper for `agent_id` using the given store and policy.
    #[must_use]
    pub fn new(
        store: Arc<dyn MemoryStore>,
        policy: RetentionPolicy,
        agent_id: AgentId,
    ) -> Self {
        Self { store, policy, agent_id }
    }

    /// Execute a full retention sweep for the agent and return statistics.
    ///
    /// The sweep proceeds in three phases:
    ///
    /// 1. Delete entries older than `policy.max_age_days`.
    /// 2. Delete entries with `relevance_score < policy.min_relevance`.
    /// 3. Delete oldest entries if count > `policy.max_entries_per_agent`.
    pub async fn sweep(&self) -> MemoryResult<SweepResult> {
        let mut result = SweepResult::default();

        // --- Phase 1: delete by age ---
        #[allow(clippy::cast_possible_wrap)]
        let max_age = chrono::Duration::days(self.policy.max_age_days as i64);
        let by_age = self.store.delete_by_age(&self.agent_id, max_age).await?;
        result.by_age = by_age;
        result.deleted_count += by_age;

        // --- Phase 2: delete by low relevance ---
        // Fetch all entries for this agent with relevance below the threshold.
        // We use the search API with min_relevance intentionally set to 0.0 so
        // that we get everything, then filter ourselves to identify low-relevance
        // entries (the store's search API only supports a *minimum* filter, not a
        // *maximum*, so we invert the logic here).
        let all_query = MemoryQuery {
            agent_id: Some(self.agent_id),
            query_text: String::new(),
            memory_types: None,
            limit: usize::MAX / 2, // effectively unlimited
            min_relevance: None,
            since: None,
            episode_id: None,
        };
        let all_entries = self.store.search(&all_query).await?;

        let low_relevance_ids: Vec<_> = all_entries
            .iter()
            .filter(|e| e.relevance_score < self.policy.min_relevance)
            .map(|e| e.id)
            .collect();

        let by_relevance = low_relevance_ids.len();
        for id in low_relevance_ids {
            self.store.delete_memory(id).await?;
        }
        result.by_relevance = by_relevance;
        result.deleted_count += by_relevance;

        // --- Phase 3: enforce count limit ---
        // Re-fetch the remaining entries, ordered oldest-first, and delete
        // until the count is within the limit.
        let remaining = self.store.search(&all_query).await?;
        let count = remaining.len();

        if count > self.policy.max_entries_per_agent {
            let overflow = count - self.policy.max_entries_per_agent;
            // Entries are returned ordered by relevance DESC by default.
            // Sort ascending by created_at to delete the oldest first.
            let mut by_age_order = remaining;
            by_age_order.sort_by_key(|e| e.created_at);

            let to_delete = &by_age_order[..overflow];
            let by_count = to_delete.len();

            for entry in to_delete {
                self.store.delete_memory(entry.id).await?;
            }

            result.by_count_limit = by_count;
            result.deleted_count += by_count;
        }

        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use polkagent_core::ids::AgentId;

    use crate::classification::Classification;
    use crate::sqlite::SqliteMemoryStore;
    use crate::types::{MemoryEntry, MemoryId, MemoryType};

    fn make_store() -> Arc<dyn MemoryStore> {
        Arc::new(SqliteMemoryStore::open_in_memory().unwrap())
    }

    fn make_entry(agent_id: AgentId, content: &str, relevance: f64) -> MemoryEntry {
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
            classification: Classification::default(),
        }
    }

    fn make_old_entry(agent_id: AgentId, days_ago: i64) -> MemoryEntry {
        let created_at = Utc::now() - chrono::Duration::days(days_ago);
        MemoryEntry {
            id: MemoryId::new(),
            agent_id,
            episode_id: None,
            memory_type: MemoryType::Semantic,
            content: format!("old entry {days_ago} days ago"),
            embedding: None,
            metadata: serde_json::json!({}),
            provenance: None,
            created_at,
            accessed_at: Utc::now(),
            access_count: 0,
            relevance_score: 0.9,
            confidence: 1.0,
            classification: Classification::default(),
        }
    }

    #[tokio::test]
    async fn sweep_deletes_expired_entries() {
        let store = make_store();
        let agent = AgentId::new();

        // 2 old entries (5 days old, policy max_age_days = 2) + 1 recent entry.
        store.store_memory(&make_old_entry(agent, 5)).await.unwrap();
        store.store_memory(&make_old_entry(agent, 5)).await.unwrap();
        store.store_memory(&make_entry(agent, "recent", 0.9)).await.unwrap();

        let policy = RetentionPolicy {
            max_entries_per_agent: 1000,
            max_age_days: 2,
            min_relevance: 0.0,
            sweep_interval_secs: 3600,
        };
        let sweeper = RetentionSweeper::new(Arc::clone(&store), policy, agent);
        let result = sweeper.sweep().await.unwrap();

        assert_eq!(result.by_age, 2);
        assert_eq!(result.by_relevance, 0);
        assert_eq!(result.by_count_limit, 0);
        assert_eq!(result.deleted_count, 2);
        assert_eq!(store.count_entries(&agent).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn sweep_deletes_low_relevance_entries() {
        let store = make_store();
        let agent = AgentId::new();

        store.store_memory(&make_entry(agent, "high relevance A", 0.9)).await.unwrap();
        store.store_memory(&make_entry(agent, "low relevance B", 0.05)).await.unwrap();
        store.store_memory(&make_entry(agent, "low relevance C", 0.04)).await.unwrap();

        let policy = RetentionPolicy {
            max_entries_per_agent: 1000,
            max_age_days: 365,
            min_relevance: 0.1,
            sweep_interval_secs: 3600,
        };
        let sweeper = RetentionSweeper::new(Arc::clone(&store), policy, agent);
        let result = sweeper.sweep().await.unwrap();

        assert_eq!(result.by_relevance, 2);
        assert_eq!(result.by_age, 0);
        assert_eq!(result.by_count_limit, 0);
        assert_eq!(result.deleted_count, 2);
        assert_eq!(store.count_entries(&agent).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn sweep_enforces_count_limit() {
        let store = make_store();
        let agent = AgentId::new();

        for i in 0..10 {
            store
                .store_memory(&make_entry(agent, &format!("memory {i}"), 0.9))
                .await
                .unwrap();
        }

        let policy = RetentionPolicy {
            max_entries_per_agent: 5,
            max_age_days: 365,
            min_relevance: 0.0,
            sweep_interval_secs: 3600,
        };
        let sweeper = RetentionSweeper::new(Arc::clone(&store), policy, agent);
        let result = sweeper.sweep().await.unwrap();

        assert_eq!(result.by_count_limit, 5);
        assert_eq!(result.deleted_count, 5);
        assert_eq!(store.count_entries(&agent).await.unwrap(), 5);
    }

    #[tokio::test]
    async fn sweep_result_counts_are_correct() {
        let store = make_store();
        let agent = AgentId::new();

        // 1 old, 1 low-relevance, 5 normal — then cap at 3.
        store.store_memory(&make_old_entry(agent, 10)).await.unwrap();
        store.store_memory(&make_entry(agent, "low rel", 0.01)).await.unwrap();
        for i in 0..5 {
            store
                .store_memory(&make_entry(agent, &format!("normal {i}"), 0.9))
                .await
                .unwrap();
        }

        let policy = RetentionPolicy {
            max_entries_per_agent: 3,
            max_age_days: 5,
            min_relevance: 0.05,
            sweep_interval_secs: 3600,
        };
        let sweeper = RetentionSweeper::new(Arc::clone(&store), policy, agent);
        let result = sweeper.sweep().await.unwrap();

        assert_eq!(result.by_age, 1);
        assert_eq!(result.by_relevance, 1);
        assert_eq!(result.by_count_limit, 2); // 5 remain after age+relevance, cap to 3 → delete 2
        assert_eq!(result.deleted_count, 4);
        assert_eq!(store.count_entries(&agent).await.unwrap(), 3);
    }

    #[tokio::test]
    async fn sweep_no_deletions_when_within_policy() {
        let store = make_store();
        let agent = AgentId::new();

        store.store_memory(&make_entry(agent, "alpha", 0.8)).await.unwrap();
        store.store_memory(&make_entry(agent, "beta", 0.7)).await.unwrap();

        let policy = RetentionPolicy {
            max_entries_per_agent: 100,
            max_age_days: 365,
            min_relevance: 0.0,
            sweep_interval_secs: 3600,
        };
        let sweeper = RetentionSweeper::new(Arc::clone(&store), policy, agent);
        let result = sweeper.sweep().await.unwrap();

        assert_eq!(result.deleted_count, 0);
        assert_eq!(store.count_entries(&agent).await.unwrap(), 2);
    }
}
