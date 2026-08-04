//! Claude Code harness adapter for the Polkagent platform.
//!
//! This crate provides [`ClaudeHarness`] -- an implementation of the
//! [`Harness`] trait that wraps the Claude Code CLI (`claude`) as a
//! managed subprocess. It exposes a session-based API for interacting
//! with Claude Code programmatically.
//!
//! # Subprocess management
//!
//! Each session spawns a `claude` child process with piped stdin, stdout,
//! and stderr. Messages are sent as JSON lines on stdin; events are read
//! as JSON lines from stdout. Sessions are terminated via SIGTERM with a
//! configurable timeout, falling back to SIGKILL if the process does not
//! exit in time.
//!
//! # Example
//!
//! ```rust,no_run
//! use polkagent_harness_claude::{ClaudeHarness, ClaudeHarnessConfig};
//! use polkagent_harness_trait::{Harness, HarnessConfig, SessionConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = HarnessConfig::new("claude-code");
//! let harness = ClaudeHarness::new(config, ClaudeHarnessConfig::default())?;
//!
//! let session_id = harness.start_session(SessionConfig::default()).await?;
//! harness.send_message(session_id, "Hello, Claude!").await?;
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
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info, warn};

use polkagent_harness_trait::{
    CancelMode, Harness, HarnessCapabilities, HarnessConfig, HarnessError, HarnessEvent,
    HarnessId, HarnessStatus, McpMode, SessionConfig, SessionId, SessionResumeMode,
    ToolInjection, TransportFlavor,
};

// ---------------------------------------------------------------------------
// ClaudeHarnessConfig
// ---------------------------------------------------------------------------

/// Configuration specific to the Claude Code harness.
///
/// Controls the binary path, working directory, and subprocess timeout
/// for the Claude CLI process.
#[derive(Debug, Clone)]
pub struct ClaudeHarnessConfig {
    /// Path (or bare name) of the `claude` binary to spawn.
    ///
    /// Defaults to `"claude"`, which expects the binary to be on `$PATH`.
    pub binary_path: String,

    /// Working directory for spawned subprocess sessions.
    ///
    /// If `None`, the subprocess inherits the current working directory
    /// (or uses the session-level `working_directory` if provided).
    pub working_dir: Option<PathBuf>,

    /// Maximum time to wait for graceful shutdown (SIGTERM) before
    /// sending SIGKILL.
    ///
    /// Defaults to 10 seconds.
    pub timeout: Duration,
}

impl Default for ClaudeHarnessConfig {
    fn default() -> Self {
        Self {
            binary_path: "claude".to_owned(),
            working_dir: None,
            timeout: Duration::from_secs(10),
        }
    }
}

// ---------------------------------------------------------------------------
// Session state (internal)
// ---------------------------------------------------------------------------

/// Metadata about a session, available after the session is started.
#[derive(Debug, Clone)]
pub struct SessionMetadata {
    /// The session identifier.
    pub session_id: SessionId,
    /// The binary path used to spawn the subprocess.
    pub binary_path: String,
    /// The working directory the subprocess was started in.
    pub working_dir: Option<PathBuf>,
    /// Whether the session is currently active.
    pub active: bool,
    /// Number of messages sent to this session.
    pub message_count: usize,
}

/// Internal state for a single Claude Code session.
struct SessionState {
    /// The session identifier.
    id: SessionId,
    /// The configuration used to start this session.
    #[allow(dead_code)]
    config: SessionConfig,
    /// Messages sent to this session.
    messages: Vec<String>,
    /// Whether the session is still active.
    active: bool,
    /// The child process handle.
    child: tokio::process::Child,
    /// The child's stdin handle (taken from child on creation).
    stdin: Option<tokio::process::ChildStdin>,
    /// The child's stdout handle (taken from child on creation).
    stdout: Option<tokio::process::ChildStdout>,
    /// The binary path used to spawn this session.
    binary_path: String,
    /// The working directory used for this session.
    working_dir: Option<PathBuf>,
}

// SessionState holds a Child which is not Debug, so implement manually.
impl std::fmt::Debug for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionState")
            .field("id", &self.id)
            .field("active", &self.active)
            .field("messages", &self.messages.len())
            .field("binary_path", &self.binary_path)
            .field("working_dir", &self.working_dir)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// ClaudeHarness
// ---------------------------------------------------------------------------

