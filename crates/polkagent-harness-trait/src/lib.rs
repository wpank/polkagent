//! Harness port trait for the Polkagent platform.
//!
//! This crate defines the [`Harness`] trait — the narrow adapter boundary
//! between the Polkagent kernel and any concrete AI coding agent harness
//! (Claude Code, Codex, etc.). All harness-specific subprocess management,
//! protocol handling, and I/O translation lives in leaf adapter crates;
//! the kernel depends only on this trait.
//!
//! A *harness* wraps a coding agent CLI (like `claude`) as a managed
//! subprocess, exposing a session-based API for sending messages and
//! receiving structured events.
//!
//! # Contract
//!
//! - Implementations must be `Send + Sync + 'static`.
//! - [`Harness::start_session`] creates a new interactive session with the
//!   underlying agent process.
//! - [`Harness::send_message`] delivers a user message to an active session.
//! - [`Harness::receive_events`] returns a stream of structured events from
//!   the session.
//! - [`Harness::end_session`] terminates the session and cleans up resources.
//! - Implementations must **never** log raw prompt content at verbosity
//!   levels that reach telemetry exporters.

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
use std::fmt;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::Stream;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use polkagent_executor_trait::ToolDefinition;

pub mod process;
pub mod tool_names;

// ---------------------------------------------------------------------------
// HarnessId
// ---------------------------------------------------------------------------

/// A string identifier for a harness implementation (e.g., `"claude-code"`,
/// `"codex"`).
///
/// This is a lightweight newtype around `String` that provides type safety
/// and prevents accidental mixing of harness IDs with other string values.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HarnessId(String);

impl HarnessId {
    /// Create a new `HarnessId` from a string value.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Return the inner string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HarnessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for HarnessId {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_owned()))
    }
}

impl From<&str> for HarnessId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for HarnessId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

// ---------------------------------------------------------------------------
// SessionId
// ---------------------------------------------------------------------------

/// A unique identifier for an active harness session.
///
/// This is a UUID v7 wrapper, following the same pattern as other Polkagent
/// ID types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(Uuid);

impl SessionId {
    /// Generate a new time-ordered (v7) session identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wrap an existing [`Uuid`] value.
    #[must_use]
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Return the inner [`Uuid`].
    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for SessionId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

impl From<Uuid> for SessionId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<SessionId> for Uuid {
    fn from(id: SessionId) -> Self {
        id.0
    }
}

// ---------------------------------------------------------------------------
// TransportFlavor & CliOutputFormat
// ---------------------------------------------------------------------------

/// The transport/protocol flavor used to communicate with a harness process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportFlavor {
    /// HTTP-based API (e.g., Codex `app-server`).
    HttpApi,
    /// One-shot CLI invocation that exits after a single response.
    OneShotCli {
        /// How the CLI formats its output.
        output_format: CliOutputFormat,
    },
    /// JSON-RPC over stdin/stdout.
    JsonRpcStdio,
    /// WebSocket-based streaming protocol.
    WebSocket,
    /// The harness itself is an MCP server.
    McpServer,
}

/// Output format for one-shot CLI harness invocations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliOutputFormat {
    /// Streaming JSON (one event per line, each is a complete JSON object).
    StreamJson,
    /// A single JSON envelope wrapping the entire response.
    JsonEnvelope,
    /// Newline-delimited JSON (NDJSON).
    NdJson,
    /// Plain text output.
    PlainText,
}

// ---------------------------------------------------------------------------
// SessionResumeMode
// ---------------------------------------------------------------------------

/// How a harness supports session resumption.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionResumeMode {
    /// No session resume support.
    #[default]
    None,
    /// Resume by passing a session/conversation ID flag.
    ById,
    /// Resume by replaying the conversation history.
    ByReplay,
}

// ---------------------------------------------------------------------------
// McpMode
// ---------------------------------------------------------------------------

/// How a harness interacts with MCP (Model Context Protocol) servers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpMode {
    /// No MCP support.
    #[default]
    None,
    /// The harness can be configured to connect to MCP servers.
    Configurable,
    /// MCP connections are passed through from the caller.
    Passthrough,
}

// ---------------------------------------------------------------------------
// ToolInjection
// ---------------------------------------------------------------------------

/// How tools are injected into a harness session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolInjection {
    /// No external tool injection supported.
    #[default]
    None,
    /// Tools are passed as MCP server definitions.
    McpConfig,
    /// Tools are injected via command-line flags.
    CliFlags,
    /// Tools are configured via a config file.
    ConfigFile,
}

// ---------------------------------------------------------------------------
// CancelMode
// ---------------------------------------------------------------------------

/// How in-flight requests can be cancelled.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelMode {
    /// No cancellation support; must wait for completion or kill the process.
    #[default]
    None,
    /// Cancel by sending a signal (e.g. SIGINT) to the subprocess.
    Signal,
    /// Cancel via an API call or protocol message.
    Api,
}

// ---------------------------------------------------------------------------
// HarnessCapabilities
// ---------------------------------------------------------------------------

