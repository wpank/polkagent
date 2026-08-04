//! Cursor harness adapter for the Polkagent platform.
//!
//! This crate provides [`CursorConfigurator`] -- an implementation of the
//! [`AcpConfigurator`] trait that configures the shared [`AcpHarness`] for
//! the Cursor CLI (`cursor agent acp`).
//!
//! # Usage
//!
//! ```rust,no_run
//! use polkagent_harness_cursor::CursorConfigurator;
//! use polkagent_harness_acp::AcpHarness;
//! use polkagent_harness_trait::{Harness, HarnessConfig, SessionConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = HarnessConfig::new("cursor");
//! let harness = AcpHarness::new(CursorConfigurator::default(), config);
//!
//! let session_id = harness.start_session(SessionConfig::default()).await?;
//! harness.send_message(session_id, "Hello, Cursor!").await?;
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
// CursorTransport
// ---------------------------------------------------------------------------

/// Transport mode for communicating with Cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorTransport {
    /// Full ACP mode via `cursor agent acp`.
    Acp,
    /// Simpler headless mode via `cursor agent -p "prompt" --output-format stream-json`.
    Headless,
}

impl Default for CursorTransport {
    fn default() -> Self {
        Self::Acp
    }
}

// ---------------------------------------------------------------------------
// CursorHarnessConfig
// ---------------------------------------------------------------------------

/// Configuration specific to the Cursor harness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorHarnessConfig {
    /// Transport mode for communicating with Cursor.
    pub transport: CursorTransport,
    /// Path to the `cursor` binary.
    ///
    /// If `None`, the binary is expected on `$PATH`.
    pub binary_path: Option<PathBuf>,
}

impl Default for CursorHarnessConfig {
    fn default() -> Self {
        Self {
            transport: CursorTransport::default(),
            binary_path: None,
        }
    }
}

impl CursorHarnessConfig {
    /// Create a builder for `CursorHarnessConfig`.
    pub fn builder() -> CursorHarnessConfigBuilder {
        CursorHarnessConfigBuilder::default()
    }
}

// ---------------------------------------------------------------------------
// CursorHarnessConfigBuilder
// ---------------------------------------------------------------------------

/// Builder for [`CursorHarnessConfig`].
#[derive(Debug, Default)]
pub struct CursorHarnessConfigBuilder {
    transport: Option<CursorTransport>,
    binary_path: Option<PathBuf>,
}

impl CursorHarnessConfigBuilder {
    /// Set the transport mode.
    pub fn transport(mut self, transport: CursorTransport) -> Self {
        self.transport = Some(transport);
        self
    }

    /// Set the path to the `cursor` binary.
    pub fn binary_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.binary_path = Some(path.into());
        self
    }

    /// Build the [`CursorHarnessConfig`].
    pub fn build(self) -> CursorHarnessConfig {
        CursorHarnessConfig {
            transport: self.transport.unwrap_or_default(),
            binary_path: self.binary_path,
        }
    }
}

// ---------------------------------------------------------------------------
// CursorConfigurator
// ---------------------------------------------------------------------------

/// ACP configurator for the Cursor CLI.
///
/// Implements [`AcpConfigurator`] so that an [`AcpHarness<CursorConfigurator>`]
/// can be used as a fully-featured [`Harness`] for Cursor.
#[derive(Debug, Clone)]
pub struct CursorConfigurator {
    /// Cursor-specific configuration.
    pub config: CursorHarnessConfig,
}

impl Default for CursorConfigurator {
    fn default() -> Self {
        Self {
            config: CursorHarnessConfig::default(),
        }
    }
}

impl CursorConfigurator {
    /// Create a new configurator with the given config.
    pub fn new(config: CursorHarnessConfig) -> Self {
        Self { config }
    }

    /// Resolve the binary path.
    fn binary_name(&self, harness_config: &HarnessConfig) -> String {
        harness_config
            .executable_path
            .as_ref()
            .or(self.config.binary_path.as_ref())
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "cursor".into())
    }
}

impl AcpConfigurator for CursorConfigurator {
    fn harness_id(&self) -> HarnessId {
        HarnessId::new("cursor")
    }

    fn build_config(&self, harness_config: &HarnessConfig) -> AcpConfig {
        let binary = self.binary_name(harness_config);
        let cwd = harness_config.workspace_path.clone();
        AcpConfig::cursor(binary, cwd)
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_sessions: true,
            max_context_tokens: 200_000,
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

/// Convenience type alias for a Cursor harness.
pub type CursorHarness = polkagent_harness_acp::AcpHarness<CursorConfigurator>;

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
        let configurator = CursorConfigurator::default();
        assert_eq!(configurator.harness_id().as_str(), "cursor");
    }

