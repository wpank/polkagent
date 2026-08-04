//! Cached wrapper around [`MetadataService`].
//!
//! [`CachedMetadataService`] adds an LRU cache (backed by
//! [`polkagent_cache::InMemoryCache`]) in front of the existing
//! [`MetadataService`]. Metadata snapshots are cached by a composite
//! key of `(network, spec_version)` with a configurable TTL (default
//! 5 minutes).
//!
//! On a cache hit the snapshot is returned directly without touching the
//! inner service. On a miss the inner service is consulted, and the
//! result (if any) is written into the cache for subsequent lookups.
//!
//! # Feature gate
//!
//! This module is only available when the `cached` feature is enabled:
//!
//! ```toml
//! polkagent-metadata = { path = "...", features = ["cached"] }
//! ```

use std::sync::Arc;
use std::time::Duration;

use polkagent_cache::{CacheKey, CacheStats, CacheStore, CachedValue, InMemoryCache};
use serde_json::json;
use tracing::{debug, trace};

use crate::service::MetadataService;
use crate::types::{ChainId, MetadataDrift, MetadataSnapshot};

/// Default TTL for cached metadata entries (5 minutes).
const DEFAULT_TTL: Duration = Duration::from_secs(5 * 60);

/// Default maximum number of entries in the metadata cache.
const DEFAULT_MAX_ENTRIES: usize = 64;

/// Cache namespace used for all metadata snapshot keys.
const CACHE_NAMESPACE: &str = "metadata_snapshot";

/// A caching wrapper around [`MetadataService`].
///
/// Intercepts `get_snapshot` calls and serves results from an
/// [`InMemoryCache`] when possible. Registration, pinning, drift
/// detection, and staleness checks are delegated to the inner service
/// unchanged.
///
/// Thread-safe and cheaply cloneable.
#[derive(Clone)]
pub struct CachedMetadataService {
    inner: MetadataService,
    cache: Arc<InMemoryCache>,
    ttl: Duration,
}

impl CachedMetadataService {
    /// Create a new `CachedMetadataService` with default capacity (64)
    /// and default TTL (5 minutes).
    #[must_use]
    pub fn new(inner: MetadataService) -> Self {
        Self::with_config(inner, DEFAULT_MAX_ENTRIES, DEFAULT_TTL)
    }

    /// Create a new `CachedMetadataService` with custom capacity and TTL.
    #[must_use]
    pub fn with_config(inner: MetadataService, max_entries: usize, ttl: Duration) -> Self {
        Self {
            inner,
            cache: Arc::new(InMemoryCache::with_default_ttl(max_entries, ttl)),
            ttl,
        }
    }

    /// Register a metadata snapshot.
    ///
    /// The snapshot is registered with the inner service and also
    /// written into the cache so subsequent `get_snapshot` calls can
    /// be served from cache.
    pub async fn register_snapshot(&self, snapshot: MetadataSnapshot) -> Option<MetadataDrift> {
        let key = cache_key(&snapshot.chain_id, snapshot.spec_version);

        // Serialize the snapshot into the cache.
        let value = snapshot_to_cached_value(&snapshot);
        if let Err(e) = self.cache.set(key, value, Some(self.ttl)).await {
            debug!(error = %e, "failed to write snapshot to cache; proceeding without caching");
        }

        // Delegate to the inner service for drift detection, pinning, etc.
        self.inner.register_snapshot(snapshot)
    }

    /// Get a metadata snapshot for a chain, checking the cache first.
    ///
    /// The cache is keyed by `(chain_id, spec_version)`. On a miss
    /// the inner service is consulted and the result is cached.
    pub async fn get_snapshot(
        &self,
        chain_id: &ChainId,
        spec_version: u32,
    ) -> Option<MetadataSnapshot> {
        let key = cache_key(chain_id, spec_version);

        // Try the cache first.
        if let Some(cached) = self.cache.get(&key).await {
            trace!(chain = %chain_id, spec_version, "metadata cache hit");
            return cached_value_to_snapshot(&cached);
        }

        // Cache miss -- fall through to the inner service.
        trace!(chain = %chain_id, spec_version, "metadata cache miss; consulting inner service");
        let snapshot = self.inner.get_snapshot(chain_id)?;

        // Only cache if the spec_version matches what was requested.
        if snapshot.spec_version == spec_version {
            let value = snapshot_to_cached_value(&snapshot);
            if let Err(e) = self.cache.set(key, value, Some(self.ttl)).await {
                debug!(error = %e, "failed to backfill cache after miss");
            }
        }

        Some(snapshot)
    }

    /// Get the latest snapshot for a chain from the inner service,
    /// without checking the cache. This always delegates directly.
    #[must_use]
    pub fn get_latest_snapshot(&self, chain_id: &ChainId) -> Option<MetadataSnapshot> {
        self.inner.get_snapshot(chain_id)
    }

    /// Return a reference to the cache statistics.
    pub fn cache_stats(&self) -> &CacheStats {
        self.cache.stats()
    }

