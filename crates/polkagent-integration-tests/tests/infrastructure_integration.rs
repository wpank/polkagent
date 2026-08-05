//! Cross-crate integration tests for infrastructure primitives.
//!
//! These tests verify that the cache, rate-limit, audit, scheduler, batch, and
//! retry crates work correctly when used together across crate boundaries.

// This assertion-oriented integration target uses `expect`/`unwrap` to identify
// the exact cross-crate fixture step or behavioral contract that failed.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

// =========================================================================
// 1. Cache tests
// =========================================================================

mod cache {
    use super::*;
    use polkagent_cache::{CacheKey, CacheStore, CachedValue, InMemoryCache};

    /// Store 100 items in a cache with capacity 50 and verify LRU eviction
    /// keeps the cache bounded and the most recent key survives.
    #[tokio::test]
    async fn lru_eviction_with_100_items() {
        let cache = InMemoryCache::new(50);

        // Insert 100 items into a cache that can hold 50.
        for i in 0..100u32 {
            let key = CacheKey::new("evict", format!("k{i:04}"));
            cache
                .set(key, CachedValue::new(json!(i)), None)
                .await
                .expect("set should succeed");
        }

        // The cache should hold at most 50 entries.
        let size = cache.size().await;
        assert!(
            size <= 50,
            "cache size {size} should not exceed capacity 50"
        );
        assert!(size > 0, "cache should not be empty");

        // The very first key should have been evicted.
        let k0 = CacheKey::new("evict", "k0000");
        assert!(!cache.contains(&k0).await, "k0000 should have been evicted");

        // The most recent key should still be present (it was the last
        // inserted so it sits at the head of the LRU list).
        let k99 = CacheKey::new("evict", "k0099");
        assert!(cache.contains(&k99).await, "k0099 should still be present");
    }

    /// Entries with a TTL of 0ms should expire almost immediately.
    #[tokio::test]
    async fn ttl_expiry() {
        let cache = InMemoryCache::new(100);
        let key = CacheKey::new("ttl", "ephemeral");

        // Insert with zero TTL.
        cache
            .set(
                key.clone(),
                CachedValue::new(json!("gone")),
                Some(Duration::from_millis(0)),
            )
            .await
            .expect("set");

        // Give the TTL a moment to take effect.
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert!(cache.get(&key).await.is_none(), "entry should have expired");
    }

    /// Cache stats track hits and misses accurately across operations.
    #[tokio::test]
    async fn cache_stats_hit_miss_tracking() {
        let cache = InMemoryCache::new(100);
        let key = CacheKey::new("stats", "item");

        // Miss.
        cache.get(&key).await;
        assert_eq!(cache.stats().misses(), 1);
        assert_eq!(cache.stats().hits(), 0);

        // Insert and hit.
        cache
            .set(key.clone(), CachedValue::new(json!(42)), None)
            .await
            .expect("set");
        cache.get(&key).await;
        assert_eq!(cache.stats().hits(), 1);
        assert_eq!(cache.stats().misses(), 1);
    }
}

// =========================================================================
// 2. Rate-limit tests
// =========================================================================

mod rate_limit {
    use super::*;
    use polkagent_rate_limit::{RateLimiter, TokenBucket};

    /// Create a tight token bucket and exhaust it to verify throttling.
    #[test]
    fn token_bucket_throttles_at_high_rate() {
        // 5 tokens capacity, refilling 1 token per second.
        let bucket = TokenBucket::new(5, 1.0, Duration::from_secs(1));

        // First 5 requests should be allowed (burst).
        for i in 0..5 {
            let result = bucket.try_acquire("client-1", 1);
            assert!(result.allowed, "request {i} should be allowed");
        }

        // The 6th request should be denied (no tokens left, refill is slow).
        let result = bucket.try_acquire("client-1", 1);
        assert!(
            !result.allowed,
            "6th request should be denied (bucket empty)"
        );
        assert!(
            result.retry_after.is_some(),
            "denied result should include retry_after"
        );
    }

    /// Verify that remaining tokens are reported correctly.
    #[test]
    fn token_bucket_remaining_count() {
        let bucket = TokenBucket::new(10, 0.0, Duration::from_secs(1));

        assert_eq!(bucket.remaining("any"), 10);

        bucket.try_acquire("any", 3);
        assert_eq!(bucket.remaining("any"), 7);

        bucket.try_acquire("any", 7);
        assert_eq!(bucket.remaining("any"), 0);
    }
}

// =========================================================================
// 3. Audit tests
// =========================================================================

