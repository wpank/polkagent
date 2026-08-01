//! Per-key rate limiting with `DashMap`.
//!
//! [`KeyedRateLimiter`] maintains a separate rate limiter instance for each
//! key (agent ID, tool name, IP address, etc.).  New limiters are created
//! lazily on first access using a factory function.
//!
//! Backed by [`dashmap::DashMap`] for lock-free concurrent per-key access.

use std::fmt;
use std::hash::Hash;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use tracing::trace;

use crate::quota::QuotaResult;
use crate::token_bucket::TokenBucket;
use crate::RateLimiter;

// ---------------------------------------------------------------------------
// Factory trait
// ---------------------------------------------------------------------------

/// A factory that creates [`RateLimiter`] instances for new keys.
pub trait LimiterFactory: Send + Sync {
    /// Create a new rate limiter for the given key.
    fn create(&self, key: &str) -> Box<dyn RateLimiter>;
}

/// A simple factory that always creates [`TokenBucket`] instances.
#[derive(Debug, Clone)]
pub struct TokenBucketFactory {
    /// Bucket capacity.
    pub capacity: u32,
    /// Refill rate (tokens per interval).
    pub refill_rate: f64,
    /// Refill interval.
    pub refill_interval: Duration,
}

impl LimiterFactory for TokenBucketFactory {
    fn create(&self, _key: &str) -> Box<dyn RateLimiter> {
        Box::new(TokenBucket::new(
            self.capacity,
            self.refill_rate,
            self.refill_interval,
        ))
    }
}

// ---------------------------------------------------------------------------
// KeyedRateLimiter
// ---------------------------------------------------------------------------

/// A per-key rate limiter backed by `DashMap`.
///
/// Each unique key gets its own independent rate limiter instance, created
/// on demand via the provided [`LimiterFactory`].
///
/// The type parameter `K` is the key type.  Common choices:
/// - `String` for agent IDs, IPs, tool names
/// - Custom enum for structured key spaces
///
/// # Example
///
/// ```rust,ignore
/// use std::time::Duration;
/// use polkagent_rate_limit::keyed::{KeyedRateLimiter, TokenBucketFactory};
///
/// let factory = TokenBucketFactory {
///     capacity: 100,
///     refill_rate: 10.0,
///     refill_interval: Duration::from_secs(1),
/// };
///
/// let limiter = KeyedRateLimiter::<String>::new(factory);
/// let result = limiter.check(&"agent-42".to_string(), 1);
/// assert!(result.allowed);
/// ```
pub struct KeyedRateLimiter<K: Eq + Hash + Send + Sync + 'static = String> {
    buckets: DashMap<K, Box<dyn RateLimiter>>,
    factory: Arc<dyn LimiterFactory>,
}

impl<K: Eq + Hash + Send + Sync + Clone + fmt::Display + 'static> fmt::Debug
    for KeyedRateLimiter<K>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyedRateLimiter")
            .field("keys", &self.buckets.len())
            .finish()
    }
}

impl<K: Eq + Hash + Send + Sync + Clone + fmt::Display + 'static> KeyedRateLimiter<K> {
    /// Create a new keyed rate limiter with the given factory.
    pub fn new(factory: impl LimiterFactory + 'static) -> Self {
        Self {
            buckets: DashMap::new(),
            factory: Arc::new(factory),
        }
    }

    /// Check a rate limit for the given key with a cost.
    pub fn check(&self, key: &K, cost: u32) -> QuotaResult {
        let key_str = key.to_string();
        let entry = self
            .buckets
            .entry(key.clone())
            .or_insert_with(|| self.factory.create(&key_str));
        let result = entry.try_acquire(&key_str, cost);
        trace!(key = %key_str, cost, allowed = result.allowed, "keyed limiter check");
        result
    }

    /// Return the remaining quota for a key.
    pub fn key_remaining(&self, key: &K) -> u32 {
        let key_str = key.to_string();
        match self.buckets.get(key) {
            Some(limiter) => limiter.remaining(&key_str),
            None => {
                // Key not yet tracked -- return full capacity by creating a
                // temporary limiter just for the peek.
                let temp = self.factory.create(&key_str);
                temp.remaining(&key_str)
            }
        }
    }

    /// Return the reset time for a key.
    pub fn key_reset_at(&self, key: &K) -> Option<DateTime<Utc>> {
        let key_str = key.to_string();
        self.buckets.get(key).and_then(|l| l.reset_at(&key_str))
    }

    /// Number of tracked keys.
    pub fn len(&self) -> usize {
        self.buckets.len()
    }

    /// Whether any keys are tracked.
    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }

    /// Remove a key, freeing its limiter.
    pub fn remove(&self, key: &K) {
        self.buckets.remove(key);
    }

    /// Remove all keys.
    pub fn clear(&self) {
        self.buckets.clear();
    }
}

// ---------------------------------------------------------------------------
// RateLimiter trait impl for KeyedRateLimiter<String>
// ---------------------------------------------------------------------------

/// Blanket [`RateLimiter`] implementation for `KeyedRateLimiter<String>`.
///
/// This allows a `KeyedRateLimiter<String>` to be used anywhere a
/// `dyn RateLimiter` is expected. The `&str` key is converted to a `String`
/// for the `DashMap` lookup.
impl RateLimiter for KeyedRateLimiter<String> {
    fn try_acquire(&self, key: &str, cost: u32) -> QuotaResult {
        let entry = self
            .buckets
            .entry(key.to_owned())
            .or_insert_with(|| self.factory.create(key));
        entry.try_acquire(key, cost)
    }

    fn remaining(&self, key: &str) -> u32 {
        match self.buckets.get(&key.to_owned()) {
            Some(limiter) => limiter.remaining(key),
            None => {
                let temp = self.factory.create(key);
                temp.remaining(key)
            }
        }
    }

    fn reset_at(&self, key: &str) -> Option<DateTime<Utc>> {
        self.buckets
            .get(&key.to_owned())
            .and_then(|l| l.reset_at(key))
    }
}
