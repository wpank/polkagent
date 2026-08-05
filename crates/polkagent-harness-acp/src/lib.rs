//! Shared ACP (Agent Client Protocol) client for the Polkagent platform.
//!
//! This crate provides [`AcpStdioClient`] -- a reusable JSON-RPC 2.0 over
//! stdio client that implements the Agent Client Protocol. Concrete harness
//! crates for Cursor, Goose, and Kiro use this client via the
//! [`AcpConfigurator`] trait, which supplies the command, arguments, and
//! environment for each agent CLI.
//!
//! # Protocol
//!
//! ACP uses JSON-RPC 2.0 over stdin/stdout with newline-delimited messages.
//! The handshake is:
//!
//! 1. Client sends `initialize` request
//! 2. Server responds with capabilities
//! 3. Client sends `initialized` notification
//!
//! After that, the client can create sessions, send prompts, and receive
//! streaming notifications about agent activity.
//!
//! # Example
//!
//! ```rust,no_run
//! use polkagent_harness_acp::{AcpStdioClient, AcpConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = AcpConfig::cursor("cursor", None);
//! let mut client = AcpStdioClient::new(config);
//! client.connect().await?;
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use futures::Stream;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, info, warn};

use polkagent_harness_trait::{
    Harness, HarnessCapabilities, HarnessConfig, HarnessError, HarnessEvent, HarnessId,
    HarnessStatus, SessionConfig, SessionId,
};

/// Recover adapter state after a panic poisoned a standard mutex.
///
/// Session/status state remains structurally valid because every mutation is
/// performed through ordinary collection and enum assignments. Recovery is
/// preferable to turning a diagnostic/status path into a second panic.
fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            error!("ACP harness state mutex was poisoned; recovering inner state");
            poisoned.into_inner()
        }
    }
}

// ---------------------------------------------------------------------------
// AcpError
// ---------------------------------------------------------------------------

/// Errors specific to ACP protocol communication.
#[derive(Debug, thiserror::Error)]
pub enum AcpError {
    /// Failed to spawn the ACP subprocess.
    #[error("failed to spawn ACP subprocess: {0}")]
    SpawnFailed(String),

    /// An I/O error occurred communicating with the subprocess.
    #[error("ACP I/O error: {0}")]
    IoError(String),

    /// A protocol-level error (malformed JSON-RPC, unexpected response, etc.).
    #[error("ACP protocol error: {0}")]
    ProtocolError(String),

    /// The operation timed out.
    #[error("ACP operation timed out")]
    Timeout,

    /// The client is not connected.
    #[error("ACP client is not connected")]
    NotConnected,

    /// The requested session was not found.
    #[error("ACP session not found")]
    SessionNotFound,
}

