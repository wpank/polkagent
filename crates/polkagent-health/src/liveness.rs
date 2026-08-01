//! [`LivenessProbe`] — a simple "is the process alive" check.
//!
//! Always reports [`Status::Up`] unless explicitly forced to [`Status::Down`]
//! (e.g. during graceful shutdown).

use std::time::Instant;

use async_trait::async_trait;
use chrono::Utc;
use parking_lot::RwLock;

use crate::check::HealthCheck;
use crate::types::{CheckSeverity, HealthStatus, Status};

/// A liveness probe that is `Up` by default and can be toggled to `Down`.
pub struct LivenessProbe {
    alive: RwLock<bool>,
}

impl Default for LivenessProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl LivenessProbe {
    /// Create a new liveness probe (initially alive).
    pub fn new() -> Self {
        Self {
            alive: RwLock::new(true),
        }
    }

    /// Mark the process as dead (e.g. during graceful shutdown).
    pub fn mark_dead(&self) {
        *self.alive.write() = false;
    }

    /// Mark the process as alive again.
    pub fn mark_alive(&self) {
        *self.alive.write() = true;
    }

    /// Whether the probe currently reports alive.
    pub fn is_alive(&self) -> bool {
        *self.alive.read()
    }
}

#[async_trait]
#[allow(clippy::unnecessary_literal_bound)]
impl HealthCheck for LivenessProbe {
    async fn check(&self) -> HealthStatus {
        let start = Instant::now();
        let alive = *self.alive.read();
        #[allow(clippy::cast_possible_truncation)]
        let latency_ms = start.elapsed().as_millis() as u64;

        HealthStatus {
            name: self.name().to_string(),
            status: if alive { Status::Up } else { Status::Down },
            latency_ms,
            details: None,
            checked_at: Utc::now(),
        }
    }

    fn name(&self) -> &str {
        "liveness"
    }

    fn severity(&self) -> CheckSeverity {
        CheckSeverity::Critical
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_is_alive() {
        let probe = LivenessProbe::new();
        assert!(probe.is_alive());

        let status = probe.check().await;
        assert_eq!(status.status, Status::Up);
        assert_eq!(status.name, "liveness");
    }

    #[tokio::test]
    async fn mark_dead() {
        let probe = LivenessProbe::new();
        probe.mark_dead();
        assert!(!probe.is_alive());

        let status = probe.check().await;
        assert_eq!(status.status, Status::Down);
    }

    #[tokio::test]
    async fn mark_alive_again() {
        let probe = LivenessProbe::new();
        probe.mark_dead();
        probe.mark_alive();
        assert!(probe.is_alive());

        let status = probe.check().await;
        assert_eq!(status.status, Status::Up);
    }

    #[tokio::test]
    async fn severity_is_critical() {
        let probe = LivenessProbe::new();
        assert_eq!(probe.severity(), CheckSeverity::Critical);
    }

    #[test]
    fn default_trait() {
        let probe = LivenessProbe::default();
        assert!(probe.is_alive());
    }
}
