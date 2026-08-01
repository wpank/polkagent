use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::RwLock;
use serde_json::Value;

use crate::error::CacheResult;
use crate::key::CacheKey;
use crate::lru::LruCache;
use crate::stats::CacheStats;
use crate::store::{CacheStore, CachedValue};

/// Internal value stored in the LRU slab.
#[derive(Clone, Debug)]
struct InternalEntry {
    data: Value,
    access_count: u64,
}

/// An in-memory [`CacheStore`] backed by an LRU cache.
///
/// Thread safety is achieved through a [`parking_lot::RwLock`].
pub struct InMemoryCache {
    inner: RwLock<LruCache<String, InternalEntry>>,
    stats: Arc<CacheStats>,
}

impl InMemoryCache {
    /// Create a new in-memory cache with the given capacity.
    pub fn new(max_entries: usize) -> Self {
        Self {
            inner: RwLock::new(LruCache::new(max_entries)),
            stats: Arc::new(CacheStats::new()),
        }
    }

    /// Create an in-memory cache with a default TTL.
    pub fn with_default_ttl(max_entries: usize, ttl: Duration) -> Self {
        Self {
            inner: RwLock::new(LruCache::new(max_entries).with_default_ttl(ttl)),
            stats: Arc::new(CacheStats::new()),
        }
    }

    /// Return a reference to the stats for this cache.
    pub fn stats(&self) -> &CacheStats {
        &self.stats
    }
}

#[async_trait]
impl CacheStore for InMemoryCache {
    async fn get(&self, key: &CacheKey) -> Option<CachedValue> {
        let canonical = key.canonical().to_owned();
        let mut inner = self.inner.write();
        match inner.get(&canonical) {
            Some(entry) => {
                self.stats.record_hit();
                let cached = CachedValue {
                    data: entry.data.clone(),
                    inserted_at: chrono::Utc::now(), // approximation; real insert time is in LRU
                    expires_at: None,
                    access_count: entry.access_count,
                };
                Some(cached)
            }
            None => {
                self.stats.record_miss();
                None
            }
        }
    }

    async fn set(
        &self,
        key: CacheKey,
        value: CachedValue,
        ttl: Option<Duration>,
    ) -> CacheResult<()> {
        let canonical = key.canonical().to_owned();
        let entry = InternalEntry {
            data: value.data,
            access_count: 0,
        };
        let mut inner = self.inner.write();
        inner.insert(canonical, entry, ttl);
        self.stats.record_insertion();
        Ok(())
    }

    async fn remove(&self, key: &CacheKey) -> CacheResult<bool> {
        let canonical = key.canonical().to_owned();
        let mut inner = self.inner.write();
        let existed = inner.remove(&canonical).is_some();
        if existed {
            self.stats.record_removal();
        }
        Ok(existed)
    }

    async fn clear(&self) -> CacheResult<()> {
        let mut inner = self.inner.write();
        inner.clear();
        Ok(())
    }

    async fn contains(&self, key: &CacheKey) -> bool {
        let canonical = key.canonical().to_owned();
        let mut inner = self.inner.write();
        inner.contains_key(&canonical)
    }

