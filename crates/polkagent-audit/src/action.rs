//! Audit action types.
//!
//! [`AuditAction`] enumerates every category of auditable event in the
//! platform. Each variant maps to a distinct security-relevant operation
//! that must appear in the audit trail.

use serde::{Deserialize, Serialize};
use std::fmt;

/// The category of action being audited.
///
/// Each variant corresponds to a security-relevant operation tracked by the
/// platform. The enum is non-exhaustive so that new action types can be added
/// without breaking downstream matches.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    /// An agent requested an effect (pre-execution).
    EffectRequested,
    /// An effect was executed (post-execution).
    EffectExecuted,
    /// A grant was evaluated against a policy.
    GrantEvaluated,
    /// A policy decision was rendered (allow/deny).
    PolicyDecision,
    /// A secret was accessed from the secret store.
    SecretAccessed,
    /// A configuration value was changed.
    ConfigChanged,
    /// An agent run was started.
    RunStarted,
    /// An agent run completed (success or failure).
    RunCompleted,
    /// An approval was granted by a user or operator.
    ApprovalGranted,
    /// An approval was denied by a user or operator.
    ApprovalDenied,
    /// A tool was invoked by an agent.
    ToolInvoked,
    /// A transaction was submitted to a chain.
    ChainSubmitted,
    /// An effect execution was attempted (logged before I/O begins).
    EffectAttempted,
    /// An effect execution completed successfully.
    EffectCompleted,
    /// An effect execution failed.
    EffectFailed,
}

impl AuditAction {
    /// Returns `true` if this action type represents a security-sensitive
    /// operation that should trigger elevated alerting.
    #[must_use]
    pub fn is_security_sensitive(&self) -> bool {
        matches!(
            self,
            Self::SecretAccessed
                | Self::PolicyDecision
                | Self::ApprovalDenied
                | Self::ChainSubmitted
                | Self::ConfigChanged
        )
    }
}

impl fmt::Display for AuditAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::EffectRequested => "effect_requested",
            Self::EffectExecuted => "effect_executed",
            Self::GrantEvaluated => "grant_evaluated",
            Self::PolicyDecision => "policy_decision",
            Self::SecretAccessed => "secret_accessed",
            Self::ConfigChanged => "config_changed",
            Self::RunStarted => "run_started",
            Self::RunCompleted => "run_completed",
            Self::ApprovalGranted => "approval_granted",
            Self::ApprovalDenied => "approval_denied",
            Self::ToolInvoked => "tool_invoked",
            Self::ChainSubmitted => "chain_submitted",
            Self::EffectAttempted => "effect_attempted",
            Self::EffectCompleted => "effect_completed",
            Self::EffectFailed => "effect_failed",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_action_display() {
        assert_eq!(AuditAction::EffectRequested.to_string(), "effect_requested");
        assert_eq!(AuditAction::ChainSubmitted.to_string(), "chain_submitted");
        assert_eq!(AuditAction::ToolInvoked.to_string(), "tool_invoked");
    }

    #[test]
    fn audit_action_serde_round_trip() {
        let actions = [
            AuditAction::EffectRequested,
            AuditAction::EffectExecuted,
            AuditAction::GrantEvaluated,
            AuditAction::PolicyDecision,
            AuditAction::SecretAccessed,
            AuditAction::ConfigChanged,
            AuditAction::RunStarted,
            AuditAction::RunCompleted,
            AuditAction::ApprovalGranted,
            AuditAction::ApprovalDenied,
            AuditAction::ToolInvoked,
            AuditAction::ChainSubmitted,
            AuditAction::EffectAttempted,
            AuditAction::EffectCompleted,
            AuditAction::EffectFailed,
        ];
        for action in &actions {
            let json = serde_json::to_string(action).expect("serialize");
            let back: AuditAction = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(action, &back);
        }
    }

    #[test]
    fn security_sensitive_actions() {
        assert!(AuditAction::SecretAccessed.is_security_sensitive());
        assert!(AuditAction::PolicyDecision.is_security_sensitive());
        assert!(AuditAction::ApprovalDenied.is_security_sensitive());
        assert!(AuditAction::ChainSubmitted.is_security_sensitive());
        assert!(AuditAction::ConfigChanged.is_security_sensitive());
    }

    #[test]
    fn non_security_sensitive_actions() {
        assert!(!AuditAction::EffectRequested.is_security_sensitive());
        assert!(!AuditAction::RunStarted.is_security_sensitive());
        assert!(!AuditAction::ToolInvoked.is_security_sensitive());
        assert!(!AuditAction::ApprovalGranted.is_security_sensitive());
        assert!(!AuditAction::EffectAttempted.is_security_sensitive());
        assert!(!AuditAction::EffectCompleted.is_security_sensitive());
        assert!(!AuditAction::EffectFailed.is_security_sensitive());
    }
}