mod audit {
    use super::*;
    use polkagent_audit::{
        ActionOutcome, ActorInfo, AuditAction, AuditLogger, AuditQuery, InMemoryAuditStore,
        ResourceInfo,
    };

    /// Log 10 audit entries and verify the integrity hash chain is valid.
    #[tokio::test]
    async fn ten_entry_integrity_chain() {
        let store = Arc::new(InMemoryAuditStore::new());
        let logger = AuditLogger::new(store.clone());

        for i in 0..10 {
            logger
                .log(
                    ActorInfo::agent(format!("agent-{i}")),
                    AuditAction::ToolInvoked,
                    ResourceInfo::new("tool", format!("tool-{i}")),
                    ActionOutcome::Success,
                    json!({"iteration": i}),
                )
                .await
                .expect("log should succeed");
        }

        // Verify count.
        assert_eq!(logger.count().await.expect("count"), 10);

        // Verify chain integrity.
        store
            .verify_integrity()
            .expect("integrity chain should be valid");

        // Verify each entry has a unique non-empty hash.
        let entries = store.snapshot();
        let hashes: Vec<&str> = entries.iter().map(|e| e.integrity_hash.as_str()).collect();
        for (i, hash) in hashes.iter().enumerate() {
            assert!(!hash.is_empty(), "entry {i} should have a hash");
            assert_eq!(hash.len(), 64, "BLAKE3 hash should be 64 hex chars");
        }

        // All hashes should be distinct.
        let mut unique = hashes.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), 10, "all 10 hashes should be distinct");
    }

    /// Query filtering by actor works across the crate boundary.
    #[tokio::test]
    async fn query_filters_by_actor() {
        let store = Arc::new(InMemoryAuditStore::new());
        let logger = AuditLogger::new(store.clone());

        // Log entries for two different actors.
        for _ in 0..3 {
            logger
                .log(
                    ActorInfo::agent("alice"),
                    AuditAction::RunStarted,
                    ResourceInfo::new("run", "r-1"),
                    ActionOutcome::Success,
                    serde_json::Value::Null,
                )
                .await
                .expect("log");
        }
        for _ in 0..2 {
            logger
                .log(
                    ActorInfo::agent("bob"),
                    AuditAction::ToolInvoked,
                    ResourceInfo::new("tool", "t-1"),
                    ActionOutcome::Success,
                    serde_json::Value::Null,
                )
                .await
                .expect("log");
        }

        let q = AuditQuery::new().actor("alice").build();
        let results = logger.query(&q).await.expect("query");
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|e| e.actor.id == "alice"));
    }
}

// =========================================================================
// 4. Scheduler tests
// =========================================================================

mod scheduler {
    use super::*;
    use polkagent_core::AgentId;
    use polkagent_scheduler::executor::NoOpExecutor;
    use polkagent_scheduler::{
        InMemoryTaskStore, Schedule, ScheduledTask, Scheduler, TaskAction, TaskStatus,
    };

    /// Register a task due in the past and verify `poll_once` executes it.
    #[tokio::test]
    async fn scheduler_fires_due_task() {
        let store = Arc::new(InMemoryTaskStore::new());
        let executor = Arc::new(NoOpExecutor);
        let scheduler = Scheduler::new(store.clone(), executor, Duration::from_millis(50));

        // Create a task that is already due (scheduled in the past).
        let past = chrono::Utc::now() - chrono::Duration::seconds(10);
        let mut task = ScheduledTask::new(
            "integration-test-task",
            Schedule::Once { at: past },
            AgentId::new(),
            TaskAction::Custom {
                handler: "test-handler".into(),
                payload: json!({"key": "value"}),
            },
        );
        // Force next_run_at to the past so it is picked up.
        task.next_run_at = Some(past);

        scheduler.add_task(task).await.expect("add_task");

        // Poll once and verify it executed.
        let runner = scheduler.runner();
        let executed = runner.poll_once().await.expect("poll_once");
        assert_eq!(executed, 1, "should have executed exactly 1 task");

        // The task should now be completed.
        let tasks = scheduler.list_tasks().await.expect("list_tasks");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, TaskStatus::Completed);
    }

    /// Future-scheduled tasks should not be executed by `poll_once`.
    #[tokio::test]
    async fn scheduler_skips_future_tasks() {
        let store = Arc::new(InMemoryTaskStore::new());
        let executor = Arc::new(NoOpExecutor);
        let scheduler = Scheduler::new(store, executor, Duration::from_millis(50));

        let future = chrono::Utc::now() + chrono::Duration::hours(24);
        let task = ScheduledTask::new(
            "future-task",
            Schedule::Once { at: future },
            AgentId::new(),
            TaskAction::Custom {
                handler: "noop".into(),
                payload: serde_json::Value::Null,
            },
        );
        scheduler.add_task(task).await.expect("add_task");

        let runner = scheduler.runner();
        let executed = runner.poll_once().await.expect("poll_once");
        assert_eq!(executed, 0, "future task should not be executed");
    }
}