/// Describes the capabilities of a harness implementation.
///
/// This is used by the kernel to understand what features a given harness
/// supports and to select appropriate interaction strategies.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct HarnessCapabilities {
    /// Whether the harness supports streaming output (as opposed to only
    /// batch responses).
    pub supports_streaming: bool,
    /// Whether the harness supports tool calling via the agent's built-in
    /// tool protocol.
    pub supports_tools: bool,
    /// Whether the harness supports persistent sessions that can be resumed.
    pub supports_sessions: bool,
    /// Maximum number of context tokens the underlying model supports.
    pub max_context_tokens: u32,
    /// List of model identifiers available through this harness.
    pub models: Vec<String>,

    // -- New fields (all default for backward compat) --
    /// The transport/protocol flavor used by this harness.
    #[serde(default)]
    pub transport: Option<TransportFlavor>,
    /// Override the default model for this harness.
    #[serde(default)]
    pub model_override: Option<String>,
    /// How the harness supports session resumption.
    #[serde(default)]
    pub session_resume: SessionResumeMode,
    /// How the harness handles MCP passthrough.
    #[serde(default)]
    pub mcp_passthrough: McpMode,
    /// How tools are injected into the harness.
    #[serde(default)]
    pub tool_injection: ToolInjection,
    /// How in-flight requests can be cancelled.
    #[serde(default)]
    pub cancel: CancelMode,
    /// Whether the harness is safe for concurrent/multiplexed sessions.
    #[serde(default)]
    pub multiplex_safe: bool,
}

// ---------------------------------------------------------------------------
// HarnessConfig
// ---------------------------------------------------------------------------

/// Configuration for instantiating a harness.
///
/// Provides all the information a harness needs to locate its executable,
/// set up the working directory, and configure subprocess environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessConfig {
    /// The harness implementation identifier.
    pub id: HarnessId,
    /// Path to the harness executable (e.g., `/usr/local/bin/claude`).
    ///
    /// If `None`, the harness should look for the executable on `$PATH`.
    pub executable_path: Option<PathBuf>,
    /// Working directory for the harness subprocess.
    ///
    /// If `None`, the current working directory is used.
    pub workspace_path: Option<PathBuf>,
    /// Maximum time to wait for a response from the harness before timing out.
    pub timeout: Duration,
    /// Additional environment variables to pass to the harness subprocess.
    pub env_vars: HashMap<String, String>,
}