impl From<AcpError> for HarnessError {
    fn from(e: AcpError) -> Self {
        match e {
            AcpError::SpawnFailed(msg) => HarnessError::SpawnFailed { message: msg },
            AcpError::IoError(msg) => HarnessError::IoError { message: msg },
            AcpError::ProtocolError(msg) => HarnessError::ParseError { message: msg },
            AcpError::Timeout => HarnessError::Timeout { elapsed_ms: 0 },
            AcpError::NotConnected => HarnessError::InvalidState {
                message: "ACP client is not connected".into(),
            },
            AcpError::SessionNotFound => HarnessError::InvalidState {
                message: "ACP session not found".into(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// AcpConfig
// ---------------------------------------------------------------------------

/// Configuration for an ACP subprocess.
#[derive(Debug, Clone)]
pub struct AcpConfig {
    /// The command to spawn (e.g., `"cursor"`, `"goose"`, `"kiro-cli"`).
    pub command: String,
    /// Arguments to pass to the command.
    pub args: Vec<String>,
    /// Working directory for the subprocess.
    pub cwd: Option<PathBuf>,
    /// Additional environment variables.
    pub env: HashMap<String, String>,
    /// ACP protocol version to advertise.
    pub protocol_version: String,
    /// Timeout for RPC requests.
    pub timeout: Duration,
}

impl AcpConfig {
    /// Create a configuration for the Cursor ACP agent.
    ///
    /// Spawns `cursor agent acp` (or the given binary path).
    pub fn cursor(binary: impl Into<String>, cwd: Option<PathBuf>) -> Self {
        Self {
            command: binary.into(),
            args: vec!["agent".into(), "acp".into()],
            cwd,
            env: HashMap::new(),
            protocol_version: "0.12.2".into(),
            timeout: Duration::from_secs(300),
        }
    }

    /// Create a configuration for the Goose ACP agent.
    ///
    /// Spawns `goose acp` (or the given binary path).
    pub fn goose(binary: impl Into<String>, cwd: Option<PathBuf>) -> Self {
        Self {
            command: binary.into(),
            args: vec!["acp".into()],
            cwd,
            env: HashMap::new(),
            protocol_version: "0.12.2".into(),
            timeout: Duration::from_secs(300),
        }
    }

    /// Create a configuration for the Kiro ACP agent.
    ///
    /// Spawns `kiro-cli acp` (or the given binary path).
    pub fn kiro(binary: impl Into<String>, cwd: Option<PathBuf>) -> Self {
        Self {
            command: binary.into(),
            args: vec!["acp".into()],
            cwd,
            env: HashMap::new(),
            protocol_version: "0.12.2".into(),
            timeout: Duration::from_secs(300),
        }
    }

    /// Create a configuration for the `OpenCode` ACP agent.
    ///
    /// Spawns `opencode acp` (or the given binary path).
    pub fn opencode(binary: impl Into<String>, cwd: Option<PathBuf>) -> Self {
        Self {
            command: binary.into(),
            args: vec!["acp".into()],
            cwd,
            env: HashMap::new(),
            protocol_version: "0.12.2".into(),
            timeout: Duration::from_secs(300),
        }
    }
}

// ---------------------------------------------------------------------------
// AcpNotification / AcpSessionUpdate / AcpPermission / AcpUsage
// ---------------------------------------------------------------------------

/// A notification received from the ACP server.
#[derive(Debug, Clone)]
pub enum AcpNotification {
    /// A session update notification.
    SessionUpdate {
        /// The session this update belongs to.
        session_key: String,
        /// The update payload.
        update: AcpSessionUpdate,
    },
    /// A permission request from the server.
    PermissionRequest {
        /// The JSON-RPC request ID (used to respond).
        id: u64,
        /// The session this permission request belongs to.
        session_key: String,
        /// The permission being requested.
        permission: AcpPermission,
    },
}

/// An update within an ACP session.
#[derive(Debug, Clone)]
pub enum AcpSessionUpdate {
    /// A chunk of the agent's message text.
    AgentMessageChunk {
        /// The text content of the chunk.
        text: String,
    },
    /// The agent is calling a tool.
    ToolCall {
        /// Name of the tool being called.
        tool_name: String,
        /// Arguments passed to the tool.
        arguments: serde_json::Value,
    },
    /// An update on a tool call's output.
    ToolCallUpdate {
        /// Name of the tool producing output.
        tool_name: String,
        /// The output text.
        output: String,
    },
    /// The agent's turn has ended.
    TurnEnd {
        /// Optional usage statistics.
        usage: Option<AcpUsage>,
    },
}

/// A permission the ACP server is requesting from the client.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AcpPermission {
    /// The type of permission being requested.
    #[serde(rename = "type")]
    pub permission_type: String,
    /// Description of what the agent wants to do.
    pub description: Option<String>,
    /// Additional metadata about the permission request.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Token usage statistics for an ACP session turn.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AcpUsage {
    /// Number of input tokens consumed.
    pub input_tokens: Option<u64>,
    /// Number of output tokens generated.
    pub output_tokens: Option<u64>,
}

// ---------------------------------------------------------------------------
// AcpStdioClient
// ---------------------------------------------------------------------------

/// A JSON-RPC 2.0 over stdio client for the Agent Client Protocol.
///
/// This client manages a subprocess, sends JSON-RPC requests via stdin,
/// and reads responses/notifications from stdout via a background reader
/// task.
pub struct AcpStdioClient {
    config: AcpConfig,
    child: Option<Child>,
    stdin: Option<BufWriter<ChildStdin>>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    notification_tx: mpsc::UnboundedSender<AcpNotification>,
    notification_rx: Option<mpsc::UnboundedReceiver<AcpNotification>>,
    session_id: Option<String>,
    reader_handle: Option<tokio::task::JoinHandle<()>>,
}

impl std::fmt::Debug for AcpStdioClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpStdioClient")
            .field("command", &self.config.command)
            .field("connected", &self.child.is_some())
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl AcpStdioClient {
    /// Create a new ACP client with the given configuration.
    ///
    /// The client is not connected until [`connect`] is called.
    ///
    /// [`connect`]: AcpStdioClient::connect
    pub fn new(config: AcpConfig) -> Self {
        let (notification_tx, notification_rx) = mpsc::unbounded_channel();
        Self {
            config,
            child: None,
            stdin: None,
            next_id: AtomicU64::new(1),
            pending: Arc::new(Mutex::new(HashMap::new())),
            notification_tx,
            notification_rx: Some(notification_rx),
            session_id: None,
            reader_handle: None,
        }
    }

    /// Return a reference to the configuration.
    pub fn config(&self) -> &AcpConfig {
        &self.config
    }

    /// Take the notification receiver channel.
    ///
    /// This can only be called once; subsequent calls return `None`.
    /// Use this to consume notifications from the ACP server in your
    /// own event loop.
    pub fn take_notifications(&mut self) -> Option<mpsc::UnboundedReceiver<AcpNotification>> {
        self.notification_rx.take()
    }

    /// Connect to the ACP server by spawning the subprocess and
    /// performing the initialize handshake.
    pub async fn connect(&mut self) -> Result<serde_json::Value, AcpError> {
        let mut cmd = tokio::process::Command::new(&self.config.command);
        cmd.args(&self.config.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        if let Some(ref cwd) = self.config.cwd {
            cmd.current_dir(cwd);
        }

        for (key, value) in &self.config.env {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn().map_err(|e| {
            AcpError::SpawnFailed(format!("failed to spawn '{}': {e}", self.config.command))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AcpError::SpawnFailed("failed to capture stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AcpError::SpawnFailed("failed to capture stdout".into()))?;

        self.stdin = Some(BufWriter::new(stdin));
        self.child = Some(child);

        // Start the background reader task.
        let reader = BufReader::new(stdout);
        let pending = Arc::clone(&self.pending);
        let notification_tx = self.notification_tx.clone();

        let handle = tokio::spawn(async move {
            Self::reader_loop(reader, pending, notification_tx).await;
        });
        self.reader_handle = Some(handle);

        debug!(
            command = %self.config.command,
            args = ?self.config.args,
            "ACP subprocess spawned, starting handshake"
        );

        // Send initialize request.
        let init_params = serde_json::json!({
            "clientInfo": {
                "name": "polkagent",
                "version": "0.1.0"
            },
            "capabilities": {}
        });

        let response = self.send_request("initialize", init_params).await?;

        // Send initialized notification (no id, no response expected).
        self.send_notification("initialized", serde_json::Value::Null)
            .await?;

        info!(command = %self.config.command, "ACP handshake completed");

        Ok(response)
    }

    /// Create a new ACP session.
    pub async fn new_session(
        &mut self,
        working_directory: Option<&str>,
        mcp_servers: Option<&[serde_json::Value]>,
    ) -> Result<String, AcpError> {
        let mut params = serde_json::Map::new();
        if let Some(wd) = working_directory {
            params.insert(
                "workingDirectory".into(),
                serde_json::Value::String(wd.into()),
            );
        }
        if let Some(servers) = mcp_servers {
            params.insert(
                "mcpServers".into(),
                serde_json::Value::Array(servers.to_vec()),
            );
        }

        let response = self
            .send_request("session/new", serde_json::Value::Object(params))
            .await?;

        let session_key = response
            .get("sessionKey")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                AcpError::ProtocolError("session/new response missing sessionKey".into())
            })?
            .to_owned();

        self.session_id = Some(session_key.clone());
        Ok(session_key)
    }

    /// Send a prompt to an active session.
    pub async fn send_prompt(
        &mut self,
        session_key: &str,
        prompt: &str,
    ) -> Result<serde_json::Value, AcpError> {
        let params = serde_json::json!({
            "sessionKey": session_key,
            "prompt": prompt,
        });
        self.send_request("session/prompt", params).await
    }

    /// Cancel an active session's current operation.
    pub async fn cancel(&mut self, session_key: &str) -> Result<serde_json::Value, AcpError> {
        let params = serde_json::json!({
            "sessionKey": session_key,
        });
        self.send_request("session/cancel", params).await
    }

    /// Load (resume) a previously created session.
    pub async fn load_session(&mut self, session_key: &str) -> Result<serde_json::Value, AcpError> {
        let params = serde_json::json!({
            "sessionKey": session_key,
        });
        self.send_request("session/load", params).await
    }

    /// Respond to a permission request from the server.
    pub async fn respond_permission(
        &mut self,
        request_id: u64,
        approved: bool,
    ) -> Result<(), AcpError> {
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "approved": approved,
            },
        });
        self.write_message(&response).await
    }

    /// Return the current session key, if any.
    pub fn session_key(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Check whether the client is connected.
    pub fn is_connected(&self) -> bool {
        self.child.is_some()
    }

    /// Disconnect from the ACP server, terminating the subprocess.
    pub async fn disconnect(&mut self) -> Result<(), AcpError> {
        // Drop stdin to signal EOF.
        self.stdin.take();

        if let Some(mut child) = self.child.take() {
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                let _ = tokio::process::Command::new("kill")
                    .arg("-TERM")
                    .arg(pid.to_string())
                    .output()
                    .await;

                let exited = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;

                if exited.is_err() {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                }
            }

            #[cfg(not(unix))]
            {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
        }

        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
        }

        self.session_id = None;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Allocate the next JSON-RPC request ID.
    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Send a JSON-RPC request and wait for the corresponding response.
    async fn send_request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, AcpError> {
        let id = self.next_id();

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        // Register the pending response channel.
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self
                .pending
                .lock()
                .map_err(|e| AcpError::ProtocolError(format!("pending map lock poisoned: {e}")))?;
            pending.insert(id, tx);
        }

        debug!(id = id, method = %method, "Sending ACP request");

        self.write_message(&request).await?;

        // Wait for the response with a timeout.
        let result = tokio::time::timeout(self.config.timeout, rx)
            .await
            .map_err(|_| AcpError::Timeout)?
            .map_err(|_| AcpError::ProtocolError("response channel closed unexpectedly".into()))?;

        // Check for JSON-RPC error.
        if let Some(error) = result.get("error") {
            let message = error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown error");
            return Err(AcpError::ProtocolError(format!(
                "JSON-RPC error: {message}"
            )));
        }

        Ok(result
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }

    /// Send a JSON-RPC notification (no id, no response expected).
    async fn send_notification(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<(), AcpError> {
        let mut notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
        });

        if !params.is_null() {
            if let Some(obj) = notification.as_object_mut() {
                obj.insert("params".into(), params);
            }
        }

        self.write_message(&notification).await
    }

    /// Write a JSON message to stdin as a newline-delimited line.
    async fn write_message(&mut self, message: &serde_json::Value) -> Result<(), AcpError> {
        let stdin = self.stdin.as_mut().ok_or(AcpError::NotConnected)?;

        let line = serde_json::to_string(message)
            .map_err(|e| AcpError::IoError(format!("failed to serialize message: {e}")))?;

        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| AcpError::IoError(format!("failed to write to stdin: {e}")))?;

        stdin
            .write_all(b"\n")
            .await
            .map_err(|e| AcpError::IoError(format!("failed to write newline: {e}")))?;

        stdin
            .flush()
            .await
            .map_err(|e| AcpError::IoError(format!("failed to flush stdin: {e}")))?;

        Ok(())
    }

    /// Background reader loop that processes stdout lines from the ACP server.
    async fn reader_loop(
        mut reader: BufReader<tokio::process::ChildStdout>,
        pending: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
        notification_tx: mpsc::UnboundedSender<AcpNotification>,
    ) {
        let mut line_buf = String::new();

        loop {
            line_buf.clear();
            match reader.read_line(&mut line_buf).await {
                Ok(0) => {
                    debug!("ACP reader: EOF on stdout");
                    break;
                }
                Ok(_) => {
                    let line = line_buf.trim();
                    if line.is_empty() {
                        continue;
                    }

                    let parsed: serde_json::Value = match serde_json::from_str(line) {
                        Ok(v) => v,
                        Err(e) => {
                            warn!(
                                line = %line,
                                error = %e,
                                "ACP reader: failed to parse JSON line"
                            );
                            continue;
                        }
                    };

                    if let Some(id) = parsed.get("id").and_then(serde_json::Value::as_u64) {
                        if let Some(method) =
                            parsed.get("method").and_then(serde_json::Value::as_str)
                        {
                            // Server-to-client request (has both id and method).
                            Self::handle_server_request(id, method, &parsed, &notification_tx);
                        } else {
                            // Response to one of our requests.
                            Self::handle_response(id, parsed, &pending);
                        }
                    } else if let Some(method) =
                        parsed.get("method").and_then(serde_json::Value::as_str)
                    {
                        // Notification (no id).
                        Self::handle_notification(method, &parsed, &notification_tx);
                    } else {
                        warn!("ACP reader: message with no id or method: {line}");
                    }
                }
                Err(e) => {
                    error!(error = %e, "ACP reader: error reading stdout");
                    break;
                }
            }
        }

        debug!("ACP reader loop exited");
    }

    /// Route a JSON-RPC response to its pending oneshot sender.
    fn handle_response(
        id: u64,
        value: serde_json::Value,
        pending: &Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    ) {
        let sender = {
            let mut map = match pending.lock() {
                Ok(m) => m,
                Err(e) => {
                    error!(error = %e, "ACP reader: pending map lock poisoned");
                    return;
                }
            };
            map.remove(&id)
        };

        if let Some(tx) = sender {
            if tx.send(value).is_err() {
                warn!(id = id, "ACP reader: response receiver dropped");
            }
        } else {
            warn!(id = id, "ACP reader: no pending sender for response id");
        }
    }

    /// Handle a server-to-client request (has both "id" and "method").
    fn handle_server_request(
        id: u64,
        method: &str,
        parsed: &serde_json::Value,
        notification_tx: &mpsc::UnboundedSender<AcpNotification>,
    ) {
        match method {
            "session/request_permission" => {
                let params = parsed
                    .get("params")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);

                let session_key = params
                    .get("sessionKey")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned();

                let permission: AcpPermission =
                    serde_json::from_value(params.get("permission").cloned().unwrap_or(
                        serde_json::json!({
                            "type": "unknown",
                        }),
                    ))
                    .unwrap_or(AcpPermission {
                        permission_type: "unknown".into(),
                        description: None,
                        extra: HashMap::new(),
                    });

                let notification = AcpNotification::PermissionRequest {
                    id,
                    session_key,
                    permission,
                };

                if notification_tx.send(notification).is_err() {
                    warn!("ACP reader: notification receiver dropped (permission request)");
                }
            }
            _ => {
                warn!(
                    method = %method,
                    id = id,
                    "ACP reader: unhandled server request"
                );
            }
        }
    }

    /// Handle a notification from the server (no id).
    fn handle_notification(
        method: &str,
        parsed: &serde_json::Value,
        notification_tx: &mpsc::UnboundedSender<AcpNotification>,
    ) {
        match method {
            "session/update" => {
                let params = parsed
                    .get("params")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);

                let session_key = params
                    .get("sessionKey")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned();

                let update = Self::parse_session_update(&params);

                if let Some(update) = update {
                    let notification = AcpNotification::SessionUpdate {
                        session_key,
                        update,
                    };
                    if notification_tx.send(notification).is_err() {
                        warn!("ACP reader: notification receiver dropped (session update)");
                    }
                }
            }
            _ => {
                debug!(method = %method, "ACP reader: unhandled notification method");
            }
        }
    }

    /// Parse a session/update notification's params into an `AcpSessionUpdate`.
    fn parse_session_update(params: &serde_json::Value) -> Option<AcpSessionUpdate> {
        let update_type = params.get("type").and_then(serde_json::Value::as_str)?;

        match update_type {
            "agent_message_chunk" => {
                let text = params
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                Some(AcpSessionUpdate::AgentMessageChunk { text })
            }
            "tool_call" => {
                let tool_name = params
                    .get("toolName")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                Some(AcpSessionUpdate::ToolCall {
                    tool_name,
                    arguments,
                })
            }
            "tool_call_update" => {
                let tool_name = params
                    .get("toolName")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let output = params
                    .get("output")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                Some(AcpSessionUpdate::ToolCallUpdate { tool_name, output })
            }
            "turn_end" => {
                let usage = params
                    .get("usage")
                    .and_then(|v| serde_json::from_value(v.clone()).ok());
                Some(AcpSessionUpdate::TurnEnd { usage })
            }
            _ => {
                debug!(
                    update_type = %update_type,
                    "ACP: unknown session update type"
                );
                None
            }
        }
    }
}

impl Drop for AcpStdioClient {
    fn drop(&mut self) {
        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
        }
        self.stdin.take();
    }
}