// =========================================================================
// 5. Batch tests
// =========================================================================

mod batch {
    use super::*;
    use polkagent_batch::{Batch, BatchConfig, BatchProcessor};

    /// Submit 50 items and verify all are processed successfully.
    #[tokio::test]
    async fn process_fifty_items() {
        let config = BatchConfig::parallel(4).with_max_size(0);
        let mut batch: Batch<u32> = Batch::new(config);

        for i in 0..50 {
            assert!(batch.push(i), "push {i} should succeed");
        }
        assert_eq!(batch.len(), 50);

        let counter = Arc::new(AtomicU32::new(0));
        let c = Arc::clone(&counter);

        let result = BatchProcessor::process(&mut batch, move |n| {
            let c = Arc::clone(&c);
            async move {
                c.fetch_add(1, Ordering::Relaxed);
                Ok(json!(n * 2))
            }
        })
        .await
        .expect("processing should succeed");

        assert_eq!(result.total, 50);
        assert_eq!(result.succeeded, 50);
        assert_eq!(result.failed, 0);
        assert!(result.all_succeeded());
        assert_eq!(counter.load(Ordering::Relaxed), 50);
    }

    /// Verify that failing items are recorded but processing continues
    /// under `SkipFailed` policy.
    #[tokio::test]
    async fn batch_skip_failed_policy() {
        let config = polkagent_batch::BatchConfig::sequential()
            .with_max_size(0)
            .with_error_policy(polkagent_batch::ErrorPolicy::SkipFailed);
        let mut batch: Batch<u32> = Batch::new(config);

        for i in 0..10 {
            batch.push(i);
        }

        let result = BatchProcessor::process(&mut batch, |n| async move {
            if n % 5 == 0 {
                Err(format!("item {n} failed"))
            } else {
                Ok(json!(n))
            }
        })
        .await
        .expect("processing should succeed");

        // Items 0 and 5 fail.
        assert_eq!(result.total, 10);
        assert_eq!(result.succeeded, 8);
        assert_eq!(result.failed, 2);
        assert!(result.has_failures());
    }
}

// =========================================================================
// 6. Retry tests
// =========================================================================

mod retry {
    use super::*;
    use polkagent_retry::{retry_with_policy, RetryPolicy};

    /// A function that fails twice then succeeds should be retried correctly.
    #[tokio::test]
    async fn retry_succeeds_after_two_failures() {
        let counter = Arc::new(AtomicU32::new(0));
        let policy = RetryPolicy::fixed(3, Duration::from_millis(1));

        let c = counter.clone();
        let result = retry_with_policy(&policy, || {
            let c = c.clone();
            async move {
                let attempt = c.fetch_add(1, Ordering::SeqCst);
                if attempt < 2 {
                    Err(format!("transient failure #{attempt}"))
                } else {
                    Ok("recovered")
                }
            }
        })
        .await;

        assert_eq!(result.expect("should succeed"), "recovered");
        assert_eq!(counter.load(Ordering::SeqCst), 3); // 2 failures + 1 success
    }

    /// When retries are exhausted, the error includes the correct attempt count.
    #[tokio::test]
    async fn retry_exhausted_reports_attempts() {
        let policy = RetryPolicy::fixed(2, Duration::from_millis(1));

        let result: Result<(), _> =
            retry_with_policy(&policy, || async { Err("always fails".to_string()) }).await;

        let err = result.expect_err("should fail");
        assert_eq!(err.attempts, 3); // 1 initial + 2 retries
        assert_eq!(err.last_error, "always fails");
    }
}

// =========================================================================
// 7. Combined test: Cache + Audit
// =========================================================================

mod combined {
    use super::*;
    use polkagent_audit::{
        ActionOutcome, ActorInfo, AuditAction, AuditLogger, AuditQuery, InMemoryAuditStore,
        ResourceInfo,
    };
    use polkagent_cache::{CacheKey, CacheStore, CachedValue, InMemoryCache};

