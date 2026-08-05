//! Scheduled task definitions.
//!
//! A [`ScheduledTask`] captures everything needed to describe a recurring or
//! one-shot piece of work: its identity, schedule, the action to perform, and
//! its current lifecycle status.

use chrono::{DateTime, Utc};
use polkagent_core::AgentId;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

use crate::schedule::Schedule;

// ---------------------------------------------------------------------------
// TaskId
// ---------------------------------------------------------------------------

/// Unique identifier for a scheduled task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(Uuid);

impl TaskId {
    /// Generate a new time-ordered (v7) identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wrap an existing [`Uuid`] value.
    #[must_use]
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Return the inner [`Uuid`].
    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for TaskId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

impl From<Uuid> for TaskId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<TaskId> for Uuid {
    fn from(id: TaskId) -> Self {
        id.0
    }
}

// ---------------------------------------------------------------------------
// TaskAction
// ---------------------------------------------------------------------------

/// The action a scheduled task should perform when it fires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskAction {
    /// Trigger an agent run with the given input.
    RunAgent {
        /// The agent to invoke.
        agent_id: AgentId,
        /// Input payload passed to the agent.
        input: serde_json::Value,
    },

    /// Execute a named tool directly.
    ExecuteTool {
        /// Tool registry name.
        tool_name: String,
        /// Input payload for the tool.
        input: serde_json::Value,
    },

    /// Call an external webhook.
    WebhookCall {
        /// The URL to call.
        url: String,
        /// HTTP method (GET, POST, etc.).
        method: String,
        /// Request body.
        body: serde_json::Value,
    },

    /// Invoke a custom handler by name.
    Custom {
        /// Handler identifier.
        handler: String,
        /// Arbitrary payload.
        payload: serde_json::Value,
    },
}

impl fmt::Display for TaskAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RunAgent { agent_id, .. } => write!(f, "run_agent({agent_id})"),
            Self::ExecuteTool { tool_name, .. } => write!(f, "execute_tool({tool_name})"),
            Self::WebhookCall { url, method, .. } => write!(f, "webhook({method} {url})"),
            Self::Custom { handler, .. } => write!(f, "custom({handler})"),
        }
    }
}

// ---------------------------------------------------------------------------
// TaskStatus
// ---------------------------------------------------------------------------

/// The lifecycle status of a scheduled task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Task is active and will be picked up when due.
    Active,
    /// Task is currently being executed.
    Running,
    /// Task has been paused and will not be picked up until resumed.
    Paused,
    /// Task has completed (one-shot schedules only).
    Completed,
    /// Task has been permanently disabled.
    Disabled,
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Running => write!(f, "running"),
            Self::Paused => write!(f, "paused"),
            Self::Completed => write!(f, "completed"),
            Self::Disabled => write!(f, "disabled"),
        }
    }
}

// ---------------------------------------------------------------------------
// TaskResult
// ---------------------------------------------------------------------------

/// The result of a single task execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskResult {
    /// Whether the execution succeeded.
    pub success: bool,
    /// Optional output or error message.
    pub message: Option<String>,
    /// When the execution completed.
    pub completed_at: DateTime<Utc>,
    /// How long the execution took, in milliseconds.
    pub duration_ms: u64,
}

// ---------------------------------------------------------------------------
// ScheduledTask
// ---------------------------------------------------------------------------

/// A task registered with the scheduler.
///
/// Captures the task's identity, its schedule, which agent or action to
/// invoke, and its current lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledTask {
    /// Unique identifier for this task.
    pub id: TaskId,
    /// Human-readable name.
    pub name: String,
    /// When and how often the task should run.
    pub schedule: Schedule,
    /// The owning agent (for access control and auditing).
    pub agent_id: AgentId,
    /// What the task does when it fires.
    pub action: TaskAction,
    /// Current lifecycle status.
    pub status: TaskStatus,
    /// When the task was created.
    pub created_at: DateTime<Utc>,
    /// When the task is next due to run (`None` if the schedule is exhausted).
    pub next_run_at: Option<DateTime<Utc>>,
    /// When the task last ran (`None` if it has never run).
    pub last_run_at: Option<DateTime<Utc>>,
}

impl ScheduledTask {
    /// Create a new scheduled task with the given parameters.
    ///
    /// The `next_run_at` field is computed from the schedule relative to `now`.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        schedule: Schedule,
        agent_id: AgentId,
        action: TaskAction,
    ) -> Self {
        let now = Utc::now();
        let next_run_at = schedule.next_occurrence(now);
        Self {
            id: TaskId::new(),
            name: name.into(),
            schedule,
            agent_id,
            action,
            status: TaskStatus::Active,
            created_at: now,
            next_run_at,
            last_run_at: None,
        }
    }

    /// Returns `true` if this task is due at the given time.
    #[must_use]
    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.status == TaskStatus::Active && self.next_run_at.is_some_and(|next| next <= now)
    }

    /// Advance the task after a successful execution: update `last_run_at`,
    /// compute the new `next_run_at`, and set status appropriately.
    pub fn advance(&mut self, completed_at: DateTime<Utc>) {
        self.last_run_at = Some(completed_at);
        self.next_run_at = self.schedule.next_occurrence(completed_at);
        if self.next_run_at.is_none() {
            self.status = TaskStatus::Completed;
        } else {
            self.status = TaskStatus::Active;
        }
    }
}

