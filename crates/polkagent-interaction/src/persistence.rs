//! Durable storage contracts below [`InteractionService`](crate::InteractionService).
//!
//! The service owns runtime orchestration and transcript policy. This module
//! gives persistence adapters enough information to atomically create a turn
//! with its run links and to append replayable events with store-assigned
//! sequences.

use std::collections::HashSet;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use polkagent_core::ids::{ConversationId, RunId};
use uuid::Uuid;

use crate::error::InteractionError;
use crate::event::{InteractionEvent, InteractionEventEnvelope, RunRole};
use crate::ids::{InteractionEventId, InteractionTurnId};
use crate::model::{InteractionConfig, InteractionTarget, TurnSummary};

/// One run linked to a human interaction turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionRunLink {
    /// Durable run identity.
    pub run_id: RunId,
    /// User-visible orchestration role.
    pub role: RunRole,
    /// One-based deterministic order within the turn.
    pub ordinal: u32,
}

/// Atomic input for creating a turn and all initial run correlations.
#[derive(Debug, Clone, PartialEq)]
pub struct NewInteractionTurn {
    /// Caller-generated idempotency identity for the turn.
    pub turn_id: InteractionTurnId,
    /// Parent durable interaction.
    pub conversation_id: ConversationId,
    /// One-based monotonic order within the interaction.
    pub ordinal: u32,
    /// Resolved execution target.
    pub target: InteractionTarget,
    /// Resolved turn configuration.
    pub config: InteractionConfig,
    /// User message persisted before execution begins.
    pub user_message_id: Uuid,
    /// Runs created with the interaction correlation already attached.
    pub runs: Vec<InteractionRunLink>,
    /// Caller-generated identity for the initial `turn_started` event.
    pub initial_event_id: InteractionEventId,
    /// Durable creation timestamp.
    pub started_at: DateTime<Utc>,
}

impl NewInteractionTurn {
    /// Validate invariants that do not require consulting the durable store.
    pub fn validate(&self) -> Result<(), InteractionError> {
        if self.ordinal == 0 {
            return Err(InteractionError::invalid_request(
                "interaction turn ordinal must start at one",
            ));
        }
        if self.runs.is_empty() {
            return Err(InteractionError::invalid_request(
                "interaction turn must link at least one run",
            ));
        }
        self.config.validate()?;
        if self.config.target != self.target {
            return Err(InteractionError::invalid_request(
                "interaction turn target must match resolved configuration",
            ));
        }

        let mut ordinals: Vec<u32> = self.runs.iter().map(|run| run.ordinal).collect();
        ordinals.sort_unstable();
        let expected: Vec<u32> = (1..=u32::try_from(self.runs.len()).map_err(|_| {
            InteractionError::invalid_request("interaction turn has too many linked runs")
        })?)
            .collect();
        if ordinals != expected {
            return Err(InteractionError::invalid_request(
                "linked run ordinals must be unique and contiguous from one",
            ));
        }

        let run_ids: HashSet<RunId> = self.runs.iter().map(|run| run.run_id).collect();
        if run_ids.len() != self.runs.len() {
            return Err(InteractionError::invalid_request(
                "an interaction turn cannot link the same run more than once",
            ));
        }
        Ok(())
    }
}

/// Fully decoded durable turn record.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredInteractionTurn {
    /// Stable turn summary and run correlation.
    pub summary: TurnSummary,
    /// Linked runs with stable orchestration roles and ordering.
    pub runs: Vec<InteractionRunLink>,
    /// Resolved target used to start the turn.
    pub target: InteractionTarget,
    /// Resolved turn configuration.
    pub config: InteractionConfig,
    /// Durable user transcript message.
    pub user_message_id: Uuid,
    /// Durable assistant transcript message, once recorded.
    pub assistant_message_id: Option<Uuid>,
    /// Safe terminal failure, when the turn failed.
    pub error: Option<InteractionError>,
}