// ---------------------------------------------------------------------------
// AcpConfigurator trait
// ---------------------------------------------------------------------------

/// Trait for providing ACP configuration for a specific agent.
///
/// Implement this trait for each ACP-compatible agent (Cursor, Goose, Kiro)
/// to supply the appropriate [`AcpConfig`] and harness metadata.
pub trait AcpConfigurator: Send + Sync + 'static {
    /// Return the harness identifier (e.g., `"cursor"`, `"goose"`, `"kiro"`).
    fn harness_id(&self) -> HarnessId;

    /// Build the [`AcpConfig`] from the given [`HarnessConfig`].
    fn build_config(&self, harness_config: &HarnessConfig) -> AcpConfig;

    /// Return the capabilities of this harness.
    fn capabilities(&self) -> HarnessCapabilities;
}

// ---------------------------------------------------------------------------
// AcpHarness
// ---------------------------------------------------------------------------

/// A generic ACP harness that uses [`AcpStdioClient`] internally.
///
/// Parameterized by an [`AcpConfigurator`] that provides agent-specific
/// configuration. This means Cursor, Goose, and Kiro harnesses only need
/// to implement `AcpConfigurator` to get a full `Harness` implementation.
pub struct AcpHarness<C: AcpConfigurator> {
    configurator: C,
    harness_config: HarnessConfig,
    client: tokio::sync::Mutex<Option<AcpStdioClient>>,
    status: Mutex<HarnessStatus>,
    sessions: Mutex<HashMap<SessionId, AcpSessionState>>,
}

