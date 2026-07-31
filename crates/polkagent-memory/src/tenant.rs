//! Tenant-scoped isolation for the memory store.
//!
//! [`TenantAwareStore`] wraps any [`MemoryStore`] and automatically scopes all
//! queries to a specific `agent_id`. Queries for a different agent return empty
//! results rather than errors, preserving the principle that tenants should not
//! be aware of each other's existence.
//!
//! # Example
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use polkagent_core::ids::AgentId;
//! use polkagent_memory::sqlite::SqliteMemoryStore;
//! use polkagent_memory::tenant::{TenantAwareStore, TenantScope};
//!
//! # fn example() {
//! let store = Arc::new(SqliteMemoryStore::open_in_memory().unwrap());
//! let agent = AgentId::new();
//! let scope = TenantScope::new(agent, vec!["project-x".to_string()]);
//! let tenant_store = TenantAwareStore::with_scope(store, scope);
//! # }
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use polkagent_core::ids::AgentId;

use crate::classification::Classification;
use crate::error::MemoryResult;
use crate::store::MemoryStore;
use crate::types::{Episode, EpisodeId, MemoryEntry, MemoryId, MemoryQuery};

// ---------------------------------------------------------------------------
// TenantScope
// ---------------------------------------------------------------------------

/// Identifies a tenant and their permitted logical scopes (namespaces).
#[derive(Debug, Clone)]
pub struct TenantScope {
    /// The agent identifier that owns this scope.
    pub agent_id: AgentId,
    /// Optional logical namespaces the tenant is authorised to access.
    ///
    /// This field is reserved for future fine-grained namespace filtering;
    /// the current implementation only enforces `agent_id` isolation.
    pub allowed_scopes: Vec<String>,
}

impl TenantScope {
    /// Create a new scope for the given agent.
    #[must_use]
    pub fn new(agent_id: AgentId, allowed_scopes: Vec<String>) -> Self {
        Self { agent_id, allowed_scopes }
    }
}

// ---------------------------------------------------------------------------
// TenantAwareStore
// ---------------------------------------------------------------------------

/// A [`MemoryStore`] wrapper that enforces tenant isolation.
///
/// All queries are automatically scoped to `scope.agent_id`. Attempts to
/// access entries belonging to a different agent return empty results (not
/// errors), so tenants cannot distinguish "no entries" from "entries exist
/// but are forbidden".
pub struct TenantAwareStore {
    inner: Arc<dyn MemoryStore>,
    scope: TenantScope,
}

impl TenantAwareStore {
    /// Wrap a store with a tenant scope.
    #[must_use]
    pub fn with_scope(store: Arc<dyn MemoryStore>, scope: TenantScope) -> Self {
        Self { inner: store, scope }
    }

    /// Return the tenant's `AgentId`.
    #[must_use]
    pub fn agent_id(&self) -> AgentId {
        self.scope.agent_id
    }

    /// Return a reference to the tenant scope.
    #[must_use]
    pub fn scope(&self) -> &TenantScope {
        &self.scope
    }

    /// Return a reference to the underlying store (for admin / system use).
    #[must_use]
    pub fn inner(&self) -> &Arc<dyn MemoryStore> {
        &self.inner
    }
}

#[async_trait]
impl MemoryStore for TenantAwareStore {
    async fn store_memory(&self, entry: &MemoryEntry) -> MemoryResult<MemoryId> {
        // Always store under the tenant's own agent_id (override caller-supplied).
        let mut owned = entry.clone();
        owned.agent_id = self.scope.agent_id;
        self.inner.store_memory(&owned).await
    }

    async fn get_memory(&self, id: MemoryId) -> MemoryResult<MemoryEntry> {
        let entry = self.inner.get_memory(id).await?;
        // Return entry only if it belongs to this tenant.
        if entry.agent_id != self.scope.agent_id {
            return Err(crate::error::MemoryError::NotFound(format!("memory {id}")));
        }
        Ok(entry)
    }

    async fn search(&self, query: &MemoryQuery) -> MemoryResult<Vec<MemoryEntry>> {
        // A cross-tenant query returns empty results, not an error.
        if query.agent_id != self.scope.agent_id {
            return Ok(Vec::new());
        }
        self.inner.search(query).await
    }

    async fn search_with_classification(
        &self,
        query: &MemoryQuery,
        max_classification: Classification,
    ) -> MemoryResult<Vec<MemoryEntry>> {
        if query.agent_id != self.scope.agent_id {
            return Ok(Vec::new());
        }
        self.inner.search_with_classification(query, max_classification).await
    }

    async fn update_relevance(&self, id: MemoryId, score: f64) -> MemoryResult<()> {
        // Verify ownership before updating.
        let entry = self.inner.get_memory(id).await?;
        if entry.agent_id != self.scope.agent_id {
            return Err(crate::error::MemoryError::NotFound(format!("memory {id}")));
        }
        self.inner.update_relevance(id, score).await
    }

    async fn delete_memory(&self, id: MemoryId) -> MemoryResult<()> {
        // Verify ownership before deleting.
        let entry = self.inner.get_memory(id).await?;
        if entry.agent_id != self.scope.agent_id {
            return Err(crate::error::MemoryError::NotFound(format!("memory {id}")));
        }
        self.inner.delete_memory(id).await
    }

