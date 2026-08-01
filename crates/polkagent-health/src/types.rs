//! Shared types for health status reporting.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Status of an individual health check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The component is healthy.
    Up,
    /// The component is unhealthy.
    Down,
    /// The component is partially healthy / experiencing issues.
    Degraded,
}

/// Whether a check is critical (affects overall health) or advisory (informational).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckSeverity {
    /// A critical check: if this is Down the overall status becomes Down.
    Critical,
    /// An advisory check: does not affect overall Up/Down, but Degraded is still
    /// propagated.
    Advisory,
}

/// Result of running a single health check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    /// Human-readable name of the check.
    pub name: String,
    /// Current status.
    pub status: Status,
    /// How long the check took, in milliseconds.
    pub latency_ms: u64,
    /// Optional details (arbitrary JSON).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    /// When the check was performed.
    pub checked_at: DateTime<Utc>,
}

/// Aggregated health of the entire system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverallHealth {
    /// Computed overall status.
    pub status: Status,
    /// Individual check results.
    pub checks: Vec<HealthStatus>,
    /// Process uptime in seconds.
    pub uptime_seconds: f64,
    /// Application version string.
    pub version: String,
}
