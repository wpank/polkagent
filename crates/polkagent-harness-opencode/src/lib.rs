//! OpenCode harness adapter for the Polkagent platform.
//!
//! This crate provides [`OpenCodeConfigurator`] -- an implementation of the
//! [`AcpConfigurator`] trait that configures the shared [`AcpHarness`] for
//! the OpenCode CLI (`opencode acp`).
//!
//! # Usage
//!
//! ```rust,no_run
//! use polkagent_harness_opencode::OpenCodeConfigurator;
//! use polkagent_harness_acp::AcpHarness;
//! use polkagent_harness_trait::{Harness, HarnessConfig, SessionConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = HarnessConfig::new("opencode");
//! let harness = AcpHarness::new(OpenCodeConfigurator::default(), config);
//!
//! let session_id = harness.start_session(SessionConfig::default()).await?;
//! harness.send_message(session_id, "Hello, OpenCode!").await?;
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
// OpenCodeTransport
// ---------------------------------------------------------------------------

/// Transport mode for communicating with OpenCode.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenCodeTransport {
    /// Full ACP mode via `opencode acp` (JSON-RPC 2.0 over stdio).
    #[default]
    Acp,
}

// ---------------------------------------------------------------------------
// OpenCodeHarnessConfig
// ---------------------------------------------------------------------------

/// Configuration specific to the OpenCode harness.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenCodeHarnessConfig {
    /// Transport mode for communicating with OpenCode.
    pub transport: OpenCodeTransport,
    /// Path to the `opencode` binary.
    ///
    /// If `None`, the binary is expected on `$PATH`.
    pub binary_path: Option<PathBuf>,
}

impl OpenCodeHarnessConfig {
    /// Create a builder for `OpenCodeHarnessConfig`.
    pub fn builder() -> OpenCodeHarnessConfigBuilder {
        OpenCodeHarnessConfigBuilder::default()
    }
}

// ---------------------------------------------------------------------------
// OpenCodeHarnessConfigBuilder
// ---------------------------------------------------------------------------

/// Builder for [`OpenCodeHarnessConfig`].
#[derive(Debug, Default)]
pub struct OpenCodeHarnessConfigBuilder {
    transport: Option<OpenCodeTransport>,
    binary_path: Option<PathBuf>,
}

impl OpenCodeHarnessConfigBuilder {
    /// Set the transport mode.
    pub fn transport(mut self, transport: OpenCodeTransport) -> Self {
        self.transport = Some(transport);
        self
    }

    /// Set the path to the `opencode` binary.
    pub fn binary_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.binary_path = Some(path.into());
        self
    }

    /// Build the [`OpenCodeHarnessConfig`].
    pub fn build(self) -> OpenCodeHarnessConfig {
        OpenCodeHarnessConfig {
            transport: self.transport.unwrap_or_default(),
            binary_path: self.binary_path,
        }
    }
}

// ---------------------------------------------------------------------------
// OpenCodeConfigurator
// ---------------------------------------------------------------------------

/// ACP configurator for the OpenCode CLI.
///
/// Implements [`AcpConfigurator`] so that an [`AcpHarness<OpenCodeConfigurator>`]
/// can be used as a fully-featured [`Harness`] for OpenCode.
#[derive(Debug, Clone, Default)]
pub struct OpenCodeConfigurator {
    /// OpenCode-specific configuration.
    pub config: OpenCodeHarnessConfig,
}

impl OpenCodeConfigurator {
    /// Create a new configurator with the given config.
    pub fn new(config: OpenCodeHarnessConfig) -> Self {
        Self { config }
    }

    /// Resolve the binary path.
    fn binary_name(&self, harness_config: &HarnessConfig) -> String {
        harness_config
            .executable_path
            .as_ref()
            .or(self.config.binary_path.as_ref())
            .map_or_else(|| "opencode".into(), |p| p.display().to_string())
    }
}

impl AcpConfigurator for OpenCodeConfigurator {
    fn harness_id(&self) -> HarnessId {
        HarnessId::new("opencode")
    }

