//! Scheduler runner that polls for due tasks and dispatches them.
//!
//! [`SchedulerRunner`] is the main scheduling loop. It periodically checks the
//! [`TaskStore`] for tasks whose `next_run_at` is in the past, marks them as
//! running, dispatches them to a [`TaskExecutor`], and records the result.

use std::sync::Arc;

use chrono::Utc;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use crate::error::SchedulerError;
use crate::executor::TaskExecutor;
use crate::store::TaskStore;

// ---------------------------------------------------------------------------
// RunnerState
// ---------------------------------------------------------------------------

/// The state of the scheduler runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerState {
    /// The runner has been created but not started.
    Idle,
    /// The runner is actively polling for and executing due tasks.
    Running,
    /// The runner has been requested to stop.
    Stopping,
    /// The runner has stopped.
    Stopped,
}

impl std::fmt::Display for RunnerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "idle"),
            Self::Running => write!(f, "running"),
            Self::Stopping => write!(f, "stopping"),
            Self::Stopped => write!(f, "stopped"),
        }
    }
}

// ---------------------------------------------------------------------------
// SchedulerRunner
// ---------------------------------------------------------------------------

/// The main scheduler loop that polls for due tasks and executes them.
///
/// # Usage
///
/// ```rust,no_run
/// # use polkagent_scheduler::runner::SchedulerRunner;
/// # use polkagent_scheduler::memory_store::InMemoryTaskStore;
/// # use polkagent_scheduler::executor::NoOpExecutor;
/// # use std::sync::Arc;
/// # use std::time::Duration;
/// let store = Arc::new(InMemoryTaskStore::new());
/// let executor = Arc::new(NoOpExecutor);
/// let runner = SchedulerRunner::new(store, executor, Duration::from_secs(10));
/// ```
pub struct SchedulerRunner<S, E>
where
    S: TaskStore,
    E: TaskExecutor,
{
    store: Arc<S>,
    executor: Arc<E>,
    poll_interval: std::time::Duration,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
}

