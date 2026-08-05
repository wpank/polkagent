//! Shell command execution tool.
//!
//! The [`ShellTool`] executes a shell command and captures stdout and stderr.
//! It always requires an explicit grant (`shell/execute`) and enforces a
//! configurable timeout (default 30 seconds).

use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::process::Command;
use tracing::{debug, warn};

use polkagent_core::config::DataClassification;

use crate::registry::{ToolContext, ToolError, ToolHandler, ToolResult, ToolSpec};

// ---------------------------------------------------------------------------
// ShellTool
// ---------------------------------------------------------------------------

/// Executes a shell command and returns its output.
///
/// **Grant requirement:** `shell/execute` (always required — no bypass).
///
/// # Timeout
///
/// Commands are killed after the configured timeout (default 30 s). The caller
/// may override the timeout per-invocation via the `timeout_secs` input field.
pub struct ShellTool {
    /// Default timeout applied when the caller does not specify one.
    default_timeout: Duration,
}

impl ShellTool {
    /// Create a `ShellTool` with the given default timeout.
    #[must_use]
    pub fn new(default_timeout: Duration) -> Self {
        Self { default_timeout }
    }
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new(Duration::from_secs(30))
    }
}

#[async_trait]
impl ToolHandler for ShellTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "polkagent.shell.execute".to_string(),
            description: "Execute a shell command and return stdout and stderr.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute"
                    },
                    "working_directory": {
                        "type": "string",
                        "description": "Working directory for the command (optional)"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Timeout in seconds (optional, default 30)"
                    }
                },
                "required": ["command"]
            }),
            required_grant: Some("shell/execute".to_string()),
            output_classification: DataClassification::Private,
        }
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let command_str = input
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing or invalid 'command' field".to_string(),
            })?;

        let working_dir = input.get("working_directory").and_then(Value::as_str);

        let timeout = input
            .get("timeout_secs")
            .and_then(Value::as_u64)
            .map_or(self.default_timeout, Duration::from_secs);
        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);

        debug!(command = command_str, timeout_ms, "executing shell command");

        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command_str);

        if let Some(dir) = working_dir {
            cmd.current_dir(dir);
        }

        // Capture output with timeout.
        let output = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| {
                warn!(command = command_str, timeout_ms, "shell command timed out");
                ToolError::Timeout {
                    elapsed_ms: timeout_ms,
                }
            })?
            .map_err(|e| ToolError::ExecutionFailed {
                reason: format!("failed to execute shell command: {e}"),
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        debug!(
            command = command_str,
            exit_code,
            stdout_len = stdout.len(),
            stderr_len = stderr.len(),
            "shell command completed"
        );

        Ok(ToolResult {
            output: serde_json::json!({
                "exit_code": exit_code,
                "stdout": stdout,
                "stderr": stderr,
            }),
            classification: DataClassification::Private,
            artifacts: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::ids::{AgentId, RunId, StepId};

    fn test_context() -> ToolContext {
        ToolContext {
            run_id: RunId::new(),
            agent_id: AgentId::new(),
            step_id: StepId::new(),
            grants: vec![],
            security_config: None,
        }
    }

    #[test]
    fn shell_spec() {
        let tool = ShellTool::default();
        let spec = tool.spec();
        assert_eq!(spec.name, "polkagent.shell.execute");
        assert_eq!(spec.required_grant.as_deref(), Some("shell/execute"));
        assert_eq!(spec.output_classification, DataClassification::Private);
    }

    #[tokio::test]
    async fn execute_echo() {
        let tool = ShellTool::default();
        let input = serde_json::json!({ "command": "echo hello" });
        let result = tool.execute(input, &test_context()).await;
        assert!(result.is_ok(), "echo should succeed: {result:?}");

        let r = result.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.output["exit_code"], 0);
        assert_eq!(r.output["stdout"].as_str().unwrap_or("").trim(), "hello");
    }

    #[tokio::test]
    async fn execute_failing_command() {
        let tool = ShellTool::default();
        let input = serde_json::json!({ "command": "false" });
        let result = tool.execute(input, &test_context()).await;
        assert!(result.is_ok(), "should capture exit code, not error");

        let r = result.unwrap_or_else(|e| panic!("{e}"));
        assert_ne!(r.output["exit_code"], 0);
    }

    #[tokio::test]
    async fn execute_stderr_capture() {
        let tool = ShellTool::default();
        let input = serde_json::json!({ "command": "echo error >&2" });
        let result = tool.execute(input, &test_context()).await;
        assert!(result.is_ok());

        let r = result.unwrap_or_else(|e| panic!("{e}"));
        assert!(r.output["stderr"].as_str().unwrap_or("").contains("error"));
    }

    #[tokio::test]
    async fn execute_missing_command_field() {
        let tool = ShellTool::default();
        let input = serde_json::json!({});
        let result = tool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn execute_with_timeout() {
        let tool = ShellTool::new(Duration::from_secs(1));
        // sleep 10 should be killed by the 1s timeout.
        let input = serde_json::json!({
            "command": "sleep 10",
            "timeout_secs": 1
        });
        let result = tool.execute(input, &test_context()).await;
        assert!(
            matches!(result, Err(ToolError::Timeout { .. })),
            "long-running command should time out"
        );
    }

    #[tokio::test]
    async fn execute_with_working_directory() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let dir_path = dir.path().to_string_lossy().to_string();

        let tool = ShellTool::default();
        let input = serde_json::json!({
            "command": "pwd",
            "working_directory": dir_path,
        });
        let result = tool.execute(input, &test_context()).await;
        assert!(result.is_ok());

        let r = result.unwrap_or_else(|e| panic!("{e}"));
        let stdout = r.output["stdout"].as_str().unwrap_or("").trim();
        // On macOS, /tmp can be /private/tmp. Canonicalize both for comparison.
        let expected = std::fs::canonicalize(dir.path())
            .unwrap_or_else(|e| panic!("canonicalize expected: {e}"))
            .to_string_lossy()
            .to_string();
        let actual = std::fs::canonicalize(stdout)
            .unwrap_or_else(|e| panic!("canonicalize actual '{stdout}': {e}"))
            .to_string_lossy()
            .to_string();
        assert_eq!(actual, expected);
    }

    #[test]
    fn custom_default_timeout() {
        let tool = ShellTool::new(Duration::from_secs(60));
        assert_eq!(tool.default_timeout, Duration::from_secs(60));
    }
}
