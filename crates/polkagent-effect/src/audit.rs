//! Audit integration for the effect pipeline.
//!
//! This module provides [`AuditedPipeline`], a wrapper around
//! [`EffectPipeline`] that automatically logs audit entries for every
//! effect execution lifecycle event:
//!
//! 1. **`EffectAttempted`** -- logged *before* execution begins.
//! 2. **`EffectCompleted`** -- logged after a successful outcome is recorded.
//! 3. **`EffectFailed`** -- logged after a failed outcome is recorded.
//!
//! ## Usage
//!
//! ```ignore
//! use polkagent_effect::audit::AuditedPipeline;
//!
//! let audited = AuditedPipeline::new(pipeline, audit_logger, "agent-1");
//! ```
//!
//! When no audit logger is configured the wrapper delegates directly to the
//! inner pipeline with zero overhead (the `Option<AuditLogger>` is `None`).

use polkagent_core::{EffectId, RunId};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

use polkagent_audit::{ActionOutcome, ActorInfo, AuditAction, AuditLogger, ResourceInfo};

use crate::claim::ClaimGuard;
use crate::error::PipelineError;
use crate::pipeline::{EffectIntentSpec, EffectPipeline};
use crate::types::{EffectAttempt, EffectKind, EffectOutcome, OutcomeResult};
use polkagent_store_trait::{EffectStore, StoredIntent};

// ---------------------------------------------------------------------------
// AuditContext
// ---------------------------------------------------------------------------

/// Contextual identifiers propagated into every audit entry produced by the
/// pipeline. These allow correlating audit records with a specific agent run.
#[derive(Debug, Clone)]
pub struct AuditContext {
    /// The agent that owns this pipeline instance.
    pub agent_id: String,
    /// The current run, if known. Set via [`AuditedPipeline::with_run_id`].
    pub run_id: Option<RunId>,
}

// ---------------------------------------------------------------------------
// AuditedPipeline
// ---------------------------------------------------------------------------

/// A wrapper around [`EffectPipeline`] that emits audit log entries for every
/// effect execution lifecycle event.
///
/// When `audit_logger` is `None`, all methods delegate directly to the inner
/// pipeline with no additional work, preserving backward compatibility.
#[derive(Clone)]
pub struct AuditedPipeline {
    inner: EffectPipeline,
    audit_logger: Option<AuditLogger>,
    ctx: AuditContext,
}

