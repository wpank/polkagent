//! Comprehensive tests for the polkagent-rate-limit crate.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use polkagent_rate_limit::config::{RateLimitConfig, Strategy, Tier, TierLimits};
use polkagent_rate_limit::keyed::{KeyedRateLimiter, TokenBucketFactory};
use polkagent_rate_limit::middleware::RateLimitLayer;
use polkagent_rate_limit::quota::{Quota, QuotaResult};
use polkagent_rate_limit::{
    CompositeRateLimiter, LeakyBucket, RateLimiter, SlidingWindowCounter, TokenBucket,
};

// =========================================================================
// Token bucket tests
// =========================================================================

#[test]
fn token_bucket_allows_within_capacity() {
    let bucket = TokenBucket::per_second(5, 5.0);
    for i in 0..5 {
        let result = bucket.try_acquire("key", 1);
        assert!(result.allowed, "request {i} should be allowed");
    }
}

#[test]
fn token_bucket_denies_over_capacity() {
    let bucket = TokenBucket::per_second(3, 0.0); // no refill
    for _ in 0..3 {
        assert!(bucket.try_acquire("key", 1).allowed);
    }
    let result = bucket.try_acquire("key", 1);
    assert!(!result.allowed, "should be denied after capacity exhausted");
    assert!(result.retry_after.is_some());
}

#[test]
fn token_bucket_refills_over_time() {
    // capacity=1, refill 100 tokens/s => after 20ms, ~2 tokens added
    let bucket = TokenBucket::per_second(1, 100.0);
    assert!(bucket.try_acquire("key", 1).allowed);
    assert!(!bucket.try_acquire("key", 1).allowed, "should be exhausted");

    thread::sleep(Duration::from_millis(25));

    assert!(
        bucket.try_acquire("key", 1).allowed,
        "should have refilled after sleep"
    );
}

#[test]
fn token_bucket_remaining_decreases() {
    let bucket = TokenBucket::per_second(10, 0.0);
    assert_eq!(bucket.remaining("key"), 10);
    bucket.try_acquire("key", 1);
    assert_eq!(bucket.remaining("key"), 9);
    bucket.try_acquire("key", 3);
    assert_eq!(bucket.remaining("key"), 6);
}

#[test]
fn token_bucket_cost_greater_than_one() {
    let bucket = TokenBucket::per_second(5, 0.0);
    assert!(bucket.try_acquire("key", 3).allowed);
    assert_eq!(bucket.remaining("key"), 2);
    assert!(!bucket.try_acquire("key", 3).allowed, "not enough for cost=3");
    assert!(bucket.try_acquire("key", 2).allowed, "just enough for cost=2");
}

#[test]
fn token_bucket_retry_after_is_reasonable() {
    let bucket = TokenBucket::new(1, 1.0, Duration::from_secs(1));
    bucket.try_acquire("key", 1);
    let result = bucket.try_acquire("key", 1);
    assert!(!result.allowed);
    let retry = result.retry_after.expect("should have retry_after");
    // Should be roughly 1 second (1 token / 1 token per second).
    assert!(retry.as_secs_f64() > 0.5, "retry should be > 0.5s, got {retry:?}");
    assert!(retry.as_secs_f64() < 2.0, "retry should be < 2s, got {retry:?}");
}

#[test]
fn token_bucket_reset_at_is_none() {
    let bucket = TokenBucket::per_second(10, 10.0);
    // Token buckets have continuous refill, no discrete reset.
    assert!(bucket.reset_at("key").is_none());
}

// =========================================================================
// Sliding window tests
// =========================================================================

#[test]
fn sliding_window_allows_within_limit() {
    let sw = SlidingWindowCounter::new(10, Duration::from_secs(60));
    for i in 0..10 {
        let result = sw.try_acquire("key", 1);
        assert!(result.allowed, "request {i} should be allowed");
    }
}

#[test]
fn sliding_window_denies_over_limit() {
    let sw = SlidingWindowCounter::new(3, Duration::from_secs(60));
    for _ in 0..3 {
        assert!(sw.try_acquire("key", 1).allowed);
    }
    let result = sw.try_acquire("key", 1);
    assert!(!result.allowed, "should deny request 4 in window of 3");
}