impl HarnessConfig {
    /// Create a new `HarnessConfig` with sensible defaults.
    ///
    /// Uses the given `id`, no explicit executable or workspace path,
    /// a 5-minute timeout, and no extra environment variables.
    #[must_use]
    pub fn new(id: impl Into<HarnessId>) -> Self {
        Self {
            id: id.into(),
            executable_path: None,
            workspace_path: None,
            timeout: Duration::from_secs(300),
            env_vars: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// HarnessStatus
// ---------------------------------------------------------------------------

/// The current operational status of a harness.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HarnessStatus {
    /// The harness is idle and ready to accept sessions.
    Idle,
    /// The harness is currently running a session.
    Running {
        /// When this session started.
        since: DateTime<Utc>,
        /// The identifier of the active run, if associated with one.
        run_id: Option<polkagent_core::RunId>,
    },
    /// The harness is in an error state.
    Error {
        /// Human-readable description of the error.
        message: String,
        /// When the error occurred.
        since: DateTime<Utc>,
    },
}

// ---------------------------------------------------------------------------
// HarnessEvent
// ---------------------------------------------------------------------------

/// A structured event emitted by a harness during a session.
///
/// Events are delivered via the stream returned by
/// [`Harness::receive_events`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum HarnessEvent {
    /// A new session has been established.
    SessionStarted {
        /// The session identifier.
        session_id: SessionId,
    },
    /// A text message was received from the agent.
    MessageReceived {
        /// The session this message belongs to.
        session_id: SessionId,
        /// The text content of the message.
        content: String,
    },
    /// The agent requested a tool call.
    ToolCallRequested {
        /// The session this tool call belongs to.
        session_id: SessionId,
        /// Name of the tool the agent wants to invoke.
        tool_name: String,
        /// JSON-serialized arguments for the tool.
        arguments_json: String,
    },
    /// A tool result was provided back to the agent.
    ToolResultProvided {
        /// The session this result belongs to.
        session_id: SessionId,
        /// Name of the tool that produced the result.
        tool_name: String,
        /// Whether the tool invocation produced an error.
        is_error: bool,
    },
    /// The session has ended normally.
    SessionEnded {
        /// The session that ended.
        session_id: SessionId,
    },
    /// An error occurred during the session.
    Error {
        /// The session where the error occurred.
        session_id: SessionId,
        /// Human-readable description of the error.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// SessionConfig
// ---------------------------------------------------------------------------

/// Configuration for starting a new harness session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionConfig {
    /// System prompt to initialize the session with.
    pub system_prompt: Option<String>,
    /// Tool definitions available to the agent in this session.
    pub tools: Vec<ToolDefinition>,
    /// Working directory for the session.
    ///
    /// If `None`, the harness's configured workspace path (or current
    /// working directory) is used.
    pub working_directory: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// SessionSnapshot
// ---------------------------------------------------------------------------

/// A serializable snapshot of session state for persistence and resumption.
///
/// Written to `.polkagent/state/{session_id}.json` after each turn.
/// Used by [`Harness::resume_session`] to re-attach or re-launch sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// The session identifier.
    pub session_id: SessionId,
    /// The harness that owns this session.
    pub harness_id: HarnessId,
    /// PID of the subprocess, if still alive.
    pub process_pid: Option<u32>,
    /// When the session was started.
    pub started_at: DateTime<Utc>,
    /// Working directory used by the session.
    pub working_directory: Option<PathBuf>,
    /// Number of turns (messages) exchanged.
    pub turn_count: u32,
    /// Backend-specific opaque state (e.g., thread ID, conversation ID).
    pub backend_state: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Session state persistence helpers
// ---------------------------------------------------------------------------

/// Write session state to `.polkagent/state/{session_id}.json`.
pub fn persist_session_state(state: &SessionSnapshot) -> Result<(), HarnessError> {
    let state_dir = PathBuf::from(".polkagent/state");
    std::fs::create_dir_all(&state_dir).map_err(|e| HarnessError::IoError {
        message: format!("failed to create state directory: {e}"),
    })?;
    let path = state_dir.join(format!("{}.json", state.session_id));
    let json = serde_json::to_string_pretty(state).map_err(|e| HarnessError::Internal {
        message: format!("failed to serialize session state: {e}"),
    })?;
    std::fs::write(&path, json).map_err(|e| HarnessError::IoError {
        message: format!("failed to write session state: {e}"),
    })?;
    Ok(())
}

/// Load session state from `.polkagent/state/{session_id}.json`.
pub fn load_session_state(session_id: SessionId) -> Result<SessionSnapshot, HarnessError> {
    let path = PathBuf::from(format!(".polkagent/state/{session_id}.json"));
    let json = std::fs::read_to_string(&path).map_err(|e| HarnessError::IoError {
        message: format!("failed to read session state: {e}"),
    })?;
    serde_json::from_str(&json).map_err(|e| HarnessError::ParseError {
        message: format!("failed to deserialize session state: {e}"),
    })
}

/// Remove session state file for a completed or cancelled session.
pub fn remove_session_state(session_id: SessionId) -> Result<(), HarnessError> {
    let path = PathBuf::from(format!(".polkagent/state/{session_id}.json"));
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| HarnessError::IoError {
            message: format!("failed to remove session state: {e}"),
        })?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that a [`Harness`] implementation may return.
#[derive(Debug, Error)]
pub enum HarnessError {
    /// The harness executable was not found at the configured path.
    #[error("executable not found: {path}")]
    ExecutableNotFound {
        /// The path that was searched.
        path: String,
    },

    /// The session identifier is not recognized or has already ended.
    #[error("session not found: {session_id}")]
    SessionNotFound {
        /// The invalid or expired session identifier.
        session_id: SessionId,
    },

    /// The harness subprocess failed to start.
    #[error("failed to start harness subprocess: {message}")]
    SpawnFailed {
        /// Human-readable description of the failure.
        message: String,
    },

    /// Communication with the harness subprocess failed.
    #[error("I/O error communicating with harness: {message}")]
    IoError {
        /// Human-readable description.
        message: String,
    },

    /// The harness response could not be parsed.
    #[error("failed to parse harness output: {message}")]
    ParseError {
        /// Human-readable description.
        message: String,
    },

    /// The request timed out.
    #[error("harness operation timed out after {elapsed_ms}ms")]
    Timeout {
        /// How long the operation waited before timing out.
        elapsed_ms: u64,
    },

    /// The harness is not in a valid state for the requested operation.
    #[error("invalid harness state: {message}")]
    InvalidState {
        /// Human-readable description.
        message: String,
    },

    /// An unexpected internal error.
    #[error("internal harness error: {message}")]
    Internal {
        /// Human-readable description.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// EventParser trait
// ---------------------------------------------------------------------------

/// Trait for parsing raw stdout/stderr lines into structured [`HarnessEvent`]s.
///
/// Each harness adapter implements this to translate its specific output
/// format into the unified event model.
pub trait EventParser: Send {
    /// Parse a single line from the harness process's stdout.
    fn parse_stdout_line(&mut self, line: &str) -> Vec<HarnessEvent>;

    /// Parse a single line from the harness process's stderr.
    ///
    /// The default implementation wraps each line as an [`HarnessEvent::Error`].
    fn parse_stderr_line(&mut self, line: &str) -> Vec<HarnessEvent> {
        vec![HarnessEvent::Error {
            session_id: SessionId::default(),
            message: line.to_string(),
        }]
    }

    /// Called when the subprocess exits to flush any buffered state.
    fn finalize(&mut self) -> Vec<HarnessEvent> {
        vec![]
    }
}

// ---------------------------------------------------------------------------
// ServiceStatus
// ---------------------------------------------------------------------------

/// The lifecycle status of a [`HarnessService`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStatus {
    /// The service is running and accepting requests.
    Running,
    /// The service is stopped.
    Stopped,
    /// The service is in the process of starting.
    Starting,
    /// The service status cannot be determined.
    Unknown,
}

impl fmt::Display for ServiceStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::Stopped => write!(f, "stopped"),
            Self::Starting => write!(f, "starting"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

// ---------------------------------------------------------------------------
// HarnessService trait
// ---------------------------------------------------------------------------

/// Trait for harnesses that run as long-lived services (e.g. Codex `app-server`).
///
/// Unlike one-shot CLI harnesses, a service harness must be started before
/// sessions can be created, and stopped when no longer needed.
#[async_trait]
pub trait HarnessService: Send + Sync {
    /// Return the name of this service (e.g. `"codex-app-server"`).
    fn service_name(&self) -> &str;

    /// Start the service process.
    async fn start(&self) -> Result<(), HarnessError>;

    /// Stop the service process gracefully.
    async fn stop(&self) -> Result<(), HarnessError>;

    /// Return the current lifecycle status of the service.
    async fn status(&self) -> ServiceStatus;

    /// Perform a health check against the running service.
    async fn healthcheck(&self) -> Result<(), HarnessError>;

    /// Return the PID of the service process, if running.
    fn pid(&self) -> Option<u32>;
}

// ---------------------------------------------------------------------------
// HarnessTaskRequirements & validate_for_task
// ---------------------------------------------------------------------------

/// Describes what a task requires from a harness.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct HarnessTaskRequirements {
    /// Whether the task needs tool calling support.
    pub needs_tools: bool,
    /// Whether the task needs streaming output.
    pub needs_streaming: bool,
    /// Whether the task needs MCP passthrough.
    pub needs_mcp: bool,
    /// Whether the task needs session resume support.
    pub needs_session_resume: bool,
    /// Whether the task needs in-flight cancellation.
    pub needs_cancel: bool,
}

/// A mismatch between a task's requirements and a harness's capabilities.
#[derive(Debug, Clone)]
pub struct CapabilityMismatch {
    /// The requirement that was not met.
    pub requirement: String,
    /// Why the capability is insufficient.
    pub reason: String,
}

impl fmt::Display for CapabilityMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.requirement, self.reason)
    }
}

/// Validate that a harness's capabilities satisfy a task's requirements.
///
/// Returns `Ok(())` if all requirements are met, or `Err` with a list of
/// all unmet requirements.
pub fn validate_for_task(
    capabilities: &HarnessCapabilities,
    requirements: &HarnessTaskRequirements,
) -> Result<(), Vec<CapabilityMismatch>> {
    let mut mismatches = Vec::new();

    if requirements.needs_tools && !capabilities.supports_tools {
        mismatches.push(CapabilityMismatch {
            requirement: "tools".to_string(),
            reason: "harness does not support tool calling".to_string(),
        });
    }
    if requirements.needs_streaming && !capabilities.supports_streaming {
        mismatches.push(CapabilityMismatch {
            requirement: "streaming".to_string(),
            reason: "harness does not support streaming output".to_string(),
        });
    }
    if requirements.needs_mcp && capabilities.mcp_passthrough == McpMode::None {
        mismatches.push(CapabilityMismatch {
            requirement: "mcp".to_string(),
            reason: "harness does not support MCP passthrough".to_string(),
        });
    }
    if requirements.needs_session_resume && capabilities.session_resume == SessionResumeMode::None {
        mismatches.push(CapabilityMismatch {
            requirement: "session_resume".to_string(),
            reason: "harness does not support session resumption".to_string(),
        });
    }
    if requirements.needs_cancel && capabilities.cancel == CancelMode::None {
        mismatches.push(CapabilityMismatch {
            requirement: "cancel".to_string(),
            reason: "harness does not support in-flight cancellation".to_string(),
        });
    }

    if mismatches.is_empty() {
        Ok(())
    } else {
        Err(mismatches)
    }
}

// ---------------------------------------------------------------------------
// Harness trait
// ---------------------------------------------------------------------------

/// The narrow harness port trait.
///
/// One implementation exists per coding agent harness (Claude Code, Codex,
/// etc.). The kernel depends only on this trait; all harness-specific
/// subprocess management and protocol details are confined to adapter crates.
///
/// # Contract
///
/// - Implementations must be `Send + Sync + 'static`.
/// - [`start_session`] creates a new interactive session and returns its ID.
/// - [`send_message`] delivers a message to an active session.
/// - [`receive_events`] returns a stream of events from a session.
/// - [`end_session`] terminates a session and releases resources.
/// - [`health`] performs a lightweight check that the harness is operational.
///
/// [`start_session`]: Harness::start_session
/// [`send_message`]: Harness::send_message
/// [`receive_events`]: Harness::receive_events
/// [`end_session`]: Harness::end_session
/// [`health`]: Harness::health
#[async_trait]
pub trait Harness: Send + Sync + 'static {
    /// Return the identifier for this harness implementation.
    fn id(&self) -> &HarnessId;

