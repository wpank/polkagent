//! Health check aggregation for the Polkagent platform.
//!
//! This crate provides liveness, readiness, and dependency health monitoring
//! with concurrent check execution and configurable timeouts.
//!
//! # Overview
//!
//! The core abstraction is the [`HealthCheck`] trait. Implement it for any
//! dependency you want to monitor, register checks with the
//! [`HealthAggregator`], and query aggregated status at any time.
//!
//! Built-in checks are provided for common dependencies:
//!
//! - [`TcpCheck`] — verifies a TCP port is reachable.
//! - [`HttpCheck`] — verifies an HTTP endpoint is reachable.
//! - [`SqliteCheck`] — verifies a `SQLite` database file exists.
//!
//! Higher-level probes compose these checks:
//!
//! - [`LivenessProbe`] — simple "is the process alive" signal.
//! - [`ReadinessProbe`] — runs critical dependencies before declaring ready.
//!
//! The [`HealthReporter`] runs checks periodically in a background task and
//! caches the latest [`OverallHealth`] result for consumption by a `/health`
//! endpoint or similar.
//!
//! # Example
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use polkagent_health::{
//!     HealthAggregator, LivenessProbe, ReadinessProbe,
//!     TcpCheck, SqliteCheck, CheckSeverity,
//! };
//!
//! let aggregator = HealthAggregator::new("my-app-0.1.0");
//!
//! // Liveness: always up unless explicitly killed.
//! aggregator.register(Arc::new(LivenessProbe::new()));
//!
//! // Database check (critical).
//! aggregator.register(Arc::new(SqliteCheck::new(
//!     "sqlite", "/data/app.db", CheckSeverity::Critical,
//! )));
//!
//! // Chain node (advisory).
//! aggregator.register(Arc::new(TcpCheck::new(
//!     "chain-node", "127.0.0.1", 9944, CheckSeverity::Advisory,
//! )));
//!
//! let health = aggregator.check_all().await;
//! println!("{}", serde_json::to_string_pretty(&health)?);
//! ```

pub mod aggregator;
pub mod check;
pub mod dependency;
pub mod error;
pub mod liveness;
pub mod readiness;
pub mod reporter;
pub mod types;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use aggregator::HealthAggregator;
pub use check::HealthCheck;
pub use dependency::{HttpCheck, SqliteCheck, TcpCheck};
pub use error::HealthError;
pub use liveness::LivenessProbe;
pub use readiness::ReadinessProbe;
pub use reporter::{HealthReporter, ReporterConfig};
pub use types::{CheckSeverity, HealthStatus, OverallHealth, Status};

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use std::sync::Arc;
    use std::time::Duration;

    // -----------------------------------------------------------------------
    // Integration-style tests that exercise the public API across modules
    // -----------------------------------------------------------------------

    struct CountingCheck {
        name: String,
        counter: Arc<parking_lot::Mutex<u32>>,
    }

    impl CountingCheck {
        fn new(name: &str) -> (Self, Arc<parking_lot::Mutex<u32>>) {
            let counter = Arc::new(parking_lot::Mutex::new(0));
            (
                Self {
                    name: name.into(),
                    counter: Arc::clone(&counter),
                },
                counter,
            )
        }
    }

    #[async_trait]
    impl HealthCheck for CountingCheck {
        async fn check(&self) -> HealthStatus {
            *self.counter.lock() += 1;
            HealthStatus {
                name: self.name.clone(),
                status: Status::Up,
                latency_ms: 0,
                details: None,
                checked_at: Utc::now(),
            }
        }
        fn name(&self) -> &str {
            &self.name
        }
        fn severity(&self) -> CheckSeverity {
            CheckSeverity::Advisory
        }
    }

    #[tokio::test]
    async fn end_to_end_aggregator_with_liveness_and_readiness() {
        let agg = HealthAggregator::new("integration-test-0.1.0");

        let liveness = Arc::new(LivenessProbe::new());
        let readiness = Arc::new(ReadinessProbe::new());

        // SQLite check against Cargo.toml (always exists in test cwd).
        let sqlite = Arc::new(SqliteCheck::new("test-db", "Cargo.toml", CheckSeverity::Critical));
        readiness.add_dependency(sqlite.clone());

        agg.register(liveness.clone());
        agg.register(readiness.clone());
        agg.register(sqlite);

        let health = agg.check_all().await;
        assert_eq!(health.status, Status::Up);
        assert_eq!(health.checks.len(), 3);
        assert_eq!(health.version, "integration-test-0.1.0");
    }

    #[tokio::test]
    async fn overall_health_serializes_to_json() {
        let agg = HealthAggregator::new("v1.2.3");
        agg.register(Arc::new(LivenessProbe::new()));

        let health = agg.check_all().await;
        let json = serde_json::to_string(&health).expect("should serialize");
        assert!(json.contains("\"status\":\"up\""));
        assert!(json.contains("\"version\":\"v1.2.3\""));
        assert!(json.contains("\"uptime_seconds\""));
    }

    #[tokio::test]
    async fn overall_health_deserializes_roundtrip() {
        let agg = HealthAggregator::new("v1");
        agg.register(Arc::new(LivenessProbe::new()));

        let health = agg.check_all().await;
        let json = serde_json::to_string(&health).expect("should serialize");
        let deserialized: OverallHealth =
            serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(deserialized.status, health.status);
        assert_eq!(deserialized.version, "v1");
        assert_eq!(deserialized.checks.len(), 1);
    }

    #[tokio::test]
    async fn check_invocation_count() {
        let agg = HealthAggregator::new("v1");

        let (check, counter) = CountingCheck::new("counter");
        agg.register(Arc::new(check));

        agg.check_all().await;
        agg.check_all().await;
        agg.check_all().await;

        assert_eq!(*counter.lock(), 3);
    }

    #[tokio::test]
    async fn liveness_mark_dead_propagates_to_aggregator() {
        let agg = HealthAggregator::new("v1");
        let liveness = Arc::new(LivenessProbe::new());
        agg.register(liveness.clone());

        let h1 = agg.check_all().await;
        assert_eq!(h1.status, Status::Up);

        liveness.mark_dead();

        let h2 = agg.check_all().await;
        assert_eq!(h2.status, Status::Down);
    }

    #[tokio::test]
    async fn readiness_with_down_dependency_propagates() {
        struct DownCheck;

        #[async_trait]
        impl HealthCheck for DownCheck {
            async fn check(&self) -> HealthStatus {
                HealthStatus {
                    name: "down-dep".into(),
                    status: Status::Down,
                    latency_ms: 0,
                    details: None,
                    checked_at: Utc::now(),
                }
            }
            fn name(&self) -> &str { "down-dep" }
            fn severity(&self) -> CheckSeverity { CheckSeverity::Critical }
        }

        let agg = HealthAggregator::new("v1");
        let readiness = Arc::new(ReadinessProbe::new());
        readiness.add_dependency(Arc::new(DownCheck));
        agg.register(readiness);

        let health = agg.check_all().await;
        // Readiness itself is critical, and it reports Down because its dep is down.
        assert_eq!(health.status, Status::Down);
    }

    #[tokio::test]
    async fn reporter_caches_last_result() {
        let agg = Arc::new(HealthAggregator::new("v1"));
        agg.register(Arc::new(LivenessProbe::new()));

        let config = ReporterConfig::default()
            .with_interval(Duration::from_millis(30))
            .with_check_on_start(true);
        let reporter = HealthReporter::new(agg, config);
        let handle = reporter.start().expect("should start");

        tokio::time::sleep(Duration::from_millis(50)).await;

        let cached = reporter.latest();
        assert!(cached.is_some());
        let health = cached.expect("is some");
        assert_eq!(health.status, Status::Up);

        reporter.stop().expect("should stop");
        let _ = handle.await;
    }

    #[test]
    fn status_serde_variants() {
        assert_eq!(
            serde_json::to_string(&Status::Up).expect("serialize"),
            "\"up\""
        );
        assert_eq!(
            serde_json::to_string(&Status::Down).expect("serialize"),
            "\"down\""
        );
        assert_eq!(
            serde_json::to_string(&Status::Degraded).expect("serialize"),
            "\"degraded\""
        );
    }

    #[test]
    fn severity_serde_variants() {
        assert_eq!(
            serde_json::to_string(&CheckSeverity::Critical).expect("serialize"),
            "\"critical\""
        );
        assert_eq!(
            serde_json::to_string(&CheckSeverity::Advisory).expect("serialize"),
            "\"advisory\""
        );
    }

    #[test]
    fn health_error_display() {
        let err = HealthError::Timeout {
            name: "db".into(),
            timeout_ms: 5000,
        };
        assert!(err.to_string().contains("db"));
        assert!(err.to_string().contains("5000"));

        let err = HealthError::CheckNotFound {
            name: "foo".into(),
        };
        assert!(err.to_string().contains("foo"));

        let err = HealthError::AlreadyRunning;
        assert!(err.to_string().contains("already running"));

        let err = HealthError::NotRunning;
        assert!(err.to_string().contains("not running"));

        let err = HealthError::Internal("boom".into());
        assert!(err.to_string().contains("boom"));
    }
}
