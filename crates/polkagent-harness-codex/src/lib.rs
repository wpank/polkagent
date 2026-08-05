//! Codex CLI harness adapter for the Polkagent platform.
//!
//! This crate provides [`CodexHarness`] -- an implementation of the
//! [`Harness`] trait that wraps the Codex CLI (`codex app-server`) as a
//! managed subprocess communicating over a JSON-RPC-like JSONL protocol
//! on stdio.
//!
//! # Protocol
//!
//! Codex uses a bidirectional JSON-RPC 2.0 variant over JSONL, but notably
//! **omits** the `"jsonrpc": "2.0"` field. The lifecycle is:
//!
//! 1. **Handshake**: client sends `initialize` request, waits for response,
//!    then sends `initialized` notification.
//! 2. **Turn lifecycle**: `thread/start` or `thread/resume`, then `turn/start`
//!    with the user's input. The server streams notifications.
//! 3. **Approval flow**: server may send `commandExecution/requestApproval`
//!    or `fileChange/requestApproval` requests that require a response.
//! 4. **Backpressure**: error code `-32001` triggers exponential backoff.
//!
//! # Example
//!
//! ```rust,no_run
//! use polkagent_harness_codex::{CodexHarness, CodexHarnessConfig};
//! use polkagent_harness_trait::{Harness, HarnessConfig, SessionConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = HarnessConfig::new("codex");
//! let harness = CodexHarness::new(config, CodexHarnessConfig::default())?;
//!
//! let session_id = harness.start_session(SessionConfig::default()).await?;
//! harness.send_message(session_id, "Hello, Codex!").await?;
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

pub mod protocol;
pub mod stderr_filter;

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use futures::Stream;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info, trace, warn};

use polkagent_harness_trait::{
    CancelMode, Harness, HarnessCapabilities, HarnessConfig, HarnessError, HarnessEvent, HarnessId,
    HarnessStatus, McpMode, SessionConfig, SessionId, SessionResumeMode, ToolInjection,
    TransportFlavor,
};

use crate::protocol::{
    ClientInfo, CodexNotification, InitializeParams, RawIncoming, RpcNotification, RpcRequest,
    TurnStartParams, BACKPRESSURE_ERROR_CODE,
};

/// Recover session/status state after a panic poisoned a standard mutex.
fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            error!("Codex harness state mutex was poisoned; recovering inner state");
            poisoned.into_inner()
        }
    }
}

#[cfg(unix)]
fn unix_process_id(pid: u32) -> Result<i32, HarnessError> {
    i32::try_from(pid).map_err(|error| HarnessError::Internal {
        message: format!("child process ID {pid} cannot be represented by pid_t: {error}"),
    })
}

#[derive(Default)]
struct NotificationResult {
    events: Vec<HarnessEvent>,
    turn_completed: bool,
}

async fn send_approval_response(
    stdin: &mut Option<tokio::process::ChildStdin>,
    request_id: serde_json::Value,
    approved: bool,
    operation: &str,
) {
    let Some(writer) = stdin.as_mut() else {
        warn!(
            operation,
            "cannot send Codex approval response: stdin is unavailable"
        );
        return;
    };
    let response = serde_json::json!({
        "id": request_id,
        "result": { "approved": approved },
    });
    let json = match serde_json::to_string(&response) {
        Ok(json) => json,
        Err(error) => {
            error!(operation, error = %error, "failed to serialize Codex approval response");
            return;
        }
    };
    if let Err(error) = writer.write_all(format!("{json}\n").as_bytes()).await {
        error!(operation, error = %error, "failed to write Codex approval response");
        return;
    }
    if let Err(error) = writer.flush().await {
        error!(operation, error = %error, "failed to flush Codex approval response");
    }
}

async fn handle_notification(
    notification: CodexNotification,
    session_id: SessionId,
    approval_mode: ApprovalMode,
    message_buffer: &mut String,
    stdin: &mut Option<tokio::process::ChildStdin>,
) -> NotificationResult {
    let mut result = NotificationResult::default();
    match notification {
        CodexNotification::AgentMessageDelta { delta } => message_buffer.push_str(&delta),
        CodexNotification::ItemCompleted {
            item_type,
            text,
            command,
            arguments_json,
            ..
        } => {
            if item_type.as_deref() == Some("agentMessage") {
                let content = text.unwrap_or_else(|| std::mem::take(message_buffer));
                if !content.is_empty() {
                    result.events.push(HarnessEvent::MessageReceived {
                        session_id,
                        content,
                    });
                }
                message_buffer.clear();
            } else if item_type.as_deref() == Some("commandExecution") {
                result.events.push(HarnessEvent::ToolCallRequested {
                    session_id,
                    tool_name: command.unwrap_or_default(),
                    arguments_json: arguments_json.unwrap_or_else(|| "{}".into()),
                });
            }
        }
        CodexNotification::TurnCompleted => {
            if !message_buffer.is_empty() {
                result.events.push(HarnessEvent::MessageReceived {
                    session_id,
                    content: std::mem::take(message_buffer),
                });
            }
            result.turn_completed = true;
        }
        CodexNotification::CommandApprovalRequested {
            request_id,
            command,
        } => {
            let approved = approval_mode == ApprovalMode::Auto;
            debug!(
                session_id = %session_id,
                command,
                approved,
                "handling Codex command approval request"
            );
            send_approval_response(stdin, request_id, approved, "command execution").await;
            result.events.push(HarnessEvent::ToolCallRequested {
                session_id,
                tool_name: command,
                arguments_json: "{}".into(),
            });
        }
        CodexNotification::FileChangeApprovalRequested {
            request_id,
            file_path,
        } => {
            let approved = approval_mode == ApprovalMode::Auto;
            debug!(
                session_id = %session_id,
                file_path,
                approved,
                "handling Codex file-change approval request"
            );
            send_approval_response(stdin, request_id, approved, "file change").await;
        }
        CodexNotification::TurnStarted { .. } | CodexNotification::ItemStarted { .. } => {}
        CodexNotification::Unknown { method } => {
            trace!("Unknown Codex notification: {method}");
        }
    }
    result
}