/// Claude Code harness implementation.
///
/// Wraps the `claude` CLI as a managed subprocess, providing a session-based
/// API for sending messages and receiving structured events.
///
/// - [`start_session`] spawns a `claude` child process with piped I/O.
/// - [`send_message`] writes a JSON line to the child's stdin.
/// - [`receive_events`] reads JSON lines from the child's stdout and
///   parses them into [`HarnessEvent`] values.
/// - [`end_session`] sends SIGTERM, waits with a timeout, then SIGKILL.
/// - [`health`] checks whether the configured executable path exists.
///
/// [`start_session`]: Harness::start_session
/// [`send_message`]: Harness::send_message
/// [`receive_events`]: Harness::receive_events
/// [`end_session`]: Harness::end_session
/// [`health`]: Harness::health
pub struct ClaudeHarness {
    /// The harness configuration (from the trait layer).
    config: HarnessConfig,
    /// Claude-specific configuration.
    claude_config: ClaudeHarnessConfig,
    /// Active sessions, keyed by session ID.
    sessions: Mutex<HashMap<SessionId, SessionState>>,
    /// Current operational status.
    status: Mutex<HarnessStatus>,
}

impl std::fmt::Debug for ClaudeHarness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeHarness")
            .field("config", &self.config)
            .field("claude_config", &self.claude_config)
            .field("active_sessions", &self.active_session_count())
            .finish()
    }
}

impl ClaudeHarness {
    /// Create a new `ClaudeHarness` with the given configuration.
    ///
    /// Validates that the harness ID is `"claude-code"` and performs a
    /// basic sanity check on the configuration.
    ///
    /// # Errors
    ///
    /// Returns [`HarnessError::InvalidState`] if the harness ID is not
    /// `"claude-code"`.
    pub fn new(
        config: HarnessConfig,
        claude_config: ClaudeHarnessConfig,
    ) -> Result<Self, HarnessError> {
        if config.id.as_str() != "claude-code" {
            return Err(HarnessError::InvalidState {
                message: format!(
                    "ClaudeHarness requires id \"claude-code\", got \"{}\"",
                    config.id
                ),
            });
        }

        info!(
            harness_id = %config.id,
            binary_path = %claude_config.binary_path,
            working_dir = ?claude_config.working_dir,
            timeout_secs = claude_config.timeout.as_secs(),
            "ClaudeHarness created"
        );

        Ok(Self {
            config,
            claude_config,
            sessions: Mutex::new(HashMap::new()),
            status: Mutex::new(HarnessStatus::Idle),
        })
    }

    /// Return the resolved executable path for the Claude CLI.
    ///
    /// Prefers the `HarnessConfig::executable_path` if set, then falls
    /// back to `ClaudeHarnessConfig::binary_path`.
    #[must_use]
    pub fn executable_path(&self) -> PathBuf {
        self.config
            .executable_path
            .clone()
            .unwrap_or_else(|| PathBuf::from(&self.claude_config.binary_path))
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

    /// Return the Claude-specific configuration.
    #[must_use]
    pub fn claude_config(&self) -> &ClaudeHarnessConfig {
        &self.claude_config
    }

    /// Resolve the working directory for a session.
    ///
    /// Priority: session config > claude config > harness config > inherit.
    fn resolve_working_dir(&self, session_config: &SessionConfig) -> Option<PathBuf> {
        session_config
            .working_directory
            .clone()
            .or_else(|| self.claude_config.working_dir.clone())
            .or_else(|| self.config.workspace_path.clone())
    }
}

#[async_trait]
impl Harness for ClaudeHarness {
    fn id(&self) -> &HarnessId {
        &self.config.id
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_sessions: true,
            max_context_tokens: 200_000,
            models: vec![
                "claude-opus-4-6".into(),
                "claude-sonnet-4-6".into(),
            ],
            transport: Some(TransportFlavor::JsonRpcStdio),
            model_override: None,
            session_resume: SessionResumeMode::ById,
            mcp_passthrough: McpMode::Configurable,
            tool_injection: ToolInjection::McpConfig,
            cancel: CancelMode::Signal,
            multiplex_safe: false,
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
        let exe_path = self.executable_path();
        let working_dir = self.resolve_working_dir(&config);

        debug!(
            session_id = %session_id,
            binary = %exe_path.display(),
            working_dir = ?working_dir,
            system_prompt = config.system_prompt.as_deref().unwrap_or("<none>"),
            tool_count = config.tools.len(),
            "Starting Claude Code session"
        );

        // Build the subprocess command.
        let mut cmd = tokio::process::Command::new(&exe_path);
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        if let Some(ref dir) = working_dir {
            cmd.current_dir(dir);
        }

        // Pass through any configured environment variables.
        for (key, value) in &self.config.env_vars {
            cmd.env(key, value);
        }

        // Spawn the child process.
        let mut child = cmd.spawn().map_err(|e| {
            let kind = e.kind();
            if kind == std::io::ErrorKind::NotFound {
                HarnessError::ExecutableNotFound {
                    path: exe_path.display().to_string(),
                }
            } else {
                HarnessError::SpawnFailed {
                    message: format!(
                        "failed to spawn '{}': {e}",
                        exe_path.display()
                    ),
                }
            }
        })?;

        // Take ownership of the piped handles.
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();

        let state = SessionState {
            id: session_id,
            config,
            messages: Vec::new(),
            active: true,
            child,
            stdin,
            stdout,
            binary_path: exe_path.display().to_string(),
            working_dir,
        };

        {
            let mut sessions = self.sessions.lock().expect("sessions mutex poisoned");
            sessions.insert(session_id, state);
        }

        // Update harness status to Running.
        {
            let mut status = self.status.lock().expect("status mutex poisoned");
            *status = HarnessStatus::Running {
                since: Utc::now(),
                run_id: None,
            };
        }

        info!(session_id = %session_id, "Claude Code session started");
        Ok(session_id)
    }