/// Internal per-session state for the ACP harness.
struct AcpSessionState {
    /// The ACP session key returned by the server.
    session_key: String,
    /// Whether the session is currently active.
    active: bool,
}

impl<C: AcpConfigurator> std::fmt::Debug for AcpHarness<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpHarness")
            .field("harness_id", &self.configurator.harness_id())
            .finish_non_exhaustive()
    }
}

impl<C: AcpConfigurator> AcpHarness<C> {
    /// Create a new ACP harness with the given configurator and harness config.
    pub fn new(configurator: C, harness_config: HarnessConfig) -> Self {
        Self {
            configurator,
            harness_config,
            client: tokio::sync::Mutex::new(None),
            status: Mutex::new(HarnessStatus::Idle),
            sessions: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl<C: AcpConfigurator> Harness for AcpHarness<C> {
    fn id(&self) -> &HarnessId {
        &self.harness_config.id
    }

    fn capabilities(&self) -> HarnessCapabilities {
        self.configurator.capabilities()
    }

    fn status(&self) -> HarnessStatus {
        let status = lock_or_recover(&self.status);
        status.clone()
    }

    async fn start_session(&self, config: SessionConfig) -> Result<SessionId, HarnessError> {
        let session_id = SessionId::new();

        let mut client_guard = self.client.lock().await;

        // If not connected, connect now.
        if client_guard.is_none() {
            let acp_config = self.configurator.build_config(&self.harness_config);
            let mut client = AcpStdioClient::new(acp_config);
            client.connect().await?;
            *client_guard = Some(client);
        }

        let client = client_guard
            .as_mut()
            .ok_or_else(|| HarnessError::Internal {
                message: "client not available after connect".into(),
            })?;

        let wd = config
            .working_directory
            .as_ref()
            .or(self.harness_config.workspace_path.as_ref())
            .map(|p| p.display().to_string());

        let session_key = client.new_session(wd.as_deref(), None).await?;

        {
            let mut sessions = lock_or_recover(&self.sessions);
            sessions.insert(
                session_id,
                AcpSessionState {
                    session_key,
                    active: true,
                },
            );
        }

        {
            let mut status = lock_or_recover(&self.status);
            *status = HarnessStatus::Running {
                since: Utc::now(),
                run_id: None,
            };
        }

        info!(session_id = %session_id, "ACP session started");
        Ok(session_id)
    }

    async fn send_message(&self, session_id: SessionId, message: &str) -> Result<(), HarnessError> {
        let session_key = {
            let sessions = lock_or_recover(&self.sessions);
            let state = sessions
                .get(&session_id)
                .ok_or(HarnessError::SessionNotFound { session_id })?;

            if !state.active {
                return Err(HarnessError::InvalidState {
                    message: format!("session {session_id} is no longer active"),
                });
            }

            state.session_key.clone()
        };

        let mut client_guard = self.client.lock().await;
        let client = client_guard
            .as_mut()
            .ok_or_else(|| HarnessError::InvalidState {
                message: "ACP client not connected".into(),
            })?;

        client.send_prompt(&session_key, message).await?;

        Ok(())
    }

    async fn receive_events(
        &self,
        session_id: SessionId,
    ) -> Result<Pin<Box<dyn Stream<Item = HarnessEvent> + Send>>, HarnessError> {
        {
            let sessions = lock_or_recover(&self.sessions);
            if !sessions.contains_key(&session_id) {
                return Err(HarnessError::SessionNotFound { session_id });
            }
        }

        let notification_rx = {
            let mut client_guard = self.client.lock().await;
            let client = client_guard
                .as_mut()
                .ok_or_else(|| HarnessError::InvalidState {
                    message: "ACP client not connected".into(),
                })?;
            client.take_notifications()
        };

        let rx = notification_rx.ok_or_else(|| HarnessError::InvalidState {
            message: "notification stream already consumed".into(),
        })?;

        let stream = async_stream::stream! {
            yield HarnessEvent::SessionStarted { session_id };

            let mut rx = rx;
            while let Some(notification) = rx.recv().await {
                match notification {
                    AcpNotification::SessionUpdate { update, .. } => {
                        match update {
                            AcpSessionUpdate::AgentMessageChunk { text } => {
                                yield HarnessEvent::MessageReceived {
                                    session_id,
                                    content: text,
                                };
                            }
                            AcpSessionUpdate::ToolCall {
                                tool_name,
                                arguments,
                            } => {
                                let arguments_json = serde_json::to_string(&arguments)
                                    .unwrap_or_default();
                                yield HarnessEvent::ToolCallRequested {
                                    session_id,
                                    tool_name,
                                    arguments_json,
                                };
                            }
                            AcpSessionUpdate::ToolCallUpdate { tool_name, .. } => {
                                yield HarnessEvent::ToolResultProvided {
                                    session_id,
                                    tool_name,
                                    is_error: false,
                                };
                            }
                            AcpSessionUpdate::TurnEnd { .. } => {
                                yield HarnessEvent::SessionEnded { session_id };
                            }
                        }
                    }
                    AcpNotification::PermissionRequest { .. } => {
                        // Permission requests are handled externally via
                        // respond_permission on the client.
                    }
                }
            }

            yield HarnessEvent::SessionEnded { session_id };
        };

        Ok(Box::pin(stream))
    }

    async fn end_session(&self, session_id: SessionId) -> Result<(), HarnessError> {
        let session_key = {
            let mut sessions = lock_or_recover(&self.sessions);
            let state = sessions
                .get_mut(&session_id)
                .ok_or(HarnessError::SessionNotFound { session_id })?;

            if !state.active {
                return Ok(());
            }

            state.active = false;
            state.session_key.clone()
        };

        let mut client_guard = self.client.lock().await;
        if let Some(client) = client_guard.as_mut() {
            let _ = client.cancel(&session_key).await;
        }

        {
            let sessions = lock_or_recover(&self.sessions);
            let any_active = sessions.values().any(|s| s.active);
            if !any_active {
                drop(sessions);
                let mut status = lock_or_recover(&self.status);
                *status = HarnessStatus::Idle;
            }
        }

        info!(session_id = %session_id, "ACP session ended");
        Ok(())
    }

    async fn health(&self) -> Result<bool, HarnessError> {
        let acp_config = self.configurator.build_config(&self.harness_config);
        let binary = &acp_config.command;

        debug!(binary = %binary, "ACP health check");

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

    async fn save_session_state(
        &self,
        session_id: SessionId,
    ) -> Result<polkagent_harness_trait::SessionSnapshot, HarnessError> {
        let sessions = lock_or_recover(&self.sessions);
        let session = sessions
            .get(&session_id)
            .ok_or(HarnessError::SessionNotFound { session_id })?;

        let mut backend_state = std::collections::HashMap::new();
        backend_state.insert("session_key".into(), serde_json::json!(session.session_key));

        let snapshot = polkagent_harness_trait::SessionSnapshot {
            session_id,
            harness_id: self.configurator.harness_id(),
            process_pid: None,
            started_at: chrono::Utc::now(),
            working_directory: self.harness_config.workspace_path.clone(),
            turn_count: 0,
            backend_state,
        };

        polkagent_harness_trait::persist_session_state(&snapshot)?;
        debug!(session_id = %session_id, "ACP session state saved");
        Ok(snapshot)
    }

    async fn resume_session(&self, session_id: SessionId) -> Result<SessionId, HarnessError> {
        let snapshot = polkagent_harness_trait::load_session_state(session_id)?;

        let session_key = snapshot
            .backend_state
            .get("session_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| HarnessError::ParseError {
                message: "missing session_key in saved state".into(),
            })?
            .to_owned();

        // Re-connect the ACP client if needed.
        let mut client_guard = self.client.lock().await;
        if client_guard.is_none() {
            let acp_config = self.configurator.build_config(&self.harness_config);
            let mut client = AcpStdioClient::new(acp_config);
            client.connect().await?;
            *client_guard = Some(client);
        }

        // Re-register the session in our local map.
        {
            let mut sessions = lock_or_recover(&self.sessions);
            sessions.insert(
                session_id,
                AcpSessionState {
                    session_key,
                    active: true,
                },
            );
        }

        {
            let mut status = lock_or_recover(&self.status);
            *status = HarnessStatus::Running {
                since: chrono::Utc::now(),
                run_id: None,
            };
        }

        polkagent_harness_trait::remove_session_state(session_id)?;
        info!(session_id = %session_id, "ACP session resumed");
        Ok(session_id)
    }

    async fn cancel_session(&self, session_id: SessionId) -> Result<(), HarnessError> {
        let session_key = {
            let sessions = lock_or_recover(&self.sessions);
            let state = sessions
                .get(&session_id)
                .ok_or(HarnessError::SessionNotFound { session_id })?;
            state.session_key.clone()
        };

        let mut client_guard = self.client.lock().await;
        if let Some(client) = client_guard.as_mut() {
            let _ = client.cancel(&session_key).await;
        }
        drop(client_guard);

        polkagent_harness_trait::remove_session_state(session_id).ok();
        self.end_session(session_id).await
    }

    fn health_interval(&self) -> Option<std::time::Duration> {
        Some(std::time::Duration::from_secs(30))
    }
}

// ---------------------------------------------------------------------------
// Helper: build JSON-RPC messages (used in tests and by consumers)
// ---------------------------------------------------------------------------

/// Build a JSON-RPC 2.0 request message.
pub fn build_jsonrpc_request(
    id: u64,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let mut request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
    });
    if let Some(object) = request.as_object_mut() {
        object.insert("params".into(), params);
    }
    request
}

/// Build a JSON-RPC 2.0 notification (no id).
pub fn build_jsonrpc_notification(method: &str, params: serde_json::Value) -> serde_json::Value {
    let mut msg = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
    });
    if !params.is_null() {
        if let Some(obj) = msg.as_object_mut() {
            obj.insert("params".into(), params);
        }
    }
    msg
}