#[test]
fn sliding_window_remaining_is_accurate() {
    let sw = SlidingWindowCounter::new(5, Duration::from_secs(60));
    assert_eq!(sw.remaining("key"), 5);
    sw.try_acquire("key", 1);
    assert_eq!(sw.remaining("key"), 4);
    sw.try_acquire("key", 2);
    assert_eq!(sw.remaining("key"), 2);
}

#[test]
fn sliding_window_has_reset_time() {
    let sw = SlidingWindowCounter::new(10, Duration::from_secs(60));
    sw.try_acquire("key", 1);
    let reset = sw.reset_at("key");
    assert!(reset.is_some(), "sliding window should report a reset time");
}

#[test]
fn sliding_window_resets_after_window() {
    let sw = SlidingWindowCounter::new(2, Duration::from_millis(50));
    assert!(sw.try_acquire("key", 1).allowed);
    assert!(sw.try_acquire("key", 1).allowed);
    assert!(!sw.try_acquire("key", 1).allowed);

    // Wait for the window to pass.
    thread::sleep(Duration::from_millis(120));

    assert!(
        sw.try_acquire("key", 1).allowed,
        "should be allowed after window reset"
    );
}

#[test]
fn sliding_window_approximation_accuracy() {
    // With a 100ms window and max_requests=10, after using all 10 and
    // waiting half a window, the approximation should give ~5 from prev weight.
    let sw = SlidingWindowCounter::new(10, Duration::from_millis(100));
    for _ in 0..10 {
        sw.try_acquire("key", 1);
    }
    // All exhausted in the current window.
    assert!(!sw.try_acquire("key", 1).allowed);

    // Wait for the full window to elapse so capacity is restored.
    thread::sleep(Duration::from_millis(110));

    // After a full window has passed, new requests should be allowed.
    let result = sw.try_acquire("key", 1);
    assert!(result.allowed, "should be allowed after full window elapses");
}

// =========================================================================
// Leaky bucket tests
// =========================================================================

#[test]
fn leaky_bucket_allows_within_capacity() {
    let lb = LeakyBucket::new(5, 1.0);
    for i in 0..5 {
        let result = lb.try_acquire("key", 1);
        assert!(result.allowed, "request {i} should be allowed");
    }
}

#[test]
fn leaky_bucket_denies_when_full() {
    let lb = LeakyBucket::new(3, 0.0); // no drain
    for _ in 0..3 {
        assert!(lb.try_acquire("key", 1).allowed);
    }
    let result = lb.try_acquire("key", 1);
    assert!(!result.allowed, "should deny when bucket is full");
}

#[test]
fn leaky_bucket_drains_over_time() {
    let lb = LeakyBucket::new(2, 100.0); // drain 100/s
    assert!(lb.try_acquire("key", 1).allowed);
    assert!(lb.try_acquire("key", 1).allowed);
    assert!(!lb.try_acquire("key", 1).allowed, "full");

    thread::sleep(Duration::from_millis(25));

    assert!(
        lb.try_acquire("key", 1).allowed,
        "should have drained after sleep"
    );
}

#[test]
fn leaky_bucket_remaining_tracks_level() {
    let lb = LeakyBucket::new(10, 0.0);
    assert_eq!(lb.remaining("key"), 10);
    lb.try_acquire("key", 3);
    assert_eq!(lb.remaining("key"), 7);
}

#[test]
fn leaky_bucket_retry_after_calculation() {
    let lb = LeakyBucket::new(1, 1.0); // capacity=1, drain=1/s
    lb.try_acquire("key", 1);
    let result = lb.try_acquire("key", 1);
    assert!(!result.allowed);
    let retry = result.retry_after.expect("should have retry_after");
    // Need to drain 1 unit at 1/s => ~1 second
    assert!(retry.as_secs_f64() > 0.5);
    assert!(retry.as_secs_f64() < 2.0);
}

// =========================================================================
// Composite rate limiter tests
// =========================================================================

#[test]
fn composite_allows_when_all_allow() {
    let a = TokenBucket::per_second(10, 0.0);
    let b = TokenBucket::per_second(10, 0.0);
    let composite =
        CompositeRateLimiter::new(vec![Box::new(a), Box::new(b)]);
    let result = composite.try_acquire("key", 1);
    assert!(result.allowed);
}