impl AuditedPipeline {
    /// Create a new audited pipeline wrapper.
    ///
    /// If `audit_logger` is `None`, audit logging is disabled and the wrapper
    /// is a transparent pass-through.
    #[must_use]
    pub fn new(
        inner: EffectPipeline,
        audit_logger: Option<AuditLogger>,
        agent_id: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            audit_logger,
            ctx: AuditContext {
                agent_id: agent_id.into(),
                run_id: None,
            },
        }
    }

    /// Set the run ID for subsequent audit entries.
    #[must_use]
    pub fn with_run_id(mut self, run_id: RunId) -> Self {
        self.ctx.run_id = Some(run_id);
        self
    }

    /// Return a reference to the underlying [`EffectPipeline`].
    #[must_use]
    pub fn inner(&self) -> &EffectPipeline {
        &self.inner
    }

    /// Return a reference to the underlying store.
    #[must_use]
    pub fn store(&self) -> &Arc<dyn EffectStore> {
        self.inner.store()
    }

    // -- Delegated pipeline methods ------------------------------------------

    /// Persist a new effect intent. Delegates directly to the inner pipeline.
    pub async fn propose(&self, spec: EffectIntentSpec) -> Result<EffectId, PipelineError> {
        self.inner.propose(spec).await
    }

    /// Claim the next pending intent (60 s default lease).
    pub async fn claim(&self) -> Result<Option<ClaimGuard>, PipelineError> {
        self.inner.claim().await
    }

    /// Claim with a custom lease duration.
    pub async fn claim_with_duration(
        &self,
        lease_duration: Duration,
    ) -> Result<Option<ClaimGuard>, PipelineError> {
        self.inner.claim_with_duration(lease_duration).await
    }

    /// Claim a specific intent by ID (recovery path).
    pub async fn claim_by_id(
        &self,
        intent_id: EffectId,
        lease_duration: Duration,
    ) -> Result<ClaimGuard, PipelineError> {
        self.inner.claim_by_id(intent_id, lease_duration).await
    }

    /// Fetch a single intent by ID.
    pub async fn get_intent(&self, intent_id: EffectId) -> Result<StoredIntent, PipelineError> {
        self.inner.get_intent(intent_id).await
    }

    // -- Audited lifecycle methods -------------------------------------------

    /// Record the start of an attempt, emitting an `EffectAttempted` audit
    /// entry *before* the inner call.
    pub async fn record_attempt(
        &self,
        intent_id: EffectId,
        attempt: &EffectAttempt,
        effect_kind: EffectKind,
    ) -> Result<(), PipelineError> {
        // Log EffectAttempted before delegating.
        self.log_effect_event(
            AuditAction::EffectAttempted,
            intent_id,
            effect_kind,
            ActionOutcome::Success,
            json!({
                "attempt_id": attempt.id.to_string(),
                "attempt_number": attempt.attempt_number,
                "worker_id": attempt.worker_id.to_string(),
            }),
        )
        .await;

        self.inner.record_attempt(intent_id, attempt).await
    }

    /// Record an outcome, emitting either `EffectCompleted` or `EffectFailed`
    /// after the inner call succeeds.
    pub async fn record_outcome(
        &self,
        intent_id: EffectId,
        outcome: &EffectOutcome,
        effect_kind: EffectKind,
    ) -> Result<(), PipelineError> {
        // Delegate first so that the outcome is durably stored.
        self.inner.record_outcome(intent_id, outcome).await?;

        // Determine action and audit outcome from the effect result.
        let (action, audit_outcome, context) = match &outcome.result {
            OutcomeResult::Success { data } => (
                AuditAction::EffectCompleted,
                ActionOutcome::Success,
                json!({
                    "outcome_id": outcome.id.to_string(),
                    "attempt_id": outcome.attempt_id.to_string(),
                    "data_keys": data.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()),
                }),
            ),
            OutcomeResult::Failure {
                error_class,
                message,
                retriable,
            } => (
                AuditAction::EffectFailed,
                ActionOutcome::Failure,
                json!({
                    "outcome_id": outcome.id.to_string(),
                    "attempt_id": outcome.attempt_id.to_string(),
                    "error_class": format!("{error_class:?}"),
                    "message": message,
                    "retriable": retriable,
                }),
            ),
            OutcomeResult::Timeout {
                waited_secs,
                partial_work_possible,
            } => (
                AuditAction::EffectFailed,
                ActionOutcome::Error,
                json!({
                    "outcome_id": outcome.id.to_string(),
                    "attempt_id": outcome.attempt_id.to_string(),
                    "failure_type": "timeout",
                    "waited_secs": waited_secs,
                    "partial_work_possible": partial_work_possible,
                }),
            ),
            OutcomeResult::Cancelled {
                partial_work_possible,
                reason,
            } => (
                AuditAction::EffectFailed,
                ActionOutcome::Failure,
                json!({
                    "outcome_id": outcome.id.to_string(),
                    "attempt_id": outcome.attempt_id.to_string(),
                    "failure_type": "cancelled",
                    "reason": format!("{reason:?}"),
                    "partial_work_possible": partial_work_possible,
                }),
            ),
            OutcomeResult::Unknown {
                context: ctx,
                resolution_hint,
            } => (
                AuditAction::EffectFailed,
                ActionOutcome::Error,
                json!({
                    "outcome_id": outcome.id.to_string(),
                    "attempt_id": outcome.attempt_id.to_string(),
                    "failure_type": "unknown",
                    "context": ctx,
                    "resolution_hint": format!("{resolution_hint:?}"),
                }),
            ),
        };

        self.log_effect_event(action, intent_id, effect_kind, audit_outcome, context)
            .await;

        Ok(())
    }

    // -- Internal helpers ----------------------------------------------------

    /// Log an effect lifecycle audit event.
    ///
    /// Errors from the audit logger are logged via `tracing::warn` but never
    /// propagated -- audit failures must not break the effect pipeline.
    async fn log_effect_event(
        &self,
        action: AuditAction,
        effect_id: EffectId,
        effect_kind: EffectKind,
        outcome: ActionOutcome,
        context: serde_json::Value,
    ) {
        let Some(logger) = &self.audit_logger else {
            return;
        };

        let actor = ActorInfo::agent(&self.ctx.agent_id);

        let resource = ResourceInfo::new("effect", effect_id.to_string())
            .with_description(effect_kind.discriminant_str());

        // Merge caller context with run/agent metadata.
        let full_context = json!({
            "agent_id": self.ctx.agent_id,
            "run_id": self.ctx.run_id.map(|r| r.to_string()),
            "effect_id": effect_id.to_string(),
            "effect_kind": effect_kind.discriminant_str(),
            "detail": context,
        });

        if let Err(e) = logger
            .log(actor, action, resource, outcome, full_context)
            .await
        {
            warn!(
                error = %e,
                effect_id = %effect_id,
                "failed to write audit entry for effect lifecycle event"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::tests::{make_in_memory_store, make_outcome, make_spec};
    use crate::types::{EffectKind, ErrorClass, OutcomeResult};
    use polkagent_audit::{AuditQuery, InMemoryAuditStore};
    use polkagent_core::{EffectAttemptId, EffectId, EffectOutcomeId, RunId, WorkerId};
    use std::sync::Arc;
    use std::time::Duration;

    /// Helper: build an `EffectAttempt` for test use.
    fn make_attempt(intent_id: EffectId, run_id: RunId, worker_id: WorkerId) -> EffectAttempt {
        use crate::types::AttemptState;
        use chrono::Utc;
        use polkagent_core::RetryClass;

        EffectAttempt {
            id: EffectAttemptId::new(),
            intent_id,
            run_id,
            attempt_number: 1,
            idempotency_key: crate::IdempotencyKey::generate(
                run_id,
                1,
                0,
                EffectKind::ModelCall,
                crate::IdempotencyKey::hash_params(b"test"),
            ),
            worker_id,
            lease_expires: Utc::now() + chrono::Duration::seconds(60),
            retry_class: RetryClass::Idempotent,
            state: AttemptState::Leased,
            created_at: Utc::now(),
            claimed_at: Utc::now(),
            started_at: None,
            completed_at: None,
        }
    }

    /// Helper: build a failed `EffectOutcome`.
    fn make_failed_outcome(
        intent_id: EffectId,
        attempt_id: EffectAttemptId,
        run_id: RunId,
    ) -> EffectOutcome {
        EffectOutcome {
            id: EffectOutcomeId::new(),
            attempt_id,
            intent_id,
            run_id,
            result: OutcomeResult::Failure {
                error_class: ErrorClass::ServerError,
                message: "internal error".to_string(),
                retriable: true,
            },
            observed_at: chrono::Utc::now(),
            digest: [0u8; 32],
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: Pipeline works without audit logger (backward compatible)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn pipeline_works_without_audit_logger() {
        let (store, _inner) = make_in_memory_store();
        let worker_id = WorkerId::new();
        let pipeline = EffectPipeline::new(Arc::clone(&store), worker_id);
        let audited = AuditedPipeline::new(pipeline, None, "agent-1");

        let run_id = RunId::new();
        let intent_id = audited
            .propose(make_spec(run_id, EffectKind::ModelCall))
            .await
            .expect("propose should succeed without audit logger");

        let guard = audited
            .claim_with_duration(Duration::from_secs(30))
            .await
            .expect("claim")
            .expect("guard");

        let attempt = make_attempt(intent_id, run_id, worker_id);
        audited
            .record_attempt(intent_id, &attempt, EffectKind::ModelCall)
            .await
            .expect("record_attempt should succeed without audit logger");

        let outcome = make_outcome(intent_id, attempt.id, run_id);
        audited
            .record_outcome(intent_id, &outcome, EffectKind::ModelCall)
            .await
            .expect("record_outcome should succeed without audit logger");

        // Prevent the guard from releasing the claim after outcome is recorded.
        std::mem::forget(guard);
    }

    // -----------------------------------------------------------------------
    // Test 2: Successful effect creates two audit entries (attempted + completed)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn successful_effect_creates_two_audit_entries() {
        let (store, _inner) = make_in_memory_store();
        let worker_id = WorkerId::new();
        let pipeline = EffectPipeline::new(Arc::clone(&store), worker_id);

        let audit_store = Arc::new(InMemoryAuditStore::new());
        let audit_logger = AuditLogger::new(audit_store.clone());
        let run_id = RunId::new();
        let audited = AuditedPipeline::new(pipeline, Some(audit_logger.clone()), "agent-1")
            .with_run_id(run_id);

        let intent_id = audited
            .propose(make_spec(run_id, EffectKind::ModelCall))
            .await
            .expect("propose");
        let guard = audited
            .claim_with_duration(Duration::from_secs(30))
            .await
            .expect("claim")
            .expect("guard");

        let attempt = make_attempt(intent_id, run_id, worker_id);
        audited
            .record_attempt(intent_id, &attempt, EffectKind::ModelCall)
            .await
            .expect("record_attempt");

        let outcome = make_outcome(intent_id, attempt.id, run_id);
        audited
            .record_outcome(intent_id, &outcome, EffectKind::ModelCall)
            .await
            .expect("record_outcome");

        let entries = audit_store.snapshot();
        assert_eq!(
            entries.len(),
            2,
            "expected exactly 2 audit entries (attempted + completed), got {}",
            entries.len()
        );
        assert_eq!(entries[0].action, AuditAction::EffectAttempted);
        assert_eq!(entries[1].action, AuditAction::EffectCompleted);

        std::mem::forget(guard);
    }

    // -----------------------------------------------------------------------
    // Test 3: Failed effect creates two audit entries (attempted + failed)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn failed_effect_creates_two_audit_entries() {
        let (store, _inner) = make_in_memory_store();
        let worker_id = WorkerId::new();
        let pipeline = EffectPipeline::new(Arc::clone(&store), worker_id);

        let audit_store = Arc::new(InMemoryAuditStore::new());
        let audit_logger = AuditLogger::new(audit_store.clone());
        let run_id = RunId::new();
        let audited = AuditedPipeline::new(pipeline, Some(audit_logger.clone()), "agent-1")
            .with_run_id(run_id);

        let intent_id = audited
            .propose(make_spec(run_id, EffectKind::ToolCall))
            .await
            .expect("propose");
        let guard = audited
            .claim_with_duration(Duration::from_secs(30))
            .await
            .expect("claim")
            .expect("guard");

        let attempt = make_attempt(intent_id, run_id, worker_id);
        audited
            .record_attempt(intent_id, &attempt, EffectKind::ToolCall)
            .await
            .expect("record_attempt");

        let outcome = make_failed_outcome(intent_id, attempt.id, run_id);
        audited
            .record_outcome(intent_id, &outcome, EffectKind::ToolCall)
            .await
            .expect("record_outcome");

        let entries = audit_store.snapshot();
        assert_eq!(
            entries.len(),
            2,
            "expected exactly 2 audit entries (attempted + failed), got {}",
            entries.len()
        );
        assert_eq!(entries[0].action, AuditAction::EffectAttempted);
        assert_eq!(entries[1].action, AuditAction::EffectFailed);

        std::mem::forget(guard);
    }

    // -----------------------------------------------------------------------
    // Test 4: Audit entries have correct action types
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn audit_entries_have_correct_action_types() {
        let (store, _inner) = make_in_memory_store();
        let worker_id = WorkerId::new();
        let pipeline = EffectPipeline::new(Arc::clone(&store), worker_id);

        let audit_store = Arc::new(InMemoryAuditStore::new());
        let audit_logger = AuditLogger::new(audit_store.clone());
        let run_id = RunId::new();
        let audited = AuditedPipeline::new(pipeline, Some(audit_logger.clone()), "agent-1")
            .with_run_id(run_id);

        // -- Successful effect --
        let intent_id = audited
            .propose(make_spec(run_id, EffectKind::ChainRead))
            .await
            .expect("propose");
        let guard = audited
            .claim_with_duration(Duration::from_secs(30))
            .await
            .expect("claim")
            .expect("guard");

        let attempt = make_attempt(intent_id, run_id, worker_id);
        audited
            .record_attempt(intent_id, &attempt, EffectKind::ChainRead)
            .await
            .expect("record_attempt");

        let outcome = make_outcome(intent_id, attempt.id, run_id);
        audited
            .record_outcome(intent_id, &outcome, EffectKind::ChainRead)
            .await
            .expect("record_outcome");

        std::mem::forget(guard);

        // Verify action types.
        let q_attempted = AuditQuery::new()
            .action(AuditAction::EffectAttempted)
            .build();
        let q_completed = AuditQuery::new()
            .action(AuditAction::EffectCompleted)
            .build();
        let q_failed = AuditQuery::new().action(AuditAction::EffectFailed).build();

        let attempted = audit_logger.query(&q_attempted).await.expect("query");
        let completed = audit_logger.query(&q_completed).await.expect("query");
        let failed = audit_logger.query(&q_failed).await.expect("query");

        assert_eq!(attempted.len(), 1, "should have 1 EffectAttempted entry");
        assert_eq!(completed.len(), 1, "should have 1 EffectCompleted entry");
        assert_eq!(failed.len(), 0, "should have 0 EffectFailed entries");

        // Verify outcomes on the entries.
        assert_eq!(attempted[0].outcome, ActionOutcome::Success);
        assert_eq!(completed[0].outcome, ActionOutcome::Success);
    }

    // -----------------------------------------------------------------------
    // Test 5: Agent/run context is propagated to audit entries
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn agent_run_context_propagated_to_audit_entries() {
        let (store, _inner) = make_in_memory_store();
        let worker_id = WorkerId::new();
        let pipeline = EffectPipeline::new(Arc::clone(&store), worker_id);

        let audit_store = Arc::new(InMemoryAuditStore::new());
        let audit_logger = AuditLogger::new(audit_store.clone());
        let run_id = RunId::new();
        let agent_id = "agent-ctx-test";
        let audited = AuditedPipeline::new(pipeline, Some(audit_logger.clone()), agent_id)
            .with_run_id(run_id);

        let intent_id = audited
            .propose(make_spec(run_id, EffectKind::Broadcast))
            .await
            .expect("propose");
        let guard = audited
            .claim_with_duration(Duration::from_secs(30))
            .await
            .expect("claim")
            .expect("guard");

        let attempt = make_attempt(intent_id, run_id, worker_id);
        audited
            .record_attempt(intent_id, &attempt, EffectKind::Broadcast)
            .await
            .expect("record_attempt");

        let outcome = make_outcome(intent_id, attempt.id, run_id);
        audited
            .record_outcome(intent_id, &outcome, EffectKind::Broadcast)
            .await
            .expect("record_outcome");

        std::mem::forget(guard);

        let entries = audit_store.snapshot();
        assert_eq!(entries.len(), 2);

        for entry in &entries {
            // Actor should be the agent.
            assert_eq!(entry.actor.id, agent_id);
            assert_eq!(entry.actor.actor_type, polkagent_audit::ActorType::Agent,);

            // Resource should reference the effect.
            assert_eq!(entry.resource.resource_type, "effect");
            assert_eq!(entry.resource.resource_id, intent_id.to_string());
            assert_eq!(entry.resource.description.as_deref(), Some("broadcast"),);

            // Context should contain agent_id, run_id, effect_id, effect_kind.
            let ctx = &entry.context;
            assert_eq!(ctx["agent_id"].as_str(), Some(agent_id));
            assert_eq!(ctx["run_id"].as_str(), Some(run_id.to_string().as_str()),);
            assert_eq!(
                ctx["effect_id"].as_str(),
                Some(intent_id.to_string().as_str()),
            );
            assert_eq!(ctx["effect_kind"].as_str(), Some("broadcast"));
        }
    }

    // -----------------------------------------------------------------------
    // Test 6: Effect kind is included in audit entries
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn effect_kind_included_in_audit_resource() {
        let (store, _inner) = make_in_memory_store();
        let worker_id = WorkerId::new();
        let pipeline = EffectPipeline::new(Arc::clone(&store), worker_id);

        let audit_store = Arc::new(InMemoryAuditStore::new());
        let audit_logger = AuditLogger::new(audit_store.clone());
        let run_id = RunId::new();
        let audited = AuditedPipeline::new(pipeline, Some(audit_logger), "agent-kind-test")
            .with_run_id(run_id);

        let intent_id = audited
            .propose(make_spec(run_id, EffectKind::SignatureRequest))
            .await
            .expect("propose");
        let guard = audited
            .claim_with_duration(Duration::from_secs(30))
            .await
            .expect("claim")
            .expect("guard");

        let attempt = make_attempt(intent_id, run_id, worker_id);
        audited
            .record_attempt(intent_id, &attempt, EffectKind::SignatureRequest)
            .await
            .expect("record_attempt");

        std::mem::forget(guard);

        let entries = audit_store.snapshot();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].resource.description.as_deref(),
            Some("signature_request"),
        );
        assert_eq!(
            entries[0].context["effect_kind"].as_str(),
            Some("signature_request"),
        );
    }

    // -----------------------------------------------------------------------
    // Test 7: Timeout outcome is logged as EffectFailed
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn timeout_outcome_logged_as_effect_failed() {
        let (store, _inner) = make_in_memory_store();
        let worker_id = WorkerId::new();
        let pipeline = EffectPipeline::new(Arc::clone(&store), worker_id);

        let audit_store = Arc::new(InMemoryAuditStore::new());
        let audit_logger = AuditLogger::new(audit_store.clone());
        let run_id = RunId::new();
        let audited =
            AuditedPipeline::new(pipeline, Some(audit_logger), "agent-timeout").with_run_id(run_id);

        let intent_id = audited
            .propose(make_spec(run_id, EffectKind::FinalityWatch))
            .await
            .expect("propose");
        let guard = audited
            .claim_with_duration(Duration::from_secs(30))
            .await
            .expect("claim")
            .expect("guard");

        let attempt = make_attempt(intent_id, run_id, worker_id);
        audited
            .record_attempt(intent_id, &attempt, EffectKind::FinalityWatch)
            .await
            .expect("record_attempt");

        let timeout_outcome = EffectOutcome {
            id: EffectOutcomeId::new(),
            attempt_id: attempt.id,
            intent_id,
            run_id,
            result: OutcomeResult::Timeout {
                waited_secs: 120,
                partial_work_possible: true,
            },
            observed_at: chrono::Utc::now(),
            digest: [0u8; 32],
        };

        audited
            .record_outcome(intent_id, &timeout_outcome, EffectKind::FinalityWatch)
            .await
            .expect("record_outcome");

        std::mem::forget(guard);

        let entries = audit_store.snapshot();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].action, AuditAction::EffectAttempted);
        assert_eq!(entries[1].action, AuditAction::EffectFailed);
        assert_eq!(entries[1].outcome, ActionOutcome::Error);
        assert_eq!(
            entries[1].context["detail"]["failure_type"].as_str(),
            Some("timeout"),
        );
    }
}
