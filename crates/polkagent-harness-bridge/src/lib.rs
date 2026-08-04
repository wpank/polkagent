//! Generic HTTP/WebSocket bridge harness for the Polkagent platform.
//!
//! This crate provides [`BridgeHarness`] -- a [`Harness`] implementation that
//! communicates with a remote coding agent over HTTP POST or WebSocket, rather
//! than managing a local subprocess via stdio.
//!
//! Use this harness for agents that expose an HTTP or WebSocket API instead of
//! (or in addition to) a stdio-based interface.
//!
//! # Usage
//!
//! ```rust,no_run
//! use polkagent_harness_bridge::{BridgeHarness, BridgeConfig};
//! use polkagent_harness_trait::{Harness, HarnessConfig, SessionConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let bridge_config = BridgeConfig::builder()
//!     .endpoint("http://localhost:8080/api/agent")
//!     .auth_token("my-secret-token")
//!     .build();
//! let harness_config = HarnessConfig::new("bridge");
//! let harness = BridgeHarness::new(bridge_config, harness_config);
//!
//! let session_id = harness.start_session(SessionConfig::default()).await?;
//! harness.send_message(session_id, "Hello, bridge!").await?;
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

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use futures::Stream;
use serde::{Deserialize, Serialize};

use polkagent_harness_trait::{
    CancelMode, Harness, HarnessCapabilities, HarnessConfig, HarnessError, HarnessEvent, HarnessId,
    HarnessStatus, McpMode, SessionConfig, SessionId, SessionResumeMode, ToolInjection,
    TransportFlavor,
};

// ---------------------------------------------------------------------------
// BridgeTransport
// ---------------------------------------------------------------------------

/// Transport mode for the bridge harness.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeTransport {
    /// HTTP POST-based request/response.
    #[default]
    HttpPost,
    /// WebSocket streaming connection.
    WebSocket,
}

// ---------------------------------------------------------------------------
// BridgeConfig
// ---------------------------------------------------------------------------

/// Configuration for the bridge harness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeConfig {
    /// The endpoint URL for the remote agent.
    pub endpoint: String,
    /// Optional authentication token (sent as `Authorization: Bearer <token>`).
    pub auth_token: Option<String>,
    /// Request timeout.
    pub timeout: Duration,
    /// Transport mode.
    pub transport: BridgeTransport,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://localhost:8080".into(),
            auth_token: None,
            timeout: Duration::from_secs(300),
            transport: BridgeTransport::default(),
        }
    }
}

impl BridgeConfig {
    /// Create a builder for `BridgeConfig`.
    pub fn builder() -> BridgeConfigBuilder {
        BridgeConfigBuilder::default()
    }
}

// ---------------------------------------------------------------------------
// BridgeConfigBuilder
// ---------------------------------------------------------------------------

/// Builder for [`BridgeConfig`].
#[derive(Debug, Default)]
pub struct BridgeConfigBuilder {
    endpoint: Option<String>,
    auth_token: Option<String>,
    timeout: Option<Duration>,
    transport: Option<BridgeTransport>,
}

impl BridgeConfigBuilder {
    /// Set the endpoint URL.
    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// Set the authentication token.
    pub fn auth_token(mut self, token: impl Into<String>) -> Self {
        self.auth_token = Some(token.into());
        self
    }

    /// Set the request timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Set the transport mode.
    pub fn transport(mut self, transport: BridgeTransport) -> Self {
        self.transport = Some(transport);
        self
    }

    /// Build the [`BridgeConfig`].
    pub fn build(self) -> BridgeConfig {
        BridgeConfig {
            endpoint: self
                .endpoint
                .unwrap_or_else(|| "http://localhost:8080".into()),
            auth_token: self.auth_token,
            timeout: self.timeout.unwrap_or(Duration::from_secs(300)),
            transport: self.transport.unwrap_or_default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Request/Response types
// ---------------------------------------------------------------------------

/// A message request sent to the remote agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BridgeRequest {
    session_id: SessionId,
    message: String,
}

/// A message response received from the remote agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
struct BridgeResponse {
    session_id: SessionId,
    content: String,
}

/// A session start request sent to the remote agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BridgeStartRequest {
    session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_prompt: Option<String>,
}

/// A session end request sent to the remote agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BridgeEndRequest {
    session_id: SessionId,
}

