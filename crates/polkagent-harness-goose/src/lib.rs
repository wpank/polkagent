//! Goose harness adapter for the Polkagent platform.
//!
//! This crate provides [`GooseConfigurator`] -- an implementation of the
//! [`AcpConfigurator`] trait that configures the shared
//! [`AcpHarness`](polkagent_harness_acp::AcpHarness) for
//! the Goose CLI (`goose acp`).
//!
//! # Usage
//!
//! ```rust,no_run
//! use polkagent_harness_goose::GooseConfigurator;
//! use polkagent_harness_acp::AcpHarness;
//! use polkagent_harness_trait::{Harness, HarnessConfig, SessionConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = HarnessConfig::new("goose");
//! let harness = AcpHarness::new(GooseConfigurator::default(), config);
//!
//! let session_id = harness.start_session(SessionConfig::default()).await?;
//! harness.send_message(session_id, "Hello, Goose!").await?;
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
// GooseTransport
// ---------------------------------------------------------------------------

/// Transport mode for communicating with Goose.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GooseTransport {
    /// Full ACP mode via `goose acp` (JSON-RPC 2.0 over stdio).
    #[default]
    Acp,
    /// HTTP/WebSocket alternate transport (deferred -- not yet implemented).
    HttpWebSocket,
}

// ---------------------------------------------------------------------------
// GooseHarnessConfig
// ---------------------------------------------------------------------------

/// Configuration specific to the Goose harness.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GooseHarnessConfig {
    /// Transport mode for communicating with Goose.
    pub transport: GooseTransport,
    /// Path to the `goose` binary.
    ///
    /// If `None`, the binary is expected on `$PATH`.
    pub binary_path: Option<PathBuf>,
}

impl GooseHarnessConfig {
    /// Create a builder for `GooseHarnessConfig`.
    pub fn builder() -> GooseHarnessConfigBuilder {
        GooseHarnessConfigBuilder::default()
    }
}

// ---------------------------------------------------------------------------
// GooseHarnessConfigBuilder
// ---------------------------------------------------------------------------

/// Builder for [`GooseHarnessConfig`].
#[derive(Debug, Default)]
pub struct GooseHarnessConfigBuilder {
    transport: Option<GooseTransport>,
    binary_path: Option<PathBuf>,
}

impl GooseHarnessConfigBuilder {
    /// Set the transport mode.
    #[must_use]
    pub fn transport(mut self, transport: GooseTransport) -> Self {
        self.transport = Some(transport);
        self
    }

    /// Set the path to the `goose` binary.
    #[must_use]
    pub fn binary_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.binary_path = Some(path.into());
        self
    }

    /// Build the [`GooseHarnessConfig`].
    pub fn build(self) -> GooseHarnessConfig {
        GooseHarnessConfig {
            transport: self.transport.unwrap_or_default(),
            binary_path: self.binary_path,
        }
    }
}

// ---------------------------------------------------------------------------
// GooseConfigurator
// ---------------------------------------------------------------------------

/// ACP configurator for the Goose CLI.
///
/// Implements [`AcpConfigurator`] so that an
/// [`AcpHarness`](polkagent_harness_acp::AcpHarness) can be used as a
/// fully-featured [`Harness`](polkagent_harness_trait::Harness) for Goose.
#[derive(Debug, Clone, Default)]
pub struct GooseConfigurator {
    /// Goose-specific configuration.
    pub config: GooseHarnessConfig,
}

impl GooseConfigurator {
    /// Create a new configurator with the given config.
    pub fn new(config: GooseHarnessConfig) -> Self {
        Self { config }
    }

    /// Resolve the binary path.
    fn binary_name(&self, harness_config: &HarnessConfig) -> String {
        harness_config
            .executable_path
            .as_ref()
            .or(self.config.binary_path.as_ref())
            .map_or_else(|| "goose".into(), |p| p.display().to_string())
    }
}

impl AcpConfigurator for GooseConfigurator {
    fn harness_id(&self) -> HarnessId {
        HarnessId::new("goose")
    }