// ---------------------------------------------------------------------------
// ApprovalMode
// ---------------------------------------------------------------------------

/// How the harness handles Codex approval requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApprovalMode {
    /// Automatically approve all tool execution and file change requests.
    #[default]
    Auto,
    /// Deny all approval requests.
    Deny,
}

// ---------------------------------------------------------------------------
// CodexHarnessConfig
// ---------------------------------------------------------------------------

/// Configuration specific to the Codex harness.
#[derive(Debug, Clone)]
pub struct CodexHarnessConfig {
    /// Path (or bare name) of the `codex` binary to spawn.
    ///
    /// Defaults to `"codex"`, which expects the binary to be on `$PATH`.
    pub binary_path: String,

    /// Working directory for spawned subprocess sessions.
    pub working_dir: Option<PathBuf>,

    /// Maximum time to wait for graceful shutdown before SIGKILL.
    ///
    /// Defaults to 10 seconds.
    pub timeout: Duration,

    /// How to handle Codex approval requests.
    pub approval_mode: ApprovalMode,

    /// Base delay for exponential backoff on backpressure errors.
    ///
    /// Defaults to 100ms.
    pub backpressure_base_delay: Duration,

    /// Maximum number of backpressure retries before giving up.
    ///
    /// Defaults to 5.
    pub backpressure_max_retries: u32,
}

impl Default for CodexHarnessConfig {
    fn default() -> Self {
        Self {
            binary_path: "codex".to_owned(),
            working_dir: None,
            timeout: Duration::from_secs(10),
            approval_mode: ApprovalMode::Auto,
            backpressure_base_delay: Duration::from_millis(100),
            backpressure_max_retries: 5,
        }
    }
}

// ---------------------------------------------------------------------------
// Session state (internal)
// ---------------------------------------------------------------------------

/// Internal state for a single Codex session.
struct SessionState {
    /// The session identifier.
    id: SessionId,
    /// Whether the session is still active.
    active: bool,
    /// The child process handle.
    child: tokio::process::Child,
    /// The child's stdin handle (taken from child on creation).
    stdin: Option<tokio::process::ChildStdin>,
    /// The child's stdout handle (taken from child on creation).
    stdout: Option<tokio::process::ChildStdout>,
    /// Messages sent to this session.
    messages: Vec<String>,
    /// The binary path used to spawn this session (kept for diagnostics).
    #[allow(dead_code)]
    binary_path: String,
    /// The working directory used for this session (kept for diagnostics).
    #[allow(dead_code)]
    working_dir: Option<PathBuf>,
    /// Whether the initialization handshake has completed.
    initialized: bool,
    /// The current thread ID (set after thread/start).
    thread_id: Option<String>,
}

impl std::fmt::Debug for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionState")
            .field("id", &self.id)
            .field("active", &self.active)
            .field("messages", &self.messages.len())
            .field("initialized", &self.initialized)
            .field("thread_id", &self.thread_id)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// CodexHarness
// ---------------------------------------------------------------------------

/// Codex CLI harness implementation.
///
/// Wraps the `codex app-server --listen stdio://` command as a managed
/// subprocess, implementing the JSON-RPC-like JSONL protocol for
/// bidirectional communication.
pub struct CodexHarness {
    /// The harness configuration (from the trait layer).
    config: HarnessConfig,
    /// Codex-specific configuration.
    codex_config: CodexHarnessConfig,
    /// Active sessions, keyed by session ID.
    sessions: Mutex<HashMap<SessionId, SessionState>>,
    /// Current operational status.
    status: Mutex<HarnessStatus>,
    /// Monotonic request ID counter for JSON-RPC.
    next_request_id: AtomicU64,
}

impl std::fmt::Debug for CodexHarness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexHarness")
            .field("config", &self.config)
            .field("codex_config", &self.codex_config)
            .field("active_sessions", &self.active_session_count())
            .finish_non_exhaustive()
    }
}

impl CodexHarness {
    /// Create a new `CodexHarness` with the given configuration.
    pub fn new(
        config: HarnessConfig,
        codex_config: CodexHarnessConfig,
    ) -> Result<Self, HarnessError> {
        if config.id.as_str() != "codex" {
            return Err(HarnessError::InvalidState {
                message: format!("CodexHarness requires id \"codex\", got \"{}\"", config.id),
            });
        }

        info!(
            harness_id = %config.id,
            binary_path = %codex_config.binary_path,
            working_dir = ?codex_config.working_dir,
            approval_mode = ?codex_config.approval_mode,
            "CodexHarness created"
        );

        Ok(Self {
            config,
            codex_config,
            sessions: Mutex::new(HashMap::new()),
            status: Mutex::new(HarnessStatus::Idle),
            next_request_id: AtomicU64::new(1),
        })
    }

    /// Return the resolved executable path for the Codex CLI.
    #[must_use]
    pub fn executable_path(&self) -> PathBuf {
        self.config
            .executable_path
            .clone()
            .unwrap_or_else(|| PathBuf::from(&self.codex_config.binary_path))
    }

    /// Return the number of active sessions.
    #[must_use]
    pub fn active_session_count(&self) -> usize {
        let sessions = lock_or_recover(&self.sessions);
        sessions.values().filter(|s| s.active).count()
    }

    /// Return the Codex-specific configuration.
    #[must_use]
    pub fn codex_config(&self) -> &CodexHarnessConfig {
        &self.codex_config
    }

