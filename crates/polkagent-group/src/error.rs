//! Error types for the polkagent-group crate.

use thiserror::Error;

use polkagent_core::ids::AgentId;

use crate::types::GroupId;

/// All errors that the group coordination layer can produce.
#[derive(Debug, Error)]
pub enum GroupError {
    /// The requested group was not found.
    #[error("group not found: {0}")]
    NotFound(GroupId),

    /// A group with this ID or name already exists.
    #[error("group already exists: {0}")]
    AlreadyExists(GroupId),

    /// The agent is already a member of the group.
    #[error("agent {0} is already a member of group {1}")]
    AlreadyMember(AgentId, GroupId),

    /// The agent is not a member of the group.
    #[error("agent {0} is not a member of group {1}")]
    NotMember(AgentId, GroupId),

    /// A quorum decision could not be reached.
    #[error("quorum not met for group {0}: {1}")]
    QuorumNotMet(GroupId, String),

    /// The requested spend exceeds the group or member budget.
    #[error("budget exceeded for group {0}: {1}")]
    BudgetExceeded(GroupId, String),

    /// The agent does not have permission to perform this operation.
    #[error("permission denied for agent {0} in group {1}: {2}")]
    PermissionDenied(AgentId, GroupId, String),

    /// A group definition or policy update violates a domain invariant.
    #[error("invalid group {0}: {1}")]
    InvalidGroup(GroupId, String),

    /// A general internal error.
    #[error("internal error: {0}")]
    Internal(String),
}

/// Convenience alias for results from this crate.
pub type GroupResult<T> = Result<T, GroupError>;
