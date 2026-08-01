//! Plugin lifecycle management.
//!
//! The [`PluginLifecycle`] trait defines the lifecycle hooks that every
//! plugin can implement: initialization, startup, shutdown, and health
//! checking. The [`PluginState`] enum tracks where a plugin is in its
//! lifecycle.
//!
//! Since this is a WASM-free plugin system, lifecycle hooks are dispatched
//! as regular async trait calls.

use std::fmt;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::PluginError;

// ---------------------------------------------------------------------------
// PluginState
// ---------------------------------------------------------------------------

/// The lifecycle state of a plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginState {
    /// The plugin has been discovered but not yet initialized.
    Discovered,
    /// The plugin has been initialized (init hook called successfully).
    Initialized,
    /// The plugin is running (start hook called successfully).
    Running,
    /// The plugin has been stopped (stop hook called successfully).
    Stopped,
    /// The plugin failed a lifecycle transition.
    Failed,
}

impl fmt::Display for PluginState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Discovered => write!(f, "discovered"),
            Self::Initialized => write!(f, "initialized"),
            Self::Running => write!(f, "running"),
            Self::Stopped => write!(f, "stopped"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

// ---------------------------------------------------------------------------
// HealthStatus
// ---------------------------------------------------------------------------

/// The result of a plugin health check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    /// Whether the plugin considers itself healthy.
    pub healthy: bool,
    /// An optional human-readable status message.
    pub message: String,
    /// When the check was performed.
    pub checked_at: DateTime<Utc>,
}

impl HealthStatus {
    /// Create a healthy status.
    pub fn healthy(message: impl Into<String>) -> Self {
        Self {
            healthy: true,
            message: message.into(),
            checked_at: Utc::now(),
        }
    }