    /// Return the capabilities of this harness implementation.
    fn capabilities(&self) -> HarnessCapabilities;

    /// Return the current operational status of this harness.
    fn status(&self) -> HarnessStatus;

    /// Start a new interactive session with the underlying agent.
    ///
    /// Returns a [`SessionId`] that can be used to send messages and
    /// receive events.
    async fn start_session(&self, config: SessionConfig) -> Result<SessionId, HarnessError>;

    /// Send a user message to an active session.
    ///
    /// The message is delivered to the underlying agent's stdin (or
    /// equivalent input channel).
    async fn send_message(&self, session_id: SessionId, message: &str) -> Result<(), HarnessError>;

    /// Return a stream of events from the given session.
    ///
    /// The stream yields [`HarnessEvent`] items as they arrive from the
    /// underlying agent. The stream ends when the session is terminated
    /// (either by calling [`end_session`] or by the agent exiting).
    ///
    /// [`end_session`]: Harness::end_session
    async fn receive_events(
        &self,
        session_id: SessionId,
    ) -> Result<Pin<Box<dyn Stream<Item = HarnessEvent> + Send>>, HarnessError>;

    /// Terminate an active session and release associated resources.
    ///
    /// After this call, the `session_id` is no longer valid.
    async fn end_session(&self, session_id: SessionId) -> Result<(), HarnessError>;

