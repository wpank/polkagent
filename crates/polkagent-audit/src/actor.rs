//! Actor identity types for audit entries.
//!
//! Every audit entry records *who* performed the action via an [`ActorInfo`]
//! struct. Actors are classified as agents, human users, or system-level
//! processes.

use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// ActorType
// ---------------------------------------------------------------------------

/// The category of entity that performed an audited action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorType {
    /// An autonomous agent managed by the platform.
    Agent,
    /// A human user interacting through a client or API.
    User,
    /// An internal system process (scheduler, reaper, migration, etc.).
    System,
}

impl fmt::Display for ActorType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Agent => write!(f, "agent"),
            Self::User => write!(f, "user"),
            Self::System => write!(f, "system"),
        }
    }
}

// ---------------------------------------------------------------------------
// ActorInfo
// ---------------------------------------------------------------------------

/// Identity information for the entity that performed an audited action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorInfo {
    /// What kind of actor this is.
    pub actor_type: ActorType,
    /// Unique identifier for the actor (agent ID, user ID, or service name).
    pub id: String,
    /// Optional human-readable display name.
    pub name: Option<String>,
    /// Optional source IP address (relevant for user-initiated actions).
    pub ip: Option<String>,
}

impl ActorInfo {
    /// Create actor info for an agent.
    #[must_use]
    pub fn agent(id: impl Into<String>) -> Self {
        Self {
            actor_type: ActorType::Agent,
            id: id.into(),
            name: None,
            ip: None,
        }
    }

    /// Create actor info for a user.
    #[must_use]
    pub fn user(id: impl Into<String>) -> Self {
        Self {
            actor_type: ActorType::User,
            id: id.into(),
            name: None,
            ip: None,
        }
    }

    /// Create actor info for a system process.
    #[must_use]
    pub fn system(id: impl Into<String>) -> Self {
        Self {
            actor_type: ActorType::System,
            id: id.into(),
            name: None,
            ip: None,
        }
    }

    /// Set the display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the source IP address.
    #[must_use]
    pub fn with_ip(mut self, ip: impl Into<String>) -> Self {
        self.ip = Some(ip.into());
        self
    }
}

impl fmt::Display for ActorInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.actor_type, self.id)?;
        if let Some(name) = &self.name {
            write!(f, " ({name})")?;
        }
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
    reason = "unit-test audit assertions intentionally panic with focused diagnostics"
)]
mod tests {
    use super::*;

    #[test]
    fn actor_type_display() {
        assert_eq!(ActorType::Agent.to_string(), "agent");
        assert_eq!(ActorType::User.to_string(), "user");
        assert_eq!(ActorType::System.to_string(), "system");
    }

    #[test]
    fn actor_type_serde_round_trip() {
        for t in [ActorType::Agent, ActorType::User, ActorType::System] {
            let json = serde_json::to_string(&t).expect("serialize");
            let back: ActorType = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(t, back);
        }
    }

    #[test]
    fn actor_info_agent_builder() {
        let actor = ActorInfo::agent("agent-42").with_name("Governance Bot");
        assert_eq!(actor.actor_type, ActorType::Agent);
        assert_eq!(actor.id, "agent-42");
        assert_eq!(actor.name.as_deref(), Some("Governance Bot"));
        assert!(actor.ip.is_none());
    }

    #[test]
    fn actor_info_user_builder() {
        let actor = ActorInfo::user("user-7")
            .with_name("Alice")
            .with_ip("192.168.1.1");
        assert_eq!(actor.actor_type, ActorType::User);
        assert_eq!(actor.id, "user-7");
        assert_eq!(actor.name.as_deref(), Some("Alice"));
        assert_eq!(actor.ip.as_deref(), Some("192.168.1.1"));
    }

    #[test]
    fn actor_info_system_builder() {
        let actor = ActorInfo::system("scheduler");
        assert_eq!(actor.actor_type, ActorType::System);
        assert_eq!(actor.id, "scheduler");
        assert!(actor.name.is_none());
    }

    #[test]
    fn actor_info_display_without_name() {
        let actor = ActorInfo::agent("a-1");
        assert_eq!(actor.to_string(), "agent:a-1");
    }

    #[test]
    fn actor_info_display_with_name() {
        let actor = ActorInfo::user("u-1").with_name("Bob");
        assert_eq!(actor.to_string(), "user:u-1 (Bob)");
    }

    #[test]
    fn actor_info_serde_round_trip() {
        let actor = ActorInfo::user("u-99")
            .with_name("Charlie")
            .with_ip("10.0.0.1");
        let json = serde_json::to_string(&actor).expect("serialize");
        let back: ActorInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(actor, back);
    }
}