#[test]
fn composite_denies_when_any_deny() {
    let generous = TokenBucket::per_second(100, 0.0);
    let strict = TokenBucket::per_second(2, 0.0);

    let composite = CompositeRateLimiter::new(vec![
        Box::new(generous),
        Box::new(strict),
    ]);

    assert!(composite.try_acquire("key", 1).allowed);
    assert!(composite.try_acquire("key", 1).allowed);
    // Third request: strict limiter exhausted
    assert!(
        !composite.try_acquire("key", 1).allowed,
        "strict limiter should deny"
    );
}

#[test]
fn composite_remaining_is_minimum() {
    let a = TokenBucket::per_second(10, 0.0);
    let b = TokenBucket::per_second(5, 0.0);

    let composite =
        CompositeRateLimiter::new(vec![Box::new(a), Box::new(b)]);

    // Use one from each => a has 9, b has 4 => min is 4
    composite.try_acquire("key", 1);
    assert_eq!(composite.remaining("key"), 4);
}

#[test]
fn composite_builder_pattern() {
    let composite = CompositeRateLimiter::new(Vec::new())
        .with(Box::new(TokenBucket::per_second(10, 0.0)))
        .with(Box::new(TokenBucket::per_second(5, 0.0)));
    assert!(composite.try_acquire("key", 1).allowed);
}

// =========================================================================
// Keyed rate limiter tests
// =========================================================================

#[test]
fn keyed_per_key_isolation() {
    let factory = TokenBucketFactory {
        capacity: 2,
        refill_rate: 0.0,
        refill_interval: Duration::from_secs(1),
    };
    let keyed = KeyedRateLimiter::<String>::new(factory);

    // Exhaust key A
    assert!(keyed.check(&"a".to_string(), 1).allowed);
    assert!(keyed.check(&"a".to_string(), 1).allowed);
    assert!(!keyed.check(&"a".to_string(), 1).allowed);

    // Key B should be independent
    assert!(keyed.check(&"b".to_string(), 1).allowed);
}

#[test]
fn keyed_tracks_key_count() {
    let factory = TokenBucketFactory {
        capacity: 10,
        refill_rate: 1.0,
        refill_interval: Duration::from_secs(1),
    };
    let keyed = KeyedRateLimiter::<String>::new(factory);

    assert!(keyed.is_empty());
    keyed.check(&"agent-1".to_string(), 1);
    keyed.check(&"agent-2".to_string(), 1);
    assert_eq!(keyed.len(), 2);
}

#[test]
fn keyed_remove_clears_key() {
    let factory = TokenBucketFactory {
        capacity: 2,
        refill_rate: 0.0,
        refill_interval: Duration::from_secs(1),
    };
    let keyed = KeyedRateLimiter::<String>::new(factory);

    let key = "user".to_string();
    keyed.check(&key, 1);
    keyed.check(&key, 1);
    assert!(!keyed.check(&key, 1).allowed, "exhausted");

    keyed.remove(&key);

    // After removal, a new limiter is created with full capacity.
    assert!(keyed.check(&key, 1).allowed, "should work after remove");
}

#[test]
fn keyed_remaining_for_unseen_key() {
    let factory = TokenBucketFactory {
        capacity: 20,
        refill_rate: 1.0,
        refill_interval: Duration::from_secs(1),
    };
    let keyed = KeyedRateLimiter::<String>::new(factory);
    assert_eq!(keyed.key_remaining(&"never-seen".to_string()), 20);
}

#[test]
fn keyed_clear_removes_all() {
    let factory = TokenBucketFactory {
        capacity: 10,
        refill_rate: 1.0,
        refill_interval: Duration::from_secs(1),
    };
    let keyed = KeyedRateLimiter::<String>::new(factory);

    keyed.check(&"a".to_string(), 1);
    keyed.check(&"b".to_string(), 1);
    keyed.check(&"c".to_string(), 1);
    assert_eq!(keyed.len(), 3);

    keyed.clear();
    assert!(keyed.is_empty());
}

// =========================================================================
// Quota tests
// =========================================================================

#[test]
fn quota_capacity_includes_burst() {
    let q = Quota::new(100, Duration::from_secs(60)).with_burst(20);
    assert_eq!(q.capacity(), 120);
}

