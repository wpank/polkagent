//! Audit entry — the core record in the audit trail.
//!
//! Each [`AuditEntry`] represents a single auditable event. Entries are
//! append-only and include an [`integrity_hash`](AuditEntry::integrity_hash)
//! that chains them together for tamper detection.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::action::AuditAction;
use crate::actor::ActorInfo;

// ---------------------------------------------------------------------------
// AuditId
// ---------------------------------------------------------------------------

/// Unique identifier for an audit entry (UUID v7, time-ordered).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuditId(Uuid);

impl AuditId {
    /// Generate a new time-ordered (v7) audit identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wrap an existing [`Uuid`].
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

impl Default for AuditId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AuditId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for AuditId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

// ---------------------------------------------------------------------------
// ActionOutcome
// ---------------------------------------------------------------------------

/// The result of an audited action.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionOutcome {
    /// The action succeeded.
    Success,
    /// The action failed.
    Failure,
    /// The action was denied by policy.
    Denied,
    /// The action produced an error.
    Error,
}

impl std::fmt::Display for ActionOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::Failure => write!(f, "failure"),
            Self::Denied => write!(f, "denied"),
            Self::Error => write!(f, "error"),
        }
    }
}

// ---------------------------------------------------------------------------
// ResourceInfo
// ---------------------------------------------------------------------------

/// Information about the resource targeted by an audited action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceInfo {
    /// The type of resource (e.g. "run", "effect", "secret", "config").
    pub resource_type: String,
    /// The unique identifier of the resource.
    pub resource_id: String,
    /// Optional human-readable description.
    pub description: Option<String>,
}

impl ResourceInfo {
    /// Create a new resource info.
    #[must_use]
    pub fn new(resource_type: impl Into<String>, resource_id: impl Into<String>) -> Self {
        Self {
            resource_type: resource_type.into(),
            resource_id: resource_id.into(),
            description: None,
        }
    }

    /// Set the description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

impl std::fmt::Display for ResourceInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.resource_type, self.resource_id)?;
        if let Some(desc) = &self.description {
            write!(f, " ({desc})")?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// AuditEntry
// ---------------------------------------------------------------------------

/// A single record in the audit trail.
///
/// Entries are append-only. The [`integrity_hash`](Self::integrity_hash) field
/// contains `BLAKE3(prev_hash + entry_json)` to form a tamper-evident chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unique, time-ordered identifier.
    pub id: AuditId,
    /// When the event occurred (UTC).
    pub timestamp: DateTime<Utc>,
    /// Who performed the action.
    pub actor: ActorInfo,
    /// What action was performed.
    pub action: AuditAction,
    /// What resource was targeted.
    pub resource: ResourceInfo,
    /// The outcome of the action.
    pub outcome: ActionOutcome,
    /// Arbitrary additional context (JSON value).
    pub context: serde_json::Value,
    /// BLAKE3 integrity hash chaining this entry to the previous one.
    pub integrity_hash: String,
}

impl AuditEntry {
    /// Create a new audit entry with all fields specified and an empty
    /// integrity hash. The hash should be computed by the integrity module
    /// before the entry is stored.
    #[must_use]
    pub fn new(
        actor: ActorInfo,
        action: AuditAction,
        resource: ResourceInfo,
        outcome: ActionOutcome,
        context: serde_json::Value,
    ) -> Self {
        Self {
            id: AuditId::new(),
            timestamp: Utc::now(),
            actor,
            action,
            resource,
            outcome,
            context,
            integrity_hash: String::new(),
        }
    }
}

impl std::fmt::Display for AuditEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] {} {} {} -> {}",
            self.timestamp.format("%Y-%m-%dT%H:%M:%S%.3fZ"),
            self.actor,
            self.action,
            self.resource,
            self.outcome,
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
    reason = "unit-test audit assertions intentionally panic with focused diagnostics"
)]
mod tests {
    use super::*;
    use crate::actor::ActorInfo;

    #[test]
    fn audit_id_uniqueness() {
        let a = AuditId::new();
        let b = AuditId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn audit_id_display_round_trip() {
        let id = AuditId::new();
        let s = id.to_string();
        let parsed: AuditId = s.parse().expect("valid UUID");
        assert_eq!(id, parsed);
    }

    #[test]
    fn audit_id_serde_round_trip() {
        let id = AuditId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        let back: AuditId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }

    #[test]
    fn action_outcome_display() {
        assert_eq!(ActionOutcome::Success.to_string(), "success");
        assert_eq!(ActionOutcome::Failure.to_string(), "failure");
        assert_eq!(ActionOutcome::Denied.to_string(), "denied");
        assert_eq!(ActionOutcome::Error.to_string(), "error");
    }

    #[test]
    fn action_outcome_serde_round_trip() {
        for outcome in [
            ActionOutcome::Success,
            ActionOutcome::Failure,
            ActionOutcome::Denied,
            ActionOutcome::Error,
        ] {
            let json = serde_json::to_string(&outcome).expect("serialize");
            let back: ActionOutcome = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(outcome, back);
        }
    }

    #[test]
    fn resource_info_display_without_description() {
        let r = ResourceInfo::new("run", "run-123");
        assert_eq!(r.to_string(), "run:run-123");
    }

    #[test]
    fn resource_info_display_with_description() {
        let r = ResourceInfo::new("secret", "db-password").with_description("Production DB");
        assert_eq!(r.to_string(), "secret:db-password (Production DB)");
    }

    #[test]
    fn resource_info_serde_round_trip() {
        let r = ResourceInfo::new("effect", "eff-1").with_description("Chain submit");
        let json = serde_json::to_string(&r).expect("serialize");
        let back: ResourceInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(r, back);
    }

    #[test]
    fn audit_entry_creation() {
        let entry = AuditEntry::new(
            ActorInfo::agent("agent-1"),
            AuditAction::RunStarted,
            ResourceInfo::new("run", "run-42"),
            ActionOutcome::Success,
            serde_json::json!({"trigger": "api"}),
        );
        assert!(!entry.id.to_string().is_empty());
        assert_eq!(entry.actor.id, "agent-1");
        assert_eq!(entry.action, AuditAction::RunStarted);
        assert_eq!(entry.outcome, ActionOutcome::Success);
        assert!(entry.integrity_hash.is_empty());
    }

    #[test]
    fn audit_entry_display() {
        let entry = AuditEntry::new(
            ActorInfo::user("u-1").with_name("Alice"),
            AuditAction::SecretAccessed,
            ResourceInfo::new("secret", "api-key"),
            ActionOutcome::Success,
            serde_json::Value::Null,
        );
        let display = entry.to_string();
        assert!(display.contains("user:u-1 (Alice)"));
        assert!(display.contains("secret_accessed"));
        assert!(display.contains("secret:api-key"));
        assert!(display.contains("success"));
    }

    #[test]
    fn audit_entry_serde_round_trip() {
        let mut entry = AuditEntry::new(
            ActorInfo::system("scheduler"),
            AuditAction::ConfigChanged,
            ResourceInfo::new("config", "autonomy-level"),
            ActionOutcome::Success,
            serde_json::json!({"old": "supervised", "new": "autonomous"}),
        );
        entry.integrity_hash = "abc123".to_string();

        let json = serde_json::to_string(&entry).expect("serialize");
        let back: AuditEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(entry.id, back.id);
        assert_eq!(entry.actor, back.actor);
        assert_eq!(entry.action, back.action);
        assert_eq!(entry.resource, back.resource);
        assert_eq!(entry.outcome, back.outcome);
        assert_eq!(entry.context, back.context);
        assert_eq!(entry.integrity_hash, back.integrity_hash);
    }
}