/// Build a JSON-RPC 2.0 response message.
pub fn build_jsonrpc_response(id: u64, result: serde_json::Value) -> serde_json::Value {
    let mut response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
    });
    if let Some(object) = response.as_object_mut() {
        object.insert("result".into(), result);
    }
    response
}

/// Build a JSON-RPC 2.0 error response.
pub fn build_jsonrpc_error(id: u64, code: i64, message: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        },
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Protocol fixture assertions use `expect` to identify the exact malformed
// message, channel, or subprocess contract that failed.
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use polkagent_harness_trait::{CancelMode, McpMode, SessionResumeMode, ToolInjection};

    // -----------------------------------------------------------------------
    // AcpConfig constructor tests
    // -----------------------------------------------------------------------

    #[test]
    fn config_cursor_defaults() {
        let config = AcpConfig::cursor("cursor", None);
        assert_eq!(config.command, "cursor");
        assert_eq!(config.args, vec!["agent", "acp"]);
        assert!(config.cwd.is_none());
        assert_eq!(config.protocol_version, "0.12.2");
        assert_eq!(config.timeout, Duration::from_secs(300));
    }

    #[test]
    fn config_cursor_with_cwd() {
        let config = AcpConfig::cursor("cursor", Some(PathBuf::from("/workspace")));
        assert_eq!(config.cwd, Some(PathBuf::from("/workspace")));
    }

    #[test]
    fn config_cursor_custom_binary() {
        let config = AcpConfig::cursor("/usr/local/bin/cursor", None);
        assert_eq!(config.command, "/usr/local/bin/cursor");
    }

    #[test]
    fn config_goose_defaults() {
        let config = AcpConfig::goose("goose", None);
        assert_eq!(config.command, "goose");
        assert_eq!(config.args, vec!["acp"]);
        assert!(config.cwd.is_none());
        assert_eq!(config.protocol_version, "0.12.2");
    }

    #[test]
    fn config_goose_with_cwd() {
        let config = AcpConfig::goose("goose", Some(PathBuf::from("/tmp/project")));
        assert_eq!(config.cwd, Some(PathBuf::from("/tmp/project")));
    }

    #[test]
    fn config_kiro_defaults() {
        let config = AcpConfig::kiro("kiro-cli", None);
        assert_eq!(config.command, "kiro-cli");
        assert_eq!(config.args, vec!["acp"]);
        assert!(config.cwd.is_none());
        assert_eq!(config.protocol_version, "0.12.2");
    }

    #[test]
    fn config_kiro_with_cwd() {
        let config = AcpConfig::kiro("kiro-cli", Some(PathBuf::from("/home/user/proj")));
        assert_eq!(config.cwd, Some(PathBuf::from("/home/user/proj")));
    }

    // -----------------------------------------------------------------------
    // JSON-RPC message formatting tests
    // -----------------------------------------------------------------------

    #[test]
    fn jsonrpc_request_format() {
        let msg = build_jsonrpc_request(
            1,
            "initialize",
            serde_json::json!({"clientInfo": {"name": "polkagent", "version": "0.1.0"}}),
        );

        assert_eq!(msg["jsonrpc"], "2.0");
        assert_eq!(msg["id"], 1);
        assert_eq!(msg["method"], "initialize");
        assert_eq!(msg["params"]["clientInfo"]["name"], "polkagent");
    }

    #[test]
    fn jsonrpc_notification_format() {
        let msg = build_jsonrpc_notification("initialized", serde_json::Value::Null);

        assert_eq!(msg["jsonrpc"], "2.0");
        assert_eq!(msg["method"], "initialized");
        assert!(msg.get("id").is_none());
        assert!(msg.get("params").is_none());
    }

    #[test]
    fn jsonrpc_notification_with_params() {
        let msg = build_jsonrpc_notification(
            "session/update",
            serde_json::json!({"sessionKey": "abc", "type": "turn_end"}),
        );

        assert_eq!(msg["jsonrpc"], "2.0");
        assert_eq!(msg["method"], "session/update");
        assert!(msg.get("id").is_none());
        assert_eq!(msg["params"]["sessionKey"], "abc");
    }

    #[test]
    fn jsonrpc_response_format() {
        let msg = build_jsonrpc_response(1, serde_json::json!({"capabilities": {}}));

        assert_eq!(msg["jsonrpc"], "2.0");
        assert_eq!(msg["id"], 1);
        assert_eq!(msg["result"]["capabilities"], serde_json::json!({}));
    }

    #[test]
    fn jsonrpc_error_format() {
        let msg = build_jsonrpc_error(1, -32600, "Invalid request");

        assert_eq!(msg["jsonrpc"], "2.0");
        assert_eq!(msg["id"], 1);
        assert_eq!(msg["error"]["code"], -32600);
        assert_eq!(msg["error"]["message"], "Invalid request");
    }

    // -----------------------------------------------------------------------
    // Initialize handshake message tests
    // -----------------------------------------------------------------------

    #[test]
    fn initialize_request_structure() {
        let msg = build_jsonrpc_request(
            1,
            "initialize",
            serde_json::json!({
                "clientInfo": {
                    "name": "polkagent",
                    "version": "0.1.0"
                },
                "capabilities": {}
            }),
        );

        assert_eq!(msg["method"], "initialize");
        assert_eq!(msg["params"]["clientInfo"]["name"], "polkagent");
        assert_eq!(msg["params"]["clientInfo"]["version"], "0.1.0");
        assert_eq!(msg["params"]["capabilities"], serde_json::json!({}));
    }

    #[test]
    fn initialized_notification_structure() {
        let msg = build_jsonrpc_notification("initialized", serde_json::Value::Null);

        assert_eq!(msg["method"], "initialized");
        assert!(msg.get("id").is_none());
    }

    // -----------------------------------------------------------------------
    // Session message format tests
    // -----------------------------------------------------------------------

    #[test]
    fn session_new_request_format() {
        let msg = build_jsonrpc_request(
            2,
            "session/new",
            serde_json::json!({
                "workingDirectory": "/home/user/project",
                "mcpServers": []
            }),
        );

        assert_eq!(msg["method"], "session/new");
        assert_eq!(msg["params"]["workingDirectory"], "/home/user/project");
    }

    #[test]
    fn session_prompt_request_format() {
        let msg = build_jsonrpc_request(
            3,
            "session/prompt",
            serde_json::json!({
                "sessionKey": "session-abc-123",
                "prompt": "Hello, agent!"
            }),
        );

        assert_eq!(msg["method"], "session/prompt");
        assert_eq!(msg["params"]["sessionKey"], "session-abc-123");
        assert_eq!(msg["params"]["prompt"], "Hello, agent!");
    }

    #[test]
    fn session_cancel_request_format() {
        let msg = build_jsonrpc_request(
            4,
            "session/cancel",
            serde_json::json!({"sessionKey": "session-abc-123"}),
        );

        assert_eq!(msg["method"], "session/cancel");
        assert_eq!(msg["params"]["sessionKey"], "session-abc-123");
    }

    #[test]
    fn session_load_request_format() {
        let msg = build_jsonrpc_request(
            5,
            "session/load",
            serde_json::json!({"sessionKey": "session-abc-123"}),
        );

        assert_eq!(msg["method"], "session/load");
        assert_eq!(msg["params"]["sessionKey"], "session-abc-123");
    }

    // -----------------------------------------------------------------------
    // Notification parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_agent_message_chunk_update() {
        let params = serde_json::json!({
            "sessionKey": "session-1",
            "type": "agent_message_chunk",
            "text": "Hello from the agent"
        });

        let update = AcpStdioClient::parse_session_update(&params);
        assert!(update.is_some());
        match update {
            Some(AcpSessionUpdate::AgentMessageChunk { text }) => {
                assert_eq!(text, "Hello from the agent");
            }
            _ => panic!("expected AgentMessageChunk"),
        }
    }

    #[test]
    fn parse_tool_call_update() {
        let params = serde_json::json!({
            "sessionKey": "session-1",
            "type": "tool_call",
            "toolName": "file_read",
            "arguments": {"path": "/tmp/test.txt"}
        });

        let update = AcpStdioClient::parse_session_update(&params);
        assert!(update.is_some());
        match update {
            Some(AcpSessionUpdate::ToolCall {
                tool_name,
                arguments,
            }) => {
                assert_eq!(tool_name, "file_read");
                assert_eq!(arguments["path"], "/tmp/test.txt");
            }
            _ => panic!("expected ToolCall"),
        }
    }

    #[test]
    fn parse_tool_call_update_notification() {
        let params = serde_json::json!({
            "sessionKey": "session-1",
            "type": "tool_call_update",
            "toolName": "file_read",
            "output": "file contents here"
        });

        let update = AcpStdioClient::parse_session_update(&params);
        assert!(update.is_some());
        match update {
            Some(AcpSessionUpdate::ToolCallUpdate { tool_name, output }) => {
                assert_eq!(tool_name, "file_read");
                assert_eq!(output, "file contents here");
            }
            _ => panic!("expected ToolCallUpdate"),
        }
    }

    #[test]
    fn parse_turn_end_with_usage() {
        let params = serde_json::json!({
            "sessionKey": "session-1",
            "type": "turn_end",
            "usage": {
                "input_tokens": 1500,
                "output_tokens": 500
            }
        });

        let update = AcpStdioClient::parse_session_update(&params);
        assert!(update.is_some());
        match update {
            Some(AcpSessionUpdate::TurnEnd { usage }) => {
                let usage = usage.as_ref().expect("usage should be present");
                assert_eq!(usage.input_tokens, Some(1500));
                assert_eq!(usage.output_tokens, Some(500));
            }
            _ => panic!("expected TurnEnd"),
        }
    }

    #[test]
    fn parse_turn_end_without_usage() {
        let params = serde_json::json!({
            "sessionKey": "session-1",
            "type": "turn_end"
        });

        let update = AcpStdioClient::parse_session_update(&params);
        assert!(update.is_some());
        match update {
            Some(AcpSessionUpdate::TurnEnd { usage }) => {
                assert!(usage.is_none());
            }
            _ => panic!("expected TurnEnd"),
        }
    }

    #[test]
    fn parse_unknown_update_type_returns_none() {
        let params = serde_json::json!({
            "sessionKey": "session-1",
            "type": "unknown_type"
        });

        let update = AcpStdioClient::parse_session_update(&params);
        assert!(update.is_none());
    }

    #[test]
    fn parse_missing_type_returns_none() {
        let params = serde_json::json!({"sessionKey": "session-1"});
        let update = AcpStdioClient::parse_session_update(&params);
        assert!(update.is_none());
    }

    // -----------------------------------------------------------------------
    // Permission request/response formatting tests
    // -----------------------------------------------------------------------

    #[test]
    fn permission_request_parsing() {
        let server_request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "session/request_permission",
            "params": {
                "sessionKey": "session-1",
                "permission": {
                    "type": "file_write",
                    "description": "Write to /tmp/output.txt"
                }
            }
        });

        let id = server_request["id"].as_u64().expect("id");
        assert_eq!(id, 42);

        let params = &server_request["params"];
        let session_key = params["sessionKey"].as_str().expect("sessionKey");
        assert_eq!(session_key, "session-1");

        let permission: AcpPermission =
            serde_json::from_value(params["permission"].clone()).expect("permission");
        assert_eq!(permission.permission_type, "file_write");
        assert_eq!(
            permission.description.as_deref(),
            Some("Write to /tmp/output.txt")
        );
    }

    #[test]
    fn permission_response_approved() {
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 42,
            "result": {"approved": true}
        });

        assert_eq!(response["id"], 42);
        assert_eq!(response["result"]["approved"], true);
    }

    #[test]
    fn permission_response_denied() {
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 42,
            "result": {"approved": false}
        });

        assert_eq!(response["id"], 42);
        assert_eq!(response["result"]["approved"], false);
    }

    #[test]
    fn permission_serde_round_trip() {
        let permission = AcpPermission {
            permission_type: "shell_execute".into(),
            description: Some("Run `ls -la`".into()),
            extra: HashMap::new(),
        };

        let json = serde_json::to_value(&permission).expect("serialize");
        assert_eq!(json["type"], "shell_execute");
        assert_eq!(json["description"], "Run `ls -la`");

        let back: AcpPermission = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.permission_type, "shell_execute");
        assert_eq!(back.description.as_deref(), Some("Run `ls -la`"));
    }

    // -----------------------------------------------------------------------
    // AcpUsage serde tests
    // -----------------------------------------------------------------------

    #[test]
    fn usage_serde_round_trip() {
        let usage = AcpUsage {
            input_tokens: Some(1000),
            output_tokens: Some(500),
        };

        let json = serde_json::to_value(&usage).expect("serialize");
        assert_eq!(json["input_tokens"], 1000);
        assert_eq!(json["output_tokens"], 500);

        let back: AcpUsage = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.input_tokens, Some(1000));
        assert_eq!(back.output_tokens, Some(500));
    }

    #[test]
    fn usage_with_nulls() {
        let usage = AcpUsage {
            input_tokens: None,
            output_tokens: None,
        };

        let json = serde_json::to_value(&usage).expect("serialize");
        assert!(json["input_tokens"].is_null());
        assert!(json["output_tokens"].is_null());
    }

    // -----------------------------------------------------------------------
    // Error type tests
    // -----------------------------------------------------------------------

    #[test]
    fn error_display() {
        let err = AcpError::SpawnFailed("no such file".into());
        assert!(err.to_string().contains("no such file"));

        let err = AcpError::IoError("broken pipe".into());
        assert!(err.to_string().contains("broken pipe"));

        let err = AcpError::ProtocolError("unexpected response".into());
        assert!(err.to_string().contains("unexpected response"));

        let err = AcpError::Timeout;
        assert!(err.to_string().contains("timed out"));

        let err = AcpError::NotConnected;
        assert!(err.to_string().contains("not connected"));

        let err = AcpError::SessionNotFound;
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn error_conversion_to_harness_error() {
        let err: HarnessError = AcpError::SpawnFailed("fail".into()).into();
        assert!(matches!(err, HarnessError::SpawnFailed { .. }));

        let err: HarnessError = AcpError::IoError("io".into()).into();
        assert!(matches!(err, HarnessError::IoError { .. }));

        let err: HarnessError = AcpError::ProtocolError("proto".into()).into();
        assert!(matches!(err, HarnessError::ParseError { .. }));

        let err: HarnessError = AcpError::Timeout.into();
        assert!(matches!(err, HarnessError::Timeout { .. }));

        let err: HarnessError = AcpError::NotConnected.into();
        assert!(matches!(err, HarnessError::InvalidState { .. }));

        let err: HarnessError = AcpError::SessionNotFound.into();
        assert!(matches!(err, HarnessError::InvalidState { .. }));
    }

    // -----------------------------------------------------------------------
    // Client construction tests
    // -----------------------------------------------------------------------

    #[test]
    fn client_new_is_disconnected() {
        let config = AcpConfig::cursor("cursor", None);
        let client = AcpStdioClient::new(config);
        assert!(!client.is_connected());
        assert!(client.session_key().is_none());
    }

    #[test]
    fn client_id_counter_increments() {
        let config = AcpConfig::cursor("cursor", None);
        let client = AcpStdioClient::new(config);
        let id1 = client.next_id();
        let id2 = client.next_id();
        let id3 = client.next_id();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
    }

    #[test]
    fn client_take_notifications_returns_some_once() {
        let config = AcpConfig::cursor("cursor", None);
        let mut client = AcpStdioClient::new(config);
        assert!(client.take_notifications().is_some());
        assert!(client.take_notifications().is_none());
    }

    #[test]
    fn client_config_accessor() {
        let config = AcpConfig::goose("goose", Some(PathBuf::from("/tmp")));
        let client = AcpStdioClient::new(config);
        assert_eq!(client.config().command, "goose");
        assert_eq!(client.config().cwd, Some(PathBuf::from("/tmp")));
    }

    // -----------------------------------------------------------------------
    // Mock stdin/stdout protocol tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn connect_to_echo_server() {
        let mut config = AcpConfig::cursor("bash", None);
        config.args = vec![
            "-c".into(),
            r#"read line; echo '{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}'; read line"#
                .into(),
        ];
        config.timeout = Duration::from_secs(5);

        let mut client = AcpStdioClient::new(config);
        let result = client.connect().await;
        assert!(result.is_ok(), "connect failed: {result:?}");

        assert!(client.is_connected());

        let response = result.expect("checked ok");
        assert_eq!(response["capabilities"], serde_json::json!({}));

        let _ = client.disconnect().await;
    }

    #[tokio::test]
    async fn connect_spawn_failure() {
        let config = AcpConfig::cursor("nonexistent-binary-that-does-not-exist-12345", None);
        let mut client = AcpStdioClient::new(config);
        let result = client.connect().await;
        assert!(result.is_err());
        match result {
            Err(AcpError::SpawnFailed(msg)) => {
                assert!(msg.contains("nonexistent-binary"));
            }
            other => panic!("expected SpawnFailed, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn connect_timeout_on_no_response() {
        let mut config = AcpConfig::cursor("sleep", None);
        config.args = vec!["60".into()];
        config.timeout = Duration::from_millis(200);

        let mut client = AcpStdioClient::new(config);
        let result = client.connect().await;
        assert!(matches!(result, Err(AcpError::Timeout)));

        let _ = client.disconnect().await;
    }

    #[tokio::test]
    async fn session_new_via_mock() {
        let mut config = AcpConfig::cursor("bash", None);
        config.args = vec![
            "-c".into(),
            concat!(
                r#"read line; echo '{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}'; "#,
                r#"read line; "#,
                r#"read line; echo '{"jsonrpc":"2.0","id":2,"result":{"sessionKey":"test-session-key"}}'; "#,
                r#"read line"#,
            )
            .into(),
        ];
        config.timeout = Duration::from_secs(5);

        let mut client = AcpStdioClient::new(config);
        client.connect().await.expect("connect");

        let session_key = client.new_session(Some("/tmp"), None).await;
        assert!(session_key.is_ok(), "new_session failed: {session_key:?}");
        assert_eq!(session_key.expect("checked ok"), "test-session-key");
        assert_eq!(client.session_key(), Some("test-session-key"));

        let _ = client.disconnect().await;
    }

    #[tokio::test]
    async fn notification_routing() {
        let (tx, mut rx) = mpsc::unbounded_channel::<AcpNotification>();

        let notification_json = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionKey": "test-session",
                "type": "agent_message_chunk",
                "text": "Hello!"
            }
        });

        AcpStdioClient::handle_notification("session/update", &notification_json, &tx);

        let received = rx.try_recv();
        assert!(received.is_ok());
        match received.expect("checked ok") {
            AcpNotification::SessionUpdate {
                session_key,
                update,
            } => {
                assert_eq!(session_key, "test-session");
                match update {
                    AcpSessionUpdate::AgentMessageChunk { text } => {
                        assert_eq!(text, "Hello!");
                    }
                    _ => panic!("expected AgentMessageChunk"),
                }
            }
            AcpNotification::PermissionRequest { .. } => panic!("expected SessionUpdate"),
        }
    }

    #[tokio::test]
    async fn response_routing() {
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let (tx, rx) = oneshot::channel();
        {
            let mut map = pending.lock().expect("lock");
            map.insert(42, tx);
        }

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 42,
            "result": {"status": "ok"}
        });

        AcpStdioClient::handle_response(42, response, &pending);

        let received = rx.await;
        assert!(received.is_ok());
        let value = received.expect("checked ok");
        assert_eq!(value["result"]["status"], "ok");
    }

    #[tokio::test]
    async fn server_request_permission_routing() {
        let (tx, mut rx) = mpsc::unbounded_channel::<AcpNotification>();

        let server_request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 99,
            "method": "session/request_permission",
            "params": {
                "sessionKey": "session-1",
                "permission": {
                    "type": "file_write",
                    "description": "Write to /tmp/test.txt"
                }
            }
        });

        AcpStdioClient::handle_server_request(
            99,
            "session/request_permission",
            &server_request,
            &tx,
        );

        let received = rx.try_recv();
        assert!(received.is_ok());
        match received.expect("checked ok") {
            AcpNotification::PermissionRequest {
                id,
                session_key,
                permission,
            } => {
                assert_eq!(id, 99);
                assert_eq!(session_key, "session-1");
                assert_eq!(permission.permission_type, "file_write");
                assert_eq!(
                    permission.description.as_deref(),
                    Some("Write to /tmp/test.txt")
                );
            }
            AcpNotification::SessionUpdate { .. } => panic!("expected PermissionRequest"),
        }
    }

    #[tokio::test]
    async fn disconnect_cleans_up() {
        let mut config = AcpConfig::cursor("bash", None);
        config.args = vec![
            "-c".into(),
            concat!(
                r#"read line; echo '{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}'; "#,
                r#"read line; sleep 60"#,
            )
            .into(),
        ];
        config.timeout = Duration::from_secs(5);

        let mut client = AcpStdioClient::new(config);
        client.connect().await.expect("connect");
        assert!(client.is_connected());

        client.disconnect().await.expect("disconnect");
        assert!(!client.is_connected());
        assert!(client.session_key().is_none());
    }

    #[test]
    fn write_message_requires_connection() {
        let config = AcpConfig::cursor("cursor", None);
        let mut client = AcpStdioClient::new(config);

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(client.write_message(&serde_json::json!({})));
        assert!(matches!(result, Err(AcpError::NotConnected)));
    }

    // -----------------------------------------------------------------------
    // AcpHarness tests (with a fake configurator)
    // -----------------------------------------------------------------------

    struct FakeConfigurator;

    impl AcpConfigurator for FakeConfigurator {
        fn harness_id(&self) -> HarnessId {
            HarnessId::new("fake-acp")
        }

        fn build_config(&self, _: &HarnessConfig) -> AcpConfig {
            AcpConfig::cursor("nonexistent", None)
        }

        fn capabilities(&self) -> HarnessCapabilities {
            HarnessCapabilities {
                supports_streaming: true,
                supports_tools: true,
                supports_sessions: true,
                max_context_tokens: 128_000,
                models: vec!["test-model".into()],
                transport: None,
                model_override: None,
                session_resume: SessionResumeMode::default(),
                mcp_passthrough: McpMode::default(),
                tool_injection: ToolInjection::default(),
                cancel: CancelMode::default(),
                multiplex_safe: false,
            }
        }
    }

    #[test]
    fn acp_harness_id() {
        let config = HarnessConfig::new("fake-acp");
        let harness = AcpHarness::new(FakeConfigurator, config);
        assert_eq!(harness.id().as_str(), "fake-acp");
    }

    #[test]
    fn acp_harness_capabilities() {
        let config = HarnessConfig::new("fake-acp");
        let harness = AcpHarness::new(FakeConfigurator, config);
        let caps = harness.capabilities();
        assert!(caps.supports_streaming);
        assert!(caps.supports_tools);
        assert!(caps.supports_sessions);
        assert_eq!(caps.max_context_tokens, 128_000);
        assert_eq!(caps.models, vec!["test-model"]);
    }

    #[test]
    fn acp_harness_initial_status_is_idle() {
        let config = HarnessConfig::new("fake-acp");
        let harness = AcpHarness::new(FakeConfigurator, config);
        assert!(matches!(harness.status(), HarnessStatus::Idle));
    }

    #[tokio::test]
    async fn acp_harness_health_nonexistent_binary() {
        let config = HarnessConfig::new("fake-acp");
        let harness = AcpHarness::new(FakeConfigurator, config);
        let result = harness.health().await;
        assert!(result.is_ok());
        assert!(!result.expect("checked ok"));
    }

    #[tokio::test]
    async fn acp_harness_end_session_not_found() {
        let config = HarnessConfig::new("fake-acp");
        let harness = AcpHarness::new(FakeConfigurator, config);
        let result = harness.end_session(SessionId::new()).await;
        assert!(matches!(result, Err(HarnessError::SessionNotFound { .. })));
    }

    #[tokio::test]
    async fn acp_harness_send_message_not_found() {
        let config = HarnessConfig::new("fake-acp");
        let harness = AcpHarness::new(FakeConfigurator, config);
        let result = harness.send_message(SessionId::new(), "hello").await;
        assert!(matches!(result, Err(HarnessError::SessionNotFound { .. })));
    }

    /// Compile-time check: `AcpHarness<FakeConfigurator>` is `Send + Sync`.
    #[allow(dead_code)]
    fn _acp_harness_is_send_sync(_h: &dyn Harness) {}
}
