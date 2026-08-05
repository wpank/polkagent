//! [`HealthAggregator`] — runs registered checks concurrently and computes
//! overall system status.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use parking_lot::RwLock;
use tracing::{debug, warn};

use crate::check::HealthCheck;
use crate::error::HealthError;
use crate::types::{CheckSeverity, HealthStatus, OverallHealth, Status};

/// A registered check entry.
struct RegisteredCheck {
    check: Arc<dyn HealthCheck>,
    severity: CheckSeverity,
    timeout: Duration,
}

/// Aggregates multiple [`HealthCheck`]s, runs them concurrently, and computes
/// the [`OverallHealth`] of the system.
pub struct HealthAggregator {
    checks: RwLock<Vec<RegisteredCheck>>,
    start_time: Instant,
    version: String,
    default_timeout: Duration,
}

impl HealthAggregator {
    /// Create a new aggregator.
    ///
    /// `version` is the application version string reported in
    /// [`OverallHealth`].
    pub fn new(version: impl Into<String>) -> Self {
        Self {
            checks: RwLock::new(Vec::new()),
            start_time: Instant::now(),
            version: version.into(),
            default_timeout: Duration::from_secs(5),
        }
    }

    /// Set the default timeout applied to any check that does not specify one.
    #[must_use]
    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Register a health check using the severity reported by the check itself,
    /// and the aggregator's default timeout.
    pub fn register(&self, check: Arc<dyn HealthCheck>) {
        let severity = check.severity();
        let timeout = self.default_timeout;
        self.checks.write().push(RegisteredCheck {
            check,
            severity,
            timeout,
        });
    }

    /// Register a health check with an explicit timeout.
    pub fn register_with_timeout(&self, check: Arc<dyn HealthCheck>, timeout: Duration) {
        let severity = check.severity();
        self.checks.write().push(RegisteredCheck {
            check,
            severity,
            timeout,
        });
    }

    /// How many checks are registered.
    pub fn check_count(&self) -> usize {
        self.checks.read().len()
    }

    /// Run all registered checks concurrently (each with its own timeout) and
    /// return the aggregated [`OverallHealth`].
    pub async fn check_all(&self) -> OverallHealth {
        let entries: Vec<(Arc<dyn HealthCheck>, CheckSeverity, Duration)> = {
            let guard = self.checks.read();
            guard
                .iter()
                .map(|r| (Arc::clone(&r.check), r.severity, r.timeout))
                .collect()
        };

        let mut handles = Vec::with_capacity(entries.len());
        for (check, severity, timeout) in entries {
            handles.push(tokio::spawn(async move {
                let result = run_check_with_timeout(&*check, timeout).await;
                (result, severity)
            }));
        }

        let mut results: Vec<HealthStatus> = Vec::with_capacity(handles.len());
        let mut severities: Vec<CheckSeverity> = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.await {
                Ok((status, severity)) => {
                    severities.push(severity);
                    results.push(status);
                }
                Err(e) => {
                    warn!(error = %e, "health check task panicked");
                    results.push(HealthStatus {
                        name: "unknown".into(),
                        status: Status::Down,
                        latency_ms: 0,
                        details: Some(serde_json::json!({ "error": e.to_string() })),
                        checked_at: Utc::now(),
                    });
                    severities.push(CheckSeverity::Critical);
                }
            }
        }

        let overall = compute_overall_status(&results, &severities);
        let uptime = self.start_time.elapsed().as_secs_f64();

        OverallHealth {
            status: overall,
            checks: results,
            uptime_seconds: uptime,
            version: self.version.clone(),
        }
    }

    /// Run a single named check.
    pub async fn check_one(&self, name: &str) -> Result<HealthStatus, HealthError> {
        let entry = {
            let guard = self.checks.read();
            guard
                .iter()
                .find(|r| r.check.name() == name)
                .map(|r| (Arc::clone(&r.check), r.timeout))
        };

        match entry {
            Some((check, timeout)) => Ok(run_check_with_timeout(&*check, timeout).await),
            None => Err(HealthError::CheckNotFound {
                name: name.to_string(),
            }),
        }
    }
}

