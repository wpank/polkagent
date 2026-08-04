//! Retry, circuit breaker, bulkhead, and timeout patterns for Polkagent.
//!
//! This crate provides resilience primitives for building reliable distributed
//! systems on the Polkagent platform:
//!
//! - **Retry with backoff**: configurable retry policies with fixed, exponential,
//!   and linear backoff strategies.
//! - **Circuit breaker**: protects downstream services by failing fast when
//!   error rates exceed a threshold.
//! - **Bulkhead**: limits concurrency to prevent resource exhaustion.
//! - **Timeout**: wraps async operations with configurable timeouts.
//! - **Error classification**: pluggable classification of errors as retryable,
//!   non-retryable, or circuit-breaking.
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use polkagent_retry::policy::RetryPolicy;
//! use polkagent_retry::executor::retry_with_policy;
//! use std::time::Duration;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let policy = RetryPolicy::exponential(3, Duration::from_millis(100));
//! let result = retry_with_policy(&policy, || async {
//!     // your fallible async operation
//!     Ok::<_, String>("success")
//! }).await;
//! # Ok(())
//! # }
//! ```

pub mod backoff;
pub mod bulkhead;
pub mod circuit_breaker;
pub mod classifier;
pub mod error;
pub mod executor;
pub mod fallback;
pub mod model_executor;
pub mod policy;
pub mod provider_health;
pub mod retry_class;
pub mod timeout;

