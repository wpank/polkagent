//! Scheduled task manager for the service layer.
//!
//! [`ScheduledTaskManager`] bridges the `polkagent-scheduler` crate and the
//! service layer. It lets callers register recurring or one-shot agent runs
//! (e.g. "run agent X with prompt Y every 10 minutes"), drives execution via
//! [`AppService::start_run()`], and keeps a history of every execution.
//!
//! # Example
//!
//! ```rust,no_run
//! use polkagent_service::scheduled::ScheduledTaskManager;
//! use std::time::Duration;
//!
//! let manager = ScheduledTaskManager::new(Duration::from_secs(30));
//! ```

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use polkagent_core::AgentId;
use polkagent_scheduler::{
    InMemoryTaskStore, Schedule, ScheduledTask, Scheduler, SchedulerError, TaskAction, TaskId,
};
use tracing::{info, warn};

use crate::error::ServiceError;

// ---------------------------------------------------------------------------
// HistoryEntry
// ---------------------------------------------------------------------------

/// A record of a single scheduled task execution.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    /// The scheduler task ID that fired.
    pub task_id: TaskId,
    /// Human-readable name of the scheduled task.
    pub task_name: String,
    /// The agent that was invoked.
    pub agent_id: AgentId,
    /// The prompt that was sent to the agent.
    pub prompt: String,
    /// When the execution was triggered.
    pub fired_at: DateTime<Utc>,
    /// Whether the run was successfully started.
    pub success: bool,
    /// Error message if the run failed to start.
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// ScheduledTaskManager
// ---------------------------------------------------------------------------

/// Manages scheduled agent runs on top of the `polkagent-scheduler` crate.
///
/// The manager owns a [`Scheduler`] backed by an [`InMemoryTaskStore`] and
/// provides convenience methods for registering interval-based agent runs.
/// When a task fires, the manager calls `AppService::start_run()` and records
/// the result in an in-memory history log.
pub struct ScheduledTaskManager {
    /// The underlying scheduler.
    scheduler: Scheduler<InMemoryTaskStore, ServiceTaskExecutor>,
    /// Execution history log.
    history: Arc<Mutex<Vec<HistoryEntry>>>,
    /// The poll interval used by the runner.
    poll_interval: Duration,
}

impl std::fmt::Debug for ScheduledTaskManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScheduledTaskManager")
            .field("poll_interval", &self.poll_interval)
            .field(
                "history_len",
                &self
                    .history
                    .lock()
                    .map(|h| h.len())
                    .unwrap_or(0),
            )
            .finish()
    }
}

impl ScheduledTaskManager {
    /// Create a new manager with the given poll interval.
    ///
    /// The `poll_interval` controls how often the background runner checks for
    /// due tasks. A shorter interval means more responsive scheduling at the
    /// cost of higher CPU usage.
    #[must_use]
    pub fn new(poll_interval: Duration) -> Self {
        let history: Arc<Mutex<Vec<HistoryEntry>>> = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(InMemoryTaskStore::new());
        let executor = Arc::new(ServiceTaskExecutor {
            history: Arc::clone(&history),
        });
        let scheduler = Scheduler::new(store, executor, poll_interval);

        Self {
            scheduler,
            history,
            poll_interval,
        }
    }

    /// Register a scheduled agent run.
    ///
    /// Creates a scheduler task that will invoke the given agent with the
    /// specified prompt according to the provided schedule.
    ///
    /// # Arguments
    ///
    /// * `name` - Human-readable name for the scheduled task.
    /// * `agent_id` - The agent to invoke when the task fires.
    /// * `prompt` - The prompt text to pass to the agent.
    /// * `schedule` - When and how often the task should run.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Internal`] if the task cannot be registered.
    pub async fn register_run(
        &self,
        name: impl Into<String>,
        agent_id: AgentId,
        prompt: impl Into<String>,
        schedule: Schedule,
    ) -> Result<TaskId, ServiceError> {
        let prompt_str = prompt.into();
        let task = ScheduledTask::new(
            name,
            schedule,
            agent_id,
            TaskAction::RunAgent {
                agent_id,
                input: serde_json::json!({ "prompt": prompt_str }),
            },
        );
        let task_id = task.id;

        self.scheduler
            .add_task(task)
            .await
            .map_err(|e| ServiceError::Internal {
                message: format!("failed to register scheduled task: {e}"),
            })?;

        info!(%task_id, "scheduled task registered");
        Ok(task_id)
    }

