//! Error types for the scheduler subsystem.
//!
//! [`SchedulerError`] covers all failure modes specific to task scheduling:
//! invalid cron expressions, task-not-found lookups, duplicate task IDs,
//! execution failures, and internal invariant violations.

use thiserror::Error;

/// The error type for `polkagent-scheduler`.
#[derive(Debug, Error)]
pub enum SchedulerError {
    /// A cron expression could not be parsed.
    #[error("invalid cron expression: {message}")]
    InvalidCronExpression {
        /// Human-readable description of the parsing failure.
        message: String,
    },

    /// A referenced task was not found in the store.
    #[error("task not found: {task_id}")]
    TaskNotFound {
        /// The ID of the missing task.
        task_id: String,
    },

    /// A task with the same ID already exists.
    #[error("task already exists: {task_id}")]
    TaskAlreadyExists {
        /// The conflicting task ID.
        task_id: String,
    },

    /// A task execution failed.
    #[error("task execution failed: {message}")]
    ExecutionFailed {
        /// Description of the execution failure.
        message: String,
    },

    /// A schedule has no valid next occurrence (e.g. a `Once` schedule in the
    /// past).
    #[error("schedule exhausted: no future occurrence")]
    ScheduleExhausted,

    /// A validation check failed on a task or schedule definition.
    #[error("validation failed for field '{field}': {message}")]
    Validation {
        /// Which field or constraint failed.
        field: String,
        /// Human-readable description of the failure.
        message: String,
    },

    /// The scheduler runner is not in the expected state for the requested
    /// operation.
    #[error("runner state error: {message}")]
    RunnerState {
        /// Description of the state violation.
        message: String,
    },

    /// An error from the underlying persistence layer.
    #[error("store error: {message}")]
    Store {
        /// Description of the storage failure.
        message: String,
    },

    /// JSON serialization / deserialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// An internal invariant was violated.
    #[error("internal error: {message}")]
    Internal {
        /// Description of the violated invariant.
        message: String,
    },
}

impl SchedulerError {
    /// Construct a `TaskNotFound` error.
    pub fn task_not_found(task_id: impl std::fmt::Display) -> Self {
        Self::TaskNotFound {
            task_id: task_id.to_string(),
        }
    }

    /// Construct a `TaskAlreadyExists` error.
    pub fn task_already_exists(task_id: impl std::fmt::Display) -> Self {
        Self::TaskAlreadyExists {
            task_id: task_id.to_string(),
        }
    }

    /// Construct a `Validation` error.
    pub fn validation(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Validation {
            field: field.into(),
            message: message.into(),
        }
    }

    /// Construct an `Internal` error from any displayable value.
    pub fn internal(message: impl std::fmt::Display) -> Self {
        Self::Internal {
            message: message.to_string(),
        }
    }

    /// Construct a `Store` error.
    pub fn store(message: impl std::fmt::Display) -> Self {
        Self::Store {
            message: message.to_string(),
        }
    }

    /// Returns `true` if this error indicates an invariant violation.
    #[must_use]
    pub fn is_invariant_violation(&self) -> bool {
        matches!(self, Self::Internal { .. })
    }

    /// Returns `true` if the error might resolve on retry.
    #[must_use]
    pub fn is_potentially_transient(&self) -> bool {
        matches!(self, Self::ExecutionFailed { .. } | Self::Store { .. })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_not_found_display() {
        let err = SchedulerError::task_not_found("task-123");
        assert!(err.to_string().contains("task-123"));
    }

    #[test]
    fn task_already_exists_display() {
        let err = SchedulerError::task_already_exists("task-456");
        assert!(err.to_string().contains("task-456"));
    }

    #[test]
    fn validation_error_display() {
        let err = SchedulerError::validation("name", "must not be empty");
        let msg = err.to_string();
        assert!(msg.contains("name"));
        assert!(msg.contains("must not be empty"));
    }

    #[test]
    fn internal_error_is_invariant_violation() {
        let err = SchedulerError::internal("broken invariant");
        assert!(err.is_invariant_violation());
    }

    #[test]
    fn execution_failed_is_transient() {
        let err = SchedulerError::ExecutionFailed {
            message: "timeout".into(),
        };
        assert!(err.is_potentially_transient());
    }

    #[test]
    fn store_error_is_transient() {
        let err = SchedulerError::store("connection refused");
        assert!(err.is_potentially_transient());
    }

    #[test]
    fn schedule_exhausted_display() {
        let err = SchedulerError::ScheduleExhausted;
        assert!(err.to_string().contains("exhausted"));
    }

    #[test]
    fn invalid_cron_expression_display() {
        let err = SchedulerError::InvalidCronExpression {
            message: "expected 5 fields".into(),
        };
        assert!(err.to_string().contains("expected 5 fields"));
    }

    #[test]
    fn runner_state_error_not_transient() {
        let err = SchedulerError::RunnerState {
            message: "already running".into(),
        };
        assert!(!err.is_potentially_transient());
        assert!(!err.is_invariant_violation());
    }
}
