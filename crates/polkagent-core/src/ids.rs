//! Typed identifier newtypes for all Polkagent domain entities.
//!
//! Each ID type is a newtype around [`uuid::Uuid`], using UUID v7 for
//! time-ordered generation. All IDs implement the full set of traits needed
//! for use as map keys, serde fields, and display targets.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Internal macro that stamps out the repetitive newtype boilerplate.
macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
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

            /// Return the bytes of the inner UUID.
            #[must_use]
            pub fn as_bytes(&self) -> &[u8; 16] {
                self.0.as_bytes()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }

        impl From<Uuid> for $name {
            fn from(uuid: Uuid) -> Self {
                Self(uuid)
            }
        }

        impl From<$name> for Uuid {
            fn from(id: $name) -> Self {
                id.0
            }
        }
    };
}

define_id!(
    /// Unique identifier for an [`AgentSpec`](crate::agent::AgentSpec).
    AgentId
);

define_id!(
    /// Unique identifier for a [`Run`](crate::run::Run).
    RunId
);

define_id!(
    /// Unique identifier for a [`Turn`](crate::turn::Turn).
    TurnId
);

define_id!(
    /// Unique identifier for a [`Step`](crate::turn::Step).
    StepId
);

define_id!(
    /// Unique identifier for an [`EffectIntent`](crate::effect::EffectIntent).
    EffectId
);

define_id!(
    /// Unique identifier for an [`Artifact`](crate::artifact::Artifact).
    ArtifactId
);

define_id!(
    /// Unique identifier for a [`RunEvent`](crate::event::RunEvent).
    EventId
);

define_id!(
    /// Unique identifier for a conversation (the multi-turn session context).
    ConversationId
);

define_id!(
    /// Unique identifier for a principal (user, operator, or service account).
    PrincipalId
);

// ---------------------------------------------------------------------------
// Additional IDs referenced across domain types
// ---------------------------------------------------------------------------

define_id!(
    /// Unique identifier for an `EffectAttempt`.
    EffectAttemptId
);

define_id!(
    /// Unique identifier for an `EffectOutcome`.
    EffectOutcomeId
);

define_id!(
    /// Unique identifier for a `ResolvedGrant`.
    GrantId
);

define_id!(
    /// Unique identifier for an approval request or decision.
    ApprovalId
);

define_id!(
    /// Unique identifier for a worker process/task claiming effect intents.
    WorkerId
);

define_id!(
    /// Unique identifier for a [`UsageRecord`](crate::usage::UsageRecord).
    UsageRecordId
);

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_id_new_is_unique() {
        let a = AgentId::new();
        let b = AgentId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn run_id_display_and_from_str_round_trip() {
        let id = RunId::new();
        let s = id.to_string();
        let parsed: RunId = s.parse().expect("valid UUID string");
        assert_eq!(id, parsed);
    }

    #[test]
    fn all_id_types_serialize_as_plain_uuid_string() {
        let id = TurnId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        // The value should be a quoted UUID string (no wrapper object).
        assert!(json.starts_with('"'));
        assert!(json.ends_with('"'));
        let round_trip: TurnId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, round_trip);
    }

    #[test]
    fn effect_id_from_uuid() {
        let uuid = Uuid::now_v7();
        let id = EffectId::from_uuid(uuid);
        assert_eq!(id.as_uuid(), uuid);
    }

    #[test]
    fn step_id_as_bytes_length() {
        let id = StepId::new();
        assert_eq!(id.as_bytes().len(), 16);
    }

    #[test]
    fn conversation_id_default_is_unique() {
        let a = ConversationId::default();
        let b = ConversationId::default();
        assert_ne!(a, b);
    }

    #[test]
    fn artifact_id_serde_round_trip() {
        let id = ArtifactId::new();
        let json = serde_json::to_string(&id).unwrap();
        let back: ArtifactId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn event_id_from_str_invalid_returns_error() {
        let result: Result<EventId, _> = "not-a-uuid".parse();
        assert!(result.is_err());
    }

    #[test]
    fn principal_id_into_uuid() {
        let id = PrincipalId::new();
        let uuid: Uuid = id.into();
        let back: PrincipalId = uuid.into();
        assert_eq!(id, back);
    }
}