// Re-export key types at the crate root for convenience.
pub use backoff::BackoffStrategy;
pub use bulkhead::Bulkhead;
pub use circuit_breaker::{CircuitBreaker, CircuitState};
pub use classifier::{ErrorClass, ErrorClassifier};
pub use error::{BulkheadFull, CircuitOpen, RetryError, RetryExhausted};
pub use executor::{retry_with_policy, retry_with_policy_and_classifier};
pub use fallback::{FallbackChain, FallbackEntry, FallbackTrigger, ModelRoute};
pub use model_executor::RetryModelExecutor;
pub use policy::RetryPolicy;
pub use provider_health::{HealthState, LatencyTracker, ProviderHealthStatus};
pub use retry_class::{classify_provider_error, RetryClass};
pub use timeout::TimeoutWrapper;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    // ───────────────────────────── Backoff tests ─────────────────────────────

    #[test]
    fn test_fixed_backoff_returns_constant_delay() {
        let strategy = BackoffStrategy::Fixed(Duration::from_millis(200));
        for attempt in 0..5 {
            assert_eq!(strategy.delay(attempt, 0), Duration::from_millis(200));
        }
    }

    #[test]
    fn test_exponential_backoff_doubles_each_attempt() {
        let strategy = BackoffStrategy::Exponential {
            base: Duration::from_millis(100),
            max: Duration::from_secs(10),
            jitter: false,
        };
        assert_eq!(strategy.delay(0, 0), Duration::from_millis(100));
        assert_eq!(strategy.delay(1, 0), Duration::from_millis(200));
        assert_eq!(strategy.delay(2, 0), Duration::from_millis(400));
        assert_eq!(strategy.delay(3, 0), Duration::from_millis(800));
    }

    #[test]
    fn test_exponential_backoff_caps_at_max() {
        let strategy = BackoffStrategy::Exponential {
            base: Duration::from_millis(100),
            max: Duration::from_millis(500),
            jitter: false,
        };
        // 2^3 * 100 = 800, capped at 500
        assert_eq!(strategy.delay(3, 0), Duration::from_millis(500));
        assert_eq!(strategy.delay(10, 0), Duration::from_millis(500));
    }

    #[test]
    fn test_exponential_backoff_with_jitter_produces_bounded_values() {
        let strategy = BackoffStrategy::Exponential {
            base: Duration::from_millis(100),
            max: Duration::from_secs(10),
            jitter: true,
        };

        for seed in 0..20 {
            let delay = strategy.delay(2, seed); // base delay 400ms
                                                 // With jitter: [200, 400]
            assert!(
                delay >= Duration::from_millis(200) && delay <= Duration::from_millis(400),
                "delay {delay:?} out of expected jitter range for seed {seed}"
            );
        }
    }

    #[test]
    fn test_jitter_produces_different_values_for_different_seeds() {
        let strategy = BackoffStrategy::Exponential {
            base: Duration::from_millis(1000),
            max: Duration::from_secs(10),
            jitter: true,
        };
        let d1 = strategy.delay(2, 1);
        let d2 = strategy.delay(2, 9999);
        // With a large enough base, different seeds should produce different jitter
        // (they might not if the range is very small, but 4000ms range is fine).
        assert_ne!(
            d1, d2,
            "different seeds should generally produce different delays"
        );
    }

    #[test]
    fn test_linear_backoff_increments_linearly() {
        let strategy = BackoffStrategy::Linear {
            step: Duration::from_millis(100),
            max: Duration::from_millis(500),
        };
        assert_eq!(strategy.delay(0, 0), Duration::from_millis(100)); // 100 * 1
        assert_eq!(strategy.delay(1, 0), Duration::from_millis(200)); // 100 * 2
        assert_eq!(strategy.delay(2, 0), Duration::from_millis(300)); // 100 * 3
        assert_eq!(strategy.delay(3, 0), Duration::from_millis(400)); // 100 * 4
        assert_eq!(strategy.delay(4, 0), Duration::from_millis(500)); // 100 * 5, capped
        assert_eq!(strategy.delay(10, 0), Duration::from_millis(500)); // still capped
    }

    #[test]
    fn test_exponential_overflow_saturates() {
        let strategy = BackoffStrategy::Exponential {
            base: Duration::from_millis(100),
            max: Duration::from_millis(5000),
            jitter: false,
        };
        // attempt 40: 2^40 * 100ms would overflow u64 ms, but should clamp
        let delay = strategy.delay(40, 0);
        assert_eq!(delay, Duration::from_millis(5000));
    }

    // ──────────────────────────── Policy tests ──────────────────────────────

    #[test]
    fn test_policy_exponential_constructor() {
        let policy = RetryPolicy::exponential(3, Duration::from_millis(100));
        assert_eq!(policy.max_retries, 3);
        assert!(matches!(
            policy.backoff,
            BackoffStrategy::Exponential { jitter: false, .. }
        ));
    }

    #[test]
    fn test_policy_fixed_constructor() {
        let policy = RetryPolicy::fixed(5, Duration::from_millis(50));
        assert_eq!(policy.max_retries, 5);
        assert!(matches!(policy.backoff, BackoffStrategy::Fixed(_)));
    }

    #[test]
    fn test_policy_linear_constructor() {
        let policy = RetryPolicy::linear(2, Duration::from_millis(100), Duration::from_secs(1));
        assert_eq!(policy.max_retries, 2);
        assert!(matches!(policy.backoff, BackoffStrategy::Linear { .. }));
    }

    #[test]
    fn test_policy_with_per_attempt_timeout() {
        let policy = RetryPolicy::exponential(3, Duration::from_millis(100))
            .with_per_attempt_timeout(Duration::from_secs(5));
        assert_eq!(policy.per_attempt_timeout, Some(Duration::from_secs(5)));
    }

    #[test]
    fn test_policy_should_retry_within_limit() {
        let policy = RetryPolicy::fixed(3, Duration::from_millis(10));
        let classifier = classifier::AlwaysRetry;
        assert!(policy.should_retry(&"error", 0, &classifier));
        assert!(policy.should_retry(&"error", 2, &classifier));
        assert!(!policy.should_retry(&"error", 3, &classifier));
    }

    #[test]
    fn test_policy_should_not_retry_non_retryable() {
        let policy = RetryPolicy::fixed(3, Duration::from_millis(10));
        let classifier = classifier::NeverRetry;
        assert!(!policy.should_retry(&"error", 0, &classifier));
    }

    #[test]
    fn test_policy_serialization_roundtrip() {
        let policy = RetryPolicy::exponential(3, Duration::from_millis(100))
            .with_per_attempt_timeout(Duration::from_secs(5));
        let json = serde_json::to_string(&policy).expect("serialize");
        let deserialized: RetryPolicy = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.max_retries, 3);
        assert_eq!(
            deserialized.per_attempt_timeout,
            Some(Duration::from_secs(5))
        );
    }

    // ────────────────────────── Classifier tests ────────────────────────────

    #[test]
    fn test_always_retry_classifier() {
        let classifier = classifier::AlwaysRetry;
        assert_eq!(classifier.classify(&"any error"), ErrorClass::Retryable);
    }

    #[test]
    fn test_never_retry_classifier() {
        let classifier = classifier::NeverRetry;
        assert_eq!(classifier.classify(&"any error"), ErrorClass::NonRetryable);
    }

    #[test]
    fn test_pattern_classifier_retryable() {
        let classifier = classifier::PatternClassifier::new(
            vec!["timeout".to_string(), "connection refused".to_string()],
            vec!["out of memory".to_string()],
        );
        assert_eq!(
            classifier.classify(&"connection refused by host"),
            ErrorClass::Retryable
        );
    }

    #[test]
    fn test_pattern_classifier_circuit_break() {
        let classifier = classifier::PatternClassifier::new(
            vec!["timeout".to_string()],
            vec!["out of memory".to_string()],
        );
        assert_eq!(
            classifier.classify(&"out of memory: heap exhausted"),
            ErrorClass::CircuitBreak
        );
    }

    #[test]
    fn test_pattern_classifier_non_retryable_default() {
        let classifier = classifier::PatternClassifier::new(
            vec!["timeout".to_string()],
            vec!["oom".to_string()],
        );
        assert_eq!(
            classifier.classify(&"permission denied"),
            ErrorClass::NonRetryable
        );
    }

    #[test]
    fn test_fn_classifier() {
        let classifier = classifier::FnClassifier::new(|e: &&str| {
            if e.contains("retry") {
                ErrorClass::Retryable
            } else {
                ErrorClass::NonRetryable
            }
        });
        assert_eq!(classifier.classify(&"please retry"), ErrorClass::Retryable);
        assert_eq!(classifier.classify(&"fatal"), ErrorClass::NonRetryable);
    }

    // ────────────────────────── Executor tests ──────────────────────────────

    #[tokio::test]
    async fn test_retry_succeeds_on_first_attempt() {
        let policy = RetryPolicy::fixed(3, Duration::from_millis(1));
        let result = retry_with_policy(&policy, || async { Ok::<_, String>("ok") }).await;
        assert_eq!(result.expect("should succeed"), "ok");
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_failures() {
        let counter = Arc::new(AtomicU32::new(0));
        let policy = RetryPolicy::fixed(3, Duration::from_millis(1));

        let c = counter.clone();
        let result = retry_with_policy(&policy, || {
            let c = c.clone();
            async move {
                let attempt = c.fetch_add(1, Ordering::SeqCst);
                if attempt < 2 {
                    Err(format!("fail #{attempt}"))
                } else {
                    Ok("recovered")
                }
            }
        })
        .await;

        assert_eq!(result.expect("should succeed"), "recovered");
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let counter = Arc::new(AtomicU32::new(0));
        let policy = RetryPolicy::fixed(2, Duration::from_millis(1));

        let c = counter.clone();
        let result: Result<&str, RetryExhausted<String>> = retry_with_policy(&policy, || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err("always fails".to_string())
            }
        })
        .await;

        let err = result.expect_err("should fail");
        assert_eq!(err.attempts, 3); // initial + 2 retries
        assert_eq!(err.last_error, "always fails");
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_non_retryable_stops_immediately() {
        let counter = Arc::new(AtomicU32::new(0));
        let policy = RetryPolicy::fixed(5, Duration::from_millis(1));
        let classifier = classifier::NeverRetry;

        let c = counter.clone();
        let result: Result<&str, RetryExhausted<String>> =
            retry_with_policy_and_classifier(&policy, &classifier, &mut || {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err("permanent error".to_string())
                }
            })
            .await;

        let err = result.expect_err("should fail");
        assert_eq!(err.attempts, 1); // Only one attempt
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_circuit_break_stops_immediately() {
        let counter = Arc::new(AtomicU32::new(0));
        let policy = RetryPolicy::fixed(5, Duration::from_millis(1));
        let classifier = classifier::FnClassifier::new(|_: &String| ErrorClass::CircuitBreak);

        let c = counter.clone();
        let result: Result<&str, RetryExhausted<String>> =
            retry_with_policy_and_classifier(&policy, &classifier, &mut || {
                let c = c.clone();
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Err("circuit break error".to_string())
                }
            })
            .await;

        let err = result.expect_err("should fail");
        assert_eq!(err.attempts, 1);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_measures_elapsed_time() {
        let policy = RetryPolicy::fixed(2, Duration::from_millis(10));

        let start = Instant::now();
        let result: Result<(), RetryExhausted<String>> =
            retry_with_policy(&policy, || async { Err("fail".to_string()) }).await;

        let err = result.expect_err("should fail");
        let wall_time = start.elapsed();

        // Should have spent at least 20ms (2 waits of 10ms each)
        assert!(
            err.total_elapsed >= Duration::from_millis(15),
            "elapsed {:?} should be >= 15ms",
            err.total_elapsed
        );
        assert!(
            wall_time >= Duration::from_millis(15),
            "wall time {:?} should be >= 15ms",
            wall_time
        );
    }

    // ────────────────────── Circuit Breaker tests ───────────────────────────

    #[test]
    fn test_circuit_breaker_starts_closed() {
        let cb = CircuitBreaker::new(5, 3, Duration::from_secs(30));
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.check().is_ok());
    }

    #[test]
    fn test_circuit_breaker_opens_after_threshold() {
        let cb = CircuitBreaker::new(3, 1, Duration::from_secs(30));

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.record_failure();

        // Should now be open
        assert!(matches!(cb.state(), CircuitState::Open { .. }));
        assert!(cb.check().is_err());
    }

    #[test]
    fn test_circuit_breaker_success_resets_failure_count() {
        let cb = CircuitBreaker::new(3, 1, Duration::from_secs(30));

        cb.record_failure();
        cb.record_failure();
        cb.record_success(); // resets consecutive failures
        cb.record_failure();
        cb.record_failure();

        // Should still be closed (only 2 consecutive failures)
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_transitions_to_half_open() {
        let cb = CircuitBreaker::new(2, 1, Duration::from_millis(1));

        cb.record_failure();
        cb.record_failure();
        assert!(matches!(cb.state(), CircuitState::Open { .. }));

        // Wait for open_duration to elapse
        std::thread::sleep(Duration::from_millis(10));

        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn test_circuit_breaker_closes_from_half_open_on_success() {
        let cb = CircuitBreaker::new(2, 2, Duration::from_millis(1));

        // Open the circuit
        cb.record_failure();
        cb.record_failure();

        // Wait for half-open
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Two successes to close
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::HalfOpen); // need 2
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_reopens_from_half_open_on_failure() {
        let cb = CircuitBreaker::new(2, 2, Duration::from_millis(1));

        cb.record_failure();
        cb.record_failure();

        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Failure in half-open reopens
        cb.record_failure();
        assert!(matches!(cb.state(), CircuitState::Open { .. }));
    }

    #[test]
    fn test_circuit_breaker_check_returns_remaining_duration() {
        let cb = CircuitBreaker::new(1, 1, Duration::from_secs(60));
        cb.record_failure();

        let err = cb.check().expect_err("should be open");
        assert!(
            err.remaining <= Duration::from_secs(60),
            "remaining {:?} should be <= 60s",
            err.remaining
        );
        assert!(
            err.remaining > Duration::from_secs(50),
            "remaining {:?} should be > 50s",
            err.remaining
        );
    }

    #[test]
    fn test_circuit_breaker_manual_reset() {
        let cb = CircuitBreaker::new(1, 1, Duration::from_secs(300));
        cb.record_failure();
        assert!(matches!(cb.state(), CircuitState::Open { .. }));

        cb.reset();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.check().is_ok());
    }

    #[test]
    fn test_circuit_breaker_config_access() {
        let cb = CircuitBreaker::new(5, 3, Duration::from_secs(30));
        let cfg = cb.config();
        assert_eq!(cfg.failure_threshold, 5);
        assert_eq!(cfg.success_threshold, 3);
        assert_eq!(cfg.open_duration, Duration::from_secs(30));
    }

    // ──────────────────────── Bulkhead tests ────────────────────────────────

    #[tokio::test]
    async fn test_bulkhead_allows_within_limit() {
        let bh = Bulkhead::new(2, Duration::from_secs(1));
        let result = bh.execute(|| async { 42 }).await;
        assert_eq!(result.expect("should succeed"), 42);
    }

    #[tokio::test]
    async fn test_bulkhead_rejects_when_full() {
        let bh = Arc::new(Bulkhead::new(1, Duration::from_millis(50)));

        // Hold a permit with a long-running task
        let bh2 = bh.clone();
        let handle = tokio::spawn(async move {
            bh2.execute(|| async {
                tokio::time::sleep(Duration::from_secs(2)).await;
                "long"
            })
            .await
        });

        // Give the spawned task time to acquire the permit
        tokio::time::sleep(Duration::from_millis(10)).await;

        // This should fail because the single permit is held
        let result = bh.execute(|| async { "blocked" }).await;
        assert!(result.is_err(), "should be rejected");
        let err = result.unwrap_err();
        assert_eq!(err.waited, Duration::from_millis(50));

        handle.abort();
    }

    #[tokio::test]
    async fn test_bulkhead_concurrent_execution() {
        let bh = Arc::new(Bulkhead::new(3, Duration::from_secs(1)));
        let counter = Arc::new(AtomicU32::new(0));

        let mut handles = vec![];
        for _ in 0..3 {
            let bh = bh.clone();
            let counter = counter.clone();
            handles.push(tokio::spawn(async move {
                bh.execute(|| async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                })
                .await
            }));
        }

        for h in handles {
            let _ = h.await.expect("task should not panic");
        }

        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn test_bulkhead_reports_available_permits() {
        let bh = Bulkhead::new(10, Duration::from_secs(1));
        assert_eq!(bh.available_permits(), 10);
        assert_eq!(bh.max_concurrent(), 10);
    }

    // ────────────────────────── Timeout tests ───────────────────────────────

    #[tokio::test]
    async fn test_timeout_wrapper_succeeds_within_limit() {
        let tw = TimeoutWrapper::new(Duration::from_secs(1));
        let result: Result<&str, RetryError<String>> = tw.execute(|| async { Ok("fast") }).await;
        assert_eq!(result.expect("should succeed"), "fast");
    }

    #[tokio::test]
    async fn test_timeout_wrapper_times_out() {
        let tw = TimeoutWrapper::new(Duration::from_millis(10));
        let result: Result<&str, RetryError<String>> = tw
            .execute(|| async {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok("too slow")
            })
            .await;

        assert!(
            matches!(result, Err(RetryError::Timeout(_))),
            "expected Timeout, got {result:?}"
        );
    }

    #[tokio::test]
    async fn test_timeout_wrapper_propagates_inner_error() {
        let tw = TimeoutWrapper::new(Duration::from_secs(1));
        let result: Result<(), RetryError<String>> =
            tw.execute(|| async { Err("inner fail".to_string()) }).await;

        match result {
            Err(RetryError::Exhausted(e)) => assert_eq!(e.last_error, "inner fail"),
            other => panic!("expected Exhausted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_timeout_run_returns_elapsed() {
        let tw = TimeoutWrapper::new(Duration::from_millis(10));
        let result: Result<(), timeout::TimeoutError<String>> = tw
            .run(|| async {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok(())
            })
            .await;

        assert!(result.unwrap_err().is_timeout());
    }

    #[tokio::test]
    async fn test_timeout_run_returns_inner_error() {
        let tw = TimeoutWrapper::new(Duration::from_secs(1));
        let result: Result<(), timeout::TimeoutError<String>> =
            tw.run(|| async { Err("oops".to_string()) }).await;

        let err = result.unwrap_err();
        assert!(!err.is_timeout());
        assert_eq!(err.into_inner().expect("should have inner"), "oops");
    }

    // ───────────────────────── Error type tests ─────────────────────────────

    #[test]
    fn test_retry_exhausted_display() {
        let err = RetryExhausted {
            last_error: "boom",
            attempts: 3,
            total_elapsed: Duration::from_millis(150),
        };
        let msg = err.to_string();
        assert!(msg.contains("3 attempts"), "message: {msg}");
        assert!(msg.contains("boom"), "message: {msg}");
    }

    #[test]
    fn test_circuit_open_display() {
        let err = CircuitOpen {
            remaining: Duration::from_secs(25),
        };
        let msg = err.to_string();
        assert!(msg.contains("25s"), "message: {msg}");
    }

    #[test]
    fn test_bulkhead_full_display() {
        let err = BulkheadFull {
            waited: Duration::from_millis(500),
        };
        let msg = err.to_string();
        assert!(msg.contains("500ms"), "message: {msg}");
    }

    #[test]
    fn test_retry_error_variants() {
        let e1: RetryError<String> = RetryError::Timeout(Duration::from_secs(1));
        assert!(e1.to_string().contains("timed out"));

        let e2: RetryError<String> = RetryError::CircuitOpen(CircuitOpen {
            remaining: Duration::from_secs(10),
        });
        assert!(e2.to_string().contains("circuit breaker"));

        let e3: RetryError<String> = RetryError::BulkheadFull(BulkheadFull {
            waited: Duration::from_millis(100),
        });
        assert!(e3.to_string().contains("bulkhead"));
    }

    // ──────────────────────── Integration-style tests ───────────────────────

    #[tokio::test]
    async fn test_retry_with_exponential_backoff_timing() {
        let policy = RetryPolicy::exponential(2, Duration::from_millis(20));

        let start = Instant::now();
        let _result: Result<(), RetryExhausted<String>> =
            retry_with_policy(&policy, || async { Err("fail".to_string()) }).await;
        let elapsed = start.elapsed();

        // Expect: delay(0) = 20ms, delay(1) = 40ms -> ~60ms total
        assert!(
            elapsed >= Duration::from_millis(50),
            "elapsed {elapsed:?} should be >= 50ms for exponential backoff"
        );
    }

    #[tokio::test]
    async fn test_retry_zero_retries_executes_once() {
        let counter = Arc::new(AtomicU32::new(0));
        let policy = RetryPolicy::fixed(0, Duration::from_millis(1));

        let c = counter.clone();
        let result: Result<(), RetryExhausted<String>> = retry_with_policy(&policy, || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err("fail".to_string())
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_with_classifier_selective_retry() {
        let counter = Arc::new(AtomicU32::new(0));
        let policy = RetryPolicy::fixed(5, Duration::from_millis(1));

        // Only retry errors containing "transient"
        let classifier = classifier::PatternClassifier::new(vec!["transient".to_string()], vec![]);

        let c = counter.clone();
        let result: Result<&str, RetryExhausted<String>> =
            retry_with_policy_and_classifier(&policy, &classifier, &mut || {
                let c = c.clone();
                async move {
                    let attempt = c.fetch_add(1, Ordering::SeqCst);
                    if attempt == 0 {
                        Err("transient error".to_string())
                    } else {
                        Err("permanent error".to_string())
                    }
                }
            })
            .await;

        assert!(result.is_err());
        // Should have: attempt 0 (transient, retried) -> attempt 1 (permanent, stop)
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_circuit_breaker_from_config() {
        let config = circuit_breaker::CircuitBreakerConfig {
            failure_threshold: 10,
            success_threshold: 5,
            open_duration: Duration::from_secs(60),
        };
        let cb = CircuitBreaker::from_config(config);
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.config().failure_threshold, 10);
    }

    #[test]
    fn test_circuit_breaker_debug_impl() {
        let cb = CircuitBreaker::new(3, 2, Duration::from_secs(10));
        let debug_str = format!("{cb:?}");
        assert!(debug_str.contains("CircuitBreaker"));
        assert!(debug_str.contains("Closed"));
    }

    #[tokio::test]
    async fn test_bulkhead_releases_permits_after_completion() {
        let bh = Arc::new(Bulkhead::new(1, Duration::from_secs(1)));

        // First call succeeds and releases permit
        let result1 = bh.execute(|| async { 1 }).await;
        assert!(result1.is_ok());

        // Second call should also succeed since permit was released
        let result2 = bh.execute(|| async { 2 }).await;
        assert!(result2.is_ok());
        assert_eq!(result2.expect("ok"), 2);
    }
}
