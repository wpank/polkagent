//! In-memory [`DeliveryStore`] implementation for use in tests and development.
//!
//! All data is held in a `parking_lot::RwLock<HashMap<…>>` so the store can be
//! shared across async tasks without `async Mutex` overhead. There is no
//! persistence: all state is lost when the store is dropped.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::Utc;
use parking_lot::RwLock;
use uuid::Uuid;

use crate::error::{Result, WebhookError};
use crate::store::{DeliveryRecord, DeliveryStatus, DeliveryStore};

// ---------------------------------------------------------------------------
// InMemoryDeliveryStore
// ---------------------------------------------------------------------------

/// An in-memory [`DeliveryStore`] suitable for unit tests and prototyping.
#[derive(Debug, Default)]
pub struct InMemoryDeliveryStore {
    records: RwLock<HashMap<Uuid, DeliveryRecord>>,
}

impl InMemoryDeliveryStore {
    /// Create a new, empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the total number of delivery records stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.read().len()
    }

    /// Return `true` if the store contains no records.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.read().is_empty()
    }
}

#[async_trait]
impl DeliveryStore for InMemoryDeliveryStore {
    async fn create(&self, record: DeliveryRecord) -> Result<DeliveryRecord> {
        let mut map = self.records.write();
        if map.contains_key(&record.id) {
            return Err(WebhookError::Duplicate(record.id));
        }
        map.insert(record.id, record.clone());
        Ok(record)
    }

    async fn get(&self, id: Uuid) -> Result<DeliveryRecord> {
        let map = self.records.read();
        map.get(&id)
            .cloned()
            .ok_or(WebhookError::DeliveryNotFound(id))
    }

    async fn update(&self, record: DeliveryRecord) -> Result<DeliveryRecord> {
        let mut map = self.records.write();
        if !map.contains_key(&record.id) {
            return Err(WebhookError::DeliveryNotFound(record.id));
        }
        map.insert(record.id, record.clone());
        Ok(record)
    }

    async fn list_by_webhook(&self, webhook_id: Uuid, limit: usize) -> Result<Vec<DeliveryRecord>> {
        let map = self.records.read();
        let mut records: Vec<DeliveryRecord> = map
            .values()
            .filter(|r| r.webhook_id == webhook_id)
            .cloned()
            .collect();
        // Sort by created_at descending (most recent first).
        records.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        records.truncate(limit);
        Ok(records)
    }

    async fn list_pending_retries(&self, limit: usize) -> Result<Vec<DeliveryRecord>> {
        let now = Utc::now();
        let map = self.records.read();
        let mut records: Vec<DeliveryRecord> = map
            .values()
            .filter(|r| {
                r.status == DeliveryStatus::Failed && r.next_retry_at.is_some_and(|t| t <= now)
            })
            .cloned()
            .collect();
        records.sort_by_key(|r| r.next_retry_at);
        records.truncate(limit);
        Ok(records)
    }

    async fn count_by_status(&self, webhook_id: Uuid, status: DeliveryStatus) -> Result<usize> {
        let map = self.records.read();
        let count = map
            .values()
            .filter(|r| r.webhook_id == webhook_id && r.status == status)
            .count();
        Ok(count)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(webhook_id: Uuid, event_type: &str) -> DeliveryRecord {
        DeliveryRecord::new(webhook_id, event_type)
    }

    #[tokio::test]
    async fn create_and_get() {
        let store = InMemoryDeliveryStore::new();
        let wid = Uuid::now_v7();
        let record = make_record(wid, "run.started");
        let rid = record.id;

        store.create(record).await.expect("create");
        let fetched = store.get(rid).await.expect("get");
        assert_eq!(fetched.id, rid);
        assert_eq!(fetched.event_type, "run.started");
    }

    #[tokio::test]
    async fn create_duplicate_fails() {
        let store = InMemoryDeliveryStore::new();
        let record = make_record(Uuid::nil(), "run.started");
        store.create(record.clone()).await.expect("first create");
        let result = store.create(record).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_nonexistent_fails() {
        let store = InMemoryDeliveryStore::new();
        let result = store.get(Uuid::nil()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn update_existing() {
        let store = InMemoryDeliveryStore::new();
        let mut record = make_record(Uuid::nil(), "run.started");
        store.create(record.clone()).await.expect("create");

        record.mark_delivered(200);
        store.update(record.clone()).await.expect("update");

        let fetched = store.get(record.id).await.expect("get");
        assert_eq!(fetched.status, DeliveryStatus::Delivered);
    }

    #[tokio::test]
    async fn update_nonexistent_fails() {
        let store = InMemoryDeliveryStore::new();
        let record = make_record(Uuid::nil(), "run.started");
        let result = store.update(record).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn list_by_webhook_filters_correctly() {
        let store = InMemoryDeliveryStore::new();
        let wid_a = Uuid::now_v7();
        let wid_b = Uuid::now_v7();

        store
            .create(make_record(wid_a, "run.started"))
            .await
            .expect("a1");
        store
            .create(make_record(wid_a, "run.completed"))
            .await
            .expect("a2");
        store
            .create(make_record(wid_b, "run.started"))
            .await
            .expect("b1");

        let list = store.list_by_webhook(wid_a, 100).await.expect("list");
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|r| r.webhook_id == wid_a));
    }

    #[tokio::test]
    async fn list_by_webhook_respects_limit() {
        let store = InMemoryDeliveryStore::new();
        let wid = Uuid::now_v7();

        for _ in 0..5 {
            store
                .create(make_record(wid, "run.started"))
                .await
                .expect("create");
        }

        let list = store.list_by_webhook(wid, 2).await.expect("list");
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn count_by_status() {
        let store = InMemoryDeliveryStore::new();
        let wid = Uuid::now_v7();

        store
            .create(make_record(wid, "run.started"))
            .await
            .expect("c1");
        let mut r2 = make_record(wid, "run.completed");
        store.create(r2.clone()).await.expect("c2");
        r2.mark_delivered(200);
        store.update(r2).await.expect("update");

        let pending = store
            .count_by_status(wid, DeliveryStatus::Pending)
            .await
            .expect("count pending");
        let delivered = store
            .count_by_status(wid, DeliveryStatus::Delivered)
            .await
            .expect("count delivered");
        // The first record stays pending but the test creates a new record ID each time.
        // We just verify both statuses are tracked.
        assert_eq!(pending, 1);
        assert_eq!(delivered, 1);
    }

    #[tokio::test]
    async fn len_and_is_empty() {
        let store = InMemoryDeliveryStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);

        store
            .create(make_record(Uuid::nil(), "run.started"))
            .await
            .expect("create");
        assert!(!store.is_empty());
        assert_eq!(store.len(), 1);
    }
}