    fn build_config(&self, harness_config: &HarnessConfig) -> AcpConfig {
        let binary = self.binary_name(harness_config);
        let cwd = harness_config.workspace_path.clone();
        AcpConfig::goose(binary, cwd)
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_sessions: true,
            max_context_tokens: 128_000,
            models: vec!["claude-sonnet-4-6".into(), "gpt-4o".into()],
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

/// Convenience type alias for a Goose harness.
pub type GooseHarness = polkagent_harness_acp::AcpHarness<GooseConfigurator>;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_harness_acp::AcpHarness;
    use polkagent_harness_trait::{Harness, HarnessStatus};

    // -- GooseConfigurator ---------------------------------------------------

    #[test]
    fn configurator_harness_id() {
        let configurator = GooseConfigurator::default();
        assert_eq!(configurator.harness_id().as_str(), "goose");
    }

    #[test]
    fn configurator_builds_acp_config() {
        let configurator = GooseConfigurator::default();
        let harness_config = HarnessConfig::new("goose");
        let acp_config = configurator.build_config(&harness_config);
        assert_eq!(acp_config.command, "goose");
        assert_eq!(acp_config.args, vec!["acp"]);
    }

    #[test]
    fn configurator_custom_binary() {
        let configurator = GooseConfigurator::new(
            GooseHarnessConfig::builder()
                .binary_path("/opt/goose")
                .build(),
        );
        let harness_config = HarnessConfig::new("goose");
        let acp_config = configurator.build_config(&harness_config);
        assert_eq!(acp_config.command, "/opt/goose");
    }

    #[test]
    fn configurator_harness_config_binary_wins() {
        let configurator = GooseConfigurator::new(
            GooseHarnessConfig::builder()
                .binary_path("/goose-from-config")
                .build(),
        );
        let mut harness_config = HarnessConfig::new("goose");
        harness_config.executable_path = Some(PathBuf::from("/goose-from-harness"));
        let acp_config = configurator.build_config(&harness_config);
        assert_eq!(acp_config.command, "/goose-from-harness");
    }

    #[test]
    fn configurator_default_binary_from_path() {
        let configurator = GooseConfigurator::default();
        let harness_config = HarnessConfig::new("goose");
        let acp_config = configurator.build_config(&harness_config);
        assert_eq!(acp_config.command, "goose");
    }

    // -- AcpHarness integration -----------------------------------------------

    #[test]
    fn harness_id_is_goose() {
        let config = HarnessConfig::new("goose");
        let harness = AcpHarness::new(GooseConfigurator::default(), config);
        assert_eq!(harness.id().as_str(), "goose");
    }

    #[test]
    fn initial_status_is_idle() {
        let config = HarnessConfig::new("goose");
        let harness = AcpHarness::new(GooseConfigurator::default(), config);
        assert!(matches!(harness.status(), HarnessStatus::Idle));
    }

    // -- Capabilities ---------------------------------------------------------

    #[test]
    fn capabilities_supports_streaming() {
        let configurator = GooseConfigurator::default();
        let caps = configurator.capabilities();
        assert!(caps.supports_streaming);
    }

    #[test]
    fn capabilities_supports_tools() {
        let configurator = GooseConfigurator::default();
        let caps = configurator.capabilities();
        assert!(caps.supports_tools);
    }

    #[test]
    fn capabilities_supports_sessions() {
        let configurator = GooseConfigurator::default();
        let caps = configurator.capabilities();
        assert!(caps.supports_sessions);
    }

    #[test]
    fn capabilities_max_context_tokens() {
        let configurator = GooseConfigurator::default();
        let caps = configurator.capabilities();
        assert_eq!(caps.max_context_tokens, 128_000);
    }

    #[test]
    fn capabilities_models() {
        let configurator = GooseConfigurator::default();
        let caps = configurator.capabilities();
        assert!(caps.models.contains(&"claude-sonnet-4-6".into()));
        assert!(caps.models.contains(&"gpt-4o".into()));
    }

    #[test]
    fn capabilities_transport() {
        let configurator = GooseConfigurator::default();
        let caps = configurator.capabilities();
        assert_eq!(caps.transport, Some(TransportFlavor::JsonRpcStdio));
    }

    #[test]
    fn capabilities_session_resume() {
        let configurator = GooseConfigurator::default();
        let caps = configurator.capabilities();
        assert_eq!(caps.session_resume, SessionResumeMode::ById);
    }

    #[test]
    fn capabilities_mcp_passthrough() {
        let configurator = GooseConfigurator::default();
        let caps = configurator.capabilities();
        assert_eq!(caps.mcp_passthrough, McpMode::Configurable);
    }

    #[test]
    fn capabilities_cancel_mode() {
        let configurator = GooseConfigurator::default();
        let caps = configurator.capabilities();
        assert_eq!(caps.cancel, CancelMode::Signal);
    }

    #[test]
    fn capabilities_not_multiplex_safe() {
        let configurator = GooseConfigurator::default();
        let caps = configurator.capabilities();
        assert!(!caps.multiplex_safe);
    }

    // -- GooseHarnessConfig ---------------------------------------------------

    #[test]
    fn config_defaults() {
        let config = GooseHarnessConfig::default();
        assert_eq!(config.transport, GooseTransport::Acp);
        assert!(config.binary_path.is_none());
    }

    #[test]
    fn config_builder_defaults() {
        let config = GooseHarnessConfig::builder().build();
        assert_eq!(config.transport, GooseTransport::Acp);
        assert!(config.binary_path.is_none());
    }

    #[test]
    fn config_builder_with_transport() {
        let config = GooseHarnessConfig::builder()
            .transport(GooseTransport::HttpWebSocket)
            .build();
        assert_eq!(config.transport, GooseTransport::HttpWebSocket);
    }

    #[test]
    fn config_builder_with_binary_path() {
        let config = GooseHarnessConfig::builder()
            .binary_path("/usr/local/bin/goose")
            .build();
        assert_eq!(
            config.binary_path,
            Some(PathBuf::from("/usr/local/bin/goose"))
        );
    }

    #[test]
    fn config_builder_with_all_values() {
        let config = GooseHarnessConfig::builder()
            .transport(GooseTransport::HttpWebSocket)
            .binary_path("/usr/local/bin/goose")
            .build();
        assert_eq!(config.transport, GooseTransport::HttpWebSocket);
        assert_eq!(
            config.binary_path,
            Some(PathBuf::from("/usr/local/bin/goose"))
        );
    }

    // -- GooseTransport serde -------------------------------------------------

    #[test]
    fn transport_acp_serde_round_trip() {
        let acp = GooseTransport::Acp;
        let json = serde_json::to_string(&acp).expect("serialize");
        assert_eq!(json, r#""acp""#);
        let back: GooseTransport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, GooseTransport::Acp);
    }

    #[test]
    fn transport_http_websocket_serde_round_trip() {
        let ws = GooseTransport::HttpWebSocket;
        let json = serde_json::to_string(&ws).expect("serialize");
        assert_eq!(json, r#""http_web_socket""#);
        let back: GooseTransport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, GooseTransport::HttpWebSocket);
    }

    // -- GooseHarnessConfig serde ---------------------------------------------

    #[test]
    fn config_serde_round_trip() {
        let config = GooseHarnessConfig {
            transport: GooseTransport::HttpWebSocket,
            binary_path: Some(PathBuf::from("/opt/goose")),
        };
        let json = serde_json::to_string(&config).expect("serialize");
        assert!(json.contains("http_web_socket"));
        assert!(json.contains("/opt/goose"));
        let back: GooseHarnessConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.transport, GooseTransport::HttpWebSocket);
        assert_eq!(back.binary_path, Some(PathBuf::from("/opt/goose")));
    }