    /// Allocate the next request ID.
    fn next_id(&self) -> u64 {
        self.next_request_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Resolve the working directory for a session.
    fn resolve_working_dir(&self, session_config: &SessionConfig) -> Option<PathBuf> {
        session_config
            .working_directory
            .clone()
            .or_else(|| self.codex_config.working_dir.clone())
            .or_else(|| self.config.workspace_path.clone())
    }

    /// Send a raw JSON line to the child process stdin.
    ///
    /// This takes stdin out of the session, writes outside the lock, then
    /// puts it back. Callers must hold no lock on `self.sessions`.
    async fn write_line(&self, session_id: SessionId, json_line: &str) -> Result<(), HarnessError> {
        let mut taken_stdin = {
            let mut sessions = lock_or_recover(&self.sessions);
            let session = sessions
                .get_mut(&session_id)
                .ok_or(HarnessError::SessionNotFound { session_id })?;

            if !session.active {
                return Err(HarnessError::InvalidState {
                    message: format!("session {session_id} is no longer active"),
                });
            }

            session.stdin.take().ok_or_else(|| HarnessError::IoError {
                message: format!("session {session_id} stdin is not available"),
            })?
        };

        let line_bytes = format!("{json_line}\n");
        let write_result = taken_stdin.write_all(line_bytes.as_bytes()).await;
        let flush_result = if write_result.is_ok() {
            taken_stdin.flush().await
        } else {
            Ok(())
        };

        // Put stdin back.
        {
            let mut sessions = lock_or_recover(&self.sessions);
            if let Some(session) = sessions.get_mut(&session_id) {
                session.stdin = Some(taken_stdin);
            }
        }

        write_result.map_err(|e| HarnessError::IoError {
            message: format!("failed to write to session {session_id} stdin: {e}"),
        })?;

        flush_result.map_err(|e| HarnessError::IoError {
            message: format!("failed to flush session {session_id} stdin: {e}"),
        })?;

        Ok(())
    }

    /// Send the initialize handshake: `initialize` request + `initialized` notification.
    async fn perform_handshake(&self, session_id: SessionId) -> Result<(), HarnessError> {
        let req_id = self.next_id();

        // Send initialize request.
        let init_req = RpcRequest {
            id: req_id,
            method: "initialize",
            params: InitializeParams {
                client_info: ClientInfo {
                    name: "polkagent".into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                },
                capabilities: None,
            },
        };

        let json = serde_json::to_string(&init_req).map_err(|e| HarnessError::IoError {
            message: format!("failed to serialize initialize request: {e}"),
        })?;

        debug!(session_id = %session_id, "Sending initialize request");
        self.write_line(session_id, &json).await?;

        // Read lines from stdout until we get the initialize response.
        let stdout = {
            let mut sessions = lock_or_recover(&self.sessions);
            let session = sessions
                .get_mut(&session_id)
                .ok_or(HarnessError::SessionNotFound { session_id })?;
            session.stdout.take().ok_or_else(|| HarnessError::IoError {
                message: format!("session {session_id} stdout not available for handshake"),
            })?
        };

        let mut reader = BufReader::new(stdout);
        let mut line_buf = String::new();

        let handshake_timeout = Duration::from_secs(30);
        let result =
            tokio::time::timeout(handshake_timeout, async {
                loop {
                    line_buf.clear();
                    let bytes_read = reader.read_line(&mut line_buf).await.map_err(|e| {
                        HarnessError::IoError {
                            message: format!("failed to read handshake response: {e}"),
                        }
                    })?;

                    if bytes_read == 0 {
                        return Err(HarnessError::IoError {
                            message: "Codex stdout closed during handshake".into(),
                        });
                    }

                    let trimmed = line_buf.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    match serde_json::from_str::<RawIncoming>(trimmed) {
                        Ok(raw) => {
                            // Check if this is our response.
                            if raw.id.as_ref().and_then(serde_json::Value::as_u64) == Some(req_id) {
                                if let Some(err) = &raw.error {
                                    return Err(HarnessError::IoError {
                                        message: format!(
                                            "initialize failed: {} (code {})",
                                            err.message, err.code
                                        ),
                                    });
                                }
                                debug!(session_id = %session_id, "Initialize response received");
                                return Ok(());
                            }
                            // Not our response, skip.
                        }
                        Err(_) => {
                            trace!("Ignoring non-JSON line during handshake: {trimmed}");
                        }
                    }
                }
            })
            .await;

        // Reconstruct the stdout from the reader and put it back.
        let stdout = reader.into_inner();
        {
            let mut sessions = lock_or_recover(&self.sessions);
            if let Some(session) = sessions.get_mut(&session_id) {
                session.stdout = Some(stdout);
            }
        }

        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(HarnessError::Timeout {
                    elapsed_ms: u64::try_from(handshake_timeout.as_millis()).unwrap_or(u64::MAX),
                });
            }
        }

        // Send `initialized` notification.
        let notif = RpcNotification {
            method: "initialized",
            params: serde_json::json!({}),
        };
        let json = serde_json::to_string(&notif).map_err(|e| HarnessError::IoError {
            message: format!("failed to serialize initialized notification: {e}"),
        })?;
        self.write_line(session_id, &json).await?;

        // Mark session as initialized.
        {
            let mut sessions = lock_or_recover(&self.sessions);
            if let Some(session) = sessions.get_mut(&session_id) {
                session.initialized = true;
            }
        }

