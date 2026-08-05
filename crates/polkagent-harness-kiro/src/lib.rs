//! Kiro harness adapter for the Polkagent platform.
//!
//! This crate provides [`KiroConfigurator`] -- an implementation of the
//! [`AcpConfigurator`] trait that configures the shared
//! [`AcpHarness`](polkagent_harness_acp::AcpHarness) for
//! the Kiro CLI (`kiro-cli acp`).
//!
//! # Usage
//!
//! ```rust,no_run
//! use polkagent_harness_kiro::KiroConfigurator;
//! use polkagent_harness_acp::AcpHarness;
//! use polkagent_harness_trait::{Harness, HarnessConfig, SessionConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = HarnessConfig::new("kiro");
//! let harness = AcpHarness::new(KiroConfigurator::default(), config);
//!
//! let session_id = harness.start_session(SessionConfig::default()).await?;
//! harness.send_message(session_id, "Hello, Kiro!").await?;
//! harness.end_session(session_id).await?;
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(
    missing_docs,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used
)]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use polkagent_harness_acp::{AcpConfig, AcpConfigurator};
use polkagent_harness_trait::{
    CancelMode, HarnessCapabilities, HarnessConfig, HarnessId, McpMode, SessionResumeMode,
    ToolInjection, TransportFlavor,
};

// ---------------------------------------------------------------------------
// KiroTransport
// ---------------------------------------------------------------------------

/// Transport mode for communicating with Kiro.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KiroTransport {
    /// Full ACP mode via `kiro-cli acp`.
    #[default]
    Acp,
}

// ---------------------------------------------------------------------------
// KiroHarnessConfig
// ---------------------------------------------------------------------------

/// Configuration specific to the Kiro harness.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KiroHarnessConfig {
    /// Transport mode for communicating with Kiro.
    pub transport: KiroTransport,
    /// Path to the `kiro-cli` binary.
    ///
    /// If `None`, the binary is expected on `$PATH`.
    pub binary_path: Option<PathBuf>,
}

impl KiroHarnessConfig {
    /// Create a builder for `KiroHarnessConfig`.
    pub fn builder() -> KiroHarnessConfigBuilder {
        KiroHarnessConfigBuilder::default()
    }
}

// ---------------------------------------------------------------------------
// KiroHarnessConfigBuilder
// ---------------------------------------------------------------------------

/// Builder for [`KiroHarnessConfig`].
#[derive(Debug, Default)]
pub struct KiroHarnessConfigBuilder {
    transport: Option<KiroTransport>,
    binary_path: Option<PathBuf>,
}

impl KiroHarnessConfigBuilder {
    /// Set the transport mode.
    #[must_use]
    pub fn transport(mut self, transport: KiroTransport) -> Self {
        self.transport = Some(transport);
        self
    }

    /// Set the path to the `kiro-cli` binary.
    #[must_use]
    pub fn binary_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.binary_path = Some(path.into());
        self
    }

    /// Build the [`KiroHarnessConfig`].
    pub fn build(self) -> KiroHarnessConfig {
        KiroHarnessConfig {
            transport: self.transport.unwrap_or_default(),
            binary_path: self.binary_path,
        }
    }
}

// ---------------------------------------------------------------------------
// KiroConfigurator
// ---------------------------------------------------------------------------

/// ACP configurator for the Kiro CLI.
///
/// Implements [`AcpConfigurator`] so that an
/// [`AcpHarness`](polkagent_harness_acp::AcpHarness) can be used as a
/// fully-featured [`Harness`](polkagent_harness_trait::Harness) for Kiro.
#[derive(Debug, Clone, Default)]
pub struct KiroConfigurator {
    /// Kiro-specific configuration.
    pub config: KiroHarnessConfig,
}

impl KiroConfigurator {
    /// Create a new configurator with the given config.
    pub fn new(config: KiroHarnessConfig) -> Self {
        Self { config }
    }

    /// Resolve the binary path.
    fn binary_name(&self, harness_config: &HarnessConfig) -> String {
        harness_config
            .executable_path
            .as_ref()
            .or(self.config.binary_path.as_ref())
            .map_or_else(|| "kiro-cli".into(), |p| p.display().to_string())
    }
}

impl AcpConfigurator for KiroConfigurator {
    fn harness_id(&self) -> HarnessId {
        HarnessId::new("kiro")
    }

    fn build_config(&self, harness_config: &HarnessConfig) -> AcpConfig {
        let binary = self.binary_name(harness_config);
        let cwd = harness_config.workspace_path.clone();
        AcpConfig::kiro(binary, cwd)
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_sessions: true,
            max_context_tokens: 200_000,
            models: vec!["claude-sonnet-4-6".into()],
            transport: Some(TransportFlavor::JsonRpcStdio),
            model_override: None,
            session_resume: SessionResumeMode::ById,
            mcp_passthrough: McpMode::Configurable,
            tool_injection: ToolInjection::McpConfig,
            cancel: CancelMode::Signal,
            multiplex_safe: false,
        }
    }
}

