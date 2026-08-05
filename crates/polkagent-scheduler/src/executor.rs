//! Task executor trait for dispatching scheduled task actions.
//!
//! Implementations of [`TaskExecutor`] define how scheduled tasks are actually
//! run. The scheduler runner calls `execute` for each due task, passing the
//! full [`ScheduledTask`] so the executor can inspect the action, agent ID,
//! and any other context it needs.

use async_trait::async_trait;

use crate::error::SchedulerError;
use crate::task::{ScheduledTask, TaskResult};

// ---------------------------------------------------------------------------
// TaskExecutor
// ---------------------------------------------------------------------------

/// Trait for executing scheduled tasks.
///
/// Implementations are responsible for dispatching the task's [`TaskAction`]
/// (invoking an agent run, calling a tool, hitting a webhook, etc.) and
/// returning a [`TaskResult`] describing the outcome.
///
/// [`TaskAction`]: crate::task::TaskAction
#[async_trait]
pub trait TaskExecutor: Send + Sync {
    /// Execute the given scheduled task and return its result.
    ///
    /// # Errors
    ///
    /// Returns [`SchedulerError::ExecutionFailed`] if the task could not be
    /// executed for any reason (network failure, invalid input, handler
    /// panic, etc.).
    async fn execute(&self, task: &ScheduledTask) -> Result<TaskResult, SchedulerError>;
}

// ---------------------------------------------------------------------------
// NoOpExecutor (for testing)
// ---------------------------------------------------------------------------

/// A no-op executor that immediately returns success.
///
/// Useful for testing the scheduler runner without requiring real side effects.
#[derive(Debug, Default, Clone)]
pub struct NoOpExecutor;

#[async_trait]
impl TaskExecutor for NoOpExecutor {
    async fn execute(&self, _task: &ScheduledTask) -> Result<TaskResult, SchedulerError> {
        Ok(TaskResult {
            success: true,
            message: Some("no-op".into()),
            completed_at: chrono::Utc::now(),
            duration_ms: 0,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Test fixtures use explicit panic boundaries to identify broken invariants.
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "unit-test scheduler assertions intentionally panic with focused diagnostics"
)]
mod tests {
    use super::*;
    use crate::schedule::Schedule;
    use crate::task::{ScheduledTask, TaskAction};
    use chrono::{TimeZone, Utc};
    use polkagent_core::AgentId;

    fn make_task() -> ScheduledTask {
        ScheduledTask::new(
            "test-task",
            Schedule::Once {
                at: Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
            },
            AgentId::new(),
            TaskAction::Custom {
                handler: "test".into(),
                payload: serde_json::Value::Null,
            },
        )
    }

    #[tokio::test]
    async fn noop_executor_returns_success() {
        let executor = NoOpExecutor;
        let task = make_task();
        let result = executor.execute(&task).await.expect("should succeed");
        assert!(result.success);
        assert_eq!(result.message.as_deref(), Some("no-op"));
    }
}