    async fn send_message(
        &self,
        session_id: SessionId,
        message: &str,
    ) -> Result<(), HarnessError> {
        // Serialize the message as a JSON line.
        let json_line = serde_json::to_string(&serde_json::json!({
            "type": "message",
            "content": message,
        }))
        .map_err(|e| HarnessError::IoError {
            message: format!("failed to serialize message: {e}"),
        })?;

        let line_bytes = format!("{json_line}\n");

        // Take the stdin handle out of the session under the lock, record
        // the message, then drop the lock before performing async I/O.
        let mut taken_stdin = {
            let mut sessions = self.sessions.lock().expect("sessions mutex poisoned");
            let session = sessions.get_mut(&session_id).ok_or_else(|| {
                HarnessError::SessionNotFound { session_id }
            })?;

            if !session.active {
                return Err(HarnessError::InvalidState {
                    message: format!("session {session_id} is no longer active"),
                });
            }

            debug!(
                session_id = %session_id,
                message_len = message.len(),
                "Writing message to Claude Code session stdin"
            );

            // Record the message.
            session.messages.push(message.to_owned());

            // Take the stdin handle so we can write outside the lock.
            session.stdin.take().ok_or_else(|| {
                HarnessError::IoError {
                    message: format!("session {session_id} stdin is not available"),
                }
            })?
            // MutexGuard (`sessions`) is dropped here at the end of the block.
        };

        // Write the JSON line and flush (no mutex held).
        let write_result = taken_stdin.write_all(line_bytes.as_bytes()).await;
        let flush_result = if write_result.is_ok() {
            taken_stdin.flush().await
        } else {
            Ok(())
        };

        // Put stdin back into the session.
        {
            let mut sessions = self.sessions.lock().expect("sessions mutex poisoned");
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

    async fn receive_events(
        &self,
        session_id: SessionId,
    ) -> Result<Pin<Box<dyn Stream<Item = HarnessEvent> + Send>>, HarnessError> {
        // Take the stdout handle from the session.
        let stdout = {
            let mut sessions = self.sessions.lock().expect("sessions mutex poisoned");
            let session = sessions.get_mut(&session_id).ok_or_else(|| {
                HarnessError::SessionNotFound { session_id }
            })?;

            session.stdout.take().ok_or_else(|| {
                HarnessError::InvalidState {
                    message: format!(
                        "session {session_id} stdout already consumed \
                         (event stream can only be created once)"
                    ),
                }
            })?
        };

        debug!(
            session_id = %session_id,
            "Creating event stream from Claude Code session stdout"
        );

        let reader = BufReader::new(stdout);
        let lines = reader.lines();

        let stream = async_stream::stream! {
            // Emit a SessionStarted event first.
            yield HarnessEvent::SessionStarted { session_id };

            let mut lines = lines;
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        let line = line.trim().to_owned();
                        if line.is_empty() {
                            continue;
                        }

                        // Try to parse as a HarnessEvent.
                        match serde_json::from_str::<HarnessEvent>(&line) {
                            Ok(event) => yield event,
                            Err(_) => {
                                // If it's not a valid HarnessEvent JSON,
                                // wrap the raw line as a MessageReceived.
                                yield HarnessEvent::MessageReceived {
                                    session_id,
                                    content: line,
                                };
                            }
                        }
                    }
                    Ok(None) => {
                        // EOF -- child's stdout closed.
                        break;
                    }
                    Err(e) => {
                        yield HarnessEvent::Error {
                            session_id,
                            message: format!("error reading stdout: {e}"),
                        };
                        break;
                    }
                }
            }

            // Emit a SessionEnded event when the stream closes.
            yield HarnessEvent::SessionEnded { session_id };
        };

        Ok(Box::pin(stream))
    }