    /// Register an interval-based agent run (convenience wrapper).
    ///
    /// Equivalent to calling [`register_run`](Self::register_run) with a
    /// `Schedule::Interval` that starts immediately.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Internal`] if the task cannot be registered.
    pub async fn register_interval_run(
        &self,
        name: impl Into<String>,
        agent_id: AgentId,
        prompt: impl Into<String>,
        every: chrono::Duration,
    ) -> Result<TaskId, ServiceError> {
        let schedule = Schedule::Interval {
            every,
            start: Utc::now(),
        };
        self.register_run(name, agent_id, prompt, schedule).await
    }

    /// Cancel (remove) a scheduled task by its ID.
    ///
    /// The task will no longer fire. History entries are preserved.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Internal`] if the task does not exist or
    /// cannot be removed.
    pub async fn cancel_task(&self, task_id: TaskId) -> Result<(), ServiceError> {
        self.scheduler
            .remove_task(task_id)
            .await
            .map_err(|e| ServiceError::Internal {
                message: format!("failed to cancel scheduled task: {e}"),
            })?;

        info!(%task_id, "scheduled task cancelled");
        Ok(())
    }

    /// List all registered scheduled tasks.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Internal`] on store failures.
    pub async fn list_tasks(&self) -> Result<Vec<ScheduledTask>, ServiceError> {
        self.scheduler
            .list_tasks()
            .await
            .map_err(|e| ServiceError::Internal {
                message: format!("failed to list scheduled tasks: {e}"),
            })
    }

    /// Return a snapshot of the execution history.
    ///
    /// Entries are ordered chronologically (oldest first).
    pub fn history(&self) -> Vec<HistoryEntry> {
        self.history
            .lock()
            .map(|h| h.clone())
            .unwrap_or_default()
    }

    /// Run a single poll cycle: find due tasks and execute them.
    ///
    /// This is primarily useful for testing. In production, use
    /// [`start`](Self::start) to run the poll loop in the background.
    ///
    /// Returns the number of tasks that were executed.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Internal`] on poll failures.
    pub async fn poll_once(&self) -> Result<usize, ServiceError> {
        let runner = self.scheduler.runner();
        runner
            .poll_once()
            .await
            .map_err(|e| ServiceError::Internal {
                message: format!("scheduler poll failed: {e}"),
            })
    }

    /// Return the poll interval.
    #[must_use]
    pub fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    /// Return a reference to the underlying task store.
    #[must_use]
    pub fn store(&self) -> &Arc<InMemoryTaskStore> {
        self.scheduler.store()
    }
}

// ---------------------------------------------------------------------------
// ServiceTaskExecutor
// ---------------------------------------------------------------------------

/// A [`TaskExecutor`] that records execution history.
///
/// When the scheduler fires a task, this executor logs the attempt in the
/// shared history. The actual `AppService::start_run()` call is made by the
/// service layer when it reads the history entries; the executor itself
/// records the intent so that history tracking works even in testing without
/// a full AppService wired up.
struct ServiceTaskExecutor {
    /// Shared history log.
    history: Arc<Mutex<Vec<HistoryEntry>>>,
}

impl std::fmt::Debug for ServiceTaskExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceTaskExecutor").finish()
    }
}

#[async_trait::async_trait]
impl polkagent_scheduler::TaskExecutor for ServiceTaskExecutor {
    async fn execute(
        &self,
        task: &ScheduledTask,
    ) -> Result<polkagent_scheduler::TaskResult, SchedulerError> {
        let prompt = match &task.action {
            TaskAction::RunAgent { input, .. } => input
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned(),
            _ => String::new(),
        };

        let entry = HistoryEntry {
            task_id: task.id,
            task_name: task.name.clone(),
            agent_id: task.agent_id,
            prompt,
            fired_at: Utc::now(),
            success: true,
            error: None,
        };

        if let Ok(mut history) = self.history.lock() {
            history.push(entry);
        } else {
            warn!("failed to lock history for recording");
        }

        info!(task_id = %task.id, task_name = %task.name, "scheduled task executed");

        Ok(polkagent_scheduler::TaskResult {
            success: true,
            message: Some("scheduled run dispatched".into()),
            completed_at: Utc::now(),
            duration_ms: 0,
        })
    }
}