    async fn size(&self) -> usize {
        let inner = self.inner.read();
        inner.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::CacheKey;
    use serde_json::json;

    #[tokio::test]
    async fn set_and_get() {
        let cache = InMemoryCache::new(10);
        let key = CacheKey::new("test", "k1");
        let val = CachedValue::new(json!({"hello": "world"}));
        cache.set(key.clone(), val, None).await.expect("set failed");

        let result = cache.get(&key).await;
        assert!(result.is_some());
        assert_eq!(result.expect("missing").data, json!({"hello": "world"}));
    }

    #[tokio::test]
    async fn miss_returns_none() {
        let cache = InMemoryCache::new(10);
        let key = CacheKey::new("test", "missing");
        assert!(cache.get(&key).await.is_none());
    }

    #[tokio::test]
    async fn remove_returns_true_when_present() {
        let cache = InMemoryCache::new(10);
        let key = CacheKey::new("test", "k1");
        cache
            .set(key.clone(), CachedValue::new(json!(1)), None)
            .await
            .expect("set failed");
        assert!(cache.remove(&key).await.expect("remove failed"));
        assert!(!cache.remove(&key).await.expect("remove failed"));
    }

    #[tokio::test]
    async fn clear_removes_all() {
        let cache = InMemoryCache::new(10);
        for i in 0..5 {
            let key = CacheKey::new("test", format!("k{i}"));
            cache
                .set(key, CachedValue::new(json!(i)), None)
                .await
                .expect("set failed");
        }
        assert_eq!(cache.size().await, 5);
        cache.clear().await.expect("clear failed");
        assert_eq!(cache.size().await, 0);
    }

    #[tokio::test]
    async fn lru_eviction_in_memory() {
        let cache = InMemoryCache::new(2);
        let k1 = CacheKey::new("test", "a");
        let k2 = CacheKey::new("test", "b");
        let k3 = CacheKey::new("test", "c");

        cache.set(k1.clone(), CachedValue::new(json!(1)), None).await.expect("set");
        cache.set(k2.clone(), CachedValue::new(json!(2)), None).await.expect("set");
        cache.set(k3.clone(), CachedValue::new(json!(3)), None).await.expect("set");

        // "a" should have been evicted
        assert!(cache.get(&k1).await.is_none());
        assert!(cache.get(&k2).await.is_some());
        assert!(cache.get(&k3).await.is_some());
    }

    #[tokio::test]
    async fn stats_track_hits_and_misses() {
        let cache = InMemoryCache::new(10);
        let key = CacheKey::new("test", "k1");
        cache.set(key.clone(), CachedValue::new(json!(1)), None).await.expect("set");

        cache.get(&key).await; // hit
        cache.get(&key).await; // hit
        cache.get(&CacheKey::new("test", "nope")).await; // miss

        assert_eq!(cache.stats().hits(), 2);
        assert_eq!(cache.stats().misses(), 1);
        assert!((cache.stats().hit_rate() - 2.0 / 3.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn contains_checks_existence() {
        let cache = InMemoryCache::new(10);
        let key = CacheKey::new("test", "k1");
        assert!(!cache.contains(&key).await);
        cache.set(key.clone(), CachedValue::new(json!(1)), None).await.expect("set");
        assert!(cache.contains(&key).await);
    }

    #[tokio::test]
    async fn namespace_isolation() {
        let cache = InMemoryCache::new(10);
        let k1 = CacheKey::new("ns1", "id");
        let k2 = CacheKey::new("ns2", "id");

        cache.set(k1.clone(), CachedValue::new(json!("a")), None).await.expect("set");
        cache.set(k2.clone(), CachedValue::new(json!("b")), None).await.expect("set");

        let v1 = cache.get(&k1).await.expect("ns1 should exist");
        let v2 = cache.get(&k2).await.expect("ns2 should exist");
        assert_eq!(v1.data, json!("a"));
        assert_eq!(v2.data, json!("b"));
    }

    #[tokio::test]
    async fn ttl_expiry_in_memory() {
        let cache = InMemoryCache::new(10);
        let key = CacheKey::new("test", "expiring");
        cache
            .set(
                key.clone(),
                CachedValue::new(json!(1)),
                Some(Duration::from_millis(0)),
            )
            .await
            .expect("set");
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(cache.get(&key).await.is_none());
    }

    #[tokio::test]
    async fn concurrent_access() {
        let cache = Arc::new(InMemoryCache::new(1000));
        let mut handles = Vec::new();

        for i in 0..50 {
            let cache = Arc::clone(&cache);
            handles.push(tokio::spawn(async move {
                let key = CacheKey::new("concurrent", format!("k{i}"));
                cache
                    .set(key.clone(), CachedValue::new(json!(i)), None)
                    .await
                    .expect("set failed");
                let result = cache.get(&key).await;
                assert!(result.is_some(), "key {i} missing after insert");
            }));
        }

        for h in handles {
            h.await.expect("task panicked");
        }

        assert!(cache.size().await <= 1000);
    }
}
