//! GitHub Copilot CLI harness adapter for the Polkagent platform.
//!
//! This crate provides [`CopilotHarness`] -- an implementation of the
//! [`Harness`] trait that wraps the GitHub Copilot CLI (`gh copilot`) as a
//! one-shot subprocess. Each message invocation spawns a new process, collects
//! the JSON envelope output, and parses it into [`HarnessEvent`] values.
//!
//! # Tier 2 one-shot
//!
//! GitHub Copilot CLI is a **Tier 2 one-shot** harness: each `send_message`
//! spawns `gh copilot suggest` (or `gh copilot explain`), waits for the
//! process to exit, and parses the complete JSON envelope response. There is
//! no persistent subprocess or stdin/stdout streaming.
//!
//! # Output format
//!
//! The CLI produces a single JSON envelope on stdout containing the response
//! text and metadata. The harness parses this envelope and extracts the
//! `response` field as the agent message.
//!
//! # Example
//!
//! ```rust,no_run
//! use polkagent_harness_copilot::{CopilotHarness, CopilotHarnessConfig};
//! use polkagent_harness_trait::{Harness, HarnessConfig, SessionConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = HarnessConfig::new("copilot");
//! let harness = CopilotHarness::new(config, CopilotHarnessConfig::default())?;
//!
//! let session_id = harness.start_session(SessionConfig::default()).await?;
//! harness.send_message(session_id, "How do I list files in Rust?").await?;
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
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use futures::Stream;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use polkagent_harness_trait::{
    CancelMode, CliOutputFormat, Harness, HarnessCapabilities, HarnessConfig, HarnessError,
    HarnessEvent, HarnessId, HarnessStatus, McpMode, SessionConfig, SessionId,
    SessionResumeMode, ToolInjection, TransportFlavor,
    process::ChildProcessRunner,
};

// ---------------------------------------------------------------------------
// CopilotCommand
// ---------------------------------------------------------------------------

/// The Copilot CLI subcommand to invoke.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopilotCommand {
    /// `gh copilot suggest` -- ask for a code suggestion.
    #[default]
    Suggest,
    /// `gh copilot explain` -- ask for an explanation.
    Explain,
}

impl CopilotCommand {
    /// Return the CLI subcommand string.
    fn as_args(&self) -> &[&str] {
        match self {
            Self::Suggest => &["copilot", "suggest"],
            Self::Explain => &["copilot", "explain"],
        }
    }
}

// ---------------------------------------------------------------------------
// CopilotHarnessConfig
// ---------------------------------------------------------------------------

/// Configuration specific to the GitHub Copilot CLI harness.
///
/// Controls the binary path, default subcommand, working directory, and
/// subprocess timeout.
#[derive(Debug, Clone)]
pub struct CopilotHarnessConfig {
    /// Path (or bare name) of the `gh` binary to spawn.
    ///
    /// Defaults to `"gh"`, which expects the binary to be on `$PATH`.
    pub binary_path: String,

    /// Default subcommand to use when sending messages.
    ///
    /// Defaults to [`CopilotCommand::Suggest`].
    pub default_command: CopilotCommand,

    /// Working directory for spawned subprocess sessions.
    ///
    /// If `None`, the subprocess inherits the current working directory
    /// (or uses the session-level `working_directory` if provided).
    pub working_dir: Option<PathBuf>,

    /// Maximum time to wait for a one-shot invocation to complete.
    ///
    /// Defaults to 60 seconds.
    pub timeout: Duration,
}

impl Default for CopilotHarnessConfig {
    fn default() -> Self {
        Self {
            binary_path: "gh".to_owned(),
            default_command: CopilotCommand::default(),
            working_dir: None,
            timeout: Duration::from_secs(60),
        }
    }
}

// ---------------------------------------------------------------------------
// CopilotEnvelope
// ---------------------------------------------------------------------------

/// The JSON envelope returned by `gh copilot` on stdout.
///
/// The exact schema may vary; we parse the known fields and treat
/// anything else as opaque metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopilotEnvelope {
    /// The response text from Copilot.
    #[serde(alias = "text", alias = "message", alias = "suggestion")]
    pub response: Option<String>,

    /// An error message, if the request failed.
    pub error: Option<String>,

    /// The type/kind of response (e.g., "shell", "gh", "git").
    #[serde(rename = "type", alias = "kind")]
    pub response_type: Option<String>,

    /// The Copilot session or conversation ID, if returned.
    #[serde(alias = "conversation_id")]
    pub session_id: Option<String>,
}

