//! Rate limiting strategies for the Polkagent platform.
//!
//! This crate provides multiple rate-limiting algorithms that can be composed
//! and applied per-key (agent, tool, IP, etc.).
//!
//! # Algorithms
//!
//! | Algorithm | Module | Use case |
//! |-----------|--------|----------|
//! | Token bucket | [`token_bucket`] | General-purpose, bursty traffic |
//! | Sliding window | [`sliding_window`] | Accurate window counting, O(1) memory |
//! | Leaky bucket | [`leaky_bucket`] | Smooth output rate |
//!
//! # Higher-level abstractions
//!
//! - [`composite`] -- combine multiple limiters (e.g. per-second AND per-minute)
//! - [`keyed`] -- per-key rate limiting via `DashMap`
//! - [`config`] -- tier-based configuration (free / basic / premium)
//! - [`middleware`] -- Tower `Layer` integration
//!
//! # Core trait
//!
//! All rate limiters implement [`RateLimiter`], which provides a uniform
//! interface for checking, querying remaining budget, and reset times.
//!
//! ```rust,ignore
//! use polkagent_rate_limit::{RateLimiter, QuotaResult};
//!
//! fn check_limit(limiter: &dyn RateLimiter, key: &str) {
//!     let result = limiter.try_acquire(key, 1);
//!     if result.allowed {
//!         // proceed
//!     } else {
//!         // back off for result.retry_after
//!     }
//! }
//! ```

pub mod composite;
pub mod config;
pub mod error;
pub mod keyed;
pub mod leaky_bucket;
pub mod middleware;
pub mod quota;
pub mod sliding_window;
pub mod token_bucket;

pub use composite::CompositeRateLimiter;
pub use config::RateLimitConfig;
pub use error::RateLimitError;
pub use keyed::KeyedRateLimiter;
pub use leaky_bucket::LeakyBucket;
pub use middleware::RateLimitLayer;
pub use quota::{Quota, QuotaResult};
pub use sliding_window::SlidingWindowCounter;
pub use token_bucket::TokenBucket;

use chrono::{DateTime, Utc};

/// Convert a floating-point capacity to the public integer quota shape.
///
/// Limiter state is bounded by an originating `u32` capacity, but clamping
/// here also makes the conversion robust against future arithmetic drift.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is floored and clamped to the full u32 domain before conversion"
)]
fn floor_to_u32(value: f64) -> u32 {
    value.floor().clamp(0.0, f64::from(u32::MAX)) as u32
}

// ---------------------------------------------------------------------------
// RateLimiter trait
// ---------------------------------------------------------------------------

/// A rate limiter that can check, track, and report on per-key request budgets.
///
/// Implementors must be thread-safe (`Send + Sync`) so they can be shared
/// across async tasks and request handlers.
pub trait RateLimiter: Send + Sync {
    /// Attempt to acquire `cost` units of capacity for `key`.
    ///
    /// Returns a [`QuotaResult`] indicating whether the request was allowed,
    /// how many units remain, and when the budget resets.
    fn try_acquire(&self, key: &str, cost: u32) -> QuotaResult;

    /// Return the number of units remaining for `key` without consuming any.
    fn remaining(&self, key: &str) -> u32;

    /// Return the time when the rate-limit window resets for `key`, if known.
    fn reset_at(&self, key: &str) -> Option<DateTime<Utc>>;
}
