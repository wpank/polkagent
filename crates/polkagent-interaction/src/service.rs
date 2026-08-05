//! Surface-neutral interaction service and replay-aware event stream traits.

use async_trait::async_trait;
use polkagent_core::ids::{ApprovalId, ConversationId};
use thiserror::Error;

use crate::error::InteractionError;
use crate::event::InteractionEventEnvelope;
use crate::ids::InteractionTurnId;
use crate::model::{
    ConfigUpdate, CreateInteractionRequest, InteractionConfig, InteractionSummary,
    ListInteractionsRequest, PromptRequest, SubscriptionRequest, TurnHandle, TurnSummary,
};

/// Failure observed while consuming an interaction event stream.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum StreamError {
    /// The bounded live channel dropped events before the consumer caught up.
    ///
    /// The caller must reload a durable projection and subscribe again after
    /// `resume_after_sequence`; it must not silently continue with a gap.
    #[error(
        "interaction event stream lagged after sequence {last_seen_sequence:?}; reload and resume after {resume_after_sequence}"
    )]
    Lagged {
        /// Last sequence the consumer successfully received, if any.
        last_seen_sequence: Option<u64>,
        /// Durable checkpoint after which a new subscription should start.
        resume_after_sequence: u64,
    },
    /// The producer closed the subscription.
    #[error("interaction event stream closed")]
    Closed,
    /// The durable event backend failed.
    #[error(transparent)]
    Backend(InteractionError),
}

impl From<InteractionError> for StreamError {
    fn from(error: InteractionError) -> Self {
        Self::Backend(error)
    }
}

/// Replay-aware event receiver implemented by a runtime adapter.
#[async_trait]
pub trait InteractionEventStream: Send {
    /// Receive the next event in strictly increasing sequence order.
    async fn recv(&mut self) -> Result<InteractionEventEnvelope, StreamError>;

    /// Last sequence successfully returned by [`Self::recv`], if any.
    fn checkpoint(&self) -> Option<u64>;
}

/// Type-erased interaction event stream suitable for adapter ownership.
pub type BoxInteractionEventStream = Box<dyn InteractionEventStream>;

/// A durably created turn and an already-attached event subscription.
///
/// Attaching before execution starts prevents callers from missing the first
/// live event. The durable sequence in [`TurnHandle`] remains authoritative if
/// this bounded receiver later reports lag.
pub struct StartedTurn {
    /// Durable turn and run correlation.
    pub handle: TurnHandle,
    /// Replay-aware event receiver for this interaction turn.
    pub events: BoxInteractionEventStream,
}

impl std::fmt::Debug for StartedTurn {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StartedTurn")
            .field("handle", &self.handle)
            .field("events", &"<interaction event stream>")
            .finish()
    }
}

/// Headless application boundary shared by terminal, TUI, API, and ACP.
///
/// Implementations own transactionality, persistence, runtime orchestration,
/// authorization, and replay. Surface adapters only translate their protocol
/// into these operations and render the returned projections/events.
#[async_trait]
pub trait InteractionService: Send + Sync {
    /// Create and persist a new interaction.
    async fn new_interaction(
        &self,
        request: CreateInteractionRequest,
    ) -> Result<InteractionSummary, InteractionError>;

    /// List durable interactions in stable recency order.
    async fn list_interactions(
        &self,
        request: ListInteractionsRequest,
    ) -> Result<Vec<InteractionSummary>, InteractionError>;

    /// Load one durable interaction projection.
    async fn load_interaction(
        &self,
        conversation_id: ConversationId,
    ) -> Result<InteractionSummary, InteractionError>;

    /// List durable turns for one interaction in ordinal order.
    async fn list_turns(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<TurnSummary>, InteractionError>;

    /// Delete or tombstone an interaction according to runtime retention policy.
    async fn delete_interaction(
        &self,
        conversation_id: ConversationId,
    ) -> Result<(), InteractionError>;

    /// Persist a user prompt, create its turn/run links, attach a receiver, and
    /// then start execution.
    async fn prompt(&self, request: PromptRequest) -> Result<StartedTurn, InteractionError>;

    /// Request cancellation of one interaction turn and all linked active work.
    async fn cancel_turn(&self, turn_id: InteractionTurnId) -> Result<(), InteractionError>;

    /// Atomically update one interaction-scoped configuration property.
    async fn set_config_option(
        &self,
        conversation_id: ConversationId,
        update: ConfigUpdate,
    ) -> Result<InteractionConfig, InteractionError>;

    /// Approve one real pending effect request.
    async fn approve(
        &self,
        conversation_id: ConversationId,
        approval_id: ApprovalId,
    ) -> Result<(), InteractionError>;

    /// Deny one real pending effect request with an optional safe rationale.
    async fn deny(
        &self,
        conversation_id: ConversationId,
        approval_id: ApprovalId,
        reason: Option<String>,
    ) -> Result<(), InteractionError>;

    /// Replay durable events after the requested checkpoint, then follow a
    /// bounded live stream.
    async fn subscribe(
        &self,
        request: SubscriptionRequest,
    ) -> Result<BoxInteractionEventStream, InteractionError>;
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use polkagent_core::ids::ConversationId;

    use super::*;
    use crate::event::InteractionEvent;

    struct FakeStream {
        events: VecDeque<InteractionEventEnvelope>,
        checkpoint: Option<u64>,
    }

    #[async_trait]
    impl InteractionEventStream for FakeStream {
        async fn recv(&mut self) -> Result<InteractionEventEnvelope, StreamError> {
            let event = self.events.pop_front().ok_or(StreamError::Closed)?;
            self.checkpoint = Some(event.sequence);
            Ok(event)
        }

        fn checkpoint(&self) -> Option<u64> {
            self.checkpoint
        }
    }

    #[tokio::test]
    async fn stream_checkpoint_advances_only_after_delivery() {
        let event = InteractionEventEnvelope::new(
            ConversationId::new(),
            InteractionTurnId::new(),
            7,
            InteractionEvent::TurnTimedOut,
        )
        .expect("valid event");
        let mut stream = FakeStream {
            events: VecDeque::from([event.clone()]),
            checkpoint: None,
        };

        assert_eq!(stream.checkpoint(), None);
        assert_eq!(stream.recv().await, Ok(event));
        assert_eq!(stream.checkpoint(), Some(7));
        assert_eq!(stream.recv().await, Err(StreamError::Closed));
        assert_eq!(stream.checkpoint(), Some(7));
    }

    #[test]
    fn lag_error_exposes_explicit_recovery_checkpoint() {
        let error = StreamError::Lagged {
            last_seen_sequence: Some(4),
            resume_after_sequence: 9,
        };
        assert_eq!(
            error.to_string(),
            "interaction event stream lagged after sequence Some(4); reload and resume after 9"
        );
    }

    #[test]
    fn started_turn_debug_does_not_require_or_expose_backend_stream_details() {
        let handle = TurnHandle {
            turn_id: InteractionTurnId::new(),
            conversation_id: ConversationId::new(),
            run_ids: Vec::new(),
            first_event_sequence: 1,
        };
        let started = StartedTurn {
            handle: handle.clone(),
            events: Box::new(FakeStream {
                events: VecDeque::new(),
                checkpoint: None,
            }),
        };

        let debug = format!("{started:?}");
        assert!(debug.contains(&handle.turn_id.to_string()));
        assert!(debug.contains("<interaction event stream>"));
    }
}