impl CopilotEnvelope {
    /// Parse a JSON envelope from raw stdout output.
    ///
    /// Tries to parse the entire output as a single JSON object. If that
    /// fails, falls back to treating the output as plain text.
    pub fn parse(raw: &str) -> Result<Self, HarnessError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(HarnessError::ParseError {
                message: "empty response from Copilot CLI".to_string(),
            });
        }

        // Try JSON parse first.
        if trimmed.starts_with('{') {
            serde_json::from_str(trimmed).map_err(|e| HarnessError::ParseError {
                message: format!("failed to parse Copilot JSON envelope: {e}"),
            })
        } else {
            // Treat as plain text response.
            Ok(Self {
                response: Some(trimmed.to_string()),
                error: None,
                response_type: None,
                session_id: None,
            })
        }
    }

    /// Extract the response text, falling back to an error message or
    /// a generic "no response" message.
    pub fn response_text(&self) -> String {
        if let Some(ref resp) = self.response {
            resp.clone()
        } else if let Some(ref err) = self.error {
            format!("[error] {err}")
        } else {
            "[no response from Copilot]".to_string()
        }
    }

    /// Whether this envelope represents an error response.
    pub fn is_error(&self) -> bool {
        self.error.is_some() && self.response.is_none()
    }
}

// ---------------------------------------------------------------------------
// Session state (internal)
// ---------------------------------------------------------------------------

/// Metadata about a Copilot session, available after the session is started.
#[derive(Debug, Clone)]
pub struct SessionMetadata {
    /// The session identifier.
    pub session_id: SessionId,
    /// The binary path used to invoke Copilot.
    pub binary_path: String,
    /// The working directory for subprocess invocations.
    pub working_dir: Option<PathBuf>,
    /// Whether the session is currently active.
    pub active: bool,
    /// Number of messages sent to this session.
    pub message_count: usize,
}

/// Internal state for a single Copilot session.
struct SessionState {
    /// The session identifier.
    id: SessionId,
    /// The configuration used to start this session.
    #[allow(dead_code)]
    config: SessionConfig,
    /// Messages sent to this session (prompt text).
    messages: Vec<String>,
    /// Responses received for this session.
    responses: Vec<String>,
    /// Whether the session is still active.
    active: bool,
    /// The binary path used for this session.
    binary_path: String,
    /// The working directory used for this session.
    working_dir: Option<PathBuf>,
}

impl std::fmt::Debug for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionState")
            .field("id", &self.id)
            .field("active", &self.active)
            .field("messages", &self.messages.len())
            .field("responses", &self.responses.len())
            .field("binary_path", &self.binary_path)
            .field("working_dir", &self.working_dir)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// CopilotHarness
// ---------------------------------------------------------------------------

/// GitHub Copilot CLI harness implementation.
///
/// Wraps `gh copilot` as a one-shot subprocess, spawning a new process for
/// each message. The harness parses the JSON envelope response and emits
/// structured [`HarnessEvent`] values.
///
/// - [`start_session`] creates a logical session (no subprocess is spawned).
/// - [`send_message`] spawns `gh copilot suggest/explain`, waits for
///   completion, and records the response.
/// - [`receive_events`] returns a stream of collected events.
/// - [`end_session`] marks the session as inactive.
/// - [`health`] checks whether the `gh` binary is available.
///
/// [`start_session`]: Harness::start_session
/// [`send_message`]: Harness::send_message
/// [`receive_events`]: Harness::receive_events
/// [`end_session`]: Harness::end_session
/// [`health`]: Harness::health
pub struct CopilotHarness {
    /// The harness configuration (from the trait layer).
    config: HarnessConfig,
    /// Copilot-specific configuration.
    copilot_config: CopilotHarnessConfig,
    /// Active sessions, keyed by session ID.
    sessions: Mutex<HashMap<SessionId, SessionState>>,
    /// Current operational status.
    status: Mutex<HarnessStatus>,
}

impl std::fmt::Debug for CopilotHarness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CopilotHarness")
            .field("config", &self.config)
            .field("copilot_config", &self.copilot_config)
            .field("active_sessions", &self.active_session_count())
            .finish_non_exhaustive()
    }
}

impl CopilotHarness {
    /// Create a new `CopilotHarness` with the given configuration.
    ///
    /// Validates that the harness ID is `"copilot"` and performs a
    /// basic sanity check on the configuration.
    pub fn new(
        config: HarnessConfig,
        copilot_config: CopilotHarnessConfig,
    ) -> Result<Self, HarnessError> {
        if config.id.as_str() != "copilot" {
            return Err(HarnessError::InvalidState {
                message: format!(
                    "CopilotHarness requires id \"copilot\", got \"{}\"",
                    config.id
                ),
            });
        }

        info!(
            harness_id = %config.id,
            binary_path = %copilot_config.binary_path,
            working_dir = ?copilot_config.working_dir,
            timeout_secs = copilot_config.timeout.as_secs(),
            default_command = ?copilot_config.default_command,
            "CopilotHarness created"
        );

        Ok(Self {
            config,
            copilot_config,
            sessions: Mutex::new(HashMap::new()),
            status: Mutex::new(HarnessStatus::Idle),
        })
    }

    /// Return the resolved executable path for the `gh` CLI.
    ///
    /// Prefers the `HarnessConfig::executable_path` if set, then falls
    /// back to `CopilotHarnessConfig::binary_path`.
    #[must_use]
    pub fn executable_path(&self) -> PathBuf {
        self.config
            .executable_path
            .clone()
            .unwrap_or_else(|| PathBuf::from(&self.copilot_config.binary_path))
    }

    /// Return the number of active sessions.
    #[must_use]
    pub fn active_session_count(&self) -> usize {
        let sessions = self.sessions.lock().expect("sessions mutex poisoned");
        sessions.values().filter(|s| s.active).count()
    }