        debug!(session_id = %session_id, "Codex handshake complete");
        Ok(())
    }

    /// Send `thread/start` and wait for the response to extract the thread ID.
    async fn start_thread(&self, session_id: SessionId) -> Result<String, HarnessError> {
        let req_id = self.next_id();
        let req = RpcRequest {
            id: req_id,
            method: "thread/start",
            params: protocol::ThreadStartParams::default(),
        };
        let json = serde_json::to_string(&req).map_err(|e| HarnessError::IoError {
            message: format!("failed to serialize thread/start: {e}"),
        })?;

        debug!(session_id = %session_id, "Sending thread/start");
        self.write_line(session_id, &json).await?;

        // Wait for the thread/start response to get the thread ID.
        let stdout = {
            let mut sessions = lock_or_recover(&self.sessions);
            let session = sessions
                .get_mut(&session_id)
                .ok_or(HarnessError::SessionNotFound { session_id })?;
            session
                .stdout
                .take()
                .ok_or_else(|| HarnessError::InvalidState {
                    message: format!("session {session_id} stdout not available for thread/start"),
                })?
        };

        let mut reader = BufReader::new(stdout);
        let mut line_buf = String::new();

        let timeout_dur = Duration::from_secs(30);
        let result =
            tokio::time::timeout(timeout_dur, async {
                loop {
                    line_buf.clear();
                    let bytes_read = reader.read_line(&mut line_buf).await.map_err(|e| {
                        HarnessError::IoError {
                            message: format!("failed to read thread/start response: {e}"),
                        }
                    })?;
                    if bytes_read == 0 {
                        return Err(HarnessError::IoError {
                            message: "Codex stdout closed during thread/start".into(),
                        });
                    }
                    let trimmed = line_buf.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<RawIncoming>(trimmed) {
                        Ok(raw) => {
                            if raw.id.as_ref().and_then(serde_json::Value::as_u64) == Some(req_id) {
                                if let Some(err) = &raw.error {
                                    return Err(HarnessError::IoError {
                                        message: format!(
                                            "thread/start failed: {} (code {})",
                                            err.message, err.code
                                        ),
                                    });
                                }
                                // Extract thread ID from result.thread.id
                                let thread_id = raw
                                    .result
                                    .as_ref()
                                    .and_then(|r| r.get("thread"))
                                    .and_then(|t| t.get("id"))
                                    .and_then(|id| id.as_str())
                                    .ok_or_else(|| HarnessError::IoError {
                                        message: "thread/start response missing thread.id".into(),
                                    })?
                                    .to_owned();
                                return Ok(thread_id);
                            }
                        }
                        Err(_) => {
                            trace!("Ignoring non-JSON line during thread/start: {trimmed}");
                        }
                    }
                }
            })
            .await;

        // Put stdout back.
        let stdout = reader.into_inner();
        {
            let mut sessions = lock_or_recover(&self.sessions);
            if let Some(session) = sessions.get_mut(&session_id) {
                session.stdout = Some(stdout);
            }
        }

        match result {
            Ok(Ok(thread_id)) => {
                // Store thread ID in session.
                {
                    let mut sessions = lock_or_recover(&self.sessions);
                    if let Some(session) = sessions.get_mut(&session_id) {
                        session.thread_id = Some(thread_id.clone());
                    }
                }
                debug!(session_id = %session_id, thread_id = %thread_id, "thread/start complete");
                Ok(thread_id)
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err(HarnessError::Timeout {
                elapsed_ms: u64::try_from(timeout_dur.as_millis()).unwrap_or(u64::MAX),
            }),
        }
    }

    /// Send `turn/start` with the user's input.
    async fn start_turn(
        &self,
        session_id: SessionId,
        thread_id: &str,
        user_input: &str,
    ) -> Result<(), HarnessError> {
        let req_id = self.next_id();
        let req = RpcRequest {
            id: req_id,
            method: "turn/start",
            params: TurnStartParams {
                thread_id: thread_id.to_owned(),
                input: vec![protocol::UserInputText {
                    r#type: "text".into(),
                    text: user_input.to_owned(),
                }],
            },
        };
        let json = serde_json::to_string(&req).map_err(|e| HarnessError::IoError {
            message: format!("failed to serialize turn/start: {e}"),
        })?;

        debug!(session_id = %session_id, "Sending turn/start");
        self.write_line(session_id, &json).await
    }
}