// ---------------------------------------------------------------------------
// BridgeHarness
// ---------------------------------------------------------------------------

/// A harness that communicates with a remote coding agent over HTTP or
/// WebSocket.
///
/// Unlike the ACP-based harnesses that manage a local subprocess, this
/// harness forwards messages to an already-running remote agent endpoint.
pub struct BridgeHarness {
    harness_id: HarnessId,
    config: BridgeConfig,
    #[allow(dead_code)]
    harness_config: HarnessConfig,
    client: reqwest::Client,
    status: Mutex<HarnessStatus>,
    sessions: Mutex<HashMap<SessionId, SessionState>>,
}

/// Internal state for a tracked session.
#[derive(Debug)]
struct SessionState {
    #[allow(dead_code)]
    started_at: chrono::DateTime<Utc>,
}

impl std::fmt::Debug for BridgeHarness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeHarness")
            .field("harness_id", &self.harness_id)
            .field("endpoint", &self.config.endpoint)
            .finish()
    }
}

impl BridgeHarness {
    /// Create a new bridge harness with the given configuration.
    pub fn new(config: BridgeConfig, harness_config: HarnessConfig) -> Self {
        let mut builder = reqwest::Client::builder().timeout(config.timeout);

        if let Some(ref token) = config.auth_token {
            let mut headers = reqwest::header::HeaderMap::new();
            if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")) {
                headers.insert(reqwest::header::AUTHORIZATION, val);
            }
            builder = builder.default_headers(headers);
        }

        let client = builder.build().unwrap_or_else(|_| reqwest::Client::new());

        Self {
            harness_id: HarnessId::new("bridge"),
            config,
            harness_config,
            client,
            status: Mutex::new(HarnessStatus::Idle),
            sessions: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl Harness for BridgeHarness {
    fn id(&self) -> &HarnessId {
        &self.harness_id
    }

    fn capabilities(&self) -> HarnessCapabilities {
        let transport = match self.config.transport {
            BridgeTransport::HttpPost => TransportFlavor::HttpApi,
            BridgeTransport::WebSocket => TransportFlavor::WebSocket,
        };
        HarnessCapabilities {
            supports_streaming: matches!(self.config.transport, BridgeTransport::WebSocket),
            supports_tools: true,
            supports_sessions: true,
            max_context_tokens: 200_000,
            models: vec![],
            transport: Some(transport),
            model_override: None,
            session_resume: SessionResumeMode::None,
            mcp_passthrough: McpMode::None,
            tool_injection: ToolInjection::None,
            cancel: CancelMode::None,
            multiplex_safe: true,
        }
    }

    fn status(&self) -> HarnessStatus {
        self.status
            .lock()
            .map_or(HarnessStatus::Idle, |guard| guard.clone())
    }

    async fn start_session(&self, config: SessionConfig) -> Result<SessionId, HarnessError> {
        let session_id = SessionId::new();

        let request = BridgeStartRequest {
            session_id,
            system_prompt: config.system_prompt,
        };

        let url = format!("{}/sessions", self.config.endpoint);
        let resp = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| HarnessError::SpawnFailed {
                message: format!("bridge request failed: {e}"),
            })?;

        if !resp.status().is_success() {
            return Err(HarnessError::SpawnFailed {
                message: format!("bridge returned status {}", resp.status()),
            });
        }

        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.insert(
                session_id,
                SessionState {
                    started_at: Utc::now(),
                },
            );
        }

        if let Ok(mut status) = self.status.lock() {
            *status = HarnessStatus::Running {
                since: Utc::now(),
                run_id: None,
            };
        }

        Ok(session_id)
    }