    /// Return the messages recorded for a given session.
    ///
    /// Returns `None` if the session does not exist.
    #[must_use]
    pub fn session_messages(&self, session_id: SessionId) -> Option<Vec<String>> {
        let sessions = self.sessions.lock().expect("sessions mutex poisoned");
        sessions.get(&session_id).map(|s| s.messages.clone())
    }

    /// Return the responses recorded for a given session.
    ///
    /// Returns `None` if the session does not exist.
    #[must_use]
    pub fn session_responses(&self, session_id: SessionId) -> Option<Vec<String>> {
        let sessions = self.sessions.lock().expect("sessions mutex poisoned");
        sessions.get(&session_id).map(|s| s.responses.clone())
    }

    /// Return metadata about a session.
    ///
    /// Returns `None` if the session does not exist.
    #[must_use]
    pub fn session_metadata(&self, session_id: SessionId) -> Option<SessionMetadata> {
        let sessions = self.sessions.lock().expect("sessions mutex poisoned");
        sessions.get(&session_id).map(|s| SessionMetadata {
            session_id: s.id,
            binary_path: s.binary_path.clone(),
            working_dir: s.working_dir.clone(),
            active: s.active,
            message_count: s.messages.len(),
        })
    }

    /// Return the Copilot-specific configuration.
    #[must_use]
    pub fn copilot_config(&self) -> &CopilotHarnessConfig {
        &self.copilot_config
    }

    /// Resolve the working directory for a session.
    ///
    /// Priority: session config > copilot config > harness config > inherit.
    fn resolve_working_dir(&self, session_config: &SessionConfig) -> Option<PathBuf> {
        session_config
            .working_directory
            .clone()
            .or_else(|| self.copilot_config.working_dir.clone())
            .or_else(|| self.config.workspace_path.clone())
    }

    /// Build a [`ChildProcessRunner`] configured for Copilot CLI invocations.
    fn build_runner(&self, working_dir: Option<&PathBuf>) -> ChildProcessRunner {
        let mut runner = ChildProcessRunner::new(self.executable_path())
            .with_timeout(self.copilot_config.timeout);

        if let Some(dir) = working_dir {
            runner = runner.with_working_dir(dir);
        }

        // Pass through configured environment variables.
        for (key, value) in &self.config.env_vars {
            runner = runner.with_env(key, value);
        }

        runner
    }
}

