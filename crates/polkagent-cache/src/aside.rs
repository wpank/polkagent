use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tracing::instrument;

use crate::error::{CacheError, CacheResult};
use crate::key::CacheKey;
use crate::store::{CacheStore, CachedValue};

/// The type of an async loader function used by `CacheAside`.
///
/// Given a cache key, returns the value to populate into the cache (or an
/// error string).
pub type LoaderFn = Arc<
    dyn Fn(CacheKey) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, String>> + Send>>
        + Send
        + Sync,
>;

/// Implements the *cache-aside* (read-through) pattern on top of any
/// [`CacheStore`].
///
/// On a cache miss, `CacheAside` invokes a configurable loader function to
/// fetch the value, stores it in the underlying cache, and returns it to the
/// caller.
pub struct CacheAside<S: CacheStore> {
    store: Arc<S>,
    loader: LoaderFn,
    default_ttl: Option<Duration>,
}

impl<S: CacheStore> CacheAside<S> {
    /// Create a new `CacheAside` wrapper around the given store and loader.
    pub fn new(store: Arc<S>, loader: LoaderFn) -> Self {
        Self {
            store,
            loader,
            default_ttl: None,
        }
    }

    /// Set the default TTL for values populated by the loader.
    #[must_use]
    pub fn with_default_ttl(mut self, ttl: Duration) -> Self {
        self.default_ttl = Some(ttl);
        self
    }

    /// Get a value from the cache, falling back to the loader on miss.
    ///
    /// On a successful load, the value is written into the cache with the
    /// configured TTL before being returned.
    #[instrument(skip(self), fields(key = %key))]
    pub async fn get_or_fetch(&self, key: CacheKey) -> CacheResult<CachedValue> {
        // Try the cache first.
        if let Some(cached) = self.store.get(&key).await {
            tracing::debug!("cache hit");
            return Ok(cached);
        }

        tracing::debug!("cache miss, invoking loader");

        // Call the loader.
        let data = (self.loader)(key.clone())
            .await
            .map_err(|reason| CacheError::LoaderFailed {
                key: key.to_string(),
                reason,
            })?;

        let value = CachedValue::new(data);

        // Populate the cache.
        self.store
            .set(key.clone(), value.clone(), self.default_ttl)
            .await?;

        // Re-read from the store so that the caller gets the same view as a
        // future cache hit would.
        Ok(self.store.get(&key).await.unwrap_or(value))
    }

    /// Invalidate a key and re-fetch it via the loader.
    #[instrument(skip(self), fields(key = %key))]
    pub async fn refresh(&self, key: CacheKey) -> CacheResult<CachedValue> {
        let _ = self.store.remove(&key).await;
        self.get_or_fetch(key).await
    }

    /// Return a reference to the underlying store.
    pub fn store(&self) -> &S {
        &self.store
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::CacheKey;
    use crate::memory::InMemoryCache;
    use serde_json::json;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn make_loader(call_count: Arc<AtomicU32>) -> LoaderFn {
        Arc::new(move |key: CacheKey| {
            let cc = Arc::clone(&call_count);
            Box::pin(async move {
                cc.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"loaded": key.name()}))
            })
        })
    }

    fn make_failing_loader() -> LoaderFn {
        Arc::new(move |_key: CacheKey| Box::pin(async move { Err("network error".to_owned()) }))
    }

    #[tokio::test]
    async fn fetches_on_miss_and_caches() {
        let store = Arc::new(InMemoryCache::new(10));
        let calls = Arc::new(AtomicU32::new(0));
        let aside = CacheAside::new(Arc::clone(&store), make_loader(Arc::clone(&calls)));

        let key = CacheKey::new("test", "item1");

        // First call should invoke the loader.
        let v1 = aside.get_or_fetch(key.clone()).await.expect("fetch failed");
        assert_eq!(v1.data, json!({"loaded": "item1"}));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Second call should hit the cache.
        let v2 = aside.get_or_fetch(key.clone()).await.expect("fetch failed");
        assert_eq!(v2.data, json!({"loaded": "item1"}));
        assert_eq!(calls.load(Ordering::SeqCst), 1); // no extra call
    }

    #[tokio::test]
    async fn refresh_invalidates_and_reloads() {
        let store = Arc::new(InMemoryCache::new(10));
        let calls = Arc::new(AtomicU32::new(0));
        let aside = CacheAside::new(Arc::clone(&store), make_loader(Arc::clone(&calls)));

        let key = CacheKey::new("test", "item1");
        aside.get_or_fetch(key.clone()).await.expect("fetch");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        aside.refresh(key.clone()).await.expect("refresh");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn loader_failure_returns_error() {
        let store = Arc::new(InMemoryCache::new(10));
        let aside = CacheAside::new(Arc::clone(&store), make_failing_loader());

        let key = CacheKey::new("test", "fail");
        let err = aside.get_or_fetch(key).await.unwrap_err();
        assert!(err.is_loader_failed());
    }

    #[tokio::test]
    async fn default_ttl_is_applied() {
        let store = Arc::new(InMemoryCache::new(10));
        let calls = Arc::new(AtomicU32::new(0));
        let aside = CacheAside::new(Arc::clone(&store), make_loader(Arc::clone(&calls)))
            .with_default_ttl(Duration::from_millis(0));

        let key = CacheKey::new("test", "ttl");
        aside.get_or_fetch(key.clone()).await.expect("fetch");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Let TTL expire.
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Should re-fetch because the entry expired.
        aside.get_or_fetch(key).await.expect("re-fetch");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
