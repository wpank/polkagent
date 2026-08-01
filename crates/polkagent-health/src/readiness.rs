//! [`ReadinessProbe`] — checks all critical dependencies before declaring
//! the service ready to accept traffic.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use parking_lot::RwLock;
use tracing::debug;

use crate::check::HealthCheck;
use crate::types::{CheckSeverity, HealthStatus, Status};

/// A readiness probe that runs a set of critical dependency checks.
///
/// The service is "ready" only when all registered critical checks pass and
/// the probe has not been manually marked unready.
pub struct ReadinessProbe {
    dependencies: RwLock<Vec<Arc<dyn HealthCheck>>>,
    forced_unready: RwLock<bool>,
}

impl Default for ReadinessProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl ReadinessProbe {
    /// Create a new readiness probe with no dependencies.
    pub fn new() -> Self {
        Self {
            dependencies: RwLock::new(Vec::new()),
            forced_unready: RwLock::new(false),
        }
    }

    /// Register a critical dependency that must be `Up` for readiness.
    pub fn add_dependency(&self, check: Arc<dyn HealthCheck>) {
        self.dependencies.write().push(check);
    }

    /// Manually force the probe to report not-ready (e.g. during drain).
    pub fn force_unready(&self) {
        *self.forced_unready.write() = true;
    }

    /// Clear the forced-unready flag.
    pub fn clear_forced_unready(&self) {
        *self.forced_unready.write() = false;
    }

    /// Whether the probe is currently forced unready.
    pub fn is_forced_unready(&self) -> bool {
        *self.forced_unready.read()
    }

    /// Number of registered dependencies.
    pub fn dependency_count(&self) -> usize {
        self.dependencies.read().len()
    }
}

#[async_trait]
#[allow(clippy::unnecessary_literal_bound)]
impl HealthCheck for ReadinessProbe {
    async fn check(&self) -> HealthStatus {
        if *self.forced_unready.read() {
            return HealthStatus {
                name: self.name().to_string(),
                status: Status::Down,
                latency_ms: 0,
                details: Some(serde_json::json!({ "reason": "forced unready" })),
                checked_at: Utc::now(),
            };
        }

        let deps: Vec<Arc<dyn HealthCheck>> = {
            let guard = self.dependencies.read();
            guard.clone()
        };

        if deps.is_empty() {
            return HealthStatus {
                name: self.name().to_string(),
                status: Status::Up,
                latency_ms: 0,
                details: None,
                checked_at: Utc::now(),
            };
        }

        let mut all_up = true;
        let mut any_degraded = false;
        let mut sub_results = Vec::with_capacity(deps.len());
        let start = std::time::Instant::now();

        for dep in &deps {
            let result = dep.check().await;
            match result.status {
                Status::Down => all_up = false,
                Status::Degraded => any_degraded = true,
                Status::Up => {}
            }
            sub_results.push(serde_json::json!({
                "name": result.name,
                "status": result.status,
            }));
        }

        #[allow(clippy::cast_possible_truncation)]
        let latency_ms = start.elapsed().as_millis() as u64;
        let status = if !all_up {
            Status::Down
        } else if any_degraded {
            Status::Degraded
        } else {
            Status::Up
        };

        debug!(check = "readiness", ?status, deps = sub_results.len(), "readiness check completed");

        HealthStatus {
            name: self.name().to_string(),
            status,
            latency_ms,
            details: Some(serde_json::json!({ "dependencies": sub_results })),
            checked_at: Utc::now(),
        }
    }

    fn name(&self) -> &str {
        "readiness"
    }

    fn severity(&self) -> CheckSeverity {
        CheckSeverity::Critical
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Status;

    // A simple stub check returning a fixed status.
    struct StubCheck {
        check_name: String,
        status: Status,
    }

    impl StubCheck {
        fn new(name: &str, status: Status) -> Self {
            Self {
                check_name: name.into(),
                status,
            }
        }
    }

    #[async_trait]
    impl HealthCheck for StubCheck {
        async fn check(&self) -> HealthStatus {
            HealthStatus {
                name: self.check_name.clone(),
                status: self.status,
                latency_ms: 0,
                details: None,
                checked_at: Utc::now(),
            }
        }

        fn name(&self) -> &str {
            &self.check_name
        }

        fn severity(&self) -> CheckSeverity {
            CheckSeverity::Critical
        }
    }

    #[tokio::test]
    async fn empty_deps_is_ready() {
        let probe = ReadinessProbe::new();
        let status = probe.check().await;
        assert_eq!(status.status, Status::Up);
    }

    #[tokio::test]
    async fn all_deps_up_is_ready() {
        let probe = ReadinessProbe::new();
        probe.add_dependency(Arc::new(StubCheck::new("db", Status::Up)));
        probe.add_dependency(Arc::new(StubCheck::new("cache", Status::Up)));

        let status = probe.check().await;
        assert_eq!(status.status, Status::Up);
    }

    #[tokio::test]
    async fn one_dep_down_is_not_ready() {
        let probe = ReadinessProbe::new();
        probe.add_dependency(Arc::new(StubCheck::new("db", Status::Up)));
        probe.add_dependency(Arc::new(StubCheck::new("cache", Status::Down)));

        let status = probe.check().await;
        assert_eq!(status.status, Status::Down);
    }

    #[tokio::test]
    async fn one_dep_degraded() {
        let probe = ReadinessProbe::new();
        probe.add_dependency(Arc::new(StubCheck::new("db", Status::Up)));
        probe.add_dependency(Arc::new(StubCheck::new("cache", Status::Degraded)));

        let status = probe.check().await;
        assert_eq!(status.status, Status::Degraded);
    }

    #[tokio::test]
    async fn forced_unready() {
        let probe = ReadinessProbe::new();
        probe.force_unready();

        let status = probe.check().await;
        assert_eq!(status.status, Status::Down);
        assert!(probe.is_forced_unready());
    }

    #[tokio::test]
    async fn clear_forced_unready() {
        let probe = ReadinessProbe::new();
        probe.force_unready();
        probe.clear_forced_unready();

        let status = probe.check().await;
        assert_eq!(status.status, Status::Up);
        assert!(!probe.is_forced_unready());
    }

    #[tokio::test]
    async fn dependency_count() {
        let probe = ReadinessProbe::new();
        assert_eq!(probe.dependency_count(), 0);
        probe.add_dependency(Arc::new(StubCheck::new("a", Status::Up)));
        assert_eq!(probe.dependency_count(), 1);
    }

    #[test]
    fn default_trait() {
        let probe = ReadinessProbe::default();
        assert!(!probe.is_forced_unready());
        assert_eq!(probe.dependency_count(), 0);
    }
}