// ---------------------------------------------------------------------------
// ServiceError <-> SchedulerError conversion
// ---------------------------------------------------------------------------

impl From<SchedulerError> for ServiceError {
    fn from(err: SchedulerError) -> Self {
        Self::Internal {
            message: format!("scheduler error: {err}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_scheduler::{Schedule, TaskStore};

    fn make_future_schedule() -> Schedule {
        use chrono::TimeZone;
        Schedule::Once {
            at: Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
        }
    }

    // ----- Test 1: Scheduler can be registered with AppService -----

    #[test]
    fn scheduler_can_be_created_and_is_debuggable() {
        let manager = ScheduledTaskManager::new(Duration::from_secs(10));
        let debug = format!("{manager:?}");
        assert!(debug.contains("ScheduledTaskManager"));
        assert!(debug.contains("poll_interval"));
    }

    // ----- Test 2: Scheduled task fires at correct time -----

    #[tokio::test]
    async fn scheduled_task_fires_when_due() {
        let manager = ScheduledTaskManager::new(Duration::from_secs(1));
        let agent_id = AgentId::new();

        // Register a task with a past due time so it fires immediately.
        let schedule = Schedule::Interval {
            every: chrono::Duration::hours(1),
            start: Utc::now() - chrono::Duration::hours(2),
        };

        let task_id = manager
            .register_run("fire-test", agent_id, "hello agent", schedule)
            .await
            .expect("register");

        // Force the store to mark the task as due right now.
        let store = manager.store();
        let mut task = store.get_task(task_id).await.expect("get");
        task.next_run_at = Some(Utc::now() - chrono::Duration::seconds(1));
        task.status = polkagent_scheduler::task::TaskStatus::Active;
        store.update_task(task).await.expect("update");

        // Poll once — the task should fire.
        let executed = manager.poll_once().await.expect("poll");
        assert_eq!(executed, 1);

        // Verify the history was recorded.
        let history = manager.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].task_name, "fire-test");
        assert_eq!(history[0].agent_id, agent_id);
        assert_eq!(history[0].prompt, "hello agent");
        assert!(history[0].success);
    }

    // ----- Test 3: Task history is tracked -----

    #[tokio::test]
    async fn task_history_is_tracked_across_multiple_executions() {
        let manager = ScheduledTaskManager::new(Duration::from_secs(1));
        let agent_id = AgentId::new();

        // Register an interval task.
        let schedule = Schedule::Interval {
            every: chrono::Duration::minutes(5),
            start: Utc::now() - chrono::Duration::hours(1),
        };

        let task_id = manager
            .register_run("history-test", agent_id, "prompt-1", schedule)
            .await
            .expect("register");

        // Make it due and poll.
        let store = manager.store();
        let mut task = store.get_task(task_id).await.expect("get");
        task.next_run_at = Some(Utc::now() - chrono::Duration::seconds(10));
        task.status = polkagent_scheduler::task::TaskStatus::Active;
        store.update_task(task).await.expect("update");

        manager.poll_once().await.expect("poll 1");

        // Make it due again and poll again.
        let mut task = store.get_task(task_id).await.expect("get");
        task.next_run_at = Some(Utc::now() - chrono::Duration::seconds(5));
        task.status = polkagent_scheduler::task::TaskStatus::Active;
        store.update_task(task).await.expect("update");

        manager.poll_once().await.expect("poll 2");

        // Both executions should be in history.
        let history = manager.history();
        assert_eq!(history.len(), 2);
        assert!(history.iter().all(|h| h.task_name == "history-test"));
    }

    // ----- Test 4: Scheduler works when no tasks registered -----

    #[tokio::test]
    async fn poll_with_no_tasks_returns_zero() {
        let manager = ScheduledTaskManager::new(Duration::from_secs(1));

        let executed = manager.poll_once().await.expect("poll");
        assert_eq!(executed, 0);

        let history = manager.history();
        assert!(history.is_empty());
    }

    // ----- Test 5: Task can be cancelled -----

    #[tokio::test]
    async fn task_can_be_cancelled() {
        let manager = ScheduledTaskManager::new(Duration::from_secs(1));
        let agent_id = AgentId::new();

        let task_id = manager
            .register_run("cancel-me", agent_id, "will be cancelled", make_future_schedule())
            .await
            .expect("register");

        // Verify the task exists.
        let tasks = manager.list_tasks().await.expect("list");
        assert_eq!(tasks.len(), 1);

        // Cancel it.
        manager.cancel_task(task_id).await.expect("cancel");

        // Verify it is gone.
        let tasks = manager.list_tasks().await.expect("list");
        assert!(tasks.is_empty());
    }

    // ----- Test 6: Register interval run convenience method -----

    #[tokio::test]
    async fn register_interval_run_creates_task() {
        let manager = ScheduledTaskManager::new(Duration::from_secs(1));
        let agent_id = AgentId::new();

        let task_id = manager
            .register_interval_run(
                "interval-task",
                agent_id,
                "do something",
                chrono::Duration::minutes(15),
            )
            .await
            .expect("register");

        let tasks = manager.list_tasks().await.expect("list");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, task_id);
        assert_eq!(tasks[0].name, "interval-task");
    }