    fn build_config(&self, harness_config: &HarnessConfig) -> AcpConfig {
        let binary = self.binary_name(harness_config);
        let cwd = harness_config.workspace_path.clone();
        AcpConfig::opencode(binary, cwd)
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

/// Convenience type alias for an OpenCode harness.
pub type OpenCodeHarness = polkagent_harness_acp::AcpHarness<OpenCodeConfigurator>;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_harness_acp::AcpHarness;
    use polkagent_harness_trait::{Harness, HarnessStatus};

    #[test]
    fn configurator_harness_id() {
        let configurator = OpenCodeConfigurator::default();
        assert_eq!(configurator.harness_id().as_str(), "opencode");
    }

    #[test]
    fn configurator_builds_acp_config() {
        let configurator = OpenCodeConfigurator::default();
        let harness_config = HarnessConfig::new("opencode");
        let acp_config = configurator.build_config(&harness_config);
        assert_eq!(acp_config.command, "opencode");
        assert_eq!(acp_config.args, vec!["acp"]);
    }

    #[test]
    fn configurator_custom_binary() {
        let configurator = OpenCodeConfigurator::new(
            OpenCodeHarnessConfig::builder()
                .binary_path("/opt/opencode")
                .build(),
        );
        let harness_config = HarnessConfig::new("opencode");
        let acp_config = configurator.build_config(&harness_config);
        assert_eq!(acp_config.command, "/opt/opencode");
    }

    #[test]
    fn configurator_harness_config_binary_wins() {
        let configurator = OpenCodeConfigurator::new(
            OpenCodeHarnessConfig::builder()
                .binary_path("/opencode-from-config")
                .build(),
        );
        let mut harness_config = HarnessConfig::new("opencode");
        harness_config.executable_path = Some(PathBuf::from("/opencode-from-harness"));
        let acp_config = configurator.build_config(&harness_config);
        assert_eq!(acp_config.command, "/opencode-from-harness");
    }

    #[test]
    fn harness_id_is_opencode() {
        let config = HarnessConfig::new("opencode");
        let harness = AcpHarness::new(OpenCodeConfigurator::default(), config);
        assert_eq!(harness.id().as_str(), "opencode");
    }

    #[test]
    fn capabilities_values() {
        let configurator = OpenCodeConfigurator::default();
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
    fn initial_status_is_idle() {
        let config = HarnessConfig::new("opencode");
        let harness = AcpHarness::new(OpenCodeConfigurator::default(), config);
        assert!(matches!(harness.status(), HarnessStatus::Idle));
    }

    #[test]
    fn config_defaults() {
        let config = OpenCodeHarnessConfig::default();
        assert_eq!(config.transport, OpenCodeTransport::Acp);
        assert!(config.binary_path.is_none());
    }

    #[test]
    fn transport_serde_round_trip() {
        let acp = OpenCodeTransport::Acp;
        let json = serde_json::to_string(&acp).expect("serialize");
        assert_eq!(json, r#""acp""#);
        let back: OpenCodeTransport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, OpenCodeTransport::Acp);
    }

    #[test]
    fn config_builder_defaults() {
        let config = OpenCodeHarnessConfig::builder().build();
        assert_eq!(config.transport, OpenCodeTransport::Acp);
        assert!(config.binary_path.is_none());
    }

    #[test]
    fn config_builder_with_values() {
        let config = OpenCodeHarnessConfig::builder()
            .transport(OpenCodeTransport::Acp)
            .binary_path("/usr/local/bin/opencode")
            .build();
        assert_eq!(config.transport, OpenCodeTransport::Acp);
        assert_eq!(
            config.binary_path,
            Some(PathBuf::from("/usr/local/bin/opencode"))
        );
    }

    #[test]
    fn config_serde_round_trip() {
        let config = OpenCodeHarnessConfig {
            transport: OpenCodeTransport::Acp,
            binary_path: Some(PathBuf::from("/opt/opencode")),
        };
        let json = serde_json::to_string(&config).expect("serialize");
        assert!(json.contains("acp"));
        assert!(json.contains("/opt/opencode"));
        let back: OpenCodeHarnessConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.transport, OpenCodeTransport::Acp);
        assert_eq!(back.binary_path, Some(PathBuf::from("/opt/opencode")));
    }

    #[test]
    fn type_alias_compiles() {
        let config = HarnessConfig::new("opencode");
        let _harness: OpenCodeHarness = AcpHarness::new(OpenCodeConfigurator::default(), config);
    }

    #[test]
    fn acp_config_with_workspace_path() {
        let configurator = OpenCodeConfigurator::default();
        let mut harness_config = HarnessConfig::new("opencode");
        harness_config.workspace_path = Some(PathBuf::from("/tmp/workspace"));
        let acp_config = configurator.build_config(&harness_config);
        assert_eq!(acp_config.cwd, Some(PathBuf::from("/tmp/workspace")));
    }

    #[test]
    fn acp_config_without_workspace_path() {
        let configurator = OpenCodeConfigurator::default();
        let harness_config = HarnessConfig::new("opencode");
        let acp_config = configurator.build_config(&harness_config);
        assert!(acp_config.cwd.is_none());
    }
}
