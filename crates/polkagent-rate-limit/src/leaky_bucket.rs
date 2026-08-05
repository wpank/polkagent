//! Leaky-bucket rate limiter.
//!
//! The leaky bucket models a queue with a fixed drain rate.  Requests fill
//! the bucket; if the bucket overflows, the request is rejected.  The bucket
//! drains at a steady rate, producing smooth output regardless of bursty
//! input.
//!
//! Unlike the token bucket (which allows bursts up to capacity), the leaky
//! bucket enforces a strict average rate with bounded queue depth.

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use tracing::trace;

use crate::quota::QuotaResult;
use crate::{floor_to_u32, RateLimiter};

// ---------------------------------------------------------------------------
// Inner state
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Inner {
    /// Current water level (pending requests).
    level: f64,
    /// Maximum capacity (queue depth).
    capacity: f64,
    /// Drain rate in units per second.
    drain_rate: f64,
    /// Last time the bucket was drained.
    last_drain: Instant,
}

impl Inner {
    /// Drain the bucket based on elapsed time.
    fn drain(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.last_drain).as_secs_f64();
        let drained = elapsed * self.drain_rate;
        self.level = (self.level - drained).max(0.0);
        self.last_drain = now;
    }

    /// Try to add `cost` units to the bucket.
    fn try_add(&mut self, cost: u32) -> bool {
        let cost_f = f64::from(cost);
        if self.level + cost_f <= self.capacity {
            self.level += cost_f;
            true
        } else {
            false
        }
    }

    /// Peek at the current level (with virtual drain, no mutation).
    fn current_level(&self) -> f64 {
        let elapsed = Instant::now().duration_since(self.last_drain).as_secs_f64();
        let drained = elapsed * self.drain_rate;
        (self.level - drained).max(0.0)
    }

    /// Estimate how long until there is room for `cost` more units.
    fn time_until_room(&self, cost: u32) -> Duration {
        let cost_f = f64::from(cost);
        let current = self.current_level();
        let overflow = (current + cost_f) - self.capacity;
        if overflow <= 0.0 {
            return Duration::ZERO;
        }
        if self.drain_rate <= 0.0 {
            return Duration::from_secs(3600);
        }
        Duration::from_secs_f64(overflow / self.drain_rate)
    }
}

// ---------------------------------------------------------------------------
// LeakyBucket
// ---------------------------------------------------------------------------

/// A leaky-bucket rate limiter for smooth rate enforcement.
///
/// # Example
///
/// ```rust,ignore
/// use polkagent_rate_limit::LeakyBucket;
///
/// // Capacity of 10, draining at 2 units/second.
/// let bucket = LeakyBucket::new(10, 2.0);
/// let result = bucket.try_acquire("task-queue", 1);
/// assert!(result.allowed);
/// ```
#[derive(Debug)]
pub struct LeakyBucket {
    inner: Mutex<Inner>,
}

impl LeakyBucket {
    /// Create a new leaky bucket.
    ///
    /// # Arguments
    ///
    /// * `capacity` -- maximum queue depth.
    /// * `drain_rate` -- units drained per second.
    pub fn new(capacity: u32, drain_rate: f64) -> Self {
        Self {
            inner: Mutex::new(Inner {
                level: 0.0,
                capacity: f64::from(capacity),
                drain_rate,
                last_drain: Instant::now(),
            }),
        }
    }
}

impl RateLimiter for LeakyBucket {
    fn try_acquire(&self, key: &str, cost: u32) -> QuotaResult {
        let mut inner = self.inner.lock();
        let now = Instant::now();
        inner.drain(now);

        if inner.try_add(cost) {
            let remaining = floor_to_u32(inner.capacity - inner.level);
            trace!(
                key,
                cost,
                remaining,
                level = inner.level,
                "leaky bucket: allowed"
            );
            QuotaResult::allowed(remaining, None)
        } else {
            let retry_after = inner.time_until_room(cost);
            trace!(
                key,
                cost,
                ?retry_after,
                level = inner.level,
                "leaky bucket: denied"
            );
            QuotaResult::denied(0, None, Some(retry_after))
        }
    }

    fn remaining(&self, _key: &str) -> u32 {
        let inner = self.inner.lock();
        let current = inner.current_level();
        floor_to_u32(inner.capacity - current)
    }

    fn reset_at(&self, _key: &str) -> Option<DateTime<Utc>> {
        // Leaky buckets drain continuously; no discrete reset time.
        None
    }
}