/// Convenience type alias for a Kiro harness.
pub type KiroHarness = polkagent_harness_acp::AcpHarness<KiroConfigurator>;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Assertion-oriented serialization tests intentionally fail fast on invalid fixtures.
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use polkagent_harness_acp::AcpHarness;
    use polkagent_harness_trait::{Harness, HarnessStatus};

    #[test]
    fn configurator_harness_id() {
        let configurator = KiroConfigurator::default();
        assert_eq!(configurator.harness_id().as_str(), "kiro");
    }

    #[test]
    fn configurator_builds_acp_config() {
        let configurator = KiroConfigurator::default();
        let harness_config = HarnessConfig::new("kiro");
        let acp_config = configurator.build_config(&harness_config);
        assert_eq!(acp_config.command, "kiro-cli");
        assert_eq!(acp_config.args, vec!["acp"]);
    }

    #[test]
    fn configurator_custom_binary() {
        let configurator = KiroConfigurator::new(
            KiroHarnessConfig::builder()
                .binary_path("/opt/kiro-cli")
                .build(),
        );
        let harness_config = HarnessConfig::new("kiro");
        let acp_config = configurator.build_config(&harness_config);
        assert_eq!(acp_config.command, "/opt/kiro-cli");
    }

    #[test]
    fn configurator_harness_config_binary_wins() {
        let configurator = KiroConfigurator::new(
            KiroHarnessConfig::builder()
                .binary_path("/kiro-from-config")
                .build(),
        );
        let mut harness_config = HarnessConfig::new("kiro");
        harness_config.executable_path = Some(PathBuf::from("/kiro-from-harness"));
        let acp_config = configurator.build_config(&harness_config);
        assert_eq!(acp_config.command, "/kiro-from-harness");
    }

    #[test]
    fn harness_id_is_kiro() {
        let config = HarnessConfig::new("kiro");
        let harness = AcpHarness::new(KiroConfigurator::default(), config);
        assert_eq!(harness.id().as_str(), "kiro");
    }

    #[test]
    fn capabilities_values() {
        let configurator = KiroConfigurator::default();
        let caps = configurator.capabilities();
        assert!(caps.supports_streaming);
        assert!(caps.supports_tools);
        assert!(caps.supports_sessions);
        assert_eq!(caps.max_context_tokens, 200_000);
        assert!(caps.models.contains(&"claude-sonnet-4-6".into()));
        assert_eq!(caps.transport, Some(TransportFlavor::JsonRpcStdio));
        assert_eq!(caps.session_resume, SessionResumeMode::ById);
        assert_eq!(caps.mcp_passthrough, McpMode::Configurable);
        assert_eq!(caps.tool_injection, ToolInjection::McpConfig);
        assert_eq!(caps.cancel, CancelMode::Signal);
        assert!(!caps.multiplex_safe);
    }

    #[test]
    fn capabilities_model_override_is_none() {
        let configurator = KiroConfigurator::default();
        let caps = configurator.capabilities();
        assert!(caps.model_override.is_none());
    }

    #[test]
    fn initial_status_is_idle() {
        let config = HarnessConfig::new("kiro");
        let harness = AcpHarness::new(KiroConfigurator::default(), config);
        assert!(matches!(harness.status(), HarnessStatus::Idle));
    }

    #[test]
    fn config_defaults() {
        let config = KiroHarnessConfig::default();
        assert_eq!(config.transport, KiroTransport::Acp);
        assert!(config.binary_path.is_none());
    }

    #[test]
    fn kiro_transport_serde_round_trip() {
        let acp = KiroTransport::Acp;
        let json = serde_json::to_string(&acp).expect("serialize");
        assert_eq!(json, r#""acp""#);
        let back: KiroTransport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, KiroTransport::Acp);
    }

    #[test]
    fn config_builder_defaults() {
        let config = KiroHarnessConfig::builder().build();
        assert_eq!(config.transport, KiroTransport::Acp);
        assert!(config.binary_path.is_none());
    }

    #[test]
    fn config_builder_with_values() {
        let config = KiroHarnessConfig::builder()
            .transport(KiroTransport::Acp)
            .binary_path("/usr/local/bin/kiro-cli")
            .build();
        assert_eq!(config.transport, KiroTransport::Acp);
        assert_eq!(
            config.binary_path,
            Some(PathBuf::from("/usr/local/bin/kiro-cli"))
        );
    }

    #[test]
    fn config_serde_round_trip() {
        let config = KiroHarnessConfig {
            transport: KiroTransport::Acp,
            binary_path: Some(PathBuf::from("/opt/kiro-cli")),
        };
        let json = serde_json::to_string(&config).expect("serialize");
        assert!(json.contains("acp"));
        assert!(json.contains("/opt/kiro-cli"));
        let back: KiroHarnessConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.transport, KiroTransport::Acp);
        assert_eq!(back.binary_path, Some(PathBuf::from("/opt/kiro-cli")));
    }

    #[test]
    fn default_binary_is_kiro_cli() {
        let configurator = KiroConfigurator::default();
        let harness_config = HarnessConfig::new("kiro");
        let acp_config = configurator.build_config(&harness_config);
        assert_eq!(acp_config.command, "kiro-cli");
    }

    #[test]
    fn acp_config_args_are_correct() {
        let configurator = KiroConfigurator::default();
        let harness_config = HarnessConfig::new("kiro");
        let acp_config = configurator.build_config(&harness_config);
        assert_eq!(acp_config.args.len(), 1);
        assert_eq!(acp_config.args[0], "acp");
    }

    #[test]
    fn configurator_debug_impl() {
        let configurator = KiroConfigurator::default();
        let debug = format!("{configurator:?}");
        assert!(debug.contains("KiroConfigurator"));
    }

    #[test]
    fn configurator_clone() {
        let configurator = KiroConfigurator::new(
            KiroHarnessConfig::builder()
                .binary_path("/custom/kiro")
                .build(),
        );
        let cloned = configurator.clone();
        assert_eq!(
            cloned.config.binary_path,
            Some(PathBuf::from("/custom/kiro"))
        );
    }

    #[test]
    fn type_alias_compiles() {
        let config = HarnessConfig::new("kiro");
        let _harness: KiroHarness = AcpHarness::new(KiroConfigurator::default(), config);
    }
}