#[async_trait]
impl Harness for CodexHarness {
    fn id(&self) -> &HarnessId {
        &self.config.id
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_sessions: true,
            max_context_tokens: 200_000,
            models: vec!["gpt-4.1".into(), "o3".into(), "o4-mini".into()],
            transport: Some(TransportFlavor::JsonRpcStdio),
            model_override: None,
            session_resume: SessionResumeMode::ById,
            mcp_passthrough: McpMode::None,
            tool_injection: ToolInjection::None,
            cancel: CancelMode::Signal,
            multiplex_safe: false,
        }
    }

    fn status(&self) -> HarnessStatus {
        let status = lock_or_recover(&self.status);
        status.clone()
    }

    async fn start_session(&self, config: SessionConfig) -> Result<SessionId, HarnessError> {
        let session_id = SessionId::new();
        let exe_path = self.executable_path();
        let working_dir = self.resolve_working_dir(&config);

        debug!(
            session_id = %session_id,
            binary = %exe_path.display(),
            working_dir = ?working_dir,
            "Starting Codex session"
        );

        // Build subprocess: `codex app-server --listen stdio://`
        let mut cmd = tokio::process::Command::new(&exe_path);
        cmd.arg("app-server")
            .arg("--listen")
            .arg("stdio://")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        if let Some(ref dir) = working_dir {
            cmd.current_dir(dir);
        }

        for (key, value) in &self.config.env_vars {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn().map_err(|e| {
            let kind = e.kind();
            if kind == std::io::ErrorKind::NotFound {
                HarnessError::ExecutableNotFound {
                    path: exe_path.display().to_string(),
                }
            } else {
                HarnessError::SpawnFailed {
                    message: format!("failed to spawn '{}': {e}", exe_path.display()),
                }
            }
        })?;

        // Spawn a stderr drainer that filters benign noise.
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                loop {
                    match lines.next_line().await {
                        Ok(Some(line)) => {
                            if stderr_filter::is_benign(&line) {
                                trace!(target: "codex_stderr", "{}", line);
                            } else {
                                warn!(target: "codex_stderr", "{}", line);
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            debug!("Codex stderr read error: {e}");
                            break;
                        }
                    }
                }
            });
        }

        let stdin = child.stdin.take();
        let stdout = child.stdout.take();

        let state = SessionState {
            id: session_id,
            active: true,
            child,
            stdin,
            stdout,
            messages: Vec::new(),
            binary_path: exe_path.display().to_string(),
            working_dir,
            initialized: false,
            thread_id: None,
        };

        {
            let mut sessions = lock_or_recover(&self.sessions);
            sessions.insert(session_id, state);
        }

        // Update harness status to Running.
        {
            let mut status = lock_or_recover(&self.status);
            *status = HarnessStatus::Running {
                since: Utc::now(),
                run_id: None,
            };
        }

        // Perform the initialize handshake.
        self.perform_handshake(session_id).await?;

        // Start a new thread (stores thread_id in session).
        let _thread_id = self.start_thread(session_id).await?;

        info!(session_id = %session_id, "Codex session started");
        Ok(session_id)
    }

    async fn send_message(&self, session_id: SessionId, message: &str) -> Result<(), HarnessError> {
        // Record the message and grab thread_id.
        let thread_id = {
            let mut sessions = lock_or_recover(&self.sessions);
            let session = sessions
                .get_mut(&session_id)
                .ok_or(HarnessError::SessionNotFound { session_id })?;
            if !session.active {
                return Err(HarnessError::InvalidState {
                    message: format!("session {session_id} is no longer active"),
                });
            }
            if !session.initialized {
                return Err(HarnessError::InvalidState {
                    message: format!("session {session_id} not yet initialized"),
                });
            }
            session.messages.push(message.to_owned());
            session.thread_id.clone()
        };

        let thread_id = thread_id.ok_or_else(|| HarnessError::InvalidState {
            message: format!("session {session_id} has no thread_id (thread/start not completed?)"),
        })?;

        // Send turn/start with the message.
        debug!(
            session_id = %session_id,
            message_len = message.len(),
            "Sending message to Codex session via turn/start"
        );

        // Retry with exponential backoff on backpressure.
        let base_delay = self.codex_config.backpressure_base_delay;
        let max_retries = self.codex_config.backpressure_max_retries;

        for attempt in 0..=max_retries {
            match self.start_turn(session_id, &thread_id, message).await {
                Ok(()) => return Ok(()),
                Err(HarnessError::IoError { ref message })
                    if message.contains("backpressure") && attempt < max_retries =>
                {
                    let delay = base_delay * 2u32.saturating_pow(attempt);
                    warn!(
                        session_id = %session_id,
                        attempt = attempt + 1,
                        delay_ms = delay.as_millis(),
                        "Codex backpressure, retrying"
                    );
                    tokio::time::sleep(delay).await;
                }
                Err(e) => return Err(e),
            }
        }

        Err(HarnessError::IoError {
            message: format!(
                "exceeded {max_retries} backpressure retries for session {session_id}"
            ),
        })
    }

    async fn receive_events(
        &self,
        session_id: SessionId,
    ) -> Result<Pin<Box<dyn Stream<Item = HarnessEvent> + Send>>, HarnessError> {
        let stdout = {
            let mut sessions = lock_or_recover(&self.sessions);
            let session = sessions
                .get_mut(&session_id)
                .ok_or(HarnessError::SessionNotFound { session_id })?;
            session
                .stdout
                .take()
                .ok_or_else(|| HarnessError::InvalidState {
                    message: format!(
                        "session {session_id} stdout already consumed \
                         (event stream can only be created once)"
                    ),
                })?
        };

        // Capture values needed by the stream closure.
        let approval_mode = self.codex_config.approval_mode;

        // We need to send approval responses back on stdin, so we take stdin
        // out as well. We'll manage it inside the stream.
        let stdin = {
            let mut sessions = lock_or_recover(&self.sessions);
            let session = sessions
                .get_mut(&session_id)
                .ok_or(HarnessError::SessionNotFound { session_id })?;
            session.stdin.take()
        };

        debug!(
            session_id = %session_id,
            "Creating event stream from Codex session stdout"
        );

        let reader = BufReader::new(stdout);
        let lines = reader.lines();

        let stream = async_stream::stream! {
            yield HarnessEvent::SessionStarted { session_id };

            let mut lines = lines;
            let mut stdin = stdin;
            let mut message_buffer = String::new();

            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        let trimmed = line.trim().to_owned();
                        if trimmed.is_empty() {
                            continue;
                        }

                        let Ok(raw) = serde_json::from_str::<RawIncoming>(&trimmed) else {
                            // Non-JSON output, treat as message content.
                            yield HarnessEvent::MessageReceived {
                                session_id,
                                content: trimmed,
                            };
                            continue;
                        };

                        // If it has a method, it's a notification or server request.
                        if let Some(ref method) = raw.method {
                            let notif = CodexNotification::from_raw(
                                method,
                                raw.params.as_ref(),
                                raw.id.as_ref(),
                            );

                            let notification_result = handle_notification(
                                notif,
                                session_id,
                                approval_mode,
                                &mut message_buffer,
                                &mut stdin,
                            )
                            .await;
                            for event in notification_result.events {
                                yield event;
                            }
                            if notification_result.turn_completed {
                                break;
                            }
                        } else if let Some(ref err) = raw.error {
                            // Error response.
                            if err.code == BACKPRESSURE_ERROR_CODE {
                                warn!(
                                    session_id = %session_id,
                                    "Received backpressure error from Codex"
                                );
                            } else {
                                yield HarnessEvent::Error {
                                    session_id,
                                    message: format!(
                                        "Codex error (code {}): {}",
                                        err.code, err.message
                                    ),
                                };
                            }
                        }
                        // Successful responses to our requests are silently consumed.
                    }
                    Ok(None) => {
                        // EOF.
                        break;
                    }
                    Err(e) => {
                        yield HarnessEvent::Error {
                            session_id,
                            message: format!("error reading Codex stdout: {e}"),
                        };
                        break;
                    }
                }
            }

            yield HarnessEvent::SessionEnded { session_id };
        };

        Ok(Box::pin(stream))
    }

    async fn end_session(&self, session_id: SessionId) -> Result<(), HarnessError> {
        let timeout = self.codex_config.timeout;

        let pid = {
            let mut sessions = lock_or_recover(&self.sessions);
            let session = sessions
                .get_mut(&session_id)
                .ok_or(HarnessError::SessionNotFound { session_id })?;

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
                "Ending Codex session"
            );

            session.active = false;
            session.stdin.take();
            session.stdout.take();

            session.child.id()
        };

        // Terminate the child process.
        #[cfg(unix)]
        if let Some(pid) = pid {
            use std::time::Instant;

            let raw_pid = unix_process_id(pid)?;
            debug!(session_id = %session_id, pid = raw_pid, "Sending SIGTERM to Codex process");

            // Try SIGTERM first.
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(raw_pid),
                nix::sys::signal::Signal::SIGTERM,
            );

            let deadline = Instant::now() + timeout;
            let exited = loop {
                let process_exited = {
                    let mut sessions = lock_or_recover(&self.sessions);
                    if let Some(session) = sessions.get_mut(&session_id) {
                        match session.child.try_wait() {
                            Ok(None) => false,
                            Ok(Some(_)) => true,
                            Err(error) => {
                                warn!(
                                    session_id = %session_id,
                                    error = %error,
                                    "failed to inspect Codex process; treating it as exited"
                                );
                                true
                            }
                        }
                    } else {
                        true
                    }
                    // MutexGuard dropped here.
                };

                if process_exited {
                    break true;
                }
                if Instant::now() >= deadline {
                    break false;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            };

            if !exited {
                warn!(session_id = %session_id, "SIGTERM timeout, sending SIGKILL");
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(raw_pid),
                    nix::sys::signal::Signal::SIGKILL,
                );

                let mut sessions = lock_or_recover(&self.sessions);
                if let Some(session) = sessions.get_mut(&session_id) {
                    let _ = session.child.start_kill();
                }
            }
        }

        #[cfg(not(unix))]
        {
            let mut sessions = lock_or_recover(&self.sessions);
            if let Some(session) = sessions.get_mut(&session_id) {
                let _ = session.child.start_kill();
            }
        }

        // Update status if no more active sessions.
        {
            let sessions = lock_or_recover(&self.sessions);
            let any_active = sessions.values().any(|s| s.active);
            if !any_active {
                let mut status = lock_or_recover(&self.status);
                *status = HarnessStatus::Idle;
            }
        }

        info!(session_id = %session_id, "Codex session ended");
        Ok(())
    }

    async fn health(&self) -> Result<bool, HarnessError> {
        let exe_path = self.executable_path();

        if exe_path.is_absolute() {
            return Ok(exe_path.exists());
        }

        // For bare command names, check if it's on PATH.
        let result = tokio::process::Command::new(&exe_path)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;

        match result {
            Ok(status) => Ok(status.success()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(HarnessError::IoError {
                message: format!("health check failed: {e}"),
            }),
        }
    }

    async fn save_session_state(
        &self,
        session_id: SessionId,
    ) -> Result<polkagent_harness_trait::SessionSnapshot, HarnessError> {
        let sessions = lock_or_recover(&self.sessions);
        let session = sessions
            .get(&session_id)
            .ok_or(HarnessError::SessionNotFound { session_id })?;

        let pid = session.child.id();

        let mut backend_state = std::collections::HashMap::new();
        if let Some(ref tid) = session.thread_id {
            backend_state.insert("thread_id".into(), serde_json::json!(tid));
        }
        backend_state.insert("initialized".into(), serde_json::json!(session.initialized));
        backend_state.insert(
            "messages".into(),
            serde_json::Value::Array(
                session
                    .messages
                    .iter()
                    .map(|m| serde_json::Value::String(m.clone()))
                    .collect(),
            ),
        );

        let snapshot = polkagent_harness_trait::SessionSnapshot {
            session_id,
            harness_id: self.config.id.clone(),
            process_pid: pid,
            started_at: chrono::Utc::now(),
            working_directory: session.working_dir.clone(),
            turn_count: u32::try_from(session.messages.len()).unwrap_or(u32::MAX),
            backend_state,
        };

        polkagent_harness_trait::persist_session_state(&snapshot)?;
        debug!(session_id = %session_id, "Codex session state saved");
        Ok(snapshot)
    }

    async fn resume_session(&self, session_id: SessionId) -> Result<SessionId, HarnessError> {
        let snapshot = polkagent_harness_trait::load_session_state(session_id)?;

        // Check if the old process is still alive.
        if let Some(pid) = snapshot.process_pid {
            let alive =
                nix::sys::signal::kill(nix::unistd::Pid::from_raw(unix_process_id(pid)?), None)
                    .is_ok();

            if alive {
                debug!(pid, session_id = %session_id, "Re-attaching to live Codex process");
                return Ok(session_id);
            }
        }

        // Re-launch with saved context via a new session.
        let working_dir = snapshot.working_directory.clone();
        let session_config = SessionConfig {
            working_directory: working_dir,
            ..SessionConfig::default()
        };
        let new_id = self.start_session(session_config).await?;

        polkagent_harness_trait::remove_session_state(session_id)?;
        debug!(
            old_session = %session_id,
            new_session = %new_id,
            "Codex session resumed with new process"
        );
        Ok(new_id)
    }

    async fn cancel_session(&self, session_id: SessionId) -> Result<(), HarnessError> {
        #[cfg(unix)]
        {
            let raw_pid = {
                let sessions = lock_or_recover(&self.sessions);
                let session = sessions
                    .get(&session_id)
                    .ok_or(HarnessError::SessionNotFound { session_id })?;
                session.child.id()
            };

            if let Some(pid) = raw_pid {
                debug!(session_id = %session_id, pid, "Sending SIGTERM to Codex process");
                let nix_pid = nix::unistd::Pid::from_raw(unix_process_id(pid)?);
                let _ = nix::sys::signal::kill(nix_pid, nix::sys::signal::Signal::SIGTERM);

                // Wait up to 5 seconds for clean exit.
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                loop {
                    let exited = {
                        let mut sessions = lock_or_recover(&self.sessions);
                        if let Some(session) = sessions.get_mut(&session_id) {
                            matches!(session.child.try_wait(), Ok(Some(_)))
                        } else {
                            true
                        }
                    };
                    if exited || std::time::Instant::now() >= deadline {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }

        polkagent_harness_trait::remove_session_state(session_id).ok();
        self.end_session(session_id).await
    }

    fn health_interval(&self) -> Option<std::time::Duration> {
        Some(std::time::Duration::from_secs(30))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Adapter and mock-protocol tests use `expect` to identify the exact I/O,
// session, or wire-contract invariant that failed.
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    // -- Construction -------------------------------------------------------

    #[test]
    fn codex_harness_requires_codex_id() {
        let config = HarnessConfig::new("wrong-id");
        let result = CodexHarness::new(config, CodexHarnessConfig::default());
        assert!(result.is_err());
        let err = result.expect_err("should fail");
        assert!(err.to_string().contains("codex"));
    }

    #[test]
    fn codex_harness_creates_with_valid_id() {
        let config = HarnessConfig::new("codex");
        let harness = CodexHarness::new(config, CodexHarnessConfig::default());
        assert!(harness.is_ok());
    }

    #[test]
    fn codex_harness_id() {
        let config = HarnessConfig::new("codex");
        let harness = CodexHarness::new(config, CodexHarnessConfig::default()).expect("create");
        assert_eq!(harness.id().as_str(), "codex");
    }

    #[test]
    fn codex_harness_capabilities() {
        let config = HarnessConfig::new("codex");
        let harness = CodexHarness::new(config, CodexHarnessConfig::default()).expect("create");
        let caps = harness.capabilities();
        assert!(caps.supports_streaming);
        assert!(caps.supports_tools);
        assert!(!caps.models.is_empty());
    }

    #[test]
    fn codex_harness_initial_status_is_idle() {
        let config = HarnessConfig::new("codex");
        let harness = CodexHarness::new(config, CodexHarnessConfig::default()).expect("create");
        assert!(matches!(harness.status(), HarnessStatus::Idle));
    }

    #[test]
    fn codex_harness_active_session_count_starts_at_zero() {
        let config = HarnessConfig::new("codex");
        let harness = CodexHarness::new(config, CodexHarnessConfig::default()).expect("create");
        assert_eq!(harness.active_session_count(), 0);
    }

    #[test]
    fn codex_harness_executable_path_default() {
        let config = HarnessConfig::new("codex");
        let harness = CodexHarness::new(config, CodexHarnessConfig::default()).expect("create");
        assert_eq!(harness.executable_path(), PathBuf::from("codex"));
    }

    #[test]
    fn codex_harness_executable_path_override() {
        let mut config = HarnessConfig::new("codex");
        config.executable_path = Some(PathBuf::from("/usr/local/bin/codex"));
        let harness = CodexHarness::new(config, CodexHarnessConfig::default()).expect("create");
        assert_eq!(
            harness.executable_path(),
            PathBuf::from("/usr/local/bin/codex")
        );
    }

    // -- Config -------------------------------------------------------------

    #[test]
    fn codex_harness_config_defaults() {
        let cfg = CodexHarnessConfig::default();
        assert_eq!(cfg.binary_path, "codex");
        assert!(cfg.working_dir.is_none());
        assert_eq!(cfg.timeout, Duration::from_secs(10));
        assert_eq!(cfg.approval_mode, ApprovalMode::Auto);
        assert_eq!(cfg.backpressure_base_delay, Duration::from_millis(100));
        assert_eq!(cfg.backpressure_max_retries, 5);
    }

    #[test]
    fn approval_mode_default_is_auto() {
        assert_eq!(ApprovalMode::default(), ApprovalMode::Auto);
    }

    // -- Session not found --------------------------------------------------

    #[tokio::test]
    async fn send_message_unknown_session_returns_error() {
        let config = HarnessConfig::new("codex");
        let harness = CodexHarness::new(config, CodexHarnessConfig::default()).expect("create");
        let fake_id = SessionId::new();
        let result = harness.send_message(fake_id, "hello").await;
        assert!(result.is_err());
        assert!(result
            .expect_err("err")
            .to_string()
            .contains("session not found"));
    }

    #[tokio::test]
    async fn receive_events_unknown_session_returns_error() {
        let config = HarnessConfig::new("codex");
        let harness = CodexHarness::new(config, CodexHarnessConfig::default()).expect("create");
        let fake_id = SessionId::new();
        let result = harness.receive_events(fake_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn end_session_unknown_session_returns_error() {
        let config = HarnessConfig::new("codex");
        let harness = CodexHarness::new(config, CodexHarnessConfig::default()).expect("create");
        let fake_id = SessionId::new();
        let result = harness.end_session(fake_id).await;
        assert!(result.is_err());
    }

    // -- Request ID generation ----------------------------------------------

    #[test]
    fn next_id_is_monotonic() {
        let config = HarnessConfig::new("codex");
        let harness = CodexHarness::new(config, CodexHarnessConfig::default()).expect("create");
        let id1 = harness.next_id();
        let id2 = harness.next_id();
        let id3 = harness.next_id();
        assert!(id1 < id2);
        assert!(id2 < id3);
    }

    // -- Debug formatting ---------------------------------------------------

    #[test]
    fn codex_harness_debug_format() {
        let config = HarnessConfig::new("codex");
        let harness = CodexHarness::new(config, CodexHarnessConfig::default()).expect("create");
        let debug = format!("{harness:?}");
        assert!(debug.contains("CodexHarness"));
        assert!(debug.contains("codex"));
    }

    // -- Mock stdin/stdout protocol tests -----------------------------------

    /// Helper to create a mock session with piped stdin/stdout for protocol
    /// testing.
    fn create_mock_session_io() -> (
        tokio::io::DuplexStream,
        tokio::io::DuplexStream,
        tokio::io::DuplexStream,
        tokio::io::DuplexStream,
    ) {
        let (client_stdin_write, server_stdin_read) = tokio::io::duplex(8192);
        let (server_stdout_write, client_stdout_read) = tokio::io::duplex(8192);
        (
            client_stdin_write,
            server_stdin_read,
            server_stdout_write,
            client_stdout_read,
        )
    }

    #[tokio::test]
    async fn protocol_handshake_mock() {
        // Simulate a Codex server that responds to initialize.
        let (mut client_write, mut server_read, mut server_write, mut client_read) =
            create_mock_session_io();

        // Server task: read initialize request, send response.
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(&mut server_read);
            let mut line = String::new();
            reader.read_line(&mut line).await.expect("read init");

            // Parse and verify it's an initialize request.
            let raw: serde_json::Value = serde_json::from_str(line.trim()).expect("parse init");
            assert_eq!(raw["method"], "initialize");
            let req_id = raw["id"].as_u64().expect("id");

            // Send response.
            let response = serde_json::json!({
                "id": req_id,
                "result": {
                    "name": "codex",
                    "version": "1.0",
                    "protocolVersion": "1.0"
                }
            });
            let resp_line = format!("{}\n", serde_json::to_string(&response).expect("ser"));
            server_write
                .write_all(resp_line.as_bytes())
                .await
                .expect("write resp");
            server_write.flush().await.expect("flush");

            // Read initialized notification.
            line.clear();
            reader.read_line(&mut line).await.expect("read notif");
            let raw: serde_json::Value = serde_json::from_str(line.trim()).expect("parse notif");
            assert_eq!(raw["method"], "initialized");
        });

        // Client side: write the initialize request.
        let req = protocol::RpcRequest {
            id: 1,
            method: "initialize",
            params: protocol::InitializeParams {
                client_info: protocol::ClientInfo {
                    name: "test".into(),
                    version: "0.1".into(),
                },
                capabilities: None,
            },
        };
        let json = serde_json::to_string(&req).expect("ser");
        let line = format!("{json}\n");
        client_write
            .write_all(line.as_bytes())
            .await
            .expect("write");
        client_write.flush().await.expect("flush");

        // Read response.
        let mut reader = BufReader::new(&mut client_read);
        let mut resp_line = String::new();
        reader.read_line(&mut resp_line).await.expect("read resp");
        let raw: RawIncoming = serde_json::from_str(resp_line.trim()).expect("parse");
        assert!(raw.result.is_some());
        assert_eq!(raw.id.as_ref().and_then(serde_json::Value::as_u64), Some(1));

        // Send initialized notification.
        let notif = protocol::RpcNotification {
            method: "initialized",
            params: serde_json::json!({}),
        };
        let json = serde_json::to_string(&notif).expect("ser");
        let line = format!("{json}\n");
        client_write
            .write_all(line.as_bytes())
            .await
            .expect("write");
        client_write.flush().await.expect("flush");

        server.await.expect("server task");
    }

    #[tokio::test]
    async fn protocol_turn_start_serializes_correctly() {
        let req = protocol::RpcRequest {
            id: 5,
            method: "turn/start",
            params: protocol::TurnStartParams {
                thread_id: "thread-abc".into(),
                input: vec![protocol::UserInputText {
                    r#type: "text".into(),
                    text: "Hello Codex".into(),
                }],
            },
        };
        let json = serde_json::to_string(&req).expect("serialize");
        assert!(json.contains("\"method\":\"turn/start\""));
        assert!(json.contains("\"threadId\":\"thread-abc\""));
        assert!(json.contains("\"text\":\"Hello Codex\""));
        assert!(json.contains("\"id\":5"));
        assert!(!json.contains("jsonrpc"));
    }

    #[test]
    fn notification_stream_parsing() {
        // Simulate a sequence of Codex notifications and verify parsing.
        let messages = vec![
            r#"{"method":"turn/started","params":{"turnId":"t1"}}"#,
            r#"{"method":"item/started","params":{"itemId":"i1","itemType":"agentMessage"}}"#,
            r#"{"method":"item/agentMessage/delta","params":{"delta":"Hello "}}"#,
            r#"{"method":"item/agentMessage/delta","params":{"delta":"world!"}}"#,
            r#"{"method":"item/completed","params":{"itemId":"i1","itemType":"agentMessage","text":"Hello world!"}}"#,
            r#"{"method":"turn/completed","params":{}}"#,
        ];

        let mut notifications = Vec::new();
        for msg in &messages {
            let raw: RawIncoming = serde_json::from_str(msg).expect("parse");
            if let Some(ref method) = raw.method {
                notifications.push(CodexNotification::from_raw(
                    method,
                    raw.params.as_ref(),
                    raw.id.as_ref(),
                ));
            }
        }

        assert_eq!(notifications.len(), 6);
        assert!(matches!(
            notifications[0],
            CodexNotification::TurnStarted { .. }
        ));
        assert!(matches!(
            notifications[1],
            CodexNotification::ItemStarted { .. }
        ));
        assert!(matches!(
            notifications[2],
            CodexNotification::AgentMessageDelta { .. }
        ));
        assert!(matches!(
            notifications[3],
            CodexNotification::AgentMessageDelta { .. }
        ));
        assert!(matches!(
            notifications[4],
            CodexNotification::ItemCompleted { .. }
        ));
        assert!(matches!(notifications[5], CodexNotification::TurnCompleted));
    }

    #[test]
    fn backpressure_error_detection() {
        let json = r#"{"id":10,"error":{"code":-32001,"message":"server overloaded"}}"#;
        let raw: RawIncoming = serde_json::from_str(json).expect("parse");
        let err = raw.error.expect("error");
        assert_eq!(err.code, BACKPRESSURE_ERROR_CODE);
    }
}