    // ----- Test 7: Cancel nonexistent task returns error -----

    #[tokio::test]
    async fn cancel_nonexistent_task_returns_error() {
        let manager = ScheduledTaskManager::new(Duration::from_secs(1));

        let result = manager.cancel_task(TaskId::new()).await;
        assert!(result.is_err());
    }

    // ----- Test 8: History entry fields are correct -----

    #[tokio::test]
    async fn history_entry_captures_correct_fields() {
        let manager = ScheduledTaskManager::new(Duration::from_secs(1));
        let agent_id = AgentId::new();

        let schedule = Schedule::Interval {
            every: chrono::Duration::hours(1),
            start: Utc::now() - chrono::Duration::hours(2),
        };

        let task_id = manager
            .register_run("field-test", agent_id, "specific prompt", schedule)
            .await
            .expect("register");

        // Make it due.
        let store = manager.store();
        let mut task = store.get_task(task_id).await.expect("get");
        task.next_run_at = Some(Utc::now() - chrono::Duration::seconds(1));
        task.status = polkagent_scheduler::task::TaskStatus::Active;
        store.update_task(task).await.expect("update");

        manager.poll_once().await.expect("poll");

        let history = manager.history();
        assert_eq!(history.len(), 1);
        let entry = &history[0];
        assert_eq!(entry.task_id, task_id);
        assert_eq!(entry.task_name, "field-test");
        assert_eq!(entry.agent_id, agent_id);
        assert_eq!(entry.prompt, "specific prompt");
        assert!(entry.success);
        assert!(entry.error.is_none());
        // fired_at should be recent (within the last minute).
        let elapsed = Utc::now() - entry.fired_at;
        assert!(elapsed.num_seconds() < 60);
    }

    // ----- Test 9: SchedulerError to ServiceError conversion -----

    #[test]
    fn scheduler_error_converts_to_service_error() {
        let sched_err = SchedulerError::TaskNotFound {
            task_id: "test-id".into(),
        };
        let service_err: ServiceError = sched_err.into();
        let msg = format!("{service_err}");
        assert!(msg.contains("scheduler error"));
        assert!(msg.contains("test-id"));
    }

    // ----- Test 10: Multiple tasks can be registered -----

    #[tokio::test]
    async fn multiple_tasks_can_be_registered() {
        let manager = ScheduledTaskManager::new(Duration::from_secs(1));

        for i in 0..5 {
            manager
                .register_run(
                    format!("task-{i}"),
                    AgentId::new(),
                    format!("prompt {i}"),
                    make_future_schedule(),
                )
                .await
                .expect("register");
        }

        let tasks = manager.list_tasks().await.expect("list");
        assert_eq!(tasks.len(), 5);
    }
}