    /// Perform a lightweight health check.
    ///
    /// Returns `Ok(true)` if the harness is operational (executable exists
    /// and is reachable), `Ok(false)` if it is not, or an error if the
    /// check itself fails.
    async fn health(&self) -> Result<bool, HarnessError>;

    /// Save the current session state for later resumption.
    ///
    /// Returns the captured [`SessionSnapshot`]. Implementations should
    /// also persist the snapshot to `.polkagent/state/` via
    /// [`persist_session_state`].
    async fn save_session_state(
        &self,
        session_id: SessionId,
    ) -> Result<SessionSnapshot, HarnessError> {
        let _ = session_id;
        Err(HarnessError::InvalidState {
            message: "session state persistence not supported by this harness".into(),
        })
    }

    /// Resume a previously-saved session by re-attaching to an existing
    /// subprocess or re-launching with saved context.
    ///
    /// Loads the [`SessionSnapshot`] from `.polkagent/state/` and either
    /// reconnects to a still-running process (by PID) or spawns a new
    /// subprocess pre-loaded with conversation history.
    async fn resume_session(&self, session_id: SessionId) -> Result<SessionId, HarnessError> {
        let _ = session_id;
        Err(HarnessError::InvalidState {
            message: "session resumption not supported by this harness".into(),
        })
    }

    /// Cancel an active session by sending SIGTERM and waiting for clean exit.
    ///
    /// The default implementation delegates to [`end_session`].
    ///
    /// [`end_session`]: Harness::end_session
    async fn cancel_session(&self, session_id: SessionId) -> Result<(), HarnessError> {
        self.end_session(session_id).await
    }

