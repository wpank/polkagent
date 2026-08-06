//! Surface-neutral interaction service and replay-aware event stream traits.

use async_trait::async_trait;
use polkagent_core::ids::{ApprovalId, ConversationId, PrincipalId};
use std::path::Path;
use thiserror::Error;

use crate::error::InteractionError;
use crate::event::{ApprovalView, InteractionEventEnvelope};
use crate::ids::InteractionTurnId;
use crate::model::{
    ConfigUpdate, CreateInteractionRequest, InteractionConfig, InteractionSummary,
    InteractionTranscriptTurn, ListInteractionsRequest, PromptRequest, SubscriptionRequest,
    TranscriptRequest, TurnHandle, TurnSummary,
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

/// Authenticated, deployment-bound authority for approval operations.
///
/// Surface adapters must not construct this from request payload fields. The
/// composition root binds it after authenticating the caller; the durable
/// coordinator then revalidates every field against the stored approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionApprovalAuthority {
    /// Exact tenant authorization scope.
    pub tenant_id: String,
    /// Exact workspace authorization scope.
    pub workspace_id: String,
    /// Stable authenticated principal.
    pub principal_id: PrincipalId,
    /// Stable originating surface name used in decision audit records.
    pub surface: String,
}

impl InteractionApprovalAuthority {
    /// Validate the fail-closed fields required by an approval surface.
    pub fn validate(&self) -> Result<(), InteractionError> {
        for (field, value) in [
            ("approval tenant", self.tenant_id.as_str()),
            ("approval workspace", self.workspace_id.as_str()),
            ("approval surface", self.surface.as_str()),
        ] {
            if value.trim().is_empty() || value.len() > 128 {
                return Err(InteractionError::invalid_config(format!(
                    "{field} must be non-empty and at most 128 bytes"
                )));
            }
        }
        if self.principal_id.as_uuid().is_nil() {
            return Err(InteractionError::invalid_config(
                "approval principal must be a non-nil UUID",
            ));
        }
        Ok(())
    }
}

/// Derive the stable service principal used only for approval expiry and
/// cancellation from an explicitly configured human principal.
///
/// This versioned, domain-separated derivation is process-independent and is
/// never influenced by interaction or protocol request payloads.
#[must_use]
pub fn derive_approval_service_principal(principal_id: PrincipalId) -> PrincipalId {
    const DOMAIN: &[u8] = b"polkagent.approval-service-principal.v1\0";
    let mut hasher = blake3::Hasher::new();
    hasher.update(DOMAIN);
    hasher.update(principal_id.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    // Preserve deterministic separation even for the cryptographically
    // negligible nil/equal truncations. These branches also make the
    // composition invariant explicit and testable.
    if bytes == [0; 16] || bytes == *principal_id.as_bytes() {
        bytes[15] ^= 1;
    }
    PrincipalId::from_uuid(uuid::Uuid::from_bytes(bytes))
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

    /// Verify that a caller is operating from the interaction's exact durable
    /// origin before prompt execution or editor replay.
    async fn verify_interaction_origin(
        &self,
        _conversation_id: ConversationId,
        _working_directory: &Path,
    ) -> Result<(), InteractionError> {
        Err(InteractionError::new(
            crate::InteractionErrorCode::Unsupported,
            "durable interaction working-directory provenance is unavailable",
        ))
    }

    /// List durable turns for one interaction in ordinal order.
    async fn list_turns(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<TurnSummary>, InteractionError>;

    /// Load an ordinal page of exact, turn-correlated durable transcript text.
    ///
    /// The default preserves source compatibility for surfaces and test fakes
    /// that do not yet compose a transcript store. Production durable services
    /// should override it and fail closed on broken message correlation.
    async fn load_transcript(
        &self,
        _request: TranscriptRequest,
    ) -> Result<Vec<InteractionTranscriptTurn>, InteractionError> {
        Err(InteractionError::new(
            crate::InteractionErrorCode::Unsupported,
            "durable interaction transcript projection is unavailable",
        ))
    }

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

    /// List real pending approvals in this interaction's exact authority
    /// scope. Implementations without an authenticated authority fail closed.
    async fn list_pending_approvals(
        &self,
        _conversation_id: ConversationId,
    ) -> Result<Vec<ApprovalView>, InteractionError> {
        Err(InteractionError::new(
            crate::InteractionErrorCode::Unavailable,
            "an authenticated durable approval surface is not configured",
        ))
    }

    /// Approve one real pending effect request.
    async fn approve(
        &self,
        conversation_id: ConversationId,
        approval_id: ApprovalId,
    ) -> Result<ApprovalView, InteractionError>;

    /// Deny one real pending effect request with an optional safe rationale.
    async fn deny(
        &self,
        conversation_id: ConversationId,
        approval_id: ApprovalId,
        reason: Option<String>,
    ) -> Result<ApprovalView, InteractionError>;

    /// Replay durable events after the requested checkpoint, then follow a
    /// bounded live stream.
    async fn subscribe(
        &self,
        request: SubscriptionRequest,
    ) -> Result<BoxInteractionEventStream, InteractionError>;
}

#[cfg(test)]
mod tests {
    use std::collections::{HashSet, VecDeque};

    use polkagent_core::ids::{ConversationId, PrincipalId};

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

    #[test]
    fn approval_authority_rejects_nil_principal() {
        let authority = InteractionApprovalAuthority {
            tenant_id: "tenant".to_owned(),
            workspace_id: "workspace".to_owned(),
            principal_id: PrincipalId::from_uuid(uuid::Uuid::nil()),
            surface: "test".to_owned(),
        };
        let error = authority.validate().expect_err("nil must fail closed");
        assert_eq!(error.code, crate::InteractionErrorCode::InvalidConfig);
    }

    #[test]
    fn approval_service_principal_is_stable_separate_and_collision_free_for_sample() {
        let human = PrincipalId::new();
        let derived = derive_approval_service_principal(human);
        assert_eq!(derived, derive_approval_service_principal(human));
        assert_ne!(derived, human);
        assert!(!derived.as_uuid().is_nil());

        let outputs = (1..=4_096_u128)
            .map(|value| {
                derive_approval_service_principal(PrincipalId::from_uuid(uuid::Uuid::from_u128(
                    value,
                )))
            })
            .collect::<HashSet<_>>();
        assert_eq!(outputs.len(), 4_096);
        assert!(outputs.iter().all(|value| !value.as_uuid().is_nil()));
    }
}
