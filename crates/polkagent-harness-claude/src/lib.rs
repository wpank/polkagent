//! Claude Code harness adapter for the Polkagent platform.
//!
//! This crate provides [`ClaudeHarness`] — an implementation of the
//! [`Harness`] trait that wraps the Claude Code CLI (`claude`) as a
//! managed subprocess. It exposes a session-based API for interacting
//! with Claude Code programmatically.
//!
//! # Current status
//!
//! This is a **structural stub**. The session management and subprocess
//! lifecycle are tracked in-memory, but actual subprocess spawning and
//! stdin/stdout I/O will be implemented in a future iteration. The stub
//! emits synthetic events so that downstream consumers can develop against
//! a working (if fake) harness.
//!
//! # Example
//!
//! ```rust,no_run
//! use polkagent_harness_claude::ClaudeHarness;
//! use polkagent_harness_trait::{Harness, HarnessConfig, SessionConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = HarnessConfig::new("claude-code");
//! let harness = ClaudeHarness::new(config)?;
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

use async_trait::async_trait;
use chrono::Utc;
use futures::stream;
use futures::Stream;
use tracing::{debug, info, warn};

use polkagent_harness_trait::{
    Harness, HarnessCapabilities, HarnessConfig, HarnessError, HarnessEvent, HarnessId,
    HarnessStatus, SessionConfig, SessionId,
};

// ---------------------------------------------------------------------------
// Session state (internal)
// ---------------------------------------------------------------------------

/// Internal state for a single Claude Code session.
#[derive(Debug)]
struct SessionState {
    /// The session identifier.
    #[allow(dead_code)]
    id: SessionId,
    /// The configuration used to start this session.
    #[allow(dead_code)]
    config: SessionConfig,
    /// Messages sent to this session (for stub recording).
    messages: Vec<String>,
    /// Whether the session is still active.
    active: bool,
}

// ---------------------------------------------------------------------------
// ClaudeHarness
// ---------------------------------------------------------------------------

/// Claude Code harness implementation.
///
/// Wraps the `claude` CLI as a managed subprocess, providing a session-based
/// API for sending messages and receiving structured events.
///
/// In the current stub implementation:
/// - [`start_session`] creates an in-memory session record.
/// - [`send_message`] records the message for later inspection.
/// - [`receive_events`] emits synthetic events (session started, a message
///   echo, and session ended).
/// - [`end_session`] marks the session as inactive.
/// - [`health`] checks whether the configured executable path exists on disk.
///
/// [`start_session`]: Harness::start_session
/// [`send_message`]: Harness::send_message
/// [`receive_events`]: Harness::receive_events
/// [`end_session`]: Harness::end_session
/// [`health`]: Harness::health
pub struct ClaudeHarness {
    /// The harness configuration.
    config: HarnessConfig,
    /// Active sessions, keyed by session ID.
    sessions: Mutex<HashMap<SessionId, SessionState>>,
    /// Current operational status.
    status: Mutex<HarnessStatus>,
}

