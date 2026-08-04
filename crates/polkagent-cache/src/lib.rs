//! # polkagent-cache
//!
//! Caching layer for the Polkagent platform.
//!
//! Provides an LRU cache with per-entry TTL, a cache-aside (read-through)
//! pattern, namespaced cache keys with automatic BLAKE3 hashing for long
//! keys, and thread-safe atomic statistics.
//!
//! ## Quick start
//!
//! ```rust
//! use std::sync::Arc;
//! use polkagent_cache::{
//!     CacheKey, CachedValue, CacheStore, InMemoryCache, CacheAside, CacheConfig,
//! };
//! use serde_json::json;
//!
//! # #[tokio::main] async fn main() {
//! // Create an in-memory cache with room for 1000 entries.
//! let cache = Arc::new(InMemoryCache::new(1000));
//!
//! // Insert a value.
//! let key = CacheKey::new("metadata", "polkadot");
//! let value = CachedValue::new(json!({"spec_version": 1_000_000}));
//! cache.set(key.clone(), value, None).await.unwrap();
//!
//! // Read it back.
//! let hit = cache.get(&key).await.unwrap();
//! assert_eq!(hit.data["spec_version"], 1_000_000);
//! # }
//! ```

pub mod aside;
pub mod error;
pub mod key;
pub mod lru;
pub mod memory;
pub mod stats;
pub mod store;
pub mod ttl;

// ---------------------------------------------------------------------------
// Re-exports for ergonomic top-level imports
// ---------------------------------------------------------------------------

pub use aside::{CacheAside, LoaderFn};
pub use error::{CacheError, CacheResult};
pub use key::CacheKey;
pub use memory::InMemoryCache;
pub use stats::CacheStats;
pub use store::{CacheStore, CachedValue};
pub use ttl::TtlPolicy;

// ---------------------------------------------------------------------------
// CacheConfig
// ---------------------------------------------------------------------------

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Top-level configuration for constructing a cache instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Maximum number of entries the cache may hold.
    pub max_entries: usize,

    /// Default TTL applied to entries that don't specify one.
    #[serde(default)]
    pub default_ttl: Option<Duration>,

    /// TTL policy to use.
    #[serde(default)]
    pub ttl_policy: TtlPolicy,
}

impl CacheConfig {
    /// Create a config with just a capacity limit and no TTL.
    pub fn new(max_entries: usize) -> Self {
        Self {
            max_entries,
            default_ttl: None,
            ttl_policy: TtlPolicy::None,
        }
    }

    /// Set the default TTL.
    #[must_use]
    pub fn with_default_ttl(mut self, ttl: Duration) -> Self {
        self.default_ttl = Some(ttl);
        self
    }

    /// Set the TTL policy.
    #[must_use]
    pub fn with_ttl_policy(mut self, policy: TtlPolicy) -> Self {
        self.ttl_policy = policy;
        self
    }

    /// Build an [`InMemoryCache`] from this configuration.
    pub fn build_in_memory(&self) -> InMemoryCache {
        match self.default_ttl {
            Some(ttl) => InMemoryCache::with_default_ttl(self.max_entries, ttl),
            None => InMemoryCache::new(self.max_entries),
        }
    }
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self::new(1024)
    }
}

// ---------------------------------------------------------------------------
// Integration tests (lib-level)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    #[tokio::test]
    async fn config_builds_working_cache() {
        let config = CacheConfig::new(100);
        let cache = config.build_in_memory();
        let key = CacheKey::new("test", "cfg");
        cache
            .set(key.clone(), CachedValue::new(json!(42)), None)
            .await
            .expect("set");
        let v = cache.get(&key).await.expect("get");
        assert_eq!(v.data, json!(42));
    }

    #[tokio::test]
    async fn config_default_ttl_propagates() {
        let config = CacheConfig::new(100).with_default_ttl(Duration::from_millis(0));
        let cache = config.build_in_memory();
        let key = CacheKey::new("test", "ttl");
        cache
            .set(key.clone(), CachedValue::new(json!(1)), None)
            .await
            .expect("set");
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(cache.get(&key).await.is_none());
    }

    #[tokio::test]
    async fn end_to_end_cache_aside() {
        let store = Arc::new(InMemoryCache::new(100));
        let loader: LoaderFn =
            Arc::new(|key: CacheKey| Box::pin(async move { Ok(json!({"chain": key.name()})) }));
        let aside = CacheAside::new(store, loader);

        let key = CacheKey::new("metadata", "polkadot");
        let val = aside.get_or_fetch(key.clone()).await.expect("fetch");
        assert_eq!(val.data["chain"], "polkadot");

        // Second fetch should hit cache (loader not invoked again).
        let val2 = aside.get_or_fetch(key).await.expect("fetch2");
        assert_eq!(val2.data["chain"], "polkadot");
    }

    #[tokio::test]
    async fn cache_config_serialization_round_trip() {
        let config = CacheConfig::new(500)
            .with_default_ttl(Duration::from_secs(60))
            .with_ttl_policy(TtlPolicy::Fixed(Duration::from_secs(120)));
        let json = serde_json::to_string(&config).expect("serialize");
        let deserialized: CacheConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.max_entries, 500);
        assert_eq!(deserialized.default_ttl, Some(Duration::from_secs(60)));
    }
}
