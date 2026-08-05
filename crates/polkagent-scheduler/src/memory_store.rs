//! In-memory [`TaskStore`] implementation.
//!
//! [`InMemoryTaskStore`] keeps all tasks in a `HashMap` guarded by a
//! [`parking_lot::RwLock`]. Suitable for testing and lightweight single-process
//! deployments where persistence across restarts is not required.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use std::collections::HashMap;

use crate::error::SchedulerError;
use crate::store::TaskStore;
use crate::task::{ScheduledTask, TaskId, TaskResult, TaskStatus};

// ---------------------------------------------------------------------------
// InMemoryTaskStore
// ---------------------------------------------------------------------------

/// An in-memory task store backed by a `HashMap`.
#[derive(Debug, Default)]
pub struct InMemoryTaskStore {
    tasks: RwLock<HashMap<TaskId, ScheduledTask>>,
}

impl InMemoryTaskStore {
    /// Create a new empty store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
        }
    }

    /// Return the number of tasks in the store.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tasks.read().len()
    }

    /// Return `true` if the store contains no tasks.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tasks.read().is_empty()
    }
}

#[async_trait]
impl TaskStore for InMemoryTaskStore {
    async fn create_task(&self, task: ScheduledTask) -> Result<ScheduledTask, SchedulerError> {
        let mut tasks = self.tasks.write();
        if tasks.contains_key(&task.id) {
            return Err(SchedulerError::task_already_exists(task.id));
        }
        let id = task.id;
        tasks.insert(id, task);
        Ok(tasks[&id].clone())
    }

    async fn get_task(&self, task_id: TaskId) -> Result<ScheduledTask, SchedulerError> {
        let tasks = self.tasks.read();
        tasks
            .get(&task_id)
            .cloned()
            .ok_or_else(|| SchedulerError::task_not_found(task_id))
    }

    async fn list_tasks(&self) -> Result<Vec<ScheduledTask>, SchedulerError> {
        let tasks = self.tasks.read();
        Ok(tasks.values().cloned().collect())
    }

    async fn update_task(&self, task: ScheduledTask) -> Result<ScheduledTask, SchedulerError> {
        let mut tasks = self.tasks.write();
        if !tasks.contains_key(&task.id) {
            return Err(SchedulerError::task_not_found(task.id));
        }
        tasks.insert(task.id, task.clone());
        Ok(task)
    }

    async fn delete_task(&self, task_id: TaskId) -> Result<(), SchedulerError> {
        let mut tasks = self.tasks.write();
        if tasks.remove(&task_id).is_none() {
            return Err(SchedulerError::task_not_found(task_id));
        }
        Ok(())
    }

    async fn due_tasks(&self, now: DateTime<Utc>) -> Result<Vec<ScheduledTask>, SchedulerError> {
        let tasks = self.tasks.read();
        let due: Vec<ScheduledTask> = tasks.values().filter(|t| t.is_due(now)).cloned().collect();
        Ok(due)
    }

    async fn mark_running(&self, task_id: TaskId) -> Result<(), SchedulerError> {
        let mut tasks = self.tasks.write();
        let task = tasks
            .get_mut(&task_id)
            .ok_or_else(|| SchedulerError::task_not_found(task_id))?;
        task.status = TaskStatus::Running;
        Ok(())
    }

    async fn mark_completed(
        &self,
        task_id: TaskId,
        result: TaskResult,
    ) -> Result<(), SchedulerError> {
        let mut tasks = self.tasks.write();
        let task = tasks
            .get_mut(&task_id)
            .ok_or_else(|| SchedulerError::task_not_found(task_id))?;
        task.advance(result.completed_at);
        Ok(())
    }