    /// Return the configured TTL.
    #[must_use]
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Return a reference to the inner [`MetadataService`].
    #[must_use]
    pub fn inner(&self) -> &MetadataService {
        &self.inner
    }

    /// Return the number of entries currently in the cache.
    pub async fn cache_size(&self) -> usize {
        self.cache.size().await
    }

    /// Clear the entire metadata cache.
    pub async fn clear_cache(&self) {
        let _ = self.cache.clear().await;
    }
}

impl std::fmt::Debug for CachedMetadataService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedMetadataService")
            .field("inner", &self.inner)
            .field("ttl", &self.ttl)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a [`CacheKey`] from a chain ID and spec version.
fn cache_key(chain_id: &ChainId, spec_version: u32) -> CacheKey {
    CacheKey::new(CACHE_NAMESPACE, format!("{}:{}", chain_id, spec_version))
}

/// Serialize a [`MetadataSnapshot`] into a [`CachedValue`].
fn snapshot_to_cached_value(snapshot: &MetadataSnapshot) -> CachedValue {
    // We store the snapshot as JSON so it can live inside CachedValue::data.
    let data = json!({
        "chain_id": snapshot.chain_id,
        "version": snapshot.version,
        "hash": snapshot.hash,
        "raw_bytes": snapshot.raw_bytes,
        "fetched_at": snapshot.fetched_at,
        "spec_version": snapshot.spec_version,
    });
    CachedValue::new(data)
}