    /// Recommended interval for health polling.
    ///
    /// Returns `Some(duration)` if the harness benefits from periodic
    /// health checks (e.g. long-lived subprocess harnesses), or `None`
    /// for one-shot harnesses that do not need polling.
    fn health_interval(&self) -> Option<Duration> {
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_id_display() {
        let id = HarnessId::new("claude-code");
        assert_eq!(id.to_string(), "claude-code");
        assert_eq!(id.as_str(), "claude-code");
    }

    #[test]
    fn harness_id_from_str() {
        let id: HarnessId = "codex".parse().expect("infallible");
        assert_eq!(id.as_str(), "codex");
    }

    #[test]
    fn harness_id_from_string() {
        let id = HarnessId::from("claude-code".to_owned());
        assert_eq!(id.as_str(), "claude-code");
    }

    #[test]
    fn harness_id_serde_round_trip() {
        let id = HarnessId::new("claude-code");
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, r#""claude-code""#);
        let back: HarnessId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }

    #[test]
    fn harness_id_equality() {
        let a = HarnessId::new("claude-code");
        let b = HarnessId::new("claude-code");
        let c = HarnessId::new("codex");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn session_id_new_is_unique() {
        let a = SessionId::new();
        let b = SessionId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn session_id_display_and_from_str_round_trip() {
        let id = SessionId::new();
        let s = id.to_string();
        let parsed: SessionId = s.parse().expect("valid UUID string");
        assert_eq!(id, parsed);
    }

    #[test]
    fn session_id_serde_round_trip() {
        let id = SessionId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        assert!(json.starts_with('"'));
        assert!(json.ends_with('"'));
        let back: SessionId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }

    #[test]
    fn session_id_from_uuid() {
        let uuid = Uuid::now_v7();
        let id = SessionId::from_uuid(uuid);
        assert_eq!(id.as_uuid(), uuid);
    }

    #[test]
    fn harness_capabilities_serializes() {
        let caps = HarnessCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_sessions: false,
            max_context_tokens: 200_000,
            models: vec!["claude-opus-4-6".into()],
            transport: None,
            model_override: None,
            session_resume: SessionResumeMode::default(),
            mcp_passthrough: McpMode::default(),
            tool_injection: ToolInjection::default(),
            cancel: CancelMode::default(),
            multiplex_safe: false,
        };
        let json = serde_json::to_string(&caps).expect("serialize");
        assert!(json.contains("supports_streaming"));
        assert!(json.contains("200000"));
        assert!(json.contains("claude-opus-4-6"));
    }

    #[test]
    fn harness_config_new_defaults() {
        let config = HarnessConfig::new("claude-code");
        assert_eq!(config.id.as_str(), "claude-code");
        assert!(config.executable_path.is_none());
        assert!(config.workspace_path.is_none());
        assert_eq!(config.timeout, Duration::from_secs(300));
        assert!(config.env_vars.is_empty());
    }

    #[test]
    fn harness_config_serializes() {
        let mut config = HarnessConfig::new("claude-code");
        config.executable_path = Some(PathBuf::from("/usr/local/bin/claude"));
        config.env_vars.insert("API_KEY".into(), "secret".into());
        let json = serde_json::to_string(&config).expect("serialize");
        assert!(json.contains("claude-code"));
        assert!(json.contains("/usr/local/bin/claude"));
    }

    #[test]
    fn harness_status_idle_serializes() {
        let status = HarnessStatus::Idle;
        let json = serde_json::to_string(&status).expect("serialize");
        assert!(json.contains("idle"));
    }

    #[test]
    fn harness_status_running_serializes() {
        let status = HarnessStatus::Running {
            since: Utc::now(),
            run_id: None,
        };
        let json = serde_json::to_string(&status).expect("serialize");
        assert!(json.contains("running"));
        assert!(json.contains("since"));
    }

    #[test]
    fn harness_status_error_serializes() {
        let status = HarnessStatus::Error {
            message: "connection lost".into(),
            since: Utc::now(),
        };
        let json = serde_json::to_string(&status).expect("serialize");
        assert!(json.contains("error"));
        assert!(json.contains("connection lost"));
    }

    #[test]
    fn harness_event_session_started_serializes() {
        let event = HarnessEvent::SessionStarted {
            session_id: SessionId::new(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("session_started"));
    }

    #[test]
    fn harness_event_message_received_serializes() {
        let event = HarnessEvent::MessageReceived {
            session_id: SessionId::new(),
            content: "Hello from the agent".into(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("message_received"));
        assert!(json.contains("Hello from the agent"));
    }

    #[test]
    fn harness_event_tool_call_serializes() {
        let event = HarnessEvent::ToolCallRequested {
            session_id: SessionId::new(),
            tool_name: "polkagent.file.read".into(),
            arguments_json: r#"{"path":"/tmp/test"}"#.into(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("tool_call_requested"));
        assert!(json.contains("polkagent.file.read"));
    }

    #[test]
    fn harness_event_error_serializes() {
        let event = HarnessEvent::Error {
            session_id: SessionId::new(),
            message: "subprocess crashed".into(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("subprocess crashed"));
    }

    #[test]
    fn session_config_default() {
        let config = SessionConfig::default();
        assert!(config.system_prompt.is_none());
        assert!(config.tools.is_empty());
        assert!(config.working_directory.is_none());
    }

    #[test]
    fn harness_error_display() {
        let err = HarnessError::ExecutableNotFound {
            path: "/usr/local/bin/claude".into(),
        };
        assert!(err.to_string().contains("/usr/local/bin/claude"));

        let err = HarnessError::SessionNotFound {
            session_id: SessionId::new(),
        };
        assert!(err.to_string().contains("session not found"));
    }

    #[test]
    fn harness_error_variants() {
        // Ensure all error variants are constructible.
        let _spawn = HarnessError::SpawnFailed {
            message: "fail".into(),
        };
        let _io = HarnessError::IoError {
            message: "broken pipe".into(),
        };
        let _parse = HarnessError::ParseError {
            message: "bad json".into(),
        };
        let _timeout = HarnessError::Timeout { elapsed_ms: 5000 };
        let _state = HarnessError::InvalidState {
            message: "not idle".into(),
        };
        let _internal = HarnessError::Internal {
            message: "unexpected".into(),
        };
    }

    /// Compile-time check: `Harness` can be used as a `dyn` trait object.
    #[allow(dead_code)]
    fn _harness_is_object_safe(_h: &dyn Harness) {}

    // -----------------------------------------------------------------------
    // New type tests
    // -----------------------------------------------------------------------

    #[test]
    fn transport_flavor_serde_round_trip() {
        let variants = vec![
            TransportFlavor::HttpApi,
            TransportFlavor::OneShotCli {
                output_format: CliOutputFormat::StreamJson,
            },
            TransportFlavor::JsonRpcStdio,
            TransportFlavor::WebSocket,
            TransportFlavor::McpServer,
        ];
        for v in &variants {
            let json = serde_json::to_string(v).expect("serialize");
            let back: TransportFlavor = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*v, back);
        }
    }

    #[test]
    fn cli_output_format_serde_round_trip() {
        let variants = vec![
            CliOutputFormat::StreamJson,
            CliOutputFormat::JsonEnvelope,
            CliOutputFormat::NdJson,
            CliOutputFormat::PlainText,
        ];
        for v in &variants {
            let json = serde_json::to_string(v).expect("serialize");
            let back: CliOutputFormat = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*v, back);
        }
    }

    #[test]
    fn session_resume_mode_serde_round_trip() {
        let variants = vec![
            SessionResumeMode::None,
            SessionResumeMode::ById,
            SessionResumeMode::ByReplay,
        ];
        for v in &variants {
            let json = serde_json::to_string(v).expect("serialize");
            let back: SessionResumeMode = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*v, back);
        }
    }

    #[test]
    fn mcp_mode_serde_round_trip() {
        let variants = vec![McpMode::None, McpMode::Configurable, McpMode::Passthrough];
        for v in &variants {
            let json = serde_json::to_string(v).expect("serialize");
            let back: McpMode = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*v, back);
        }
    }

    #[test]
    fn tool_injection_serde_round_trip() {
        let variants = vec![
            ToolInjection::None,
            ToolInjection::McpConfig,
            ToolInjection::CliFlags,
            ToolInjection::ConfigFile,
        ];
        for v in &variants {
            let json = serde_json::to_string(v).expect("serialize");
            let back: ToolInjection = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*v, back);
        }
    }

    #[test]
    fn cancel_mode_serde_round_trip() {
        let variants = vec![CancelMode::None, CancelMode::Signal, CancelMode::Api];
        for v in &variants {
            let json = serde_json::to_string(v).expect("serialize");
            let back: CancelMode = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*v, back);
        }
    }