    async fn end_session(
        &self,
        session_id: SessionId,
    ) -> Result<(), HarnessError> {
        let timeout = self.claude_config.timeout;

        // Phase 1: Under the lock, mark inactive and close I/O handles.
        // Extract the PID for signal-based termination.
        let pid = {
            let mut sessions = self.sessions.lock().expect("sessions mutex poisoned");
            let session = sessions.get_mut(&session_id).ok_or_else(|| {
                HarnessError::SessionNotFound { session_id }
            })?;

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
                "Ending Claude Code session"
            );

            session.active = false;

            // Drop stdin to signal EOF to the child.
            session.stdin.take();
            // Drop stdout as well.
            session.stdout.take();

            // Get the PID before dropping the lock.
            session.child.id()
            // MutexGuard dropped here.
        };

        // Phase 2: Terminate the child process (no lock held).
        #[cfg(unix)]
        if let Some(pid) = pid {
            debug!(session_id = %session_id, pid = pid, "Sending SIGTERM to child");

            // Send SIGTERM via the `kill` command (no libc/nix dependency).
            let kill_result = tokio::process::Command::new("kill")
                .arg("-TERM")
                .arg(pid.to_string())
                .output()
                .await;

            if let Err(e) = kill_result {
                warn!(
                    session_id = %session_id,
                    error = %e,
                    "Failed to send SIGTERM"
                );
            }

            // Poll try_wait in a loop with a timeout. Each iteration
            // briefly acquires the lock, checks, then releases it before
            // sleeping.
            let exited = tokio::time::timeout(timeout, async {
                loop {
                    let done = {
                        let mut sessions =
                            self.sessions.lock().expect("sessions mutex poisoned");
                        match sessions.get_mut(&session_id) {
                            Some(session) => match session.child.try_wait() {
                                Ok(Some(_)) => true,
                                Ok(None) => false,
                                Err(_) => true, // treat errors as "done"
                            },
                            None => true, // session removed
                        }
                        // MutexGuard dropped here.
                    };
                    if done {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await;

            if exited.is_ok() {
                debug!(session_id = %session_id, "Child exited after SIGTERM");
            } else {
                // Timeout -- escalate to SIGKILL.
                warn!(
                    session_id = %session_id,
                    timeout_secs = timeout.as_secs(),
                    "Child did not exit after SIGTERM, sending SIGKILL"
                );

                let sigkill_result = tokio::process::Command::new("kill")
                    .arg("-KILL")
                    .arg(pid.to_string())
                    .output()
                    .await;

                if let Err(e) = sigkill_result {
                    error!(
                        session_id = %session_id,
                        error = %e,
                        "Failed to send SIGKILL"
                    );
                }
            }
        }

        #[cfg(not(unix))]
        {
            // On non-Unix platforms, use `child.start_kill()` which is
            // synchronous (non-async), so we can hold the lock safely.
            let mut sessions = self.sessions.lock().expect("sessions mutex poisoned");
            if let Some(session) = sessions.get_mut(&session_id) {
                if let Err(e) = session.child.start_kill() {
                    warn!(
                        session_id = %session_id,
                        error = %e,
                        "Failed to kill child process"
                    );
                }
            }
        }

        // Phase 3: Reap the child and update status (under lock).
        {
            let mut sessions = self.sessions.lock().expect("sessions mutex poisoned");
            if let Some(session) = sessions.get_mut(&session_id) {
                match session.child.try_wait() {
                    Ok(Some(status)) => {
                        debug!(
                            session_id = %session_id,
                            exit_status = ?status,
                            "Child process reaped"
                        );
                    }
                    Ok(None) => {
                        debug!(
                            session_id = %session_id,
                            "Child process still running after shutdown sequence"
                        );
                    }
                    Err(e) => {
                        warn!(
                            session_id = %session_id,
                            error = %e,
                            "Error reaping child process"
                        );
                    }
                }
            }

            // If no more active sessions, return to Idle.
            let any_active = sessions.values().any(|s| s.active);
            if !any_active {
                drop(sessions);
                let mut status = self.status.lock().expect("status mutex poisoned");
                *status = HarnessStatus::Idle;
            }
        }

        info!(session_id = %session_id, "Claude Code session ended");
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
                "Claude Code health check (explicit path)"
            );
            return Ok(exists);
        }

        // If no explicit path, try to find the binary on $PATH using `which`.
        let binary = &self.claude_config.binary_path;
        debug!(
            binary = %binary,
            "Claude Code health check (searching $PATH)"
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

    async fn save_session_state(
        &self,
        session_id: SessionId,
    ) -> Result<polkagent_harness_trait::SessionSnapshot, HarnessError> {
        let sessions = self.sessions.lock().expect("sessions mutex poisoned");
        let session = sessions.get(&session_id).ok_or(HarnessError::SessionNotFound { session_id })?;

        let pid = session.child.id();

        let mut backend_state = std::collections::HashMap::new();
        backend_state.insert(
            "messages".into(),
            serde_json::Value::Array(
                session.messages.iter().map(|m| serde_json::Value::String(m.clone())).collect(),
            ),
        );

        let snapshot = polkagent_harness_trait::SessionSnapshot {
            session_id,
            harness_id: self.config.id.clone(),
            process_pid: pid,
            started_at: chrono::Utc::now(),
            working_directory: session.working_dir.clone(),
            turn_count: session.messages.len() as u32,
            backend_state,
        };

        polkagent_harness_trait::persist_session_state(&snapshot)?;
        debug!(session_id = %session_id, "Claude Code session state saved");
        Ok(snapshot)
    }

    async fn resume_session(
        &self,
        session_id: SessionId,
    ) -> Result<SessionId, HarnessError> {
        let snapshot = polkagent_harness_trait::load_session_state(session_id)?;

        // Check if the old process is still alive.
        if let Some(pid) = snapshot.process_pid {
            let alive = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid as i32),
                None,
            )
            .is_ok();

            if alive {
                debug!(pid, session_id = %session_id, "Re-attaching to live Claude process");
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
            "Claude Code session resumed with new process"
        );
        Ok(new_id)
    }

    async fn cancel_session(
        &self,
        session_id: SessionId,
    ) -> Result<(), HarnessError> {
        #[cfg(unix)]
        {
            let raw_pid = {
                let sessions = self.sessions.lock().expect("sessions mutex poisoned");
                let session = sessions.get(&session_id).ok_or(HarnessError::SessionNotFound { session_id })?;
                session.child.id()
            };

            if let Some(pid) = raw_pid {
                debug!(session_id = %session_id, pid, "Sending SIGTERM to Claude process");
                let nix_pid = nix::unistd::Pid::from_raw(pid as i32);
                let _ = nix::sys::signal::kill(nix_pid, nix::sys::signal::Signal::SIGTERM);

                // Wait up to 5 seconds for clean exit.
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                loop {
                    let exited = {
                        let mut sessions = self.sessions.lock().expect("sessions mutex poisoned");
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
mod tests {
    use super::*;
    use futures::StreamExt;
    use polkagent_harness_trait::{HarnessConfig, SessionConfig};
    use std::path::PathBuf;

    fn test_config() -> HarnessConfig {
        HarnessConfig::new("claude-code")
    }

    fn test_config_with_path(path: &str) -> HarnessConfig {
        let mut config = HarnessConfig::new("claude-code");
        config.executable_path = Some(PathBuf::from(path));
        config
    }

    /// Helper: create a harness that uses `cat` as the subprocess binary.
    /// `cat` echoes stdin to stdout line by line, which is perfect for
    /// testing the I/O pipeline without needing a real `claude` binary.
    fn cat_harness() -> ClaudeHarness {
        let config = test_config();
        let claude_config = ClaudeHarnessConfig {
            binary_path: "cat".to_owned(),
            ..ClaudeHarnessConfig::default()
        };
        ClaudeHarness::new(config, claude_config).expect("cat harness")
    }

    // -----------------------------------------------------------------------
    // Config tests
    // -----------------------------------------------------------------------

    #[test]
    fn config_defaults_are_sensible() {
        let config = ClaudeHarnessConfig::default();
        assert_eq!(config.binary_path, "claude");
        assert!(config.working_dir.is_none());
        assert_eq!(config.timeout, Duration::from_secs(10));
    }

    #[test]
    fn config_with_custom_binary_path() {
        let config = ClaudeHarnessConfig {
            binary_path: "/opt/bin/claude-custom".to_owned(),
            ..ClaudeHarnessConfig::default()
        };
        assert_eq!(config.binary_path, "/opt/bin/claude-custom");

        let harness = ClaudeHarness::new(test_config(), config).expect("should succeed");
        assert_eq!(
            harness.executable_path(),
            PathBuf::from("/opt/bin/claude-custom")
        );
    }

    #[test]
    fn config_harness_executable_path_takes_priority() {
        // HarnessConfig.executable_path should override ClaudeHarnessConfig.binary_path.
        let hconfig = test_config_with_path("/from/harness/config");
        let cconfig = ClaudeHarnessConfig {
            binary_path: "/from/claude/config".to_owned(),
            ..ClaudeHarnessConfig::default()
        };
        let harness = ClaudeHarness::new(hconfig, cconfig).expect("should succeed");
        assert_eq!(
            harness.executable_path(),
            PathBuf::from("/from/harness/config")
        );
    }

    // -----------------------------------------------------------------------
    // Existing unit tests (updated for new constructor)
    // -----------------------------------------------------------------------

    #[test]
    fn new_validates_harness_id() {
        let config = HarnessConfig::new("not-claude");
        let result = ClaudeHarness::new(config, ClaudeHarnessConfig::default());
        assert!(result.is_err());
        let err = result.expect_err("should fail");
        assert!(err.to_string().contains("claude-code"));
    }

    #[test]
    fn new_accepts_claude_code_id() {
        let harness = ClaudeHarness::new(
            test_config(),
            ClaudeHarnessConfig::default(),
        )
        .expect("should succeed");
        assert_eq!(harness.id().as_str(), "claude-code");
    }

    #[test]
    fn capabilities_returns_expected_values() {
        let harness = ClaudeHarness::new(
            test_config(),
            ClaudeHarnessConfig::default(),
        )
        .expect("should succeed");
        let caps = harness.capabilities();
        assert!(caps.supports_streaming);
        assert!(caps.supports_tools);
        assert!(caps.supports_sessions);
        assert_eq!(caps.max_context_tokens, 200_000);
        assert!(!caps.models.is_empty());
        assert!(caps.models.contains(&"claude-opus-4-6".to_owned()));
    }

    #[test]
    fn initial_status_is_idle() {
        let harness = ClaudeHarness::new(
            test_config(),
            ClaudeHarnessConfig::default(),
        )
        .expect("should succeed");
        assert!(matches!(harness.status(), HarnessStatus::Idle));
    }

    #[test]
    fn executable_path_default() {
        let harness = ClaudeHarness::new(
            test_config(),
            ClaudeHarnessConfig::default(),
        )
        .expect("should succeed");
        assert_eq!(harness.executable_path(), PathBuf::from("claude"));
    }

    #[test]
    fn executable_path_custom() {
        let harness = ClaudeHarness::new(
            test_config_with_path("/opt/bin/claude"),
            ClaudeHarnessConfig::default(),
        )
        .expect("should succeed");
        assert_eq!(harness.executable_path(), PathBuf::from("/opt/bin/claude"));
    }

    #[test]
    fn initial_active_session_count_is_zero() {
        let harness = ClaudeHarness::new(
            test_config(),
            ClaudeHarnessConfig::default(),
        )
        .expect("should succeed");
        assert_eq!(harness.active_session_count(), 0);
    }

    // -----------------------------------------------------------------------
    // Session lifecycle tests (using `cat` as subprocess)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn start_session_with_missing_binary_returns_error() {
        let config = test_config();
        let claude_config = ClaudeHarnessConfig {
            binary_path: "/nonexistent/path/to/no-such-binary-xyz".to_owned(),
            ..ClaudeHarnessConfig::default()
        };
        let harness = ClaudeHarness::new(config, claude_config).expect("construct");

        let result = harness.start_session(SessionConfig::default()).await;
        assert!(result.is_err(), "start_session should fail for missing binary");

        let err = result.expect_err("should be an error");
        // Should be ExecutableNotFound, not a panic.
        assert!(
            matches!(err, HarnessError::ExecutableNotFound { .. }),
            "expected ExecutableNotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn session_state_transitions_idle_active_idle() {
        let harness = cat_harness();

        // Initially Idle.
        assert!(
            matches!(harness.status(), HarnessStatus::Idle),
            "should start Idle"
        );

        // After starting a session: Running.
        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start");
        assert!(
            matches!(harness.status(), HarnessStatus::Running { .. }),
            "should be Running after start_session"
        );
        assert_eq!(harness.active_session_count(), 1);

        // After ending: Idle again.
        harness.end_session(session_id).await.expect("end");
        assert!(
            matches!(harness.status(), HarnessStatus::Idle),
            "should return to Idle after end_session"
        );
        assert_eq!(harness.active_session_count(), 0);
    }

    #[tokio::test]
    async fn multiple_sessions_can_be_tracked() {
        let harness = cat_harness();

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

        // End one -- the other should still be active.
        harness.end_session(s1).await.expect("end s1");
        assert_eq!(harness.active_session_count(), 1);
        assert!(matches!(harness.status(), HarnessStatus::Running { .. }));

        // End the other.
        harness.end_session(s2).await.expect("end s2");
        assert_eq!(harness.active_session_count(), 0);
        assert!(matches!(harness.status(), HarnessStatus::Idle));
    }

    #[tokio::test]
    async fn send_message_to_nonexistent_session_returns_error() {
        let harness = cat_harness();
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
        let harness = cat_harness();
        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start");

        harness.end_session(session_id).await.expect("first end");
        // Second end should succeed without error (idempotent).
        harness
            .end_session(session_id)
            .await
            .expect("second end should be idempotent");
    }

    #[tokio::test]
    async fn session_metadata_is_populated_after_start() {
        let harness = cat_harness();
        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start");

        let meta = harness
            .session_metadata(session_id)
            .expect("metadata should exist");
        assert_eq!(meta.session_id, session_id);
        assert_eq!(meta.binary_path, "cat");
        assert!(meta.active);
        assert_eq!(meta.message_count, 0);

        harness.end_session(session_id).await.expect("end");
    }

    #[tokio::test]
    async fn start_session_returns_session_id() {
        let harness = cat_harness();
        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start_session should succeed");

        assert_eq!(harness.active_session_count(), 1);
        assert!(matches!(harness.status(), HarnessStatus::Running { .. }));
        assert_ne!(session_id.to_string(), "");

        harness.end_session(session_id).await.expect("end");
    }

    #[tokio::test]
    async fn send_message_writes_to_stdin_and_records() {
        let harness = cat_harness();
        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start");

        harness
            .send_message(session_id, "Hello, Claude!")
            .await
            .expect("send_message should succeed");

        let messages = harness
            .session_messages(session_id)
            .expect("session should exist");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0], "Hello, Claude!");

        harness.end_session(session_id).await.expect("end");
    }

    #[tokio::test]
    async fn send_multiple_messages_records_all() {
        let harness = cat_harness();
        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start");

        harness.send_message(session_id, "first").await.expect("ok");
        harness.send_message(session_id, "second").await.expect("ok");
        harness.send_message(session_id, "third").await.expect("ok");

        let messages = harness
            .session_messages(session_id)
            .expect("session exists");
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0], "first");
        assert_eq!(messages[1], "second");
        assert_eq!(messages[2], "third");

        // Metadata should reflect message count.
        let meta = harness
            .session_metadata(session_id)
            .expect("metadata");
        assert_eq!(meta.message_count, 3);

        harness.end_session(session_id).await.expect("end");
    }

    #[tokio::test]
    async fn send_message_to_ended_session_fails() {
        let harness = cat_harness();
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
    async fn receive_events_reads_from_stdout() {
        let harness = cat_harness();
        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start");

        // Send a message -- `cat` will echo it back on stdout.
        harness
            .send_message(session_id, "test message")
            .await
            .expect("send");

        // Close stdin by ending the session so cat produces EOF on stdout.
        // But first, grab the event stream.
        let event_stream = harness
            .receive_events(session_id)
            .await
            .expect("receive_events should succeed");

        // End the session to close stdin, which causes `cat` to EOF.
        harness.end_session(session_id).await.expect("end");

        // Collect events with a timeout.
        let events: Vec<HarnessEvent> = tokio::time::timeout(
            Duration::from_secs(5),
            event_stream.collect(),
        )
        .await
        .expect("event collection should not time out");

        // Should have at least SessionStarted and SessionEnded.
        assert!(events.len() >= 2, "expected at least 2 events, got {}", events.len());

        // First event should be SessionStarted.
        assert!(
            matches!(&events[0], HarnessEvent::SessionStarted { .. }),
            "first event should be SessionStarted, got {:?}",
            events[0]
        );

        // Last event should be SessionEnded.
        assert!(
            matches!(events.last(), Some(HarnessEvent::SessionEnded { .. })),
            "last event should be SessionEnded, got {:?}",
            events.last()
        );

        // There should be a MessageReceived in between (the echoed JSON line).
        let has_message = events.iter().any(|e| {
            matches!(e, HarnessEvent::MessageReceived { .. })
        });
        assert!(has_message, "should have at least one MessageReceived event");
    }

    #[tokio::test]
    async fn receive_events_unknown_session_fails() {
        let harness = cat_harness();
        let result = harness.receive_events(SessionId::new()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn end_session_unknown_session_fails() {
        let harness = cat_harness();
        let result = harness.end_session(SessionId::new()).await;
        assert!(result.is_err());
        assert!(matches!(
            result.expect_err("should fail"),
            HarnessError::SessionNotFound { .. }
        ));
    }

    #[tokio::test]
    async fn end_session_returns_to_idle_when_no_active_sessions() {
        let harness = cat_harness();
        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start");

        assert!(matches!(harness.status(), HarnessStatus::Running { .. }));

        harness.end_session(session_id).await.expect("end");

        assert!(matches!(harness.status(), HarnessStatus::Idle));
    }

    #[tokio::test]
    async fn end_session_stays_running_with_other_active_sessions() {
        let harness = cat_harness();
        let s1 = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start s1");
        let _s2 = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start s2");

        harness.end_session(s1).await.expect("end s1");

        assert!(matches!(harness.status(), HarnessStatus::Running { .. }));
        assert_eq!(harness.active_session_count(), 1);

        // Clean up s2.
        harness.end_session(_s2).await.expect("end s2");
    }

    #[tokio::test]
    async fn health_with_nonexistent_path_returns_false() {
        let config = test_config_with_path("/nonexistent/path/to/claude");
        let harness = ClaudeHarness::new(
            config,
            ClaudeHarnessConfig::default(),
        )
        .expect("should succeed");
        let result = harness.health().await.expect("health check");
        assert!(!result);
    }

    #[tokio::test]
    async fn health_with_existing_path_returns_true() {
        let config = test_config_with_path("/bin/sh");
        let harness = ClaudeHarness::new(
            config,
            ClaudeHarnessConfig::default(),
        )
        .expect("should succeed");
        let result = harness.health().await.expect("health check");
        assert!(result);
    }

    #[tokio::test]
    async fn full_session_lifecycle() {
        let harness = cat_harness();

        // 1. Start in Idle.
        assert!(matches!(harness.status(), HarnessStatus::Idle));
        assert_eq!(harness.active_session_count(), 0);

        // 2. Start a session.
        let session_id = harness
            .start_session(SessionConfig {
                system_prompt: Some("You are a helpful assistant.".into()),
                tools: vec![],
                working_directory: None,
            })
            .await
            .expect("start");
        assert!(matches!(harness.status(), HarnessStatus::Running { .. }));
        assert_eq!(harness.active_session_count(), 1);

        // 3. Send messages.
        harness
            .send_message(session_id, "Hello!")
            .await
            .expect("send 1");
        harness
            .send_message(session_id, "How are you?")
            .await
            .expect("send 2");

        let messages = harness.session_messages(session_id).expect("messages");
        assert_eq!(messages.len(), 2);

        // 4. Metadata reflects state.
        let meta = harness.session_metadata(session_id).expect("meta");
        assert!(meta.active);
        assert_eq!(meta.message_count, 2);

        // 5. End session.
        harness.end_session(session_id).await.expect("end");
        assert!(matches!(harness.status(), HarnessStatus::Idle));
        assert_eq!(harness.active_session_count(), 0);

        // 6. Metadata reflects ended state.
        let meta = harness.session_metadata(session_id).expect("meta after end");
        assert!(!meta.active);
    }

    #[tokio::test]
    async fn session_config_with_tools_is_recorded() {
        use polkagent_executor_trait::ToolDefinition;

        let harness = cat_harness();
        let config = SessionConfig {
            system_prompt: Some("test prompt".into()),
            tools: vec![ToolDefinition {
                name: "polkagent.file.read".into(),
                description: "Read a file".into(),
                input_schema_json: r#"{"type":"object"}"#.into(),
            }],
            working_directory: Some(PathBuf::from("/tmp")),
        };

        let session_id = harness.start_session(config).await.expect("start");
        assert_eq!(harness.active_session_count(), 1);

        let meta = harness.session_metadata(session_id).expect("meta");
        // working_dir should reflect the session config.
        assert_eq!(meta.working_dir, Some(PathBuf::from("/tmp")));

        harness.end_session(session_id).await.expect("end");
    }

    #[tokio::test]
    async fn session_with_working_dir_from_claude_config() {
        let config = test_config();
        let claude_config = ClaudeHarnessConfig {
            binary_path: "cat".to_owned(),
            working_dir: Some(PathBuf::from("/tmp")),
            ..ClaudeHarnessConfig::default()
        };
        let harness = ClaudeHarness::new(config, claude_config).expect("construct");

        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start");

        let meta = harness.session_metadata(session_id).expect("meta");
        assert_eq!(meta.working_dir, Some(PathBuf::from("/tmp")));

        harness.end_session(session_id).await.expect("end");
    }

    #[test]
    fn claude_config_is_accessible() {
        let claude_config = ClaudeHarnessConfig {
            binary_path: "custom-claude".to_owned(),
            working_dir: Some(PathBuf::from("/workspace")),
            timeout: Duration::from_secs(30),
        };
        let harness = ClaudeHarness::new(test_config(), claude_config).expect("construct");
        let cfg = harness.claude_config();
        assert_eq!(cfg.binary_path, "custom-claude");
        assert_eq!(cfg.working_dir, Some(PathBuf::from("/workspace")));
        assert_eq!(cfg.timeout, Duration::from_secs(30));
    }

    /// Compile-time check: `ClaudeHarness` satisfies the `Harness` trait
    /// object-safety requirement.
    #[allow(dead_code)]
    fn _claude_harness_is_object_safe(h: &ClaudeHarness) {
        let _dyn: &dyn Harness = h;
    }
}