#[async_trait]
impl Harness for CopilotHarness {
    fn id(&self) -> &HarnessId {
        &self.config.id
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            supports_streaming: false,
            supports_tools: false,
            supports_sessions: false,
            max_context_tokens: 8_192,
            models: vec!["gpt-4".into()],
            transport: Some(TransportFlavor::OneShotCli {
                output_format: CliOutputFormat::JsonEnvelope,
            }),
            model_override: None,
            session_resume: SessionResumeMode::None,
            mcp_passthrough: McpMode::None,
            tool_injection: ToolInjection::None,
            cancel: CancelMode::Signal,
            multiplex_safe: true,
        }
    }

    fn status(&self) -> HarnessStatus {
        let status = self.status.lock().expect("status mutex poisoned");
        status.clone()
    }

    async fn start_session(
        &self,
        config: SessionConfig,
    ) -> Result<SessionId, HarnessError> {
        let session_id = SessionId::new();
        let working_dir = self.resolve_working_dir(&config);

        debug!(
            session_id = %session_id,
            binary = %self.executable_path().display(),
            working_dir = ?working_dir,
            "Starting Copilot session (logical, no subprocess)"
        );

        let state = SessionState {
            id: session_id,
            config,
            messages: Vec::new(),
            responses: Vec::new(),
            active: true,
            binary_path: self.executable_path().display().to_string(),
            working_dir,
        };

        {
            let mut sessions = self.sessions.lock().expect("sessions mutex poisoned");
            sessions.insert(session_id, state);
        }

        {
            let mut status = self.status.lock().expect("status mutex poisoned");
            *status = HarnessStatus::Running {
                since: Utc::now(),
                run_id: None,
            };
        }

        info!(session_id = %session_id, "Copilot session started");
        Ok(session_id)
    }

    async fn send_message(
        &self,
        session_id: SessionId,
        message: &str,
    ) -> Result<(), HarnessError> {
        // Validate session is active and record the message.
        let working_dir = {
            let mut sessions = self.sessions.lock().expect("sessions mutex poisoned");
            let session = sessions.get_mut(&session_id).ok_or(HarnessError::SessionNotFound { session_id })?;

            if !session.active {
                return Err(HarnessError::InvalidState {
                    message: format!("session {session_id} is no longer active"),
                });
            }

            debug!(
                session_id = %session_id,
                message_len = message.len(),
                "Sending message to Copilot CLI (one-shot)"
            );

            session.messages.push(message.to_owned());
            session.working_dir.clone()
        };

        // Build the one-shot runner and execute.
        let runner = self.build_runner(working_dir.as_ref());
        let cmd_args = self.copilot_config.default_command.as_args();

        // Build the full args: ["copilot", "suggest/explain", "--", "<message>"]
        let extra_args: Vec<String> = cmd_args
            .iter()
            .map(|s| (*s).to_string())
            .chain(std::iter::once("--".to_string()))
            .chain(std::iter::once(message.to_string()))
            .collect();

        let extra_refs: Vec<&str> = extra_args.iter().map(String::as_str).collect();
        let raw_output = runner.run_one_shot(&extra_refs).await?;

        // Parse the JSON envelope.
        let envelope = CopilotEnvelope::parse(&raw_output)?;
        let response_text = envelope.response_text();

        if envelope.is_error() {
            warn!(
                session_id = %session_id,
                error = ?envelope.error,
                "Copilot returned error response"
            );
        }

        // Record the response.
        {
            let mut sessions = self.sessions.lock().expect("sessions mutex poisoned");
            if let Some(session) = sessions.get_mut(&session_id) {
                session.responses.push(response_text);
            }
        }

        Ok(())
    }

    async fn receive_events(
        &self,
        session_id: SessionId,
    ) -> Result<Pin<Box<dyn Stream<Item = HarnessEvent> + Send>>, HarnessError> {
        // For a one-shot harness, we return the collected responses as events.
        let responses = {
            let sessions = self.sessions.lock().expect("sessions mutex poisoned");
            let session = sessions.get(&session_id).ok_or(HarnessError::SessionNotFound { session_id })?;
            session.responses.clone()
        };

        debug!(
            session_id = %session_id,
            response_count = responses.len(),
            "Creating event stream from collected Copilot responses"
        );

        let stream = async_stream::stream! {
            yield HarnessEvent::SessionStarted { session_id };

            for response in responses {
                yield HarnessEvent::MessageReceived {
                    session_id,
                    content: response,
                };
            }

            yield HarnessEvent::SessionEnded { session_id };
        };

        Ok(Box::pin(stream))
    }

    async fn end_session(
        &self,
        session_id: SessionId,
    ) -> Result<(), HarnessError> {
        {
            let mut sessions = self.sessions.lock().expect("sessions mutex poisoned");
            let session = sessions.get_mut(&session_id).ok_or(HarnessError::SessionNotFound { session_id })?;

            if !session.active {
                warn!(
                    session_id = %session_id,
                    "end_session called on already-inactive session"
                );
                return Ok(());
            }

            debug!(
                session_id = %session_id,
                messages_sent = session.messages.len(),
                responses_received = session.responses.len(),
                "Ending Copilot session"
            );

            session.active = false;
        }

        // If no more active sessions, return to Idle.
        {
            let sessions = self.sessions.lock().expect("sessions mutex poisoned");
            let any_active = sessions.values().any(|s| s.active);
            if !any_active {
                drop(sessions);
                let mut status = self.status.lock().expect("status mutex poisoned");
                *status = HarnessStatus::Idle;
            }
        }

        info!(session_id = %session_id, "Copilot session ended");
        Ok(())
    }

    async fn health(&self) -> Result<bool, HarnessError> {
        let exe_path = self.executable_path();

        // If an explicit path is configured, check that the file exists.
        if self.config.executable_path.is_some() {
            let exists = exe_path.exists();
            debug!(
                path = %exe_path.display(),
                exists = exists,
                "Copilot health check (explicit path)"
            );
            return Ok(exists);
        }

        // Try to find `gh` on $PATH and verify the copilot extension is available.
        let binary = &self.copilot_config.binary_path;
        debug!(
            binary = %binary,
            "Copilot health check (searching $PATH)"
        );

        let result = tokio::process::Command::new("which")
            .arg(binary)
            .output()
            .await;

        match result {
            Ok(output) => Ok(output.status.success()),
            Err(e) => {
                warn!(error = %e, "Failed to run 'which {}'", binary);
                Ok(false)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use polkagent_harness_trait::{HarnessConfig, SessionConfig};
    use std::path::PathBuf;

    fn test_config() -> HarnessConfig {
        HarnessConfig::new("copilot")
    }

    fn test_config_with_path(path: &str) -> HarnessConfig {
        let mut config = HarnessConfig::new("copilot");
        config.executable_path = Some(PathBuf::from(path));
        config
    }

    /// Helper: create a harness that uses `echo` as the subprocess binary.
    /// This gives us predictable output for testing.
    fn echo_harness() -> CopilotHarness {
        let config = test_config();
        let copilot_config = CopilotHarnessConfig {
            binary_path: "echo".to_owned(),
            ..CopilotHarnessConfig::default()
        };
        CopilotHarness::new(config, copilot_config).expect("echo harness")
    }

    // -----------------------------------------------------------------------
    // CopilotCommand tests
    // -----------------------------------------------------------------------

    #[test]
    fn copilot_command_default_is_suggest() {
        assert_eq!(CopilotCommand::default(), CopilotCommand::Suggest);
    }

    #[test]
    fn copilot_command_suggest_args() {
        let args = CopilotCommand::Suggest.as_args();
        assert_eq!(args, &["copilot", "suggest"]);
    }

    #[test]
    fn copilot_command_explain_args() {
        let args = CopilotCommand::Explain.as_args();
        assert_eq!(args, &["copilot", "explain"]);
    }

    #[test]
    fn copilot_command_serde_round_trip() {
        for cmd in [CopilotCommand::Suggest, CopilotCommand::Explain] {
            let json = serde_json::to_string(&cmd).expect("serialize");
            let back: CopilotCommand = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(cmd, back);
        }
    }

    // -----------------------------------------------------------------------
    // CopilotHarnessConfig tests
    // -----------------------------------------------------------------------

    #[test]
    fn config_defaults_are_sensible() {
        let config = CopilotHarnessConfig::default();
        assert_eq!(config.binary_path, "gh");
        assert_eq!(config.default_command, CopilotCommand::Suggest);
        assert!(config.working_dir.is_none());
        assert_eq!(config.timeout, Duration::from_secs(60));
    }

    #[test]
    fn config_with_custom_binary_path() {
        let config = CopilotHarnessConfig {
            binary_path: "/opt/bin/gh-custom".to_owned(),
            ..CopilotHarnessConfig::default()
        };
        assert_eq!(config.binary_path, "/opt/bin/gh-custom");

        let harness = CopilotHarness::new(test_config(), config).expect("should succeed");
        assert_eq!(
            harness.executable_path(),
            PathBuf::from("/opt/bin/gh-custom")
        );
    }

    #[test]
    fn config_with_explain_command() {
        let config = CopilotHarnessConfig {
            default_command: CopilotCommand::Explain,
            ..CopilotHarnessConfig::default()
        };
        assert_eq!(config.default_command, CopilotCommand::Explain);
    }

    #[test]
    fn config_harness_executable_path_takes_priority() {
        let hconfig = test_config_with_path("/from/harness/config");
        let cconfig = CopilotHarnessConfig {
            binary_path: "/from/copilot/config".to_owned(),
            ..CopilotHarnessConfig::default()
        };
        let harness = CopilotHarness::new(hconfig, cconfig).expect("should succeed");
        assert_eq!(
            harness.executable_path(),
            PathBuf::from("/from/harness/config")
        );
    }

    // -----------------------------------------------------------------------
    // CopilotEnvelope tests
    // -----------------------------------------------------------------------

    #[test]
    fn envelope_parse_json_with_response() {
        let json = r#"{"response": "Hello, world!"}"#;
        let envelope = CopilotEnvelope::parse(json).expect("parse should work");
        assert_eq!(envelope.response, Some("Hello, world!".to_string()));
        assert!(envelope.error.is_none());
        assert!(!envelope.is_error());
        assert_eq!(envelope.response_text(), "Hello, world!");
    }

    #[test]
    fn envelope_parse_json_with_error() {
        let json = r#"{"error": "rate limited"}"#;
        let envelope = CopilotEnvelope::parse(json).expect("parse should work");
        assert!(envelope.response.is_none());
        assert_eq!(envelope.error, Some("rate limited".to_string()));
        assert!(envelope.is_error());
        assert_eq!(envelope.response_text(), "[error] rate limited");
    }

    #[test]
    fn envelope_parse_json_with_both_response_and_error() {
        let json = r#"{"response": "partial", "error": "warning"}"#;
        let envelope = CopilotEnvelope::parse(json).expect("parse should work");
        assert_eq!(envelope.response, Some("partial".to_string()));
        assert!(!envelope.is_error());
        assert_eq!(envelope.response_text(), "partial");
    }

    #[test]
    fn envelope_parse_json_empty_object() {
        let json = r#"{}"#;
        let envelope = CopilotEnvelope::parse(json).expect("parse should work");
        assert!(envelope.response.is_none());
        assert!(envelope.error.is_none());
        assert!(!envelope.is_error());
        assert_eq!(envelope.response_text(), "[no response from Copilot]");
    }

    #[test]
    fn envelope_parse_plain_text() {
        let text = "Just some plain text output";
        let envelope = CopilotEnvelope::parse(text).expect("parse should work");
        assert_eq!(envelope.response, Some(text.to_string()));
        assert!(!envelope.is_error());
    }

    #[test]
    fn envelope_parse_empty_string_fails() {
        let result = CopilotEnvelope::parse("");
        assert!(result.is_err());
    }

    #[test]
    fn envelope_parse_whitespace_only_fails() {
        let result = CopilotEnvelope::parse("   \n  ");
        assert!(result.is_err());
    }

    #[test]
    fn envelope_parse_with_type_field() {
        let json = r#"{"response": "ls -la", "type": "shell"}"#;
        let envelope = CopilotEnvelope::parse(json).expect("parse should work");
        assert_eq!(envelope.response_type, Some("shell".to_string()));
    }

    #[test]
    fn envelope_parse_with_aliases() {
        // Test "text" alias for "response"
        let json = r#"{"text": "Hello from text alias"}"#;
        let envelope = CopilotEnvelope::parse(json).expect("parse should work");
        assert_eq!(envelope.response, Some("Hello from text alias".to_string()));
    }

    #[test]
    fn envelope_parse_with_suggestion_alias() {
        let json = r#"{"suggestion": "git commit -m 'fix'"}"#;
        let envelope = CopilotEnvelope::parse(json).expect("parse should work");
        assert_eq!(envelope.response, Some("git commit -m 'fix'".to_string()));
    }

    #[test]
    fn envelope_parse_with_session_id() {
        let json = r#"{"response": "ok", "conversation_id": "abc-123"}"#;
        let envelope = CopilotEnvelope::parse(json).expect("parse should work");
        assert_eq!(envelope.session_id, Some("abc-123".to_string()));
    }

    // -----------------------------------------------------------------------
    // Harness creation tests
    // -----------------------------------------------------------------------

    #[test]
    fn new_validates_harness_id() {
        let config = HarnessConfig::new("not-copilot");
        let result = CopilotHarness::new(config, CopilotHarnessConfig::default());
        assert!(result.is_err());
        let err = result.expect_err("should fail");
        assert!(err.to_string().contains("copilot"));
    }

    #[test]
    fn new_accepts_copilot_id() {
        let harness = CopilotHarness::new(
            test_config(),
            CopilotHarnessConfig::default(),
        )
        .expect("should succeed");
        assert_eq!(harness.id().as_str(), "copilot");
    }

    #[test]
    fn capabilities_returns_expected_values() {
        let harness = CopilotHarness::new(
            test_config(),
            CopilotHarnessConfig::default(),
        )
        .expect("should succeed");
        let caps = harness.capabilities();
        assert!(!caps.supports_streaming);
        assert!(!caps.supports_tools);
        assert!(!caps.supports_sessions);
        assert_eq!(caps.max_context_tokens, 8_192);
        assert!(!caps.models.is_empty());
        assert!(caps.models.contains(&"gpt-4".to_owned()));
        assert_eq!(
            caps.transport,
            Some(TransportFlavor::OneShotCli {
                output_format: CliOutputFormat::JsonEnvelope,
            })
        );
        assert!(caps.multiplex_safe);
        assert_eq!(caps.mcp_passthrough, McpMode::None);
        assert_eq!(caps.tool_injection, ToolInjection::None);
        assert_eq!(caps.session_resume, SessionResumeMode::None);
        assert_eq!(caps.cancel, CancelMode::Signal);
    }

    #[test]
    fn initial_status_is_idle() {
        let harness = CopilotHarness::new(
            test_config(),
            CopilotHarnessConfig::default(),
        )
        .expect("should succeed");
        assert!(matches!(harness.status(), HarnessStatus::Idle));
    }

    #[test]
    fn executable_path_default() {
        let harness = CopilotHarness::new(
            test_config(),
            CopilotHarnessConfig::default(),
        )
        .expect("should succeed");
        assert_eq!(harness.executable_path(), PathBuf::from("gh"));
    }

    #[test]
    fn executable_path_custom() {
        let harness = CopilotHarness::new(
            test_config_with_path("/opt/bin/gh"),
            CopilotHarnessConfig::default(),
        )
        .expect("should succeed");
        assert_eq!(harness.executable_path(), PathBuf::from("/opt/bin/gh"));
    }

    #[test]
    fn initial_active_session_count_is_zero() {
        let harness = CopilotHarness::new(
            test_config(),
            CopilotHarnessConfig::default(),
        )
        .expect("should succeed");
        assert_eq!(harness.active_session_count(), 0);
    }

    #[test]
    fn copilot_config_accessor() {
        let harness = CopilotHarness::new(
            test_config(),
            CopilotHarnessConfig::default(),
        )
        .expect("should succeed");
        assert_eq!(harness.copilot_config().binary_path, "gh");
    }

    #[test]
    fn debug_impl_does_not_panic() {
        let harness = CopilotHarness::new(
            test_config(),
            CopilotHarnessConfig::default(),
        )
        .expect("should succeed");
        let debug = format!("{harness:?}");
        assert!(debug.contains("CopilotHarness"));
    }

    // -----------------------------------------------------------------------
    // Session lifecycle tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn start_session_creates_logical_session() {
        let harness = echo_harness();

        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start_session should succeed");

        assert_eq!(harness.active_session_count(), 1);
        assert!(matches!(harness.status(), HarnessStatus::Running { .. }));
        assert_ne!(session_id.to_string(), "");
    }

    #[tokio::test]
    async fn session_state_transitions_idle_active_idle() {
        let harness = echo_harness();

        assert!(
            matches!(harness.status(), HarnessStatus::Idle),
            "should start Idle"
        );

        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start");
        assert!(
            matches!(harness.status(), HarnessStatus::Running { .. }),
            "should be Running after start_session"
        );
        assert_eq!(harness.active_session_count(), 1);

        harness.end_session(session_id).await.expect("end");
        assert!(
            matches!(harness.status(), HarnessStatus::Idle),
            "should return to Idle after end_session"
        );
        assert_eq!(harness.active_session_count(), 0);
    }

    #[tokio::test]
    async fn multiple_sessions_can_be_tracked() {
        let harness = echo_harness();

        let s1 = harness
            .start_session(SessionConfig::default())
            .await
            .expect("s1");
        let s2 = harness
            .start_session(SessionConfig::default())
            .await
            .expect("s2");

        assert_ne!(s1, s2);
        assert_eq!(harness.active_session_count(), 2);

        harness.end_session(s1).await.expect("end s1");
        assert_eq!(harness.active_session_count(), 1);
        assert!(matches!(harness.status(), HarnessStatus::Running { .. }));

        harness.end_session(s2).await.expect("end s2");
        assert_eq!(harness.active_session_count(), 0);
        assert!(matches!(harness.status(), HarnessStatus::Idle));
    }

    #[tokio::test]
    async fn send_message_to_nonexistent_session_returns_error() {
        let harness = echo_harness();
        let fake_id = SessionId::new();
        let result = harness.send_message(fake_id, "hello").await;
        assert!(result.is_err());
        assert!(
            matches!(
                result.expect_err("should fail"),
                HarnessError::SessionNotFound { .. }
            ),
            "expected SessionNotFound"
        );
    }

    #[tokio::test]
    async fn end_session_already_ended_is_idempotent() {
        let harness = echo_harness();
        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start");

        harness.end_session(session_id).await.expect("first end");
        harness
            .end_session(session_id)
            .await
            .expect("second end should be idempotent");
    }

    #[tokio::test]
    async fn end_session_nonexistent_returns_error() {
        let harness = echo_harness();
        let fake_id = SessionId::new();
        let result = harness.end_session(fake_id).await;
        assert!(result.is_err());
        assert!(matches!(
            result.expect_err("should fail"),
            HarnessError::SessionNotFound { .. }
        ));
    }

    #[tokio::test]
    async fn session_metadata_is_populated_after_start() {
        let harness = echo_harness();
        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start");

        let meta = harness
            .session_metadata(session_id)
            .expect("metadata should exist");
        assert_eq!(meta.session_id, session_id);
        assert_eq!(meta.binary_path, "echo");
        assert!(meta.active);
        assert_eq!(meta.message_count, 0);

        harness.end_session(session_id).await.expect("end");
    }

    #[tokio::test]
    async fn send_message_to_ended_session_fails() {
        let harness = echo_harness();
        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start");

        harness.end_session(session_id).await.expect("end");

        let result = harness.send_message(session_id, "hello").await;
        assert!(result.is_err());
        assert!(matches!(
            result.expect_err("should fail"),
            HarnessError::InvalidState { .. }
        ));
    }

    #[tokio::test]
    async fn send_message_records_message_and_response() {
        let harness = echo_harness();
        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start");

        // `echo` will output the args as plain text, which gets parsed as
        // a plain text CopilotEnvelope.
        harness
            .send_message(session_id, "test prompt")
            .await
            .expect("send_message should succeed");

        let messages = harness
            .session_messages(session_id)
            .expect("session should exist");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0], "test prompt");

        let responses = harness
            .session_responses(session_id)
            .expect("session should exist");
        assert_eq!(responses.len(), 1);
        // echo outputs: "copilot suggest -- test prompt"
        assert!(responses[0].contains("test prompt"));

        harness.end_session(session_id).await.expect("end");
    }

    #[tokio::test]
    async fn send_multiple_messages_records_all() {
        let harness = echo_harness();
        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start");

        harness.send_message(session_id, "first").await.expect("ok");
        harness.send_message(session_id, "second").await.expect("ok");

        let messages = harness
            .session_messages(session_id)
            .expect("session exists");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0], "first");
        assert_eq!(messages[1], "second");

        let meta = harness
            .session_metadata(session_id)
            .expect("metadata");
        assert_eq!(meta.message_count, 2);

        harness.end_session(session_id).await.expect("end");
    }

    #[tokio::test]
    async fn receive_events_returns_collected_responses() {
        let harness = echo_harness();
        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start");

        // Send a message so there's a response to receive.
        harness
            .send_message(session_id, "test")
            .await
            .expect("send");

        let event_stream = harness
            .receive_events(session_id)
            .await
            .expect("receive_events should succeed");

        let events: Vec<HarnessEvent> = tokio::time::timeout(
            Duration::from_secs(5),
            event_stream.collect(),
        )
        .await
        .expect("event collection should not time out");

        // Should have SessionStarted, MessageReceived, SessionEnded.
        assert_eq!(events.len(), 3, "expected 3 events, got {}", events.len());

        assert!(
            matches!(&events[0], HarnessEvent::SessionStarted { .. }),
            "first event should be SessionStarted"
        );
        assert!(
            matches!(&events[1], HarnessEvent::MessageReceived { .. }),
            "second event should be MessageReceived"
        );
        assert!(
            matches!(&events[2], HarnessEvent::SessionEnded { .. }),
            "last event should be SessionEnded"
        );

        harness.end_session(session_id).await.expect("end");
    }

    #[tokio::test]
    async fn receive_events_with_no_responses() {
        let harness = echo_harness();
        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start");

        // Don't send any messages, so no responses.
        let event_stream = harness
            .receive_events(session_id)
            .await
            .expect("receive_events");

        let events: Vec<HarnessEvent> = tokio::time::timeout(
            Duration::from_secs(5),
            event_stream.collect(),
        )
        .await
        .expect("should not time out");

        // Should just have SessionStarted and SessionEnded.
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], HarnessEvent::SessionStarted { .. }));
        assert!(matches!(&events[1], HarnessEvent::SessionEnded { .. }));

        harness.end_session(session_id).await.expect("end");
    }

    #[tokio::test]
    async fn receive_events_unknown_session_fails() {
        let harness = echo_harness();
        let result = harness.receive_events(SessionId::new()).await;
        assert!(result.is_err());
        let err = result.err().expect("should be an error");
        assert!(
            matches!(err, HarnessError::SessionNotFound { .. }),
            "expected SessionNotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn session_with_working_directory() {
        let harness = echo_harness();
        let session_config = SessionConfig {
            working_directory: Some(PathBuf::from("/tmp")),
            ..SessionConfig::default()
        };

        let session_id = harness
            .start_session(session_config)
            .await
            .expect("start");

        let meta = harness
            .session_metadata(session_id)
            .expect("metadata");
        assert_eq!(meta.working_dir, Some(PathBuf::from("/tmp")));

        harness.end_session(session_id).await.expect("end");
    }

    #[tokio::test]
    async fn health_check_for_existing_binary() {
        // "echo" should exist on all unix systems.
        let config = test_config();
        let copilot_config = CopilotHarnessConfig {
            binary_path: "echo".to_owned(),
            ..CopilotHarnessConfig::default()
        };
        let harness = CopilotHarness::new(config, copilot_config).expect("create");
        let healthy = harness.health().await.expect("health check");
        assert!(healthy, "echo should be found on $PATH");
    }

    #[tokio::test]
    async fn health_check_for_missing_binary() {
        let config = test_config();
        let copilot_config = CopilotHarnessConfig {
            binary_path: "no-such-binary-xyz-999".to_owned(),
            ..CopilotHarnessConfig::default()
        };
        let harness = CopilotHarness::new(config, copilot_config).expect("create");
        let healthy = harness.health().await.expect("health check");
        assert!(!healthy, "nonexistent binary should not be found");
    }

    #[tokio::test]
    async fn health_check_with_explicit_existing_path() {
        let config = test_config_with_path("/bin/echo");
        let harness = CopilotHarness::new(config, CopilotHarnessConfig::default())
            .expect("create");
        let healthy = harness.health().await.expect("health check");
        assert!(healthy, "/bin/echo should exist");
    }

    #[tokio::test]
    async fn health_check_with_explicit_missing_path() {
        let config = test_config_with_path("/nonexistent/path/to/gh");
        let harness = CopilotHarness::new(config, CopilotHarnessConfig::default())
            .expect("create");
        let healthy = harness.health().await.expect("health check");
        assert!(!healthy, "nonexistent path should not exist");
    }

    #[tokio::test]
    async fn session_messages_returns_none_for_unknown_session() {
        let harness = echo_harness();
        assert!(harness.session_messages(SessionId::new()).is_none());
    }

    #[tokio::test]
    async fn session_responses_returns_none_for_unknown_session() {
        let harness = echo_harness();
        assert!(harness.session_responses(SessionId::new()).is_none());
    }

    #[tokio::test]
    async fn session_metadata_returns_none_for_unknown_session() {
        let harness = echo_harness();
        assert!(harness.session_metadata(SessionId::new()).is_none());
    }

    // -----------------------------------------------------------------------
    // Working directory resolution tests
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_working_dir_session_takes_priority() {
        let hconfig = test_config();
        let mut copilot_config = CopilotHarnessConfig::default();
        copilot_config.working_dir = Some(PathBuf::from("/copilot"));
        let harness = CopilotHarness::new(hconfig, copilot_config).expect("create");

        let session_config = SessionConfig {
            working_directory: Some(PathBuf::from("/session")),
            ..SessionConfig::default()
        };
        let resolved = harness.resolve_working_dir(&session_config);
        assert_eq!(resolved, Some(PathBuf::from("/session")));
    }

    #[test]
    fn resolve_working_dir_copilot_config_fallback() {
        let hconfig = test_config();
        let mut copilot_config = CopilotHarnessConfig::default();
        copilot_config.working_dir = Some(PathBuf::from("/copilot"));
        let harness = CopilotHarness::new(hconfig, copilot_config).expect("create");

        let session_config = SessionConfig::default();
        let resolved = harness.resolve_working_dir(&session_config);
        assert_eq!(resolved, Some(PathBuf::from("/copilot")));
    }

    #[test]
    fn resolve_working_dir_harness_config_fallback() {
        let mut hconfig = test_config();
        hconfig.workspace_path = Some(PathBuf::from("/workspace"));
        let copilot_config = CopilotHarnessConfig::default();
        let harness = CopilotHarness::new(hconfig, copilot_config).expect("create");

        let session_config = SessionConfig::default();
        let resolved = harness.resolve_working_dir(&session_config);
        assert_eq!(resolved, Some(PathBuf::from("/workspace")));
    }

    #[test]
    fn resolve_working_dir_none_when_nothing_set() {
        let harness = echo_harness();
        let session_config = SessionConfig::default();
        let resolved = harness.resolve_working_dir(&session_config);
        assert!(resolved.is_none());
    }
}
