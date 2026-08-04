use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::CacheResult;
use crate::key::CacheKey;

/// A value stored in the cache, wrapping an arbitrary JSON payload with
/// metadata about insertion time, expiration, and access frequency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedValue {
    /// The stored payload.
    pub data: serde_json::Value,
    /// When the entry was originally inserted.
    pub inserted_at: DateTime<Utc>,
    /// When the entry expires (`None` means no expiration).
    pub expires_at: Option<DateTime<Utc>>,
    /// How many times the entry has been read since insertion.
    pub access_count: u64,
}

impl CachedValue {
    /// Create a new cached value with the current timestamp.
    pub fn new(data: serde_json::Value) -> Self {
        Self {
            data,
            inserted_at: Utc::now(),
            expires_at: None,
            access_count: 0,
        }
    }

    /// Create a cached value that expires after `ttl`.
    pub fn with_ttl(data: serde_json::Value, ttl: Duration) -> Self {
        let now = Utc::now();
        let millis = i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX);
        Self {
            data,
            inserted_at: now,
            expires_at: Some(now + chrono::Duration::milliseconds(millis)),
            access_count: 0,
        }
    }

    /// Returns `true` if this entry has expired.
    pub fn is_expired(&self) -> bool {
        self.expires_at.is_some_and(|exp| exp <= Utc::now())
    }
}

/// The primary trait that all cache backends must implement.
#[async_trait]
pub trait CacheStore: Send + Sync {
    /// Retrieve a value by key.  Returns `None` on miss or expiry.
    async fn get(&self, key: &CacheKey) -> Option<CachedValue>;

    /// Insert or update a value under `key` with an optional TTL.
    async fn set(
        &self,
        key: CacheKey,
        value: CachedValue,
        ttl: Option<Duration>,
    ) -> CacheResult<()>;

    /// Remove a key.  Returns `true` if the key existed.
    async fn remove(&self, key: &CacheKey) -> CacheResult<bool>;

    /// Remove all entries.
    async fn clear(&self) -> CacheResult<()>;

    /// Check whether a key exists (and is not expired).
    async fn contains(&self, key: &CacheKey) -> bool;

    /// Return the number of live entries.
    async fn size(&self) -> usize;
}