impl std::fmt::Debug for ClaudeHarness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeHarness")
            .field("config", &self.config)
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
    pub fn new(config: HarnessConfig) -> Result<Self, HarnessError> {
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
            executable = ?config.executable_path,
            workspace = ?config.workspace_path,
            "ClaudeHarness created"
        );

        Ok(Self {
            config,
            sessions: Mutex::new(HashMap::new()),
            status: Mutex::new(HarnessStatus::Idle),
        })
    }

    /// Return the resolved executable path for the Claude CLI.
    ///
    /// Uses the configured `executable_path` if set, otherwise defaults
    /// to `"claude"` (expecting it to be on `$PATH`).
    #[must_use]
    pub fn executable_path(&self) -> PathBuf {
        self.config
            .executable_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("claude"))
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

        debug!(
            session_id = %session_id,
            system_prompt = config.system_prompt.as_deref().unwrap_or("<none>"),
            tool_count = config.tools.len(),
            "Starting Claude Code session (stub)"
        );

        // Record the session state.
        let state = SessionState {
            id: session_id,
            config,
            messages: Vec::new(),
            active: true,
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

        info!(session_id = %session_id, "Claude Code session started (stub)");
        Ok(session_id)
    }

    async fn send_message(
        &self,
        session_id: SessionId,
        message: &str,
    ) -> Result<(), HarnessError> {
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
            "Recording message to Claude Code session (stub)"
        );

        // Stub: record the message instead of writing to subprocess stdin.
        session.messages.push(message.to_owned());

        Ok(())
    }

    async fn receive_events(
        &self,
        session_id: SessionId,
    ) -> Result<Pin<Box<dyn Stream<Item = HarnessEvent> + Send>>, HarnessError> {
        // Verify the session exists.
        {
            let sessions = self.sessions.lock().expect("sessions mutex poisoned");
            if !sessions.contains_key(&session_id) {
                return Err(HarnessError::SessionNotFound { session_id });
            }
        }

        debug!(
            session_id = %session_id,
            "Creating synthetic event stream for Claude Code session (stub)"
        );

        // Stub: emit a synthetic sequence of events.
        let events = vec![
            HarnessEvent::SessionStarted { session_id },
            HarnessEvent::MessageReceived {
                session_id,
                content: "I am Claude Code (stub harness). How can I help?".into(),
            },
            HarnessEvent::SessionEnded { session_id },
        ];

        Ok(Box::pin(stream::iter(events)))
    }

    async fn end_session(
        &self,
        session_id: SessionId,
    ) -> Result<(), HarnessError> {
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
            "Ending Claude Code session (stub)"
        );

        // Stub: mark inactive instead of killing subprocess.
        session.active = false;

        // If no more active sessions, return to Idle.
        let any_active = sessions.values().any(|s| s.active);
        if !any_active {
            drop(sessions);
            let mut status = self.status.lock().expect("status mutex poisoned");
            *status = HarnessStatus::Idle;
        }

        info!(session_id = %session_id, "Claude Code session ended (stub)");
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

        // If no explicit path, try to find `claude` on $PATH using `which`.
        debug!("Claude Code health check (searching $PATH for 'claude')");
        let result = tokio::process::Command::new("which")
            .arg("claude")
            .output()
            .await;

        match result {
            Ok(output) => Ok(output.status.success()),
            Err(e) => {
                warn!(error = %e, "Failed to run 'which claude'");
                // `which` not available is not a harness error — just report
                // that we cannot confirm health.
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
        HarnessConfig::new("claude-code")
    }

    fn test_config_with_path(path: &str) -> HarnessConfig {
        let mut config = HarnessConfig::new("claude-code");
        config.executable_path = Some(PathBuf::from(path));
        config
    }

    #[test]
    fn new_validates_harness_id() {
        let config = HarnessConfig::new("not-claude");
        let result = ClaudeHarness::new(config);
        assert!(result.is_err());
        let err = result.expect_err("should fail");
        assert!(err.to_string().contains("claude-code"));
    }

    #[test]
    fn new_accepts_claude_code_id() {
        let harness = ClaudeHarness::new(test_config()).expect("should succeed");
        assert_eq!(harness.id().as_str(), "claude-code");
    }

    #[test]
    fn capabilities_returns_expected_values() {
        let harness = ClaudeHarness::new(test_config()).expect("should succeed");
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
        let harness = ClaudeHarness::new(test_config()).expect("should succeed");
        assert!(matches!(harness.status(), HarnessStatus::Idle));
    }

    #[test]
    fn executable_path_default() {
        let harness = ClaudeHarness::new(test_config()).expect("should succeed");
        assert_eq!(harness.executable_path(), PathBuf::from("claude"));
    }

    #[test]
    fn executable_path_custom() {
        let harness = ClaudeHarness::new(
            test_config_with_path("/opt/bin/claude")
        ).expect("should succeed");
        assert_eq!(harness.executable_path(), PathBuf::from("/opt/bin/claude"));
    }

    #[test]
    fn initial_active_session_count_is_zero() {
        let harness = ClaudeHarness::new(test_config()).expect("should succeed");
        assert_eq!(harness.active_session_count(), 0);
    }

    #[tokio::test]
    async fn start_session_returns_session_id() {
        let harness = ClaudeHarness::new(test_config()).expect("should succeed");
        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start_session should succeed");

        // Session should be tracked.
        assert_eq!(harness.active_session_count(), 1);

        // Status should be Running.
        assert!(matches!(harness.status(), HarnessStatus::Running { .. }));

        // Session ID should be valid.
        assert_ne!(session_id.to_string(), "");
    }

    #[tokio::test]
    async fn start_multiple_sessions() {
        let harness = ClaudeHarness::new(test_config()).expect("should succeed");

        let s1 = harness
            .start_session(SessionConfig::default())
            .await
            .expect("first session");
        let s2 = harness
            .start_session(SessionConfig::default())
            .await
            .expect("second session");

        assert_ne!(s1, s2);
        assert_eq!(harness.active_session_count(), 2);
    }

    #[tokio::test]
    async fn send_message_records_message() {
        let harness = ClaudeHarness::new(test_config()).expect("should succeed");
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
    }

    #[tokio::test]
    async fn send_multiple_messages() {
        let harness = ClaudeHarness::new(test_config()).expect("should succeed");
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
    }

    #[tokio::test]
    async fn send_message_to_unknown_session_fails() {
        let harness = ClaudeHarness::new(test_config()).expect("should succeed");
        let fake_id = SessionId::new();
        let result = harness.send_message(fake_id, "hello").await;
        assert!(result.is_err());
        assert!(matches!(
            result.expect_err("should fail"),
            HarnessError::SessionNotFound { .. }
        ));
    }

    #[tokio::test]
    async fn send_message_to_ended_session_fails() {
        let harness = ClaudeHarness::new(test_config()).expect("should succeed");
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
    async fn receive_events_returns_synthetic_stream() {
        let harness = ClaudeHarness::new(test_config()).expect("should succeed");
        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start");

        let event_stream = harness
            .receive_events(session_id)
            .await
            .expect("receive_events should succeed");

        let events: Vec<HarnessEvent> = event_stream.collect().await;

        // Should have exactly 3 synthetic events.
        assert_eq!(events.len(), 3);

        // First: SessionStarted.
        assert!(matches!(
            &events[0],
            HarnessEvent::SessionStarted { session_id: sid } if *sid == session_id
        ));

        // Second: MessageReceived with stub content.
        match &events[1] {
            HarnessEvent::MessageReceived { session_id: sid, content } => {
                assert_eq!(*sid, session_id);
                assert!(content.contains("stub harness"));
            }
            other => panic!("expected MessageReceived, got {other:?}"),
        }

        // Third: SessionEnded.
        assert!(matches!(
            &events[2],
            HarnessEvent::SessionEnded { session_id: sid } if *sid == session_id
        ));
    }

    #[tokio::test]
    async fn receive_events_unknown_session_fails() {
        let harness = ClaudeHarness::new(test_config()).expect("should succeed");
        let result = harness.receive_events(SessionId::new()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn end_session_marks_inactive() {
        let harness = ClaudeHarness::new(test_config()).expect("should succeed");
        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start");

        assert_eq!(harness.active_session_count(), 1);

        harness
            .end_session(session_id)
            .await
            .expect("end_session should succeed");

        assert_eq!(harness.active_session_count(), 0);
    }

    #[tokio::test]
    async fn end_session_returns_to_idle_when_no_active_sessions() {
        let harness = ClaudeHarness::new(test_config()).expect("should succeed");
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
        let harness = ClaudeHarness::new(test_config()).expect("should succeed");
        let s1 = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start s1");
        let _s2 = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start s2");

        harness.end_session(s1).await.expect("end s1");

        // Still have s2 active — status should remain Running.
        assert!(matches!(harness.status(), HarnessStatus::Running { .. }));
        assert_eq!(harness.active_session_count(), 1);
    }

    #[tokio::test]
    async fn end_session_unknown_session_fails() {
        let harness = ClaudeHarness::new(test_config()).expect("should succeed");
        let result = harness.end_session(SessionId::new()).await;
        assert!(result.is_err());
        assert!(matches!(
            result.expect_err("should fail"),
            HarnessError::SessionNotFound { .. }
        ));
    }

    #[tokio::test]
    async fn end_session_idempotent_on_inactive() {
        let harness = ClaudeHarness::new(test_config()).expect("should succeed");
        let session_id = harness
            .start_session(SessionConfig::default())
            .await
            .expect("start");

        harness.end_session(session_id).await.expect("first end");
        // Second end should not error (idempotent).
        harness.end_session(session_id).await.expect("second end");
    }

    #[tokio::test]
    async fn health_with_nonexistent_path_returns_false() {
        let config = test_config_with_path("/nonexistent/path/to/claude");
        let harness = ClaudeHarness::new(config).expect("should succeed");
        let result = harness.health().await.expect("health check");
        assert!(!result);
    }

    #[tokio::test]
    async fn health_with_existing_path_returns_true() {
        // Use a path we know exists (the test binary itself).
        let config = test_config_with_path("/bin/sh");
        let harness = ClaudeHarness::new(config).expect("should succeed");
        let result = harness.health().await.expect("health check");
        assert!(result);
    }

    #[tokio::test]
    async fn full_session_lifecycle() {
        let harness = ClaudeHarness::new(test_config()).expect("should succeed");

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
        harness.send_message(session_id, "Hello!").await.expect("send 1");
        harness.send_message(session_id, "How are you?").await.expect("send 2");

        let messages = harness.session_messages(session_id).expect("messages");
        assert_eq!(messages.len(), 2);

        // 4. Receive events.
        let events: Vec<_> = harness
            .receive_events(session_id)
            .await
            .expect("events")
            .collect()
            .await;
        assert!(!events.is_empty());

        // 5. End session.
        harness.end_session(session_id).await.expect("end");
        assert!(matches!(harness.status(), HarnessStatus::Idle));
        assert_eq!(harness.active_session_count(), 0);
    }

    #[tokio::test]
    async fn session_config_with_tools_is_recorded() {
        use polkagent_executor_trait::ToolDefinition;

        let harness = ClaudeHarness::new(test_config()).expect("should succeed");
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

        // Verify session was created (basic check).
        assert_eq!(harness.active_session_count(), 1);

        harness.end_session(session_id).await.expect("end");
    }

    /// Compile-time check: `ClaudeHarness` satisfies the `Harness` trait
    /// object-safety requirement.
    #[allow(dead_code)]
    fn _claude_harness_is_object_safe(h: &ClaudeHarness) {
        let _dyn: &dyn Harness = h;
    }
}