#[test]
fn quota_refill_rate() {
    let q = Quota::new(60, Duration::from_secs(60));
    let rate = q.refill_rate();
    assert!((rate - 1.0).abs() < 0.001, "expected 1.0/s, got {rate}");
}

#[test]
fn quota_refill_rate_zero_window() {
    let q = Quota::new(100, Duration::ZERO);
    assert_eq!(q.refill_rate(), 0.0);
}

#[test]
fn quota_result_allowed() {
    let r = QuotaResult::allowed(5, None);
    assert!(r.allowed);
    assert_eq!(r.remaining, 5);
    assert!(r.retry_after.is_none());
}

#[test]
fn quota_result_denied() {
    let retry = Duration::from_millis(500);
    let r = QuotaResult::denied(0, None, Some(retry));
    assert!(!r.allowed);
    assert_eq!(r.remaining, 0);
    assert_eq!(r.retry_after, Some(retry));
}

#[test]
fn quota_serialization_roundtrip() {
    let q = Quota::new(100, Duration::from_secs(60)).with_burst(10);
    let json = serde_json::to_string(&q).expect("serialize");
    let q2: Quota = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(q, q2);
}

// =========================================================================
// Config tests
// =========================================================================

#[test]
fn config_development_defaults() {
    let cfg = RateLimitConfig::development();
    assert!(cfg.enabled);
    assert_eq!(cfg.strategy, Strategy::TokenBucket);
    assert!(cfg.limits_for_tier(Tier::Free).is_some());
    assert!(cfg.limits_for_tier(Tier::Basic).is_some());
    assert!(cfg.limits_for_tier(Tier::Premium).is_some());
}

#[test]
fn config_validate_empty_tiers() {
    let cfg = RateLimitConfig {
        strategy: Strategy::TokenBucket,
        tiers: std::collections::HashMap::new(),
        endpoint_overrides: Vec::new(),
        enabled: true,
    };
    assert!(cfg.validate().is_err());
}

#[test]
fn config_validate_all_zero_limits() {
    let mut tiers = std::collections::HashMap::new();
    tiers.insert(
        Tier::Free,
        TierLimits {
            requests_per_second: 0,
            requests_per_minute: 0,
            requests_per_hour: 0,
            burst: 0,
        },
    );
    let cfg = RateLimitConfig {
        strategy: Strategy::SlidingWindow,
        tiers,
        endpoint_overrides: Vec::new(),
        enabled: true,
    };
    assert!(cfg.validate().is_err());
}

#[test]
fn config_valid_passes_validation() {
    let cfg = RateLimitConfig::development();
    assert!(cfg.validate().is_ok());
}

#[test]
fn config_tier_limits_quotas() {
    let limits = TierLimits {
        requests_per_second: 10,
        requests_per_minute: 600,
        requests_per_hour: 36000,
        burst: 20,
    };
    let ps = limits.per_second_quota();
    assert_eq!(ps.max_requests, 10);
    assert_eq!(ps.burst, 20);
    assert_eq!(ps.window, Duration::from_secs(1));

    let pm = limits.per_minute_quota();
    assert_eq!(pm.max_requests, 600);
    assert_eq!(pm.window, Duration::from_secs(60));
}

#[test]
fn config_serialization_roundtrip() {
    let cfg = RateLimitConfig::development();
    let json = serde_json::to_string(&cfg).expect("serialize");
    let cfg2: RateLimitConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(cfg2.strategy, cfg.strategy);
    assert_eq!(cfg2.enabled, cfg.enabled);
}

#[test]
fn config_endpoint_override_lookup() {
    let mut cfg = RateLimitConfig::development();
    cfg.endpoint_overrides.push(
        polkagent_rate_limit::config::EndpointOverride {
            endpoint: "/api/v1/execute".into(),
            limits: None,
            exempt: true,
        },
    );
    let found = cfg.override_for_endpoint("/api/v1/execute");
    assert!(found.is_some());
    assert!(found.expect("override").exempt);
    assert!(cfg.override_for_endpoint("/api/v1/other").is_none());
}

// =========================================================================
// Concurrent access tests
// =========================================================================

