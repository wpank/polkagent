//! `polkagent-scheduler` — Task scheduling for the Polkagent platform.
//!
//! This crate provides scheduled and recurring agent task execution with
//! cron-like expressions, interval-based scheduling, one-shot deferred tasks,
//! and durable task persistence.
//!
//! # Module overview
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`cron`] | Cron expression parser and next-occurrence calculator |
//! | [`schedule`] | Schedule types: `Once`, `Interval`, `Cron` |
//! | [`task`] | `ScheduledTask`, `TaskAction`, `TaskStatus`, `TaskResult` |
//! | [`executor`] | `TaskExecutor` trait for dispatching task actions |
//! | [`store`] | `TaskStore` trait for durable task persistence |
//! | [`memory_store`] | In-memory `TaskStore` implementation |
//! | [`runner`] | `SchedulerRunner` — polls for due tasks and dispatches them |
//! | [`error`] | `SchedulerError` error types |
//!
//! # Quick start
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use std::time::Duration;
//!
//! use polkagent_scheduler::memory_store::InMemoryTaskStore;
//! use polkagent_scheduler::executor::NoOpExecutor;
//! use polkagent_scheduler::Scheduler;
//!
//! let store = Arc::new(InMemoryTaskStore::new());
//! let executor = Arc::new(NoOpExecutor);
//! let scheduler = Scheduler::new(store, executor, Duration::from_secs(10));
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

pub mod cron;
pub mod error;
pub mod executor;
pub mod memory_store;
pub mod runner;
pub mod schedule;
pub mod store;
pub mod task;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use cron::CronExpr;
pub use error::SchedulerError;
pub use executor::TaskExecutor;
pub use memory_store::InMemoryTaskStore;
pub use runner::SchedulerRunner;
pub use schedule::Schedule;
pub use store::TaskStore;
pub use task::{ScheduledTask, TaskAction, TaskId, TaskResult, TaskStatus};

// ---------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------

use std::sync::Arc;

/// Top-level scheduler that wires together a [`TaskStore`], a
/// [`TaskExecutor`], and a [`SchedulerRunner`].
///
/// This is the primary entry point for embedding the scheduler in an
/// application.
pub struct Scheduler<S, E>
where
    S: TaskStore + 'static,
    E: TaskExecutor + 'static,
{
    store: Arc<S>,
    executor: Arc<E>,
    poll_interval: std::time::Duration,
}

impl<S, E> Scheduler<S, E>
where
    S: TaskStore + 'static,
    E: TaskExecutor + 'static,
{
    /// Create a new scheduler.
    ///
    /// - `store` — the task persistence backend.
    /// - `executor` — the task executor for dispatching actions.
    /// - `poll_interval` — how often the runner checks for due tasks.
    pub fn new(
        store: Arc<S>,
        executor: Arc<E>,
        poll_interval: std::time::Duration,
    ) -> Self {
        Self {
            store,
            executor,
            poll_interval,
        }
    }

    /// Return a reference to the task store.
    pub fn store(&self) -> &Arc<S> {
        &self.store
    }

    /// Return a reference to the task executor.
    pub fn executor(&self) -> &Arc<E> {
        &self.executor
    }

    /// Create a [`SchedulerRunner`] ready to be started.
    pub fn runner(&self) -> SchedulerRunner<S, E> {
        SchedulerRunner::new(
            Arc::clone(&self.store),
            Arc::clone(&self.executor),
            self.poll_interval,
        )
    }

    /// Register a new task with the scheduler's store.
    pub async fn add_task(
        &self,
        task: ScheduledTask,
    ) -> Result<ScheduledTask, SchedulerError> {
        self.store.create_task(task).await
    }

    /// Retrieve a task by ID.
    pub async fn get_task(&self, id: TaskId) -> Result<ScheduledTask, SchedulerError> {
        self.store.get_task(id).await
    }

    /// List all tasks.
    pub async fn list_tasks(&self) -> Result<Vec<ScheduledTask>, SchedulerError> {
        self.store.list_tasks().await
    }

    /// Remove a task by ID.
    pub async fn remove_task(&self, id: TaskId) -> Result<(), SchedulerError> {
        self.store.delete_task(id).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use executor::NoOpExecutor;
    use polkagent_core::AgentId;

    fn make_task() -> ScheduledTask {
        ScheduledTask::new(
            "lib-test",
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
    async fn scheduler_add_and_get_task() {
        let store = Arc::new(InMemoryTaskStore::new());
        let executor = Arc::new(NoOpExecutor);
        let scheduler = Scheduler::new(store, executor, std::time::Duration::from_secs(1));

        let task = make_task();
        let id = task.id;
        scheduler.add_task(task).await.expect("add");
        let fetched = scheduler.get_task(id).await.expect("get");
        assert_eq!(fetched.name, "lib-test");
    }

    #[tokio::test]
    async fn scheduler_list_tasks() {
        let store = Arc::new(InMemoryTaskStore::new());
        let executor = Arc::new(NoOpExecutor);
        let scheduler = Scheduler::new(store, executor, std::time::Duration::from_secs(1));

        scheduler.add_task(make_task()).await.expect("add");
        let tasks = scheduler.list_tasks().await.expect("list");
        assert_eq!(tasks.len(), 1);
    }

    #[tokio::test]
    async fn scheduler_remove_task() {
        let store = Arc::new(InMemoryTaskStore::new());
        let executor = Arc::new(NoOpExecutor);
        let scheduler = Scheduler::new(store, executor, std::time::Duration::from_secs(1));

        let task = make_task();
        let id = task.id;
        scheduler.add_task(task).await.expect("add");
        scheduler.remove_task(id).await.expect("remove");

        let err = scheduler.get_task(id).await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn scheduler_creates_runner() {
        let store = Arc::new(InMemoryTaskStore::new());
        let executor = Arc::new(NoOpExecutor);
        let scheduler = Scheduler::new(store, executor, std::time::Duration::from_secs(1));

        // Just verify runner can be constructed and poll_once works.
        let runner = scheduler.runner();
        let count = runner.poll_once().await.expect("poll");
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn scheduler_store_accessor() {
        let store = Arc::new(InMemoryTaskStore::new());
        let executor = Arc::new(NoOpExecutor);
        let scheduler = Scheduler::new(store, executor, std::time::Duration::from_secs(1));
        assert!(scheduler.store().is_empty());
    }
}