impl<S, E> SchedulerRunner<S, E>
where
    S: TaskStore + 'static,
    E: TaskExecutor + 'static,
{
    /// Create a new scheduler runner.
    ///
    /// - `store` — the task persistence backend.
    /// - `executor` — the task executor for dispatching actions.
    /// - `poll_interval` — how often to check for due tasks.
    pub fn new(store: Arc<S>, executor: Arc<E>, poll_interval: std::time::Duration) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            store,
            executor,
            poll_interval,
            shutdown_tx,
            shutdown_rx,
        }
    }

    /// Request the runner to stop at the end of its current poll cycle.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Run a single poll cycle: find due tasks and execute them.
    ///
    /// Returns the number of tasks that were executed.
    pub async fn poll_once(&self) -> Result<usize, SchedulerError> {
        let now = Utc::now();
        let due_tasks = self.store.due_tasks(now).await?;

        if due_tasks.is_empty() {
            debug!("no due tasks at {now}");
            return Ok(0);
        }

        info!(count = due_tasks.len(), "found due tasks");
        let mut executed = 0;

        for task in &due_tasks {
            let task_id = task.id;
            let task_name = &task.name;

            debug!(task_id = %task_id, task_name, "executing task");

            // Mark as running.
            if let Err(e) = self.store.mark_running(task_id).await {
                warn!(task_id = %task_id, error = %e, "failed to mark task as running");
                continue;
            }

            // Execute.
            match self.executor.execute(task).await {
                Ok(result) => {
                    info!(
                        task_id = %task_id,
                        task_name,
                        success = result.success,
                        duration_ms = result.duration_ms,
                        "task completed"
                    );
                    if let Err(e) = self.store.mark_completed(task_id, result).await {
                        error!(task_id = %task_id, error = %e, "failed to mark task as completed");
                    }
                }
                Err(e) => {
                    warn!(task_id = %task_id, error = %e, "task execution failed");
                    if let Err(store_err) = self.store.mark_failed(task_id, e.to_string()).await {
                        error!(
                            task_id = %task_id,
                            error = %store_err,
                            "failed to mark task as failed"
                        );
                    }
                }
            }

            executed += 1;
        }

        Ok(executed)
    }

    /// Run the scheduler loop until shutdown is requested.
    ///
    /// This method blocks the current task. Call [`shutdown()`](Self::shutdown)
    /// from another task to stop the loop.
    pub async fn run(&mut self) -> Result<(), SchedulerError> {
        info!(
            poll_interval_ms = self.poll_interval.as_millis(),
            "scheduler runner started"
        );

        loop {
            // Check for shutdown.
            if *self.shutdown_rx.borrow() {
                info!("scheduler runner shutting down");
                break;
            }

            if let Err(e) = self.poll_once().await {
                error!(error = %e, "poll cycle failed");
            }

            // Sleep until the next poll or shutdown.
            tokio::select! {
                () = tokio::time::sleep(self.poll_interval) => {}
                _ = self.shutdown_rx.changed() => {
                    info!("scheduler runner received shutdown signal");
                    break;
                }
            }
        }

        info!("scheduler runner stopped");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::NoOpExecutor;
    use crate::memory_store::InMemoryTaskStore;
    use crate::schedule::Schedule;
    use crate::task::{ScheduledTask, TaskAction, TaskStatus};
    use chrono::TimeZone;
    use polkagent_core::AgentId;

    fn make_due_task(name: &str) -> ScheduledTask {
        let past = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let mut task = ScheduledTask::new(
            name,
            Schedule::Once { at: past },
            AgentId::new(),
            TaskAction::Custom {
                handler: "test".into(),
                payload: serde_json::Value::Null,
            },
        );
        task.next_run_at = Some(past);
        task
    }

    fn make_future_task(name: &str) -> ScheduledTask {
        let future = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
        ScheduledTask::new(
            name,
            Schedule::Once { at: future },
            AgentId::new(),
            TaskAction::Custom {
                handler: "test".into(),
                payload: serde_json::Value::Null,
            },
        )
    }

    #[tokio::test]
    async fn poll_once_no_tasks() {
        let store = Arc::new(InMemoryTaskStore::new());
        let executor = Arc::new(NoOpExecutor);
        let runner = SchedulerRunner::new(store, executor, std::time::Duration::from_secs(1));

        let count = runner.poll_once().await.expect("poll");
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn poll_once_executes_due_tasks() {
        let store = Arc::new(InMemoryTaskStore::new());
        let executor = Arc::new(NoOpExecutor);

        store
            .create_task(make_due_task("due"))
            .await
            .expect("create");
        store
            .create_task(make_future_task("not-due"))
            .await
            .expect("create");

        let runner =
            SchedulerRunner::new(store.clone(), executor, std::time::Duration::from_secs(1));

        let count = runner.poll_once().await.expect("poll");
        assert_eq!(count, 1);

        // The due task should now be completed (one-shot).
        let tasks = store.list_tasks().await.expect("list");
        let due_task = tasks.iter().find(|t| t.name == "due").expect("find");
        assert_eq!(due_task.status, TaskStatus::Completed);
    }

    #[tokio::test]
    async fn poll_once_multiple_due_tasks() {
        let store = Arc::new(InMemoryTaskStore::new());
        let executor = Arc::new(NoOpExecutor);

        store.create_task(make_due_task("a")).await.expect("create");
        store.create_task(make_due_task("b")).await.expect("create");
        store.create_task(make_due_task("c")).await.expect("create");

        let runner = SchedulerRunner::new(store, executor, std::time::Duration::from_secs(1));

        let count = runner.poll_once().await.expect("poll");
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn shutdown_stops_runner() {
        let store = Arc::new(InMemoryTaskStore::new());
        let executor = Arc::new(NoOpExecutor);
        let mut runner =
            SchedulerRunner::new(store, executor, std::time::Duration::from_millis(50));

        runner.shutdown();
        // The run loop should exit promptly.
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), runner.run()).await;
        assert!(result.is_ok(), "runner should stop within timeout");
    }

    #[tokio::test]
    async fn runner_state_display() {
        assert_eq!(RunnerState::Idle.to_string(), "idle");
        assert_eq!(RunnerState::Running.to_string(), "running");
        assert_eq!(RunnerState::Stopping.to_string(), "stopping");
        assert_eq!(RunnerState::Stopped.to_string(), "stopped");
    }
}