/// Run a single check with a timeout. If the check exceeds the timeout, a
/// `Down` status with a timeout detail is returned.
async fn run_check_with_timeout(check: &dyn HealthCheck, timeout: Duration) -> HealthStatus {
    let name = check.name().to_string();
    let start = Instant::now();
    #[allow(clippy::cast_possible_truncation)]
    let timeout_ms = timeout.as_millis() as u64;

    if let Ok(status) = tokio::time::timeout(timeout, check.check()).await {
        debug!(check = %name, status = ?status.status, latency_ms = status.latency_ms, "health check completed");
        status
    } else {
        warn!(check = %name, timeout_ms, "health check timed out");
        #[allow(clippy::cast_possible_truncation)]
        let latency_ms = start.elapsed().as_millis() as u64;
        HealthStatus {
            name,
            status: Status::Down,
            latency_ms,
            details: Some(serde_json::json!({
                "error": format!("timed out after {timeout_ms}ms"),
            })),
            checked_at: Utc::now(),
        }
    }
}

/// Compute the overall system status from individual results and their
/// severities.
///
/// Rules:
/// - If any **critical** check is `Down`, overall is `Down`.
/// - If any check (critical or advisory) is `Degraded`, overall is `Degraded`.
/// - Otherwise `Up`.
fn compute_overall_status(results: &[HealthStatus], severities: &[CheckSeverity]) -> Status {
    let mut overall = Status::Up;

    for (status, severity) in results.iter().zip(severities.iter()) {
        match (status.status, severity) {
            (Status::Down, CheckSeverity::Critical) => return Status::Down,
            (Status::Degraded, _) => overall = Status::Degraded,
            _ => {}
        }
    }

    overall
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::expect_used,
        reason = "health aggregation tests use fail-fast assertions for deterministic check fixtures"
    )]

    use super::*;
    use crate::check::HealthCheck;
    use async_trait::async_trait;
    use std::time::Duration;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    struct FixedCheck {
        check_name: String,
        status: Status,
        severity: CheckSeverity,
        delay: Duration,
    }

    impl FixedCheck {
        fn new(name: &str, status: Status, severity: CheckSeverity) -> Self {
            Self {
                check_name: name.into(),
                status,
                severity,
                delay: Duration::ZERO,
            }
        }

        fn with_delay(mut self, d: Duration) -> Self {
            self.delay = d;
            self
        }
    }

    #[async_trait]
    impl HealthCheck for FixedCheck {
        async fn check(&self) -> HealthStatus {
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            HealthStatus {
                name: self.check_name.clone(),
                status: self.status,
                latency_ms: u64::try_from(self.delay.as_millis()).unwrap_or(u64::MAX),
                details: None,
                checked_at: Utc::now(),
            }
        }

        fn name(&self) -> &str {
            &self.check_name
        }

        fn severity(&self) -> CheckSeverity {
            self.severity
        }
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn empty_aggregator_is_up() {
        let agg = HealthAggregator::new("test-0.1.0");
        let health = agg.check_all().await;
        assert_eq!(health.status, Status::Up);
        assert!(health.checks.is_empty());
    }

    #[tokio::test]
    async fn all_up_overall_up() {
        let agg = HealthAggregator::new("v1");
        agg.register(Arc::new(FixedCheck::new(
            "a",
            Status::Up,
            CheckSeverity::Critical,
        )));
        agg.register(Arc::new(FixedCheck::new(
            "b",
            Status::Up,
            CheckSeverity::Advisory,
        )));

        let health = agg.check_all().await;
        assert_eq!(health.status, Status::Up);
        assert_eq!(health.checks.len(), 2);
    }

    #[tokio::test]
    async fn critical_down_makes_overall_down() {
        let agg = HealthAggregator::new("v1");
        agg.register(Arc::new(FixedCheck::new(
            "db",
            Status::Down,
            CheckSeverity::Critical,
        )));
        agg.register(Arc::new(FixedCheck::new(
            "cache",
            Status::Up,
            CheckSeverity::Advisory,
        )));

        let health = agg.check_all().await;
        assert_eq!(health.status, Status::Down);
    }

    #[tokio::test]
    async fn advisory_down_does_not_make_overall_down() {
        let agg = HealthAggregator::new("v1");
        agg.register(Arc::new(FixedCheck::new(
            "db",
            Status::Up,
            CheckSeverity::Critical,
        )));
        agg.register(Arc::new(FixedCheck::new(
            "cache",
            Status::Down,
            CheckSeverity::Advisory,
        )));

        let health = agg.check_all().await;
        assert_ne!(health.status, Status::Down);
    }

    #[tokio::test]
    async fn any_degraded_makes_overall_degraded() {
        let agg = HealthAggregator::new("v1");
        agg.register(Arc::new(FixedCheck::new(
            "db",
            Status::Up,
            CheckSeverity::Critical,
        )));
        agg.register(Arc::new(FixedCheck::new(
            "cache",
            Status::Degraded,
            CheckSeverity::Advisory,
        )));

        let health = agg.check_all().await;
        assert_eq!(health.status, Status::Degraded);
    }

    #[tokio::test]
    async fn critical_degraded_makes_overall_degraded() {
        let agg = HealthAggregator::new("v1");
        agg.register(Arc::new(FixedCheck::new(
            "db",
            Status::Degraded,
            CheckSeverity::Critical,
        )));

        let health = agg.check_all().await;
        assert_eq!(health.status, Status::Degraded);
    }

    #[tokio::test]
    async fn critical_down_wins_over_degraded() {
        let agg = HealthAggregator::new("v1");
        agg.register(Arc::new(FixedCheck::new(
            "db",
            Status::Down,
            CheckSeverity::Critical,
        )));
        agg.register(Arc::new(FixedCheck::new(
            "api",
            Status::Degraded,
            CheckSeverity::Advisory,
        )));

        let health = agg.check_all().await;
        assert_eq!(health.status, Status::Down);
    }

    #[tokio::test]
    async fn timeout_marks_check_as_down() {
        let agg = HealthAggregator::new("v1").with_default_timeout(Duration::from_millis(50));
        agg.register(Arc::new(
            FixedCheck::new("slow", Status::Up, CheckSeverity::Critical)
                .with_delay(Duration::from_secs(5)),
        ));

        let health = agg.check_all().await;
        assert_eq!(health.checks.len(), 1);
        assert_eq!(health.checks[0].status, Status::Down);
        assert!(health.checks[0].details.is_some());
    }

    #[tokio::test]
    async fn per_check_timeout() {
        let agg = HealthAggregator::new("v1").with_default_timeout(Duration::from_secs(30));
        agg.register_with_timeout(
            Arc::new(
                FixedCheck::new("slow", Status::Up, CheckSeverity::Advisory)
                    .with_delay(Duration::from_secs(5)),
            ),
            Duration::from_millis(50),
        );

        let health = agg.check_all().await;
        assert_eq!(health.checks[0].status, Status::Down);
    }

    #[tokio::test]
    async fn checks_run_concurrently() {
        let agg = HealthAggregator::new("v1");
        // Two checks each with 50ms delay — if sequential that's 100ms+.
        // If concurrent, total should be roughly 50ms.
        agg.register(Arc::new(
            FixedCheck::new("a", Status::Up, CheckSeverity::Critical)
                .with_delay(Duration::from_millis(50)),
        ));
        agg.register(Arc::new(
            FixedCheck::new("b", Status::Up, CheckSeverity::Critical)
                .with_delay(Duration::from_millis(50)),
        ));

        let start = Instant::now();
        let _health = agg.check_all().await;
        let elapsed = start.elapsed();

        // Allow generous margin but must be less than sequential (100ms).
        assert!(elapsed < Duration::from_millis(95), "elapsed: {elapsed:?}");
    }

    #[tokio::test]
    async fn check_one_found() {
        let agg = HealthAggregator::new("v1");
        agg.register(Arc::new(FixedCheck::new(
            "db",
            Status::Up,
            CheckSeverity::Critical,
        )));

        let result = agg.check_one("db").await;
        assert!(result.is_ok());
        assert_eq!(result.expect("is ok").status, Status::Up);
    }

    #[tokio::test]
    async fn check_one_not_found() {
        let agg = HealthAggregator::new("v1");
        let result = agg.check_one("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn check_count() {
        let agg = HealthAggregator::new("v1");
        assert_eq!(agg.check_count(), 0);
        agg.register(Arc::new(FixedCheck::new(
            "a",
            Status::Up,
            CheckSeverity::Critical,
        )));
        assert_eq!(agg.check_count(), 1);
        agg.register(Arc::new(FixedCheck::new(
            "b",
            Status::Up,
            CheckSeverity::Advisory,
        )));
        assert_eq!(agg.check_count(), 2);
    }

    #[tokio::test]
    async fn uptime_is_positive() {
        let agg = HealthAggregator::new("v1");
        tokio::time::sleep(Duration::from_millis(10)).await;
        let health = agg.check_all().await;
        assert!(health.uptime_seconds > 0.0);
    }

    #[tokio::test]
    async fn version_propagated() {
        let agg = HealthAggregator::new("my-app-2.3.4");
        let health = agg.check_all().await;
        assert_eq!(health.version, "my-app-2.3.4");
    }
}