/// Deserialize a [`CachedValue`] back into a [`MetadataSnapshot`].
fn cached_value_to_snapshot(cached: &CachedValue) -> Option<MetadataSnapshot> {
    serde_json::from_value(cached.data.clone()).ok()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChainId, MetadataVersion};
    use polkagent_core::now;

    fn make_snapshot(chain: &str, data: &[u8], spec: u32) -> MetadataSnapshot {
        MetadataSnapshot::new(
            ChainId::new(chain),
            MetadataVersion::V14,
            data.to_vec(),
            now(),
            spec,
        )
    }

    fn make_service() -> CachedMetadataService {
        let inner = MetadataService::new();
        CachedMetadataService::new(inner)
    }

    fn make_service_with_ttl(ttl: Duration) -> CachedMetadataService {
        let inner = MetadataService::new();
        CachedMetadataService::with_config(inner, 64, ttl)
    }

    // -----------------------------------------------------------------------
    // 1. Cache hit returns same data
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn cache_hit_returns_same_data() {
        let svc = make_service();
        let snap = make_snapshot("polkadot", b"metadata_v1", 1_000_000);
        let expected_hash = snap.hash.clone();

        // Register so it is both in the inner service and the cache.
        svc.register_snapshot(snap).await;

        // First get -- should be a cache hit (register_snapshot caches it).
        let got = svc.get_snapshot(&ChainId::new("polkadot"), 1_000_000).await;
        assert!(got.is_some());
        let got = got.expect("snapshot should be present");
        assert_eq!(got.hash, expected_hash);
        assert_eq!(got.chain_id, ChainId::new("polkadot"));
        assert_eq!(got.spec_version, 1_000_000);

        // Verify stats show a hit.
        assert!(svc.cache_stats().hits() >= 1);
    }

    // -----------------------------------------------------------------------
    // 2. Cache miss calls inner service
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn cache_miss_calls_inner_service() {
        let inner = MetadataService::new();
        let snap = make_snapshot("polkadot", b"inner_only", 42);
        let expected_hash = snap.hash.clone();

        // Register directly on the inner service (bypassing the cache).
        inner.register_snapshot(snap);

        let svc = CachedMetadataService::new(inner);

        // The cache is empty, so this should miss and fall through.
        let got = svc.get_snapshot(&ChainId::new("polkadot"), 42).await;
        assert!(got.is_some());
        assert_eq!(got.as_ref().map(|s| &s.hash), Some(&expected_hash));

        // Should have recorded a miss.
        assert_eq!(svc.cache_stats().misses(), 1);

        // Subsequent get should be a cache hit now.
        let got2 = svc.get_snapshot(&ChainId::new("polkadot"), 42).await;
        assert!(got2.is_some());
        assert_eq!(got2.as_ref().map(|s| &s.hash), Some(&expected_hash));
        assert!(svc.cache_stats().hits() >= 1);
    }

    // -----------------------------------------------------------------------
    // 3. TTL expiry causes re-fetch
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn ttl_expiry_causes_refetch() {
        // Use a very short TTL that will expire almost immediately.
        let svc = make_service_with_ttl(Duration::from_millis(1));

        let snap = make_snapshot("polkadot", b"will_expire", 100);
        svc.register_snapshot(snap).await;

        // First hit from cache should work.
        let got1 = svc.get_snapshot(&ChainId::new("polkadot"), 100).await;
        assert!(got1.is_some());

        // Wait for TTL to expire.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // The cache entry has expired; this should be a miss that falls
        // through to the inner service.
        let got2 = svc.get_snapshot(&ChainId::new("polkadot"), 100).await;
        assert!(got2.is_some());

        // We should see at least one miss from the expired entry.
        assert!(svc.cache_stats().misses() >= 1);
    }

    // -----------------------------------------------------------------------
    // 4. Different networks cached separately
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn different_networks_cached_separately() {
        let svc = make_service();

        let snap_pdot = make_snapshot("polkadot", b"polkadot_meta", 1);
        let snap_ksm = make_snapshot("kusama", b"kusama_meta", 1);
        let pdot_hash = snap_pdot.hash.clone();
        let ksm_hash = snap_ksm.hash.clone();

        svc.register_snapshot(snap_pdot).await;
        svc.register_snapshot(snap_ksm).await;

        assert_eq!(svc.cache_size().await, 2);

        let got_pdot = svc.get_snapshot(&ChainId::new("polkadot"), 1).await;
        let got_ksm = svc.get_snapshot(&ChainId::new("kusama"), 1).await;

        assert!(got_pdot.is_some());
        assert!(got_ksm.is_some());
        assert_eq!(got_pdot.as_ref().map(|s| &s.hash), Some(&pdot_hash));
        assert_eq!(got_ksm.as_ref().map(|s| &s.hash), Some(&ksm_hash));

        // The two entries must be distinct.
        assert_ne!(pdot_hash, ksm_hash);
    }

    // -----------------------------------------------------------------------
    // 5. Cache stats are updated
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn cache_stats_are_updated() {
        let svc = make_service();

        // Start fresh.
        assert_eq!(svc.cache_stats().hits(), 0);
        assert_eq!(svc.cache_stats().misses(), 0);
        assert_eq!(svc.cache_stats().insertions(), 0);

        // Register a snapshot -- should record an insertion.
        let snap = make_snapshot("polkadot", b"stats_test", 1);
        svc.register_snapshot(snap).await;
        assert_eq!(svc.cache_stats().insertions(), 1);

        // Get from cache -- should record a hit.
        let _ = svc.get_snapshot(&ChainId::new("polkadot"), 1).await;
        assert_eq!(svc.cache_stats().hits(), 1);

        // Miss on a non-existent chain -- should record a miss.
        let _ = svc.get_snapshot(&ChainId::new("westend"), 99).await;
        assert_eq!(svc.cache_stats().misses(), 1);

        // Hit rate should be 1/(1+1) = 0.5.
        assert!((svc.cache_stats().hit_rate() - 0.5).abs() < 0.01);
    }

    // -----------------------------------------------------------------------
    // 6. Default TTL is 5 minutes
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn default_ttl_is_five_minutes() {
        let svc = make_service();
        assert_eq!(svc.ttl(), Duration::from_secs(300));
    }

    // -----------------------------------------------------------------------
    // 7. clear_cache empties the cache
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn clear_cache_empties_cache() {
        let svc = make_service();
        let snap = make_snapshot("polkadot", b"to_clear", 1);
        svc.register_snapshot(snap).await;
        assert_eq!(svc.cache_size().await, 1);

        svc.clear_cache().await;
        assert_eq!(svc.cache_size().await, 0);
    }

    // -----------------------------------------------------------------------
    // 8. get_latest_snapshot bypasses cache
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn get_latest_snapshot_bypasses_cache() {
        let inner = MetadataService::new();
        let snap = make_snapshot("polkadot", b"latest", 50);
        let expected_hash = snap.hash.clone();
        inner.register_snapshot(snap);

        let svc = CachedMetadataService::new(inner);

        // Cache is empty, but get_latest_snapshot should still work
        // because it goes directly to the inner service.
        let got = svc.get_latest_snapshot(&ChainId::new("polkadot"));
        assert!(got.is_some());
        assert_eq!(got.as_ref().map(|s| &s.hash), Some(&expected_hash));

        // No cache interactions should have occurred.
        assert_eq!(svc.cache_stats().hits(), 0);
        assert_eq!(svc.cache_stats().misses(), 0);
    }

    // -----------------------------------------------------------------------
    // 9. Same network, different spec versions cached separately
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn same_network_different_versions_cached_separately() {
        let svc = make_service();

        let snap_v1 = make_snapshot("polkadot", b"meta_v1", 1);
        let snap_v2 = make_snapshot("polkadot", b"meta_v2", 2);
        let hash_v1 = snap_v1.hash.clone();
        let hash_v2 = snap_v2.hash.clone();

        svc.register_snapshot(snap_v1).await;
        svc.register_snapshot(snap_v2).await;

        assert_eq!(svc.cache_size().await, 2);

        let got_v1 = svc.get_snapshot(&ChainId::new("polkadot"), 1).await;
        let got_v2 = svc.get_snapshot(&ChainId::new("polkadot"), 2).await;

        assert_eq!(got_v1.as_ref().map(|s| &s.hash), Some(&hash_v1));
        assert_eq!(got_v2.as_ref().map(|s| &s.hash), Some(&hash_v2));
    }
}