    #[test]
    fn harness_capabilities_backward_compat_deserialize() {
        let old_json = r#"{
            "supports_streaming": true,
            "supports_tools": false,
            "supports_sessions": true,
            "max_context_tokens": 128000,
            "models": ["gpt-4o"]
        }"#;
        let caps: HarnessCapabilities = serde_json::from_str(old_json).expect("deserialize");
        assert!(caps.supports_streaming);
        assert!(!caps.supports_tools);
        assert!(caps.transport.is_none());
        assert_eq!(caps.session_resume, SessionResumeMode::None);
        assert_eq!(caps.mcp_passthrough, McpMode::None);
        assert_eq!(caps.tool_injection, ToolInjection::None);
        assert_eq!(caps.cancel, CancelMode::None);
        assert!(!caps.multiplex_safe);
    }

    #[test]
    fn harness_capabilities_full_round_trip() {
        let caps = HarnessCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_sessions: true,
            max_context_tokens: 200_000,
            models: vec!["claude-opus-4-6".into()],
            transport: Some(TransportFlavor::JsonRpcStdio),
            model_override: Some("claude-sonnet-4-6".into()),
            session_resume: SessionResumeMode::ById,
            mcp_passthrough: McpMode::Passthrough,
            tool_injection: ToolInjection::McpConfig,
            cancel: CancelMode::Signal,
            multiplex_safe: true,
        };
        let json = serde_json::to_string(&caps).expect("serialize");
        let back: HarnessCapabilities = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.transport, Some(TransportFlavor::JsonRpcStdio));
        assert_eq!(back.session_resume, SessionResumeMode::ById);
        assert_eq!(back.mcp_passthrough, McpMode::Passthrough);
        assert_eq!(back.tool_injection, ToolInjection::McpConfig);
        assert_eq!(back.cancel, CancelMode::Signal);
        assert!(back.multiplex_safe);
    }

    fn make_caps(
        tools: bool,
        streaming: bool,
        mcp: McpMode,
        resume: SessionResumeMode,
        cancel: CancelMode,
    ) -> HarnessCapabilities {
        HarnessCapabilities {
            supports_streaming: streaming,
            supports_tools: tools,
            supports_sessions: false,
            max_context_tokens: 128_000,
            models: vec![],
            transport: None,
            model_override: None,
            session_resume: resume,
            mcp_passthrough: mcp,
            tool_injection: ToolInjection::default(),
            cancel,
            multiplex_safe: false,
        }
    }

    #[test]
    fn validate_for_task_all_satisfied() {
        let caps = make_caps(
            true,
            true,
            McpMode::Passthrough,
            SessionResumeMode::ById,
            CancelMode::Signal,
        );
        let reqs = HarnessTaskRequirements {
            needs_tools: true,
            needs_streaming: true,
            needs_mcp: true,
            needs_session_resume: true,
            needs_cancel: true,
        };
        assert!(validate_for_task(&caps, &reqs).is_ok());
    }

    #[test]
    fn validate_for_task_tools_mismatch() {
        let caps = make_caps(
            false,
            true,
            McpMode::default(),
            SessionResumeMode::default(),
            CancelMode::default(),
        );
        let reqs = HarnessTaskRequirements {
            needs_tools: true,
            needs_streaming: false,
            needs_mcp: false,
            needs_session_resume: false,
            needs_cancel: false,
        };
        let errs = validate_for_task(&caps, &reqs).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].requirement, "tools");
    }

    #[test]
    fn validate_for_task_streaming_mismatch() {
        let caps = make_caps(
            true,
            false,
            McpMode::default(),
            SessionResumeMode::default(),
            CancelMode::default(),
        );
        let reqs = HarnessTaskRequirements {
            needs_tools: false,
            needs_streaming: true,
            needs_mcp: false,
            needs_session_resume: false,
            needs_cancel: false,
        };
        let errs = validate_for_task(&caps, &reqs).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].requirement, "streaming");
    }

    #[test]
    fn validate_for_task_mcp_mismatch() {
        let caps = make_caps(
            true,
            true,
            McpMode::None,
            SessionResumeMode::default(),
            CancelMode::default(),
        );
        let reqs = HarnessTaskRequirements {
            needs_tools: false,
            needs_streaming: false,
            needs_mcp: true,
            needs_session_resume: false,
            needs_cancel: false,
        };
        let errs = validate_for_task(&caps, &reqs).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].requirement, "mcp");
    }

    #[test]
    fn validate_for_task_session_resume_mismatch() {
        let caps = make_caps(
            true,
            true,
            McpMode::Passthrough,
            SessionResumeMode::None,
            CancelMode::Signal,
        );
        let reqs = HarnessTaskRequirements {
            needs_tools: false,
            needs_streaming: false,
            needs_mcp: false,
            needs_session_resume: true,
            needs_cancel: false,
        };
        let errs = validate_for_task(&caps, &reqs).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].requirement, "session_resume");
    }

    #[test]
    fn validate_for_task_cancel_mismatch() {
        let caps = make_caps(
            true,
            true,
            McpMode::Passthrough,
            SessionResumeMode::ById,
            CancelMode::None,
        );
        let reqs = HarnessTaskRequirements {
            needs_tools: false,
            needs_streaming: false,
            needs_mcp: false,
            needs_session_resume: false,
            needs_cancel: true,
        };
        let errs = validate_for_task(&caps, &reqs).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].requirement, "cancel");
    }

    #[test]
    fn validate_for_task_multiple_mismatches() {
        let caps = make_caps(
            false,
            false,
            McpMode::None,
            SessionResumeMode::None,
            CancelMode::None,
        );
        let reqs = HarnessTaskRequirements {
            needs_tools: true,
            needs_streaming: true,
            needs_mcp: true,
            needs_session_resume: true,
            needs_cancel: true,
        };
        let errs = validate_for_task(&caps, &reqs).unwrap_err();
        assert_eq!(errs.len(), 5);
        let names: Vec<&str> = errs.iter().map(|e| e.requirement.as_str()).collect();
        assert!(names.contains(&"tools"));
        assert!(names.contains(&"streaming"));
        assert!(names.contains(&"mcp"));
        assert!(names.contains(&"session_resume"));
        assert!(names.contains(&"cancel"));
    }

    #[test]
    fn validate_for_task_no_requirements() {
        let caps = make_caps(
            false,
            false,
            McpMode::default(),
            SessionResumeMode::default(),
            CancelMode::default(),
        );
        let reqs = HarnessTaskRequirements {
            needs_tools: false,
            needs_streaming: false,
            needs_mcp: false,
            needs_session_resume: false,
            needs_cancel: false,
        };
        assert!(validate_for_task(&caps, &reqs).is_ok());
    }

    #[test]
    fn service_status_display() {
        assert_eq!(ServiceStatus::Running.to_string(), "running");
        assert_eq!(ServiceStatus::Stopped.to_string(), "stopped");
        assert_eq!(ServiceStatus::Starting.to_string(), "starting");
        assert_eq!(ServiceStatus::Unknown.to_string(), "unknown");
    }

    #[test]
    fn service_status_serde_round_trip() {
        let variants = vec![
            ServiceStatus::Running,
            ServiceStatus::Stopped,
            ServiceStatus::Starting,
            ServiceStatus::Unknown,
        ];
        for v in &variants {
            let json = serde_json::to_string(v).expect("serialize");
            let back: ServiceStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*v, back);
        }
    }

    #[test]
    fn capability_mismatch_display() {
        let m = CapabilityMismatch {
            requirement: "tools".to_string(),
            reason: "harness does not support tool calling".to_string(),
        };
        assert_eq!(
            m.to_string(),
            "tools: harness does not support tool calling"
        );
    }
}