impl fmt::Display for ScheduledTask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Task({}, name={}, status={}, action={})",
            self.id, self.name, self.status, self.action
        )
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
    use chrono::TimeZone;

    fn make_action() -> TaskAction {
        TaskAction::RunAgent {
            agent_id: AgentId::new(),
            input: serde_json::json!({"prompt": "hello"}),
        }
    }

    #[test]
    fn task_id_uniqueness() {
        let a = TaskId::new();
        let b = TaskId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn task_id_display_and_parse_round_trip() {
        let id = TaskId::new();
        let s = id.to_string();
        let parsed: TaskId = s.parse().expect("valid");
        assert_eq!(id, parsed);
    }

    #[test]
    fn task_id_serde_round_trip() {
        let id = TaskId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        let back: TaskId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }

    #[test]
    fn task_action_run_agent_display() {
        let action = make_action();
        let display = action.to_string();
        assert!(display.starts_with("run_agent("));
    }

    #[test]
    fn task_action_execute_tool_display() {
        let action = TaskAction::ExecuteTool {
            tool_name: "my_tool".into(),
            input: serde_json::Value::Null,
        };
        assert!(action.to_string().contains("my_tool"));
    }

    #[test]
    fn task_action_webhook_display() {
        let action = TaskAction::WebhookCall {
            url: "https://example.com".into(),
            method: "POST".into(),
            body: serde_json::Value::Null,
        };
        let display = action.to_string();
        assert!(display.contains("POST"));
        assert!(display.contains("example.com"));
    }

    #[test]
    fn task_action_custom_display() {
        let action = TaskAction::Custom {
            handler: "my_handler".into(),
            payload: serde_json::Value::Null,
        };
        assert!(action.to_string().contains("my_handler"));
    }

    #[test]
    fn task_action_serde_round_trip() {
        let action = make_action();
        let json = serde_json::to_string(&action).expect("serialize");
        let back: TaskAction = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(action, back);
    }

    #[test]
    fn task_status_display() {
        assert_eq!(TaskStatus::Active.to_string(), "active");
        assert_eq!(TaskStatus::Running.to_string(), "running");
        assert_eq!(TaskStatus::Paused.to_string(), "paused");
        assert_eq!(TaskStatus::Completed.to_string(), "completed");
        assert_eq!(TaskStatus::Disabled.to_string(), "disabled");
    }

    #[test]
    fn scheduled_task_is_due() {
        let schedule = Schedule::Once {
            at: Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap(),
        };
        let mut task = ScheduledTask::new("test", schedule, AgentId::new(), make_action());
        task.next_run_at = Some(Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap());
        task.status = TaskStatus::Active;

        let before = Utc.with_ymd_and_hms(2025, 5, 1, 0, 0, 0).unwrap();
        assert!(!task.is_due(before));

        let after = Utc.with_ymd_and_hms(2025, 7, 1, 0, 0, 0).unwrap();
        assert!(task.is_due(after));
    }

    #[test]
    fn scheduled_task_not_due_when_paused() {
        let schedule = Schedule::Once {
            at: Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap(),
        };
        let mut task = ScheduledTask::new("test", schedule, AgentId::new(), make_action());
        task.next_run_at = Some(Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap());
        task.status = TaskStatus::Paused;

        let after = Utc.with_ymd_and_hms(2025, 7, 1, 0, 0, 0).unwrap();
        assert!(!task.is_due(after));
    }

    #[test]
    fn scheduled_task_advance_once_completes() {
        let at = Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap();
        let schedule = Schedule::Once { at };
        let mut task = ScheduledTask::new("test", schedule, AgentId::new(), make_action());
        task.next_run_at = Some(at);

        let completed = Utc.with_ymd_and_hms(2025, 6, 1, 0, 1, 0).unwrap();
        task.advance(completed);

        assert_eq!(task.status, TaskStatus::Completed);
        assert!(task.next_run_at.is_none());
        assert_eq!(task.last_run_at, Some(completed));
    }

    #[test]
    fn scheduled_task_advance_interval_continues() {
        let start = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let schedule = Schedule::Interval {
            every: chrono::Duration::hours(1),
            start,
        };
        let mut task = ScheduledTask::new("test", schedule, AgentId::new(), make_action());

        let completed = Utc.with_ymd_and_hms(2025, 1, 1, 1, 0, 0).unwrap();
        task.advance(completed);

        assert_eq!(task.status, TaskStatus::Active);
        assert!(task.next_run_at.is_some());
        assert_eq!(task.last_run_at, Some(completed));
    }

    #[test]
    fn task_result_serde_round_trip() {
        let result = TaskResult {
            success: true,
            message: Some("done".into()),
            completed_at: Utc::now(),
            duration_ms: 123,
        };
        let json = serde_json::to_string(&result).expect("serialize");
        let back: TaskResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(result, back);
    }

    #[test]
    fn scheduled_task_display() {
        let task = ScheduledTask::new(
            "my-task",
            Schedule::Once {
                at: Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
            },
            AgentId::new(),
            make_action(),
        );
        let display = task.to_string();
        assert!(display.contains("my-task"));
    }
}