    async fn send_message(&self, session_id: SessionId, message: &str) -> Result<(), HarnessError> {
        {
            let sessions = self
                .sessions
                .lock()
                .map_err(|_| HarnessError::InvalidState {
                    message: "failed to lock sessions".into(),
                })?;
            if !sessions.contains_key(&session_id) {
                return Err(HarnessError::SessionNotFound { session_id });
            }
        }

        let request = BridgeRequest {
            session_id,
            message: message.to_string(),
        };

        let url = format!("{}/messages", self.config.endpoint);
        let resp = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| HarnessError::IoError {
                message: format!("bridge send failed: {e}"),
            })?;

        if !resp.status().is_success() {
            return Err(HarnessError::IoError {
                message: format!("bridge returned status {}", resp.status()),
            });
        }

        Ok(())
    }

    async fn receive_events(
        &self,
        session_id: SessionId,
    ) -> Result<Pin<Box<dyn Stream<Item = HarnessEvent> + Send>>, HarnessError> {
        {
            let sessions = self
                .sessions
                .lock()
                .map_err(|_| HarnessError::InvalidState {
                    message: "failed to lock sessions".into(),
                })?;
            if !sessions.contains_key(&session_id) {
                return Err(HarnessError::SessionNotFound { session_id });
            }
        }

        // Return an empty stream for now. A full implementation would
        // long-poll the HTTP endpoint or subscribe to a WebSocket channel.
        let stream = futures::stream::empty();
        Ok(Box::pin(stream))
    }

    async fn end_session(&self, session_id: SessionId) -> Result<(), HarnessError> {
        {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| HarnessError::InvalidState {
                    message: "failed to lock sessions".into(),
                })?;
            if sessions.remove(&session_id).is_none() {
                return Err(HarnessError::SessionNotFound { session_id });
            }
        }

        let request = BridgeEndRequest { session_id };

        let url = format!("{}/sessions/end", self.config.endpoint);
        // Best-effort: we don't fail the end_session if the remote call fails.
        let _ = self.client.post(&url).json(&request).send().await;

        if let Ok(mut status) = self.status.lock() {
            *status = HarnessStatus::Idle;
        }

        Ok(())
    }

    async fn health(&self) -> Result<bool, HarnessError> {
        let url = format!("{}/health", self.config.endpoint);
        match self.client.get(&url).send().await {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(_) => Ok(false),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_id_is_bridge() {
        let config = BridgeConfig::default();
        let harness_config = HarnessConfig::new("bridge");
        let harness = BridgeHarness::new(config, harness_config);
        assert_eq!(harness.id().as_str(), "bridge");
    }

    #[test]
    fn initial_status_is_idle() {
        let config = BridgeConfig::default();
        let harness_config = HarnessConfig::new("bridge");
        let harness = BridgeHarness::new(config, harness_config);
        assert!(matches!(harness.status(), HarnessStatus::Idle));
    }

    #[test]
    fn capabilities_http_post() {
        let config = BridgeConfig::builder()
            .transport(BridgeTransport::HttpPost)
            .build();
        let harness_config = HarnessConfig::new("bridge");
        let harness = BridgeHarness::new(config, harness_config);
        let caps = harness.capabilities();
        assert!(!caps.supports_streaming);
        assert!(caps.supports_tools);
        assert!(caps.supports_sessions);
        assert_eq!(caps.transport, Some(TransportFlavor::HttpApi));
        assert!(caps.multiplex_safe);
        assert_eq!(caps.session_resume, SessionResumeMode::None);
        assert_eq!(caps.mcp_passthrough, McpMode::None);
        assert_eq!(caps.cancel, CancelMode::None);
    }

    #[test]
    fn capabilities_websocket() {
        let config = BridgeConfig::builder()
            .transport(BridgeTransport::WebSocket)
            .build();
        let harness_config = HarnessConfig::new("bridge");
        let harness = BridgeHarness::new(config, harness_config);
        let caps = harness.capabilities();
        assert!(caps.supports_streaming);
        assert_eq!(caps.transport, Some(TransportFlavor::WebSocket));
    }

    #[test]
    fn config_defaults() {
        let config = BridgeConfig::default();
        assert_eq!(config.endpoint, "http://localhost:8080");
        assert!(config.auth_token.is_none());
        assert_eq!(config.timeout, Duration::from_secs(300));
        assert_eq!(config.transport, BridgeTransport::HttpPost);
    }

    #[test]
    fn config_builder_defaults() {
        let config = BridgeConfig::builder().build();
        assert_eq!(config.endpoint, "http://localhost:8080");
        assert!(config.auth_token.is_none());
        assert_eq!(config.timeout, Duration::from_secs(300));
        assert_eq!(config.transport, BridgeTransport::HttpPost);
    }

    #[test]
    fn config_builder_with_all_values() {
        let config = BridgeConfig::builder()
            .endpoint("https://agent.example.com/api")
            .auth_token("secret-token-123")
            .timeout(Duration::from_secs(60))
            .transport(BridgeTransport::WebSocket)
            .build();
        assert_eq!(config.endpoint, "https://agent.example.com/api");
        assert_eq!(config.auth_token, Some("secret-token-123".into()));
        assert_eq!(config.timeout, Duration::from_secs(60));
        assert_eq!(config.transport, BridgeTransport::WebSocket);
    }

    #[test]
    fn transport_serde_round_trip() {
        let http = BridgeTransport::HttpPost;
        let json = serde_json::to_string(&http).expect("serialize");
        assert_eq!(json, r#""http_post""#);
        let back: BridgeTransport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, BridgeTransport::HttpPost);

        let ws = BridgeTransport::WebSocket;
        let json = serde_json::to_string(&ws).expect("serialize");
        assert_eq!(json, r#""web_socket""#);
        let back: BridgeTransport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, BridgeTransport::WebSocket);
    }

    #[test]
    fn config_serde_round_trip() {
        let config = BridgeConfig {
            endpoint: "https://example.com/agent".into(),
            auth_token: Some("tok-123".into()),
            timeout: Duration::from_secs(60),
            transport: BridgeTransport::WebSocket,
        };
        let json = serde_json::to_string(&config).expect("serialize");
        assert!(json.contains("example.com"));
        assert!(json.contains("tok-123"));
        let back: BridgeConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.endpoint, "https://example.com/agent");
        assert_eq!(back.auth_token, Some("tok-123".into()));
        assert_eq!(back.transport, BridgeTransport::WebSocket);
    }

    #[test]
    fn harness_with_auth_token() {
        let config = BridgeConfig::builder()
            .endpoint("http://localhost:9090")
            .auth_token("my-secret")
            .build();
        let harness_config = HarnessConfig::new("bridge");
        let harness = BridgeHarness::new(config, harness_config);
        assert_eq!(harness.id().as_str(), "bridge");
    }

    #[test]
    fn debug_impl() {
        let config = BridgeConfig::default();
        let harness_config = HarnessConfig::new("bridge");
        let harness = BridgeHarness::new(config, harness_config);
        let debug = format!("{harness:?}");
        assert!(debug.contains("BridgeHarness"));
        assert!(debug.contains("localhost"));
    }

    #[tokio::test]
    async fn end_session_unknown_session_fails() {
        let config = BridgeConfig::default();
        let harness_config = HarnessConfig::new("bridge");
        let harness = BridgeHarness::new(config, harness_config);
        let session_id = SessionId::new();
        let result = harness.end_session(session_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn send_message_unknown_session_fails() {
        let config = BridgeConfig::default();
        let harness_config = HarnessConfig::new("bridge");
        let harness = BridgeHarness::new(config, harness_config);
        let session_id = SessionId::new();
        let result = harness.send_message(session_id, "hello").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn receive_events_unknown_session_fails() {
        let config = BridgeConfig::default();
        let harness_config = HarnessConfig::new("bridge");
        let harness = BridgeHarness::new(config, harness_config);
        let session_id = SessionId::new();
        let result = harness.receive_events(session_id).await;
        assert!(result.is_err());
    }
}