    /// Simulate a cache-aside pattern where every cache hit and miss generates
    /// an audit trail entry. Verify that the audit log accurately captures the
    /// cache access pattern and maintains chain integrity.
    #[tokio::test]
    async fn cache_operations_generate_audit_trail() {
        // Set up infrastructure.
        let cache = Arc::new(InMemoryCache::new(100));
        let audit_store = Arc::new(InMemoryAuditStore::new());
        let logger = AuditLogger::new(audit_store.clone());

        let keys: Vec<CacheKey> = (0..5)
            .map(|i| CacheKey::new("combined", format!("item-{i}")))
            .collect();

        // Phase 1: All lookups are misses; log each miss then populate the cache.
        for (i, key) in keys.iter().enumerate() {
            let result = cache.get(key).await;
            let outcome = if result.is_some() { "hit" } else { "miss" };

            logger
                .log(
                    ActorInfo::agent("cache-agent"),
                    AuditAction::ToolInvoked,
                    ResourceInfo::new("cache", format!("item-{i}")),
                    ActionOutcome::Success,
                    json!({"operation": "get", "result": outcome}),
                )
                .await
                .expect("audit log");

            // Populate cache on miss.
            if result.is_none() {
                cache
                    .set(key.clone(), CachedValue::new(json!(i)), None)
                    .await
                    .expect("cache set");
            }
        }

        // Phase 2: All lookups should now be hits.
        for (i, key) in keys.iter().enumerate() {
            let result = cache.get(key).await;
            let outcome = if result.is_some() { "hit" } else { "miss" };

            logger
                .log(
                    ActorInfo::agent("cache-agent"),
                    AuditAction::ToolInvoked,
                    ResourceInfo::new("cache", format!("item-{i}")),
                    ActionOutcome::Success,
                    json!({"operation": "get", "result": outcome}),
                )
                .await
                .expect("audit log");
        }

        // Verify audit log.
        assert_eq!(
            logger.count().await.expect("count"),
            10,
            "5 misses + 5 hits = 10 audit entries"
        );

        // Verify chain integrity across all entries.
        audit_store
            .verify_integrity()
            .expect("audit chain should be valid");

        // Query entries for the cache-agent and verify all 10 are present.
        let q = AuditQuery::new().actor("cache-agent").build();
        let entries = logger.query(&q).await.expect("query");
        assert_eq!(entries.len(), 10);

        // Verify the first 5 entries record misses.
        for entry in entries.iter().take(5) {
            assert_eq!(entry.context["result"], "miss");
        }

        // Verify the last 5 entries record hits.
        for entry in entries.iter().skip(5) {
            assert_eq!(entry.context["result"], "hit");
        }

        // Verify cache stats match expectations.
        assert_eq!(cache.stats().misses(), 5);
        assert_eq!(cache.stats().hits(), 5);
    }

    /// Audit entries are created for cache eviction scenarios and the chain
    /// remains valid.
    #[tokio::test]
    async fn cache_eviction_audit_trail() {
        let cache = Arc::new(InMemoryCache::new(3));
        let audit_store = Arc::new(InMemoryAuditStore::new());
        let logger = AuditLogger::new(audit_store.clone());

        // Fill the cache to capacity.
        for i in 0..3 {
            let key = CacheKey::new("evict", format!("k{i}"));
            cache
                .set(key, CachedValue::new(json!(i)), None)
                .await
                .expect("set");

            logger
                .log(
                    ActorInfo::agent("evict-agent"),
                    AuditAction::ToolInvoked,
                    ResourceInfo::new("cache", format!("k{i}")),
                    ActionOutcome::Success,
                    json!({"operation": "insert"}),
                )
                .await
                .expect("audit log");
        }

        // Insert one more, causing eviction.
        let overflow_key = CacheKey::new("evict", "k3");
        cache
            .set(overflow_key, CachedValue::new(json!(3)), None)
            .await
            .expect("set");

        // Check that the oldest key was evicted.
        let k0 = CacheKey::new("evict", "k0");
        let evicted = cache.get(&k0).await.is_none();

        logger
            .log(
                ActorInfo::agent("evict-agent"),
                AuditAction::ToolInvoked,
                ResourceInfo::new("cache", "k3"),
                ActionOutcome::Success,
                json!({"operation": "insert", "eviction_occurred": evicted}),
            )
            .await
            .expect("audit log");

        assert!(evicted, "k0 should have been evicted");
        assert_eq!(logger.count().await.expect("count"), 4);
        audit_store
            .verify_integrity()
            .expect("chain should be valid");
    }
}