    #[test]
    fn configurator_builds_acp_config() {
        let configurator = CursorConfigurator::default();
        let harness_config = HarnessConfig::new("cursor");
        let acp_config = configurator.build_config(&harness_config);
        assert_eq!(acp_config.command, "cursor");
        assert_eq!(acp_config.args, vec!["agent", "acp"]);
    }

    #[test]
    fn configurator_custom_binary() {
        let configurator = CursorConfigurator::new(
            CursorHarnessConfig::builder()
                .binary_path("/opt/cursor")
                .build(),
        );
        let harness_config = HarnessConfig::new("cursor");
        let acp_config = configurator.build_config(&harness_config);
        assert_eq!(acp_config.command, "/opt/cursor");
    }

    #[test]
    fn configurator_harness_config_binary_wins() {
        let configurator = CursorConfigurator::new(
            CursorHarnessConfig::builder()
                .binary_path("/cursor-from-config")
                .build(),
        );
        let mut harness_config = HarnessConfig::new("cursor");
        harness_config.executable_path = Some(PathBuf::from("/cursor-from-harness"));
        let acp_config = configurator.build_config(&harness_config);
        assert_eq!(acp_config.command, "/cursor-from-harness");
    }

    #[test]
    fn harness_id_is_cursor() {
        let config = HarnessConfig::new("cursor");
        let harness = AcpHarness::new(CursorConfigurator::default(), config);
        assert_eq!(harness.id().as_str(), "cursor");
    }

    #[test]
    fn capabilities_values() {
        let configurator = CursorConfigurator::default();
        let caps = configurator.capabilities();
        assert!(caps.supports_streaming);
        assert!(caps.supports_tools);
        assert!(caps.supports_sessions);
        assert_eq!(caps.max_context_tokens, 200_000);
        assert!(caps.models.contains(&"claude-sonnet-4-6".into()));
        assert!(caps.models.contains(&"gpt-4o".into()));
        assert_eq!(caps.transport, Some(TransportFlavor::JsonRpcStdio));
        assert_eq!(caps.session_resume, SessionResumeMode::ById);
        assert_eq!(caps.mcp_passthrough, McpMode::Configurable);
        assert_eq!(caps.tool_injection, ToolInjection::McpConfig);
        assert_eq!(caps.cancel, CancelMode::Signal);
        assert!(!caps.multiplex_safe);
    }

    #[test]
    fn initial_status_is_idle() {
        let config = HarnessConfig::new("cursor");
        let harness = AcpHarness::new(CursorConfigurator::default(), config);
        assert!(matches!(harness.status(), HarnessStatus::Idle));
    }

    #[test]
    fn config_defaults() {
        let config = CursorHarnessConfig::default();
        assert_eq!(config.transport, CursorTransport::Acp);
        assert!(config.binary_path.is_none());
    }

    #[test]
    fn cursor_transport_serde_round_trip() {
        let acp = CursorTransport::Acp;
        let json = serde_json::to_string(&acp).expect("serialize");
        assert_eq!(json, r#""acp""#);
        let back: CursorTransport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, CursorTransport::Acp);

        let headless = CursorTransport::Headless;
        let json = serde_json::to_string(&headless).expect("serialize");
        assert_eq!(json, r#""headless""#);
        let back: CursorTransport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, CursorTransport::Headless);
    }

    #[test]
    fn config_builder_defaults() {
        let config = CursorHarnessConfig::builder().build();
        assert_eq!(config.transport, CursorTransport::Acp);
        assert!(config.binary_path.is_none());
    }

    #[test]
    fn config_builder_with_values() {
        let config = CursorHarnessConfig::builder()
            .transport(CursorTransport::Headless)
            .binary_path("/usr/local/bin/cursor")
            .build();
        assert_eq!(config.transport, CursorTransport::Headless);
        assert_eq!(
            config.binary_path,
            Some(PathBuf::from("/usr/local/bin/cursor"))
        );
    }

    #[test]
    fn config_serde_round_trip() {
        let config = CursorHarnessConfig {
            transport: CursorTransport::Headless,
            binary_path: Some(PathBuf::from("/opt/cursor")),
        };
        let json = serde_json::to_string(&config).expect("serialize");
        assert!(json.contains("headless"));
        assert!(json.contains("/opt/cursor"));
        let back: CursorHarnessConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.transport, CursorTransport::Headless);
        assert_eq!(back.binary_path, Some(PathBuf::from("/opt/cursor")));
    }
}