    async fn count_entries(&self, agent_id: &AgentId) -> MemoryResult<usize> {
        // Only count this tenant's own entries; cross-tenant count = 0.
        if *agent_id != self.scope.agent_id {
            return Ok(0);
        }
        self.inner.count_entries(agent_id).await
    }

    async fn delete_by_age(
        &self,
        agent_id: &AgentId,
        max_age: chrono::Duration,
    ) -> MemoryResult<usize> {
        if *agent_id != self.scope.agent_id {
            return Ok(0);
        }
        self.inner.delete_by_age(agent_id, max_age).await
    }

    async fn create_episode(&self, episode: &Episode) -> MemoryResult<EpisodeId> {
        let mut owned = episode.clone();
        owned.agent_id = self.scope.agent_id;
        self.inner.create_episode(&owned).await
    }

    async fn get_episode(&self, id: EpisodeId) -> MemoryResult<Episode> {
        let ep = self.inner.get_episode(id).await?;
        if ep.agent_id != self.scope.agent_id {
            return Err(crate::error::MemoryError::NotFound(format!("episode {id}")));
        }
        Ok(ep)
    }

    async fn end_episode(&self, id: EpisodeId, summary: &str) -> MemoryResult<()> {
        let ep = self.inner.get_episode(id).await?;
        if ep.agent_id != self.scope.agent_id {
            return Err(crate::error::MemoryError::NotFound(format!("episode {id}")));
        }
        self.inner.end_episode(id, summary).await
    }

    async fn list_episodes(&self, agent_id: AgentId, limit: usize) -> MemoryResult<Vec<Episode>> {
        if agent_id != self.scope.agent_id {
            return Ok(Vec::new());
        }
        self.inner.list_episodes(agent_id, limit).await
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

    fn make_query(agent_id: AgentId) -> MemoryQuery {
        MemoryQuery {
            agent_id,
            query_text: String::new(),
            memory_types: None,
            limit: 100,
            min_relevance: None,
            since: None,
            episode_id: None,
        }
    }

    #[tokio::test]
    async fn scoped_query_returns_only_own_entries() {
        let store = make_store();
        let agent_a = AgentId::new();
        let agent_b = AgentId::new();

        // Store entries for both agents directly in the backing store.
        store.store_memory(&make_entry(agent_a, "entry for A")).await.unwrap();
        store.store_memory(&make_entry(agent_b, "entry for B")).await.unwrap();

        // Create a scoped store for agent_a.
        let scope = TenantScope::new(agent_a, vec![]);
        let tenant_store = TenantAwareStore::with_scope(Arc::clone(&store), scope);

        let results = tenant_store.search(&make_query(agent_a)).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "entry for A");
    }

    #[tokio::test]
    async fn cross_tenant_query_returns_empty() {
        let store = make_store();
        let agent_a = AgentId::new();
        let agent_b = AgentId::new();

        store.store_memory(&make_entry(agent_b, "entry for B")).await.unwrap();

        // Scoped to agent_a but querying agent_b's id.
        let scope = TenantScope::new(agent_a, vec![]);
        let tenant_store = TenantAwareStore::with_scope(Arc::clone(&store), scope);

        let results = tenant_store.search(&make_query(agent_b)).await.unwrap();
        assert!(results.is_empty(), "cross-tenant query must return empty results");
    }

    #[tokio::test]
    async fn tenant_store_cannot_get_other_tenants_memory() {
        let store = make_store();
        let agent_a = AgentId::new();
        let agent_b = AgentId::new();

        let entry_b = make_entry(agent_b, "secret of B");
        let id_b = entry_b.id;
        store.store_memory(&entry_b).await.unwrap();

        let scope = TenantScope::new(agent_a, vec![]);
        let tenant_store = TenantAwareStore::with_scope(Arc::clone(&store), scope);

        let result = tenant_store.get_memory(id_b).await;
        assert!(
            matches!(result, Err(crate::error::MemoryError::NotFound(_))),
            "tenant A should not be able to read tenant B's memories"
        );
    }

    #[tokio::test]
    async fn store_memory_is_always_under_tenant_agent_id() {
        let store = make_store();
        let agent_a = AgentId::new();
        let agent_b = AgentId::new();

        let scope = TenantScope::new(agent_a, vec![]);
        let tenant_store = TenantAwareStore::with_scope(Arc::clone(&store), scope);

        // Try to store an entry attributed to agent_b through the scoped store.
        let mut entry = make_entry(agent_b, "impersonation attempt");
        let id = entry.id;
        entry.agent_id = agent_b; // deliberately mismatched
        tenant_store.store_memory(&entry).await.unwrap();

        // The backing store should have recorded it under agent_a.
        let retrieved = store.get_memory(id).await.unwrap();
        assert_eq!(retrieved.agent_id, agent_a);
    }

    #[tokio::test]
    async fn count_entries_cross_tenant_returns_zero() {
        let store = make_store();
        let agent_a = AgentId::new();
        let agent_b = AgentId::new();

        store.store_memory(&make_entry(agent_b, "B's entry")).await.unwrap();

        let scope = TenantScope::new(agent_a, vec![]);
        let tenant_store = TenantAwareStore::with_scope(Arc::clone(&store), scope);

        let count = tenant_store.count_entries(&agent_b).await.unwrap();
        assert_eq!(count, 0);
    }
}