    async fn mark_failed(&self, task_id: TaskId, _error: String) -> Result<(), SchedulerError> {
        let mut tasks = self.tasks.write();
        let task = tasks
            .get_mut(&task_id)
            .ok_or_else(|| SchedulerError::task_not_found(task_id))?;
        // Revert to active so the task can be retried on its next due time.
        task.status = TaskStatus::Active;
        Ok(())
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
    use crate::task::{TaskAction, TaskResult};
    use chrono::TimeZone;
    use polkagent_core::AgentId;

    fn make_task(name: &str, at: DateTime<Utc>) -> ScheduledTask {
        let mut task = ScheduledTask::new(
            name,
            Schedule::Once { at },
            AgentId::new(),
            TaskAction::Custom {
                handler: "test".into(),
                payload: serde_json::Value::Null,
            },
        );
        task.next_run_at = Some(at);
        task
    }

    #[tokio::test]
    async fn create_and_get() {
        let store = InMemoryTaskStore::new();
        let at = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
        let task = make_task("test", at);
        let id = task.id;

        store.create_task(task).await.expect("create");
        let fetched = store.get_task(id).await.expect("get");
        assert_eq!(fetched.name, "test");
    }

    #[tokio::test]
    async fn create_duplicate_fails() {
        let store = InMemoryTaskStore::new();
        let at = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
        let task = make_task("test", at);
        let dup = task.clone();

        store.create_task(task).await.expect("create");
        let err = store.create_task(dup).await.unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn get_nonexistent_fails() {
        let store = InMemoryTaskStore::new();
        let err = store.get_task(TaskId::new()).await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn list_tasks_empty() {
        let store = InMemoryTaskStore::new();
        let tasks = store.list_tasks().await.expect("list");
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn list_tasks_returns_all() {
        let store = InMemoryTaskStore::new();
        let at = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
        store.create_task(make_task("a", at)).await.expect("create");
        store.create_task(make_task("b", at)).await.expect("create");

        let tasks = store.list_tasks().await.expect("list");
        assert_eq!(tasks.len(), 2);
    }

    #[tokio::test]
    async fn update_task() {
        let store = InMemoryTaskStore::new();
        let at = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
        let task = make_task("original", at);
        let id = task.id;

        store.create_task(task).await.expect("create");

        let mut updated = store.get_task(id).await.expect("get");
        updated.name = "updated".into();
        store.update_task(updated).await.expect("update");

        let fetched = store.get_task(id).await.expect("get");
        assert_eq!(fetched.name, "updated");
    }

    #[tokio::test]
    async fn update_nonexistent_fails() {
        let store = InMemoryTaskStore::new();
        let at = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
        let task = make_task("ghost", at);
        let err = store.update_task(task).await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn delete_task() {
        let store = InMemoryTaskStore::new();
        let at = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
        let task = make_task("delete-me", at);
        let id = task.id;

        store.create_task(task).await.expect("create");
        store.delete_task(id).await.expect("delete");

        let err = store.get_task(id).await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn delete_nonexistent_fails() {
        let store = InMemoryTaskStore::new();
        let err = store.delete_task(TaskId::new()).await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn due_tasks_filters_correctly() {
        let store = InMemoryTaskStore::new();
        let past = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let future = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap();

        store
            .create_task(make_task("past-due", past))
            .await
            .expect("create");
        store
            .create_task(make_task("not-due", future))
            .await
            .expect("create");

        let due = store.due_tasks(now).await.expect("due_tasks");
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].name, "past-due");
    }

    #[tokio::test]
    async fn due_tasks_excludes_paused() {
        let store = InMemoryTaskStore::new();
        let past = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap();

        let mut task = make_task("paused", past);
        task.status = TaskStatus::Paused;
        store.create_task(task).await.expect("create");

        let due = store.due_tasks(now).await.expect("due_tasks");
        assert!(due.is_empty());
    }

    #[tokio::test]
    async fn mark_running() {
        let store = InMemoryTaskStore::new();
        let at = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
        let task = make_task("run-me", at);
        let id = task.id;

        store.create_task(task).await.expect("create");
        store.mark_running(id).await.expect("mark_running");

        let fetched = store.get_task(id).await.expect("get");
        assert_eq!(fetched.status, TaskStatus::Running);
    }

    #[tokio::test]
    async fn mark_completed_advances_schedule() {
        let store = InMemoryTaskStore::new();
        let start = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let mut task = ScheduledTask::new(
            "interval-task",
            Schedule::Interval {
                every: chrono::Duration::hours(1),
                start,
            },
            AgentId::new(),
            TaskAction::Custom {
                handler: "test".into(),
                payload: serde_json::Value::Null,
            },
        );
        task.next_run_at = Some(start);
        let id = task.id;

        store.create_task(task).await.expect("create");

        let result = TaskResult {
            success: true,
            message: None,
            completed_at: Utc.with_ymd_and_hms(2025, 1, 1, 0, 5, 0).unwrap(),
            duration_ms: 100,
        };
        store
            .mark_completed(id, result)
            .await
            .expect("mark_completed");

        let fetched = store.get_task(id).await.expect("get");
        assert_eq!(fetched.status, TaskStatus::Active);
        assert!(fetched.next_run_at.is_some());
        assert!(fetched.last_run_at.is_some());
    }

    #[tokio::test]
    async fn mark_failed_reverts_to_active() {
        let store = InMemoryTaskStore::new();
        let at = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
        let task = make_task("fail-me", at);
        let id = task.id;

        store.create_task(task).await.expect("create");
        store.mark_running(id).await.expect("mark_running");
        store
            .mark_failed(id, "something broke".into())
            .await
            .expect("mark_failed");

        let fetched = store.get_task(id).await.expect("get");
        assert_eq!(fetched.status, TaskStatus::Active);
    }

    #[tokio::test]
    async fn len_and_is_empty() {
        let store = InMemoryTaskStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);

        let at = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
        store.create_task(make_task("a", at)).await.expect("create");
        assert!(!store.is_empty());
        assert_eq!(store.len(), 1);
    }
}
