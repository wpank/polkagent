//! The [`HealthCheck`] trait — the unit of health monitoring.

use async_trait::async_trait;

use crate::types::{CheckSeverity, HealthStatus};

/// A single health check that can be registered with the aggregator.
#[async_trait]
pub trait HealthCheck: Send + Sync {
    /// Execute the health check and return a [`HealthStatus`].
    async fn check(&self) -> HealthStatus;

    /// Human-readable name of this check (used in reporting).
    fn name(&self) -> &str;

    /// Whether this check is critical or advisory.
    fn severity(&self) -> CheckSeverity;
}
