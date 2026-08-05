//! Stable identifiers owned by the interaction domain.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Generate a time-ordered UUID v7 identifier.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// Wrap an existing UUID.
            #[must_use]
            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// Return the wrapped UUID.
            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value).map(Self)
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

define_id!(
    /// A human interaction turn, distinct from an executor-local model turn.
    InteractionTurnId
);
define_id!(
    /// A stable user-visible tool call identity across updates and recovery.
    ToolCallId
);
define_id!(
    /// A stable entry in a user-visible execution plan.
    PlanEntryId
);
define_id!(
    /// A stable interaction event identity.
    InteractionEventId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_unique_and_round_trip() {
        let first = InteractionTurnId::new();
        let second = InteractionTurnId::new();
        assert_ne!(first, second);
        assert_eq!(first.to_string().parse(), Ok(first));
        assert_eq!(InteractionTurnId::from_uuid(first.as_uuid()), first);
    }

    #[test]
    fn identifiers_serialize_as_uuid_strings() {
        let id = ToolCallId::new();
        let encoded = serde_json::to_string(&id).expect("serialize ID");
        let decoded: ToolCallId = serde_json::from_str(&encoded).expect("deserialize ID");
        assert_eq!(decoded, id);
        assert_eq!(encoded, format!("\"{id}\""));
    }
}