    #[test]
    fn config_serde_with_defaults() {
        let config = GooseHarnessConfig::default();
        let json = serde_json::to_string(&config).expect("serialize");
        let back: GooseHarnessConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.transport, GooseTransport::Acp);
        assert!(back.binary_path.is_none());
    }

    // -- GooseConfigurator construction ---------------------------------------

    #[test]
    fn configurator_new_with_config() {
        let config = GooseHarnessConfig {
            transport: GooseTransport::HttpWebSocket,
            binary_path: Some(PathBuf::from("/custom/goose")),
        };
        let configurator = GooseConfigurator::new(config);
        assert_eq!(configurator.config.transport, GooseTransport::HttpWebSocket);
        assert_eq!(
            configurator.config.binary_path,
            Some(PathBuf::from("/custom/goose"))
        );
    }

    #[test]
    fn configurator_default_config() {
        let configurator = GooseConfigurator::default();
        assert_eq!(configurator.config.transport, GooseTransport::Acp);
        assert!(configurator.config.binary_path.is_none());
    }

    // -- GooseHarness type alias ----------------------------------------------

    #[test]
    fn goose_harness_type_alias() {
        let config = HarnessConfig::new("goose");
        let harness: GooseHarness = AcpHarness::new(GooseConfigurator::default(), config);
        assert_eq!(harness.id().as_str(), "goose");
    }

    // -- AcpConfig produced by configurator -----------------------------------

    #[test]
    fn acp_config_has_correct_args() {
        let configurator = GooseConfigurator::default();
        let harness_config = HarnessConfig::new("goose");
        let acp_config = configurator.build_config(&harness_config);
        assert_eq!(acp_config.args.len(), 1);
        assert_eq!(acp_config.args[0], "acp");
    }

    #[test]
    fn acp_config_with_workspace_path() {
        let configurator = GooseConfigurator::default();
        let mut harness_config = HarnessConfig::new("goose");
        harness_config.workspace_path = Some(PathBuf::from("/tmp/workspace"));
        let acp_config = configurator.build_config(&harness_config);
        assert_eq!(acp_config.cwd, Some(PathBuf::from("/tmp/workspace")));
    }

    #[test]
    fn acp_config_without_workspace_path() {
        let configurator = GooseConfigurator::default();
        let harness_config = HarnessConfig::new("goose");
        let acp_config = configurator.build_config(&harness_config);
        assert!(acp_config.cwd.is_none());
    }
}
