//! Query builder for filtering audit entries.
//!
//! [`AuditQuery`] provides a fluent builder API for constructing filter
//! predicates over the audit log. Queries support filtering by actor, action
//! type, time range, and result limit.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::action::AuditAction;
use crate::entry::AuditEntry;

/// Default maximum number of results when no limit is specified.
const DEFAULT_LIMIT: usize = 1000;

/// A filter predicate for querying audit entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditQuery {
    /// Filter by actor ID (exact match).
    pub actor_id: Option<String>,
    /// Filter by action type.
    pub action: Option<AuditAction>,
    /// Only include entries at or after this timestamp.
    pub since: Option<DateTime<Utc>>,
    /// Only include entries at or before this timestamp.
    pub until: Option<DateTime<Utc>>,
    /// Maximum number of entries to return.
    pub limit: usize,
}

impl Default for AuditQuery {
    fn default() -> Self {
        Self {
            actor_id: None,
            action: None,
            since: None,
            until: None,
            limit: DEFAULT_LIMIT,
        }
    }
}

impl AuditQuery {
    /// Create a new query builder with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Filter by actor ID.
    #[must_use]
    pub fn actor(mut self, id: impl Into<String>) -> Self {
        self.actor_id = Some(id.into());
        self
    }

    /// Filter by action type.
    #[must_use]
    pub fn action(mut self, action: AuditAction) -> Self {
        self.action = Some(action);
        self
    }

    /// Only include entries at or after the given timestamp.
    #[must_use]
    pub fn since(mut self, since: DateTime<Utc>) -> Self {
        self.since = Some(since);
        self
    }

    /// Only include entries at or before the given timestamp.
    #[must_use]
    pub fn until(mut self, until: DateTime<Utc>) -> Self {
        self.until = Some(until);
        self
    }

    /// Set the maximum number of entries to return.
    #[must_use]
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Finalise the query (returns `self`; provided for API symmetry with
    /// typical builder patterns).
    #[must_use]
    pub fn build(self) -> Self {
        self
    }

    /// Test whether a single audit entry matches this query's predicates.
    #[must_use]
    pub fn matches(&self, entry: &AuditEntry) -> bool {
        if let Some(ref actor_id) = self.actor_id {
            if entry.actor.id != *actor_id {
                return false;
            }
        }
        if let Some(ref action) = self.action {
            if entry.action != *action {
                return false;
            }
        }
        if let Some(since) = self.since {
            if entry.timestamp < since {
                return false;
            }
        }
        if let Some(until) = self.until {
            if entry.timestamp > until {
                return false;
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::AuditAction;
    use crate::actor::ActorInfo;
    use crate::entry::{ActionOutcome, AuditEntry, ResourceInfo};
    use chrono::Duration;

    fn make_entry_for(actor_id: &str, action: AuditAction) -> AuditEntry {
        AuditEntry::new(
            ActorInfo::agent(actor_id),
            action,
            ResourceInfo::new("test", "r-1"),
            ActionOutcome::Success,
            serde_json::Value::Null,
        )
    }

    #[test]
    fn default_query_matches_everything() {
        let q = AuditQuery::new();
        let entry = make_entry_for("a-1", AuditAction::RunStarted);
        assert!(q.matches(&entry));
    }

    #[test]
    fn query_by_actor() {
        let q = AuditQuery::new().actor("a-1").build();
        let e1 = make_entry_for("a-1", AuditAction::RunStarted);
        let e2 = make_entry_for("a-2", AuditAction::RunStarted);
        assert!(q.matches(&e1));
        assert!(!q.matches(&e2));
    }

    #[test]
    fn query_by_action() {
        let q = AuditQuery::new().action(AuditAction::ToolInvoked).build();
        let e1 = make_entry_for("a-1", AuditAction::ToolInvoked);
        let e2 = make_entry_for("a-1", AuditAction::RunStarted);
        assert!(q.matches(&e1));
        assert!(!q.matches(&e2));
    }

    #[test]
    fn query_by_time_range() {
        let now = Utc::now();
        let past = now - Duration::hours(2);
        let future = now + Duration::hours(2);

        let q = AuditQuery::new().since(past).until(future).build();

        let entry = make_entry_for("a-1", AuditAction::RunStarted);
        assert!(q.matches(&entry));

        // Entry in the far past should not match.
        let mut old_entry = make_entry_for("a-1", AuditAction::RunStarted);
        old_entry.timestamp = now - Duration::hours(5);
        assert!(!q.matches(&old_entry));
    }

    #[test]
    fn query_combined_filters() {
        let q = AuditQuery::new()
            .actor("agent-x")
            .action(AuditAction::ChainSubmitted)
            .limit(10)
            .build();

        // Matches both filters.
        let e1 = make_entry_for("agent-x", AuditAction::ChainSubmitted);
        assert!(q.matches(&e1));

        // Wrong actor.
        let e2 = make_entry_for("agent-y", AuditAction::ChainSubmitted);
        assert!(!q.matches(&e2));

        // Wrong action.
        let e3 = make_entry_for("agent-x", AuditAction::RunStarted);
        assert!(!q.matches(&e3));
    }

    #[test]
    fn query_default_limit() {
        let q = AuditQuery::new();
        assert_eq!(q.limit, 1000);
    }

    #[test]
    fn query_custom_limit() {
        let q = AuditQuery::new().limit(5).build();
        assert_eq!(q.limit, 5);
    }

    #[test]
    fn query_serde_round_trip() {
        let q = AuditQuery::new()
            .actor("a-1")
            .action(AuditAction::PolicyDecision)
            .limit(50);
        let json = serde_json::to_string(&q).expect("serialize");
        let back: AuditQuery = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(q.actor_id, back.actor_id);
        assert_eq!(q.action, back.action);
        assert_eq!(q.limit, back.limit);
    }
}
