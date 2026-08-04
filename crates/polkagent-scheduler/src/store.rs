//! Trait definition for scheduled task persistence.
//!
//! [`TaskStore`] defines the operations required to durably store, query, and
//! update scheduled tasks. Implementations may be backed by an in-memory map
//! (for testing), `SQLite`, or any other persistence layer.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::SchedulerError;
use crate::task::{ScheduledTask, TaskId, TaskResult};

// ---------------------------------------------------------------------------
// TaskStore
// ---------------------------------------------------------------------------

/// Persistence interface for scheduled tasks.
///
/// All methods are async to support both in-memory and I/O-bound backends.
#[async_trait]
pub trait TaskStore: Send + Sync {
    /// Persist a new scheduled task.
    ///
    /// # Errors
    ///
    /// Returns [`SchedulerError::TaskAlreadyExists`] if a task with the same
    /// ID already exists.
    async fn create_task(&self, task: ScheduledTask) -> Result<ScheduledTask, SchedulerError>;

    /// Retrieve a task by its ID.
    ///
    /// # Errors
    ///
    /// Returns [`SchedulerError::TaskNotFound`] if no task with the given ID
    /// exists.
    async fn get_task(&self, task_id: TaskId) -> Result<ScheduledTask, SchedulerError>;

    /// List all tasks, optionally filtered by status or other criteria.
    ///
    /// Returns an empty vector if no tasks exist.
    async fn list_tasks(&self) -> Result<Vec<ScheduledTask>, SchedulerError>;

    /// Update a task in the store.
    ///
    /// The task's `id` field is used to locate the existing record. All
    /// mutable fields are replaced with the values from `task`.
    ///
    /// # Errors
    ///
    /// Returns [`SchedulerError::TaskNotFound`] if no task with the given ID
    /// exists.
    async fn update_task(&self, task: ScheduledTask) -> Result<ScheduledTask, SchedulerError>;

    /// Delete a task by its ID.
    ///
    /// # Errors
    ///
    /// Returns [`SchedulerError::TaskNotFound`] if no task with the given ID
    /// exists.
    async fn delete_task(&self, task_id: TaskId) -> Result<(), SchedulerError>;

    /// Return all tasks that are due at the given time.
    ///
    /// A task is due when `next_run_at <= now` and `status == Active`.
    async fn due_tasks(&self, now: DateTime<Utc>) -> Result<Vec<ScheduledTask>, SchedulerError>;

    /// Mark a task as currently running.
    ///
    /// # Errors
    ///
    /// Returns [`SchedulerError::TaskNotFound`] if no task with the given ID
    /// exists.
    async fn mark_running(&self, task_id: TaskId) -> Result<(), SchedulerError>;

    /// Mark a task as completed after a successful execution, recording the
    /// result and advancing its schedule.
    ///
    /// # Errors
    ///
    /// Returns [`SchedulerError::TaskNotFound`] if no task with the given ID
    /// exists.
    async fn mark_completed(
        &self,
        task_id: TaskId,
        result: TaskResult,
    ) -> Result<(), SchedulerError>;

    /// Mark a task as failed after an unsuccessful execution, recording the
    /// error message and reverting to active status for retry.
    ///
    /// # Errors
    ///
    /// Returns [`SchedulerError::TaskNotFound`] if no task with the given ID
    /// exists.
    async fn mark_failed(&self, task_id: TaskId, error: String) -> Result<(), SchedulerError>;
}