/// Idempotent input for appending one durable interaction event.
#[derive(Debug, Clone, PartialEq)]
pub struct NewInteractionEvent {
    /// Caller-generated event identity used to make retries idempotent.
    pub event_id: InteractionEventId,
    /// Parent durable interaction.
    pub conversation_id: ConversationId,
    /// Parent human interaction turn.
    pub turn_id: InteractionTurnId,
    /// Durable event timestamp.
    pub timestamp: DateTime<Utc>,
    /// Structured, surface-neutral payload.
    pub event: InteractionEvent,
}

/// Persistence boundary used by a durable interaction-service implementation.
#[async_trait]
pub trait InteractionStore: Send + Sync {
    /// Atomically create a turn, its run links, and its initial event.
    ///
    /// Retrying the same `turn_id` with identical input returns the existing
    /// record. Reusing it for different input returns a conflict.
    async fn create_turn(
        &self,
        turn: NewInteractionTurn,
    ) -> Result<StoredInteractionTurn, InteractionError>;

    /// Load one turn and all linked runs.
    async fn load_turn(
        &self,
        turn_id: InteractionTurnId,
    ) -> Result<StoredInteractionTurn, InteractionError>;

    /// List turns for an interaction in ordinal order.
    async fn list_turns(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<StoredInteractionTurn>, InteractionError>;

    /// Append one event and atomically update the turn lifecycle projection.
    ///
    /// The store assigns the next per-interaction sequence. Retrying an
    /// existing `event_id` with identical input returns its original envelope.
    async fn append_event(
        &self,
        event: NewInteractionEvent,
    ) -> Result<InteractionEventEnvelope, InteractionError>;

    /// Replay events strictly after `after_sequence`, in ascending order.
    async fn load_events(
        &self,
        conversation_id: ConversationId,
        after_sequence: u64,
        limit: u32,
    ) -> Result<Vec<InteractionEventEnvelope>, InteractionError>;
}

#[cfg(test)]
mod tests {
    use polkagent_core::config::AutonomyLevel;
    use polkagent_core::ids::AgentId;

    use super::*;

    fn new_turn(runs: Vec<InteractionRunLink>) -> NewInteractionTurn {
        let target = InteractionTarget::Agent(AgentId::new());
        NewInteractionTurn {
            turn_id: InteractionTurnId::new(),
            conversation_id: ConversationId::new(),
            ordinal: 1,
            target: target.clone(),
            config: InteractionConfig {
                target,
                model: None,
                provider: None,
                harness: None,
                autonomy: AutonomyLevel::default(),
                max_turns: None,
                budget: None,
            },
            user_message_id: Uuid::now_v7(),
            runs,
            initial_event_id: InteractionEventId::new(),
            started_at: Utc::now(),
        }
    }

    #[test]
    fn new_turn_requires_contiguous_unique_run_links() {
        let run_id = RunId::new();
        let duplicate = new_turn(vec![
            InteractionRunLink {
                run_id,
                role: RunRole::Primary,
                ordinal: 1,
            },
            InteractionRunLink {
                run_id,
                role: RunRole::Child("reviewer".to_owned()),
                ordinal: 2,
            },
        ]);
        assert!(duplicate.validate().is_err());

        let gap = new_turn(vec![InteractionRunLink {
            run_id: RunId::new(),
            role: RunRole::Primary,
            ordinal: 2,
        }]);
        assert!(gap.validate().is_err());
    }

    #[test]
    fn valid_new_turn_accepts_order_independent_links() {
        let turn = new_turn(vec![
            InteractionRunLink {
                run_id: RunId::new(),
                role: RunRole::Child("reviewer".to_owned()),
                ordinal: 2,
            },
            InteractionRunLink {
                run_id: RunId::new(),
                role: RunRole::Coordinator,
                ordinal: 1,
            },
        ]);
        assert_eq!(turn.validate(), Ok(()));
    }
}