#[test]
fn token_bucket_concurrent_access() {
    let bucket = Arc::new(TokenBucket::per_second(100, 0.0));
    let handles: Vec<_> = (0..10)
        .map(|_| {
            let bucket = Arc::clone(&bucket);
            thread::spawn(move || {
                let mut allowed = 0u32;
                for _ in 0..20 {
                    if bucket.try_acquire("key", 1).allowed {
                        allowed += 1;
                    }
                }
                allowed
            })
        })
        .collect();

    let total_allowed: u32 = handles.into_iter().map(|h| h.join().expect("join")).sum();
    // 10 threads x 20 requests = 200 total, capacity = 100, no refill.
    assert_eq!(
        total_allowed, 100,
        "exactly 100 should be allowed, got {total_allowed}"
    );
}

#[test]
fn keyed_concurrent_different_keys() {
    let factory = TokenBucketFactory {
        capacity: 5,
        refill_rate: 0.0,
        refill_interval: Duration::from_secs(1),
    };
    let keyed = Arc::new(KeyedRateLimiter::<String>::new(factory));

    let handles: Vec<_> = (0..5)
        .map(|i| {
            let keyed = Arc::clone(&keyed);
            thread::spawn(move || {
                let key = format!("agent-{i}");
                let mut allowed = 0u32;
                for _ in 0..10 {
                    if keyed.check(&key, 1).allowed {
                        allowed += 1;
                    }
                }
                allowed
            })
        })
        .collect();

    let total_allowed: u32 = handles.into_iter().map(|h| h.join().expect("join")).sum();
    // 5 keys x 5 capacity each = 25
    assert_eq!(
        total_allowed, 25,
        "5 keys x 5 capacity = 25 allowed, got {total_allowed}"
    );
}

// =========================================================================
// Middleware tests
// =========================================================================

#[test]
fn rate_limit_layer_debug() {
    let limiter: Arc<dyn RateLimiter> = Arc::new(TokenBucket::per_second(10, 10.0));
    let layer = RateLimitLayer::new(limiter, |_req: &String| Some("global".to_string()));
    let debug = format!("{layer:?}");
    assert!(debug.contains("RateLimitLayer"));
}

#[test]
fn rate_limit_layer_with_cost() {
    let limiter: Arc<dyn RateLimiter> = Arc::new(TokenBucket::per_second(10, 10.0));
    let layer = RateLimitLayer::new(limiter, |_req: &String| Some("global".to_string()))
        .with_cost(5);
    let debug = format!("{layer:?}");
    assert!(debug.contains("cost"));
}

// =========================================================================
// Error tests
// =========================================================================

#[test]
fn error_exceeded_display() {
    let err = polkagent_rate_limit::RateLimitError::Exceeded {
        retry_after_ms: 1500,
    };
    let s = err.to_string();
    assert!(s.contains("1500"), "should contain retry_after_ms");
    assert!(s.contains("rate limit exceeded"), "should describe error");
}

#[test]
fn error_quota_exhausted_display() {
    let err = polkagent_rate_limit::RateLimitError::QuotaExhausted {
        reset_at: "2026-01-01T00:00:00Z".into(),
    };
    let s = err.to_string();
    assert!(s.contains("quota exhausted"));
    assert!(s.contains("2026"));
}

#[test]
fn error_invalid_config_display() {
    let err = polkagent_rate_limit::RateLimitError::InvalidConfig {
        reason: "missing tiers".into(),
    };
    assert!(err.to_string().contains("missing tiers"));
}

// =========================================================================
// Trait object tests
// =========================================================================

#[test]
fn trait_object_works_with_token_bucket() {
    let limiter: Box<dyn RateLimiter> = Box::new(TokenBucket::per_second(5, 5.0));
    assert!(limiter.try_acquire("key", 1).allowed);
    assert_eq!(limiter.remaining("key"), 4);
}

#[test]
fn trait_object_works_with_sliding_window() {
    let limiter: Box<dyn RateLimiter> =
        Box::new(SlidingWindowCounter::new(5, Duration::from_secs(60)));
    assert!(limiter.try_acquire("key", 1).allowed);
    assert!(limiter.reset_at("key").is_some());
}

#[test]
fn trait_object_works_with_leaky_bucket() {
    let limiter: Box<dyn RateLimiter> = Box::new(LeakyBucket::new(5, 1.0));
    assert!(limiter.try_acquire("key", 1).allowed);
    assert!(limiter.reset_at("key").is_none());
}