    /// Create an unhealthy status.
    pub fn unhealthy(message: impl Into<String>) -> Self {
        Self {
            healthy: false,
            message: message.into(),
            checked_at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// PluginLifecycle trait
// ---------------------------------------------------------------------------

/// Lifecycle hooks for a plugin.
///
/// Plugins implement this trait to participate in managed lifecycle
/// transitions. The plugin manager calls these methods in order:
///
/// 1. [`init`](Self::init) — one-time setup (validate config, open resources)
/// 2. [`start`](Self::start) — begin active operation
/// 3. [`health_check`](Self::health_check) — periodic liveness check
/// 4. [`stop`](Self::stop) — graceful shutdown
///
/// Default implementations are provided that do nothing and report healthy,
/// so plugins only need to override the hooks they care about.
#[async_trait]
pub trait PluginLifecycle: Send + Sync {
    /// Initialize the plugin.
    ///
    /// Called once after the manifest has been loaded and capabilities
    /// validated. The plugin should perform any one-time setup here
    /// (e.g. validate configuration, open file handles).
    async fn init(&mut self) -> Result<(), PluginError> {
        Ok(())
    }

    /// Start the plugin.
    ///
    /// Called after successful initialization. The plugin should begin
    /// its active operation (e.g. register tools, start background tasks).
    async fn start(&mut self) -> Result<(), PluginError> {
        Ok(())
    }

    /// Stop the plugin.
    ///
    /// Called during graceful shutdown. The plugin should release any
    /// resources and clean up state.
    async fn stop(&mut self) -> Result<(), PluginError> {
        Ok(())
    }

    /// Perform a health check.
    ///
    /// Called periodically to verify the plugin is still operating
    /// correctly. The default implementation reports healthy.
    async fn health_check(&self) -> HealthStatus {
        HealthStatus::healthy("ok")
    }
}

// ---------------------------------------------------------------------------
// PluginInstance — tracks lifecycle state for a concrete plugin
// ---------------------------------------------------------------------------

/// Tracks the lifecycle state for a loaded plugin instance.
#[derive(Debug)]
pub struct PluginInstance {
    /// The plugin name.
    pub name: String,
    /// The current lifecycle state.
    pub state: PluginState,
    /// When the plugin was last state-transitioned.
    pub last_transition: DateTime<Utc>,
    /// The last health check result (if any).
    pub last_health: Option<HealthStatus>,
}

impl PluginInstance {
    /// Create a new instance in the `Discovered` state.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            state: PluginState::Discovered,
            last_transition: Utc::now(),
            last_health: None,
        }
    }

    /// Transition to a new state.
    pub fn transition(&mut self, new_state: PluginState) {
        self.state = new_state;
        self.last_transition = Utc::now();
    }

    /// Record a health check result.
    pub fn record_health(&mut self, status: HealthStatus) {
        self.last_health = Some(status);
    }

    /// Whether the plugin is in a running state.
    pub fn is_running(&self) -> bool {
        self.state == PluginState::Running
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_state_display() {
        assert_eq!(PluginState::Discovered.to_string(), "discovered");
        assert_eq!(PluginState::Initialized.to_string(), "initialized");
        assert_eq!(PluginState::Running.to_string(), "running");
        assert_eq!(PluginState::Stopped.to_string(), "stopped");
        assert_eq!(PluginState::Failed.to_string(), "failed");
    }

    #[test]
    fn health_status_healthy() {
        let status = HealthStatus::healthy("all good");
        assert!(status.healthy);
        assert_eq!(status.message, "all good");
    }

    #[test]
    fn health_status_unhealthy() {
        let status = HealthStatus::unhealthy("connection lost");
        assert!(!status.healthy);
        assert_eq!(status.message, "connection lost");
    }

    #[test]
    fn plugin_instance_lifecycle() {
        let mut instance = PluginInstance::new("test-plugin");
        assert_eq!(instance.state, PluginState::Discovered);
        assert!(!instance.is_running());

        instance.transition(PluginState::Initialized);
        assert_eq!(instance.state, PluginState::Initialized);

        instance.transition(PluginState::Running);
        assert!(instance.is_running());

        instance.transition(PluginState::Stopped);
        assert!(!instance.is_running());
        assert_eq!(instance.state, PluginState::Stopped);
    }

    #[test]
    fn plugin_instance_health_recording() {
        let mut instance = PluginInstance::new("test");
        assert!(instance.last_health.is_none());

        instance.record_health(HealthStatus::healthy("ok"));
        assert!(instance.last_health.is_some());
        assert!(instance.last_health.as_ref().map_or(false, |h| h.healthy));
    }

    struct DummyPlugin {
        init_called: bool,
        started: bool,
        stopped: bool,
    }

    impl DummyPlugin {
        fn new() -> Self {
            Self {
                init_called: false,
                started: false,
                stopped: false,
            }
        }
    }

    #[async_trait]
    impl PluginLifecycle for DummyPlugin {
        async fn init(&mut self) -> Result<(), PluginError> {
            self.init_called = true;
            Ok(())
        }

        async fn start(&mut self) -> Result<(), PluginError> {
            self.started = true;
            Ok(())
        }

        async fn stop(&mut self) -> Result<(), PluginError> {
            self.stopped = true;
            Ok(())
        }

        async fn health_check(&self) -> HealthStatus {
            HealthStatus::healthy("dummy is fine")
        }
    }

    #[tokio::test]
    async fn lifecycle_hooks_called() {
        let mut plugin = DummyPlugin::new();

        assert!(!plugin.init_called);
        plugin.init().await.expect("init should succeed");
        assert!(plugin.init_called);

        assert!(!plugin.started);
        plugin.start().await.expect("start should succeed");
        assert!(plugin.started);

        let health = plugin.health_check().await;
        assert!(health.healthy);
        assert_eq!(health.message, "dummy is fine");

        assert!(!plugin.stopped);
        plugin.stop().await.expect("stop should succeed");
        assert!(plugin.stopped);
    }

    struct FailingPlugin;

    #[async_trait]
    impl PluginLifecycle for FailingPlugin {
        async fn init(&mut self) -> Result<(), PluginError> {
            Err(PluginError::LifecycleError {
                plugin_name: "failing".into(),
                reason: "init failed on purpose".into(),
            })
        }

        async fn health_check(&self) -> HealthStatus {
            HealthStatus::unhealthy("always broken")
        }
    }

    #[tokio::test]
    async fn lifecycle_init_failure() {
        let mut plugin = FailingPlugin;
        let result = plugin.init().await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("init failed on purpose"));
    }

    #[tokio::test]
    async fn lifecycle_unhealthy_check() {
        let plugin = FailingPlugin;
        let health = plugin.health_check().await;
        assert!(!health.healthy);
        assert_eq!(health.message, "always broken");
    }
}
