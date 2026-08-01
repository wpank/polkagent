//! Sliding-window counter rate limiter.
//!
//! Uses the **fixed-window approximation** to achieve O(1) memory per key.
//! The idea: maintain counters for the current window and the previous window.
//! The effective count is:
//!
//! ```text
//! count = prev_count * (1 - elapsed_fraction) + current_count
//! ```
//!
//! This gives a smooth approximation of a true sliding window without storing
//! individual timestamps.
//!
//! # Reference
//!
//! Cloudflare blog: "How we built rate limiting capable of scaling to
//! millions of domains."

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use tracing::trace;

use crate::quota::QuotaResult;
use crate::RateLimiter;

// ---------------------------------------------------------------------------
// Inner state
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct WindowState {
    /// Count of requests in the previous window.
    prev_count: u32,
    /// Count of requests in the current window.
    curr_count: u32,
    /// When the current window started.
    window_start: Instant,
    /// The UTC timestamp corresponding to `window_start`.
    window_start_utc: DateTime<Utc>,
}

#[derive(Debug)]
struct Inner {
    /// Maximum allowed requests per window.
    max_requests: u32,
    /// Window duration.
    window_size: Duration,
    /// Per-window state.
    state: WindowState,
}

impl Inner {
    /// Rotate windows if necessary, returning the elapsed fraction of the
    /// current window.
    fn maybe_rotate(&mut self, now: Instant) -> f64 {
        let elapsed = now.duration_since(self.state.window_start);

        if elapsed >= self.window_size + self.window_size {
            // More than two windows have passed -- reset everything.
            self.state.prev_count = 0;
            self.state.curr_count = 0;
            self.state.window_start = now;
            self.state.window_start_utc = Utc::now();
            return 0.0;
        }

        if elapsed >= self.window_size {
            // Rotate: current becomes previous, start a new current.
            self.state.prev_count = self.state.curr_count;
            self.state.curr_count = 0;
            // Advance window_start by one window_size.
            self.state.window_start += self.window_size;
            self.state.window_start_utc += chrono::Duration::from_std(self.window_size)
                .unwrap_or_else(|_| chrono::Duration::zero());

            let new_elapsed = now.duration_since(self.state.window_start);
            return new_elapsed.as_secs_f64() / self.window_size.as_secs_f64();
        }

        elapsed.as_secs_f64() / self.window_size.as_secs_f64()
    }

    /// Compute the effective request count using the sliding-window
    /// approximation.
    fn effective_count(&self, elapsed_fraction: f64) -> f64 {
        let prev_weight = 1.0 - elapsed_fraction;
        f64::from(self.state.prev_count) * prev_weight + f64::from(self.state.curr_count)
    }

    /// When this window resets (UTC).
    fn reset_at_utc(&self) -> DateTime<Utc> {
        self.state.window_start_utc
            + chrono::Duration::from_std(self.window_size)
                .unwrap_or_else(|_| chrono::Duration::zero())
    }
}

// ---------------------------------------------------------------------------
// SlidingWindowCounter
// ---------------------------------------------------------------------------

/// A sliding-window rate limiter using fixed-window approximation.
///
/// Achieves O(1) memory per key by only tracking two counters (previous
/// window and current window) and interpolating between them.
///
/// # Example
///
/// ```rust,ignore
/// use std::time::Duration;
/// use polkagent_rate_limit::SlidingWindowCounter;
///
/// let limiter = SlidingWindowCounter::new(100, Duration::from_secs(60));
/// let result = limiter.try_acquire("user-1", 1);
/// assert!(result.allowed);
/// ```
#[derive(Debug)]
pub struct SlidingWindowCounter {
    inner: Mutex<Inner>,
}

impl SlidingWindowCounter {
    /// Create a new sliding-window counter.
    ///
    /// # Arguments
    ///
    /// * `max_requests` -- maximum requests allowed per `window_size`.
    /// * `window_size` -- the sliding window duration.
    pub fn new(max_requests: u32, window_size: Duration) -> Self {
        let now = Instant::now();
        Self {
            inner: Mutex::new(Inner {
                max_requests,
                window_size,
                state: WindowState {
                    prev_count: 0,
                    curr_count: 0,
                    window_start: now,
                    window_start_utc: Utc::now(),
                },
            }),
        }
    }
}

impl RateLimiter for SlidingWindowCounter {
    fn try_acquire(&self, key: &str, cost: u32) -> QuotaResult {
        let mut inner = self.inner.lock();
        let now = Instant::now();
        let elapsed_fraction = inner.maybe_rotate(now);
        let effective = inner.effective_count(elapsed_fraction);
        let reset = inner.reset_at_utc();

        if effective + f64::from(cost) <= f64::from(inner.max_requests) {
            inner.state.curr_count = inner.state.curr_count.saturating_add(cost);
            let remaining = (f64::from(inner.max_requests) - effective - f64::from(cost))
                .floor()
                .max(0.0) as u32;
            trace!(key, cost, remaining, "sliding window: allowed");
            QuotaResult::allowed(remaining, Some(reset))
        } else {
            // Estimate retry_after: time until enough previous-window weight
            // decays to allow the request.
            let over = effective + f64::from(cost) - f64::from(inner.max_requests);
            let retry_fraction = if inner.state.prev_count > 0 {
                over / f64::from(inner.state.prev_count)
            } else {
                1.0 // No decay possible; must wait for full window reset.
            };
            let retry_secs = retry_fraction * inner.window_size.as_secs_f64();
            let retry_after = Duration::from_secs_f64(retry_secs.max(0.001));

            trace!(key, cost, ?retry_after, "sliding window: denied");
            QuotaResult::denied(0, Some(reset), Some(retry_after))
        }
    }

    fn remaining(&self, _key: &str) -> u32 {
        let inner = self.inner.lock();
        let elapsed = Instant::now().duration_since(inner.state.window_start);
        let fraction = if inner.window_size.is_zero() {
            0.0
        } else {
            (elapsed.as_secs_f64() / inner.window_size.as_secs_f64()).min(1.0)
        };
        let effective = inner.effective_count(fraction);
        let remaining = f64::from(inner.max_requests) - effective;
        remaining.floor().max(0.0) as u32
    }

    fn reset_at(&self, _key: &str) -> Option<DateTime<Utc>> {
        let inner = self.inner.lock();
        Some(inner.reset_at_utc())
    }
}
