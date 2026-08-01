//! In-memory audit store.
//!
//! [`InMemoryAuditStore`] keeps all entries in a `Vec<AuditEntry>` protected
//! by a `parking_lot::RwLock`. It is intended for testing, development, and
//! short-lived processes where durability is not required.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::RwLock;

use crate::action::AuditAction;
use crate::entry::AuditEntry;
use crate::error::AuditResult;
use crate::integrity;
use crate::query::AuditQuery;
use crate::store::AuditStore;

/// An in-memory, append-only audit store backed by a `Vec`.
///
/// Thread-safe via `parking_lot::RwLock`. Each append computes the integrity
/// hash using the chain from the previous entry.
#[derive(Debug, Default)]
pub struct InMemoryAuditStore {
    entries: RwLock<Vec<AuditEntry>>,
}

impl InMemoryAuditStore {
    /// Create a new empty in-memory store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a snapshot of all entries (useful for testing).
    #[must_use]
    pub fn snapshot(&self) -> Vec<AuditEntry> {
        self.entries.read().clone()
    }

    /// Verify the integrity of the entire chain.
    pub fn verify_integrity(&self) -> AuditResult<()> {
        let entries = self.entries.read();
        integrity::verify_chain(&entries)
    }
}

#[async_trait]
impl AuditStore for InMemoryAuditStore {
    async fn append(&self, mut entry: AuditEntry) -> AuditResult<AuditEntry> {
        let mut entries = self.entries.write();

        let prev_hash = entries.last().map_or_else(
            || integrity::GENESIS_HASH.to_string(),
            |e| e.integrity_hash.clone(),
        );

        entry.integrity_hash = integrity::compute_hash(&prev_hash, &entry)?;
        entries.push(entry.clone());
        Ok(entry)
    }

    async fn query_by_actor(&self, actor_id: &str) -> AuditResult<Vec<AuditEntry>> {
        let entries = self.entries.read();
        Ok(entries
            .iter()
            .filter(|e| e.actor.id == actor_id)
            .cloned()
            .collect())
    }

    async fn query_by_action(&self, action: &AuditAction) -> AuditResult<Vec<AuditEntry>> {
        let entries = self.entries.read();
        Ok(entries
            .iter()
            .filter(|e| e.action == *action)
            .cloned()
            .collect())
    }

    async fn query_by_time_range(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> AuditResult<Vec<AuditEntry>> {
        let entries = self.entries.read();
        Ok(entries
            .iter()
            .filter(|e| e.timestamp >= from && e.timestamp <= to)
            .cloned()
            .collect())
    }

    async fn query(&self, query: &AuditQuery) -> AuditResult<Vec<AuditEntry>> {
        let entries = self.entries.read();
        Ok(entries
            .iter()
            .filter(|e| query.matches(e))
            .take(query.limit)
            .cloned()
            .collect())
    }

    async fn count(&self) -> AuditResult<usize> {
        Ok(self.entries.read().len())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::AuditAction;
    use crate::actor::ActorInfo;
    use crate::entry::{ActionOutcome, AuditEntry, ResourceInfo};

    fn make_entry(actor_id: &str, action: AuditAction) -> AuditEntry {
        AuditEntry::new(
            ActorInfo::agent(actor_id),
            action,
            ResourceInfo::new("test", "r-1"),
            ActionOutcome::Success,
            serde_json::Value::Null,
        )
    }

    #[tokio::test]
    async fn append_sets_integrity_hash() {
        let store = InMemoryAuditStore::new();
        let entry = make_entry("a-1", AuditAction::RunStarted);
        let stored = store.append(entry).await.expect("append");
        assert!(!stored.integrity_hash.is_empty());
        assert_eq!(stored.integrity_hash.len(), 64);
    }

    #[tokio::test]
    async fn append_chains_hashes() {
        let store = InMemoryAuditStore::new();
        let e1 = store
            .append(make_entry("a-1", AuditAction::RunStarted))
            .await
            .expect("append");
        let e2 = store
            .append(make_entry("a-1", AuditAction::ToolInvoked))
            .await
            .expect("append");

        // The hashes should differ because the entries differ.
        assert_ne!(e1.integrity_hash, e2.integrity_hash);

        // The chain should verify.
        store.verify_integrity().expect("integrity");
    }

    #[tokio::test]
    async fn query_by_actor_filters_correctly() {
        let store = InMemoryAuditStore::new();
        store
            .append(make_entry("a-1", AuditAction::RunStarted))
            .await
            .expect("append");
        store
            .append(make_entry("a-2", AuditAction::RunStarted))
            .await
            .expect("append");
        store
            .append(make_entry("a-1", AuditAction::ToolInvoked))
            .await
            .expect("append");

        let results = store.query_by_actor("a-1").await.expect("query");
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.actor.id == "a-1"));
    }

    #[tokio::test]
    async fn query_by_action_filters_correctly() {
        let store = InMemoryAuditStore::new();
        store
            .append(make_entry("a-1", AuditAction::RunStarted))
            .await
            .expect("append");
        store
            .append(make_entry("a-1", AuditAction::ToolInvoked))
            .await
            .expect("append");
        store
            .append(make_entry("a-2", AuditAction::ToolInvoked))
            .await
            .expect("append");

        let results = store
            .query_by_action(&AuditAction::ToolInvoked)
            .await
            .expect("query");
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.action == AuditAction::ToolInvoked));
    }

    #[tokio::test]
    async fn query_by_time_range() {
        let store = InMemoryAuditStore::new();
        let before = Utc::now();
        store
            .append(make_entry("a-1", AuditAction::RunStarted))
            .await
            .expect("append");
        let after = Utc::now();

        let results = store
            .query_by_time_range(before, after)
            .await
            .expect("query");
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn count_returns_correct_count() {
        let store = InMemoryAuditStore::new();
        assert_eq!(store.count().await.expect("count"), 0);

        store
            .append(make_entry("a-1", AuditAction::RunStarted))
            .await
            .expect("append");
        store
            .append(make_entry("a-2", AuditAction::RunCompleted))
            .await
            .expect("append");

        assert_eq!(store.count().await.expect("count"), 2);
    }

    #[tokio::test]
    async fn query_with_limit() {
        let store = InMemoryAuditStore::new();
        for i in 0..10 {
            store
                .append(make_entry(&format!("a-{i}"), AuditAction::RunStarted))
                .await
                .expect("append");
        }

        let q = AuditQuery::new().limit(3).build();
        let results = store.query(&q).await.expect("query");
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn query_combined_filters() {
        let store = InMemoryAuditStore::new();
        store
            .append(make_entry("a-1", AuditAction::RunStarted))
            .await
            .expect("append");
        store
            .append(make_entry("a-1", AuditAction::ToolInvoked))
            .await
            .expect("append");
        store
            .append(make_entry("a-2", AuditAction::ToolInvoked))
            .await
            .expect("append");

        let q = AuditQuery::new()
            .actor("a-1")
            .action(AuditAction::ToolInvoked)
            .build();
        let results = store.query(&q).await.expect("query");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].actor.id, "a-1");
        assert_eq!(results[0].action, AuditAction::ToolInvoked);
    }

    #[tokio::test]
    async fn snapshot_returns_all_entries() {
        let store = InMemoryAuditStore::new();
        store
            .append(make_entry("a-1", AuditAction::RunStarted))
            .await
            .expect("append");
        store
            .append(make_entry("a-2", AuditAction::RunCompleted))
            .await
            .expect("append");

        let snapshot = store.snapshot();
        assert_eq!(snapshot.len(), 2);
    }
}
