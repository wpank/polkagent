//! Dead Letter Queue bridge for the effect pipeline.
//!
//! This module wires the [`polkagent_outbox::DeadLetterQueue`] into the effect
//! pipeline so that effects that exhaust their retry budget are moved to the DLQ
//! instead of being silently marked as permanently failed.
//!
//! ## Entry lifecycle
//!
//! ```text
//! Effect fails → retries exhausted? ─── no ──→ schedule retry
//!                       │
//!                      yes
//!                       │
//!                       ▼
//!              DLQ.enqueue(DeadLetter)
//!                  ┌──────────┐
//!                  │   DLQ    │
//!                  └────┬─────┘
//!                       │ operator/automated reprocess
//!                       ▼
//!              re-propose intent (back to pipeline)
//! ```
//!
//! ## Key type: [`EffectDeadLetterEntry`]
//!
//! Wraps the original effect intent data, error reason, retry count, and
//! timestamp into a format suitable for the DLQ.
//!
//! ## Key type: [`DlqPipeline`]
//!
//! A thin wrapper around [`crate::pipeline::EffectPipeline`] that holds an
//! optional [`DeadLetterQueue`] reference. When a DLQ is configured, failed
//! effects are dead-lettered; when absent, the pipeline behaves identically
//! to before (backward compatible).

use chrono::{DateTime, Utc};
use polkagent_core::EffectId;
use polkagent_outbox::dlq::{DeadLetter, DeadLetterQueue, DeliveryError, InMemoryDlq};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{info, warn};

use crate::error::PipelineError;
use crate::pipeline::EffectPipeline;

// ---------------------------------------------------------------------------
// EffectDeadLetterEntry
// ---------------------------------------------------------------------------

/// A snapshot of the failed effect data stored in a DLQ entry.
///
/// This captures all context needed to understand and reprocess the failure:
/// the original effect intent, the error reason, the retry count, and the
/// wall-clock time when the effect was dead-lettered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectDeadLetterEntry {
    /// The effect intent ID that failed.
    pub intent_id: EffectId,
    /// Serialised snapshot of the original intent payload.
    pub intent_payload: serde_json::Value,
    /// The kind of effect that failed.
    pub effect_kind: String,
    /// Human-readable reason for the final failure.
    pub error_reason: String,
    /// Total number of attempts made before dead-lettering.
    pub retry_count: u32,
    /// Wall-clock time when the effect was moved to the DLQ.
    pub dead_lettered_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// DlqPipeline
// ---------------------------------------------------------------------------

/// Wrapper around [`EffectPipeline`] that integrates dead-letter queue support.
///
/// When constructed with a DLQ (`with_dlq`), effects that exhaust their retry
/// budget are moved to the dead-letter queue. When constructed without one
/// (`new`), the pipeline behaves identically to the base `EffectPipeline` --
/// failed effects are simply marked as permanently failed in the store.
#[derive(Clone)]
pub struct DlqPipeline<Q: DeadLetterQueue + Clone + 'static = InMemoryDlq> {
    pipeline: EffectPipeline,
    dlq: Option<Q>,
}

impl<Q: DeadLetterQueue + Clone + 'static> DlqPipeline<Q> {
    /// Create a DLQ-aware pipeline with an explicit dead-letter queue.
    #[must_use]
    pub fn with_dlq(pipeline: EffectPipeline, dlq: Q) -> Self {
        Self {
            pipeline,
            dlq: Some(dlq),
        }
    }

    /// Create a DLQ-aware pipeline without a dead-letter queue (backward
    /// compatible -- behaves exactly like the base pipeline).
    #[must_use]
    pub fn without_dlq(pipeline: EffectPipeline) -> Self {
        Self {
            pipeline,
            dlq: None,
        }
    }

    /// Return a reference to the underlying [`EffectPipeline`].
    #[must_use]
    pub fn pipeline(&self) -> &EffectPipeline {
        &self.pipeline
    }

    /// Return a reference to the DLQ, if configured.
    #[must_use]
    pub fn dlq(&self) -> Option<&Q> {
        self.dlq.as_ref()
    }

    /// Return `true` if a DLQ is configured.
    #[must_use]
    pub fn has_dlq(&self) -> bool {
        self.dlq.is_some()
    }

    /// Move a failed effect to the dead-letter queue after retries have been
    /// exhausted.
    ///
    /// This method:
    /// 1. Fetches the stored intent from the effect store.
    /// 2. Constructs a [`DeadLetter`] containing the original intent payload,
    ///    the error reason, and the retry count.
    /// 3. Enqueues the dead letter into the DLQ.
    /// 4. Marks the intent as `"dead_lettered"` in the effect store.
    ///
    /// If no DLQ is configured, the intent is marked as `"permanently_failed"`
    /// in the store and no dead letter is created.
    ///
    /// # Errors
    ///
    /// Returns [`PipelineError::DeadLetterQueue`] if the DLQ enqueue fails, or
    /// [`PipelineError::Store`] if the store update fails.
    pub async fn dead_letter_failed_effect(
        &self,
        intent_id: EffectId,
        error_reason: &str,
        retry_count: u32,
    ) -> Result<(), PipelineError> {
        let dlq = match &self.dlq {
            Some(q) => q,
            None => {
                // No DLQ configured -- mark as permanently failed.
                self.pipeline
                    .store()
                    .update_intent_state(intent_id, "permanently_failed")
                    .await
                    .map_err(PipelineError::Store)?;

                warn!(
                    intent_id = %intent_id,
                    error_reason = error_reason,
                    retry_count = retry_count,
                    "effect permanently failed (no DLQ configured)"
                );
                return Ok(());
            }
        };

        // 1. Fetch the stored intent for payload snapshot.
        let stored = self.pipeline.get_intent(intent_id).await?;

        // 2. Build the dead-letter entry.
        let effect_kind = stored
            .payload
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let entry = EffectDeadLetterEntry {
            intent_id,
            intent_payload: stored.payload.clone(),
            effect_kind: effect_kind.clone(),
            error_reason: error_reason.to_string(),
            retry_count,
            dead_lettered_at: Utc::now(),
        };

        let mut letter = DeadLetter::new(
            intent_id.to_string(),
            serde_json::to_value(&entry).unwrap_or_else(|_| json!({"intent_id": intent_id.to_string()})),
            retry_count,
        );

        // Attach a delivery error so the error history is populated.
        letter.error_history.push(DeliveryError::new(
            "RETRIES_EXHAUSTED",
            error_reason,
            format!("effect:{effect_kind}"),
        ));

        // 3. Enqueue into the DLQ.
        dlq.enqueue(letter).await?;

        // 4. Mark the intent as dead-lettered in the store.
        self.pipeline
            .store()
            .update_intent_state(intent_id, "dead_lettered")
            .await
            .map_err(PipelineError::Store)?;

        info!(
            intent_id = %intent_id,
            effect_kind = effect_kind,
            retry_count = retry_count,
            "effect moved to dead-letter queue"
        );

        Ok(())
    }

    /// Reprocess DLQ entries by moving them back to the main pipeline queue.
    ///
    /// For each dead-letter entry (up to `limit`):
    /// 1. Dequeue it from the DLQ.
    /// 2. Extract the original intent payload.
    /// 3. Reset the intent state to `"pending"` in the store so it can be
    ///    re-claimed by a worker.
    ///
    /// Returns the number of entries successfully reprocessed.
    ///
    /// # Errors
    ///
    /// Returns [`PipelineError::DeadLetterQueue`] if the DLQ peek/dequeue
    /// fails, or [`PipelineError::Store`] if a store update fails. Partial
    /// successes are possible: some entries may be reprocessed before an error
    /// occurs.
    pub async fn reprocess_dlq_entries(&self, limit: usize) -> Result<usize, PipelineError> {
        let dlq = match &self.dlq {
            Some(q) => q,
            None => return Ok(0),
        };

        let entries = dlq.peek(limit).await?;
        let mut reprocessed = 0usize;

        for entry in &entries {
            // Parse the original intent ID from the dead letter.
            let intent_id_str = &entry.original_message_id;

            // Try to parse as EffectId (UUID).
            let intent_id: EffectId = match intent_id_str.parse() {
                Ok(id) => id,
                Err(_) => {
                    warn!(
                        dead_letter_id = %entry.id,
                        original_message_id = intent_id_str,
                        "cannot reprocess dead letter: invalid intent ID"
                    );
                    continue;
                }
            };

            // Reset intent state to pending in the store.
            match self
                .pipeline
                .store()
                .update_intent_state(intent_id, "pending")
                .await
            {
                Ok(_) => {
                    // Remove from DLQ.
                    dlq.dequeue(&entry.id).await?;
                    reprocessed += 1;

                    info!(
                        intent_id = %intent_id,
                        dead_letter_id = %entry.id,
                        "dead-letter entry reprocessed: intent returned to pending"
                    );
                }
                Err(e) => {
                    warn!(
                        intent_id = %intent_id,
                        error = %e,
                        "failed to reset intent state during reprocessing"
                    );
                }
            }
        }

        Ok(reprocessed)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::tests::{make_in_memory_store, make_spec};
    use crate::types::EffectKind;
    use polkagent_core::{RunId, WorkerId};
    use polkagent_outbox::dlq::InMemoryDlq;
    use polkagent_store_trait::EffectStore;
    use std::sync::Arc;
    use std::time::Duration;

    /// Helper: build an `EffectPipeline` and `InMemoryDlq` wired together.
    async fn setup() -> (
        DlqPipeline<InMemoryDlq>,
        Arc<dyn EffectStore>,
        InMemoryDlq,
    ) {
        let (store, _inner) = make_in_memory_store();
        let pipeline = EffectPipeline::new(Arc::clone(&store), WorkerId::new());
        let dlq = InMemoryDlq::with_defaults();
        let dlq_pipeline = DlqPipeline::with_dlq(pipeline, dlq.clone());
        (dlq_pipeline, store, dlq)
    }

    /// Helper: propose, claim, and get the intent ID.
    async fn propose_and_claim(
        pipeline: &EffectPipeline,
        kind: EffectKind,
    ) -> EffectId {
        let run_id = RunId::new();
        let intent_id = pipeline
            .propose(make_spec(run_id, kind))
            .await
            .expect("propose");
        let _guard = pipeline
            .claim_with_duration(Duration::from_secs(60))
            .await
            .expect("claim")
            .expect("guard");
        intent_id
    }

    // -------------------------------------------------------------------
    // Test 1: Failed effect goes to DLQ after max retries
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn failed_effect_goes_to_dlq_after_max_retries() {
        let (dlq_pipeline, store, dlq) = setup().await;
        let intent_id = propose_and_claim(dlq_pipeline.pipeline(), EffectKind::ModelCall).await;

        // Simulate exhausting retries and dead-lettering.
        dlq_pipeline
            .dead_letter_failed_effect(intent_id, "connection refused after 3 attempts", 3)
            .await
            .expect("dead_letter");

        // The DLQ should now contain exactly one entry.
        let count = dlq.count().await.expect("count");
        assert_eq!(count, 1, "DLQ should contain exactly one entry");

        // The intent should be marked as dead-lettered in the store.
        let stored = store.get_intent(intent_id).await.expect("get_intent");
        assert_eq!(stored.state, "dead_lettered");
    }

    // -------------------------------------------------------------------
    // Test 2: DLQ preserves original effect data
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn dlq_preserves_original_effect_data() {
        let (dlq_pipeline, _store, dlq) = setup().await;
        let intent_id = propose_and_claim(dlq_pipeline.pipeline(), EffectKind::ChainRead).await;

        dlq_pipeline
            .dead_letter_failed_effect(intent_id, "timeout after 5s", 2)
            .await
            .expect("dead_letter");

        // Peek at the DLQ entry.
        let entries = dlq.peek(10).await.expect("peek");
        assert_eq!(entries.len(), 1);

        let letter = &entries[0];

        // Verify the original message ID matches the intent ID.
        assert_eq!(letter.original_message_id, intent_id.to_string());

        // Verify the attempt count was recorded.
        assert_eq!(letter.attempt_count, 2);

        // The payload should contain our EffectDeadLetterEntry.
        let entry: EffectDeadLetterEntry =
            serde_json::from_value(letter.payload.clone()).expect("deserialize entry");
        assert_eq!(entry.intent_id, intent_id);
        assert_eq!(entry.error_reason, "timeout after 5s");
        assert_eq!(entry.retry_count, 2);
        assert_eq!(entry.effect_kind, "chain_read");

        // The intent_payload should contain the original spec data.
        assert!(entry.intent_payload.get("kind").is_some());

        // Error history should have the final delivery error.
        assert_eq!(letter.error_history.len(), 1);
        assert_eq!(letter.error_history[0].error_code, "RETRIES_EXHAUSTED");
        assert_eq!(letter.error_history[0].error_message, "timeout after 5s");
    }

    // -------------------------------------------------------------------
    // Test 3: DLQ entries can be listed
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn dlq_entries_can_be_listed() {
        let (dlq_pipeline, _store, dlq) = setup().await;

        // Dead-letter three effects.
        for _ in 0..3 {
            let intent_id =
                propose_and_claim(dlq_pipeline.pipeline(), EffectKind::ToolCall).await;
            dlq_pipeline
                .dead_letter_failed_effect(intent_id, "service unavailable", 3)
                .await
                .expect("dead_letter");
        }

        let count = dlq.count().await.expect("count");
        assert_eq!(count, 3, "DLQ should contain three entries");

        let entries = dlq.peek(10).await.expect("peek");
        assert_eq!(entries.len(), 3);

        // Each entry should have a distinct original_message_id.
        let ids: Vec<_> = entries.iter().map(|e| &e.original_message_id).collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(
            unique.len(),
            3,
            "all DLQ entries should have distinct intent IDs"
        );
    }

    // -------------------------------------------------------------------
    // Test 4: DLQ entries can be reprocessed
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn dlq_entries_can_be_reprocessed() {
        let (dlq_pipeline, store, dlq) = setup().await;
        let intent_id = propose_and_claim(dlq_pipeline.pipeline(), EffectKind::ModelCall).await;

        // Dead-letter the effect.
        dlq_pipeline
            .dead_letter_failed_effect(intent_id, "server error", 3)
            .await
            .expect("dead_letter");

        // Confirm it's in the DLQ.
        assert_eq!(dlq.count().await.expect("count"), 1);

        // Reprocess it.
        let reprocessed = dlq_pipeline.reprocess_dlq_entries(10).await.expect("reprocess");
        assert_eq!(reprocessed, 1);

        // The DLQ should now be empty.
        assert_eq!(dlq.count().await.expect("count"), 0);

        // The intent should be back to pending in the store.
        let stored = store.get_intent(intent_id).await.expect("get_intent");
        assert_eq!(
            stored.state, "pending",
            "reprocessed intent should be in pending state"
        );
    }

    // -------------------------------------------------------------------
    // Test 5: Pipeline works without DLQ (backward compatible)
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn pipeline_works_without_dlq_backward_compatible() {
        let (store, _inner) = make_in_memory_store();
        let pipeline = EffectPipeline::new(Arc::clone(&store), WorkerId::new());
        let dlq_pipeline: DlqPipeline<InMemoryDlq> = DlqPipeline::without_dlq(pipeline);

        assert!(!dlq_pipeline.has_dlq(), "DLQ should not be configured");

        let intent_id =
            propose_and_claim(dlq_pipeline.pipeline(), EffectKind::Delivery).await;

        // Dead-lettering without a DLQ should mark as permanently_failed.
        dlq_pipeline
            .dead_letter_failed_effect(intent_id, "network error", 3)
            .await
            .expect("should succeed without DLQ");

        let stored = store.get_intent(intent_id).await.expect("get_intent");
        assert_eq!(
            stored.state, "permanently_failed",
            "without DLQ, intent should be marked permanently_failed"
        );

        // Reprocess should be a no-op.
        let reprocessed = dlq_pipeline
            .reprocess_dlq_entries(10)
            .await
            .expect("reprocess");
        assert_eq!(reprocessed, 0, "no-op when DLQ is not configured");
    }

    // -------------------------------------------------------------------
    // Test 6: DLQ integrity is maintained across operations
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn dlq_integrity_maintained_across_operations() {
        let (dlq_pipeline, _store, dlq) = setup().await;

        // Dead-letter two effects.
        let id1 = propose_and_claim(dlq_pipeline.pipeline(), EffectKind::ModelCall).await;
        let id2 = propose_and_claim(dlq_pipeline.pipeline(), EffectKind::ToolCall).await;

        dlq_pipeline
            .dead_letter_failed_effect(id1, "error A", 3)
            .await
            .expect("dl1");
        dlq_pipeline
            .dead_letter_failed_effect(id2, "error B", 5)
            .await
            .expect("dl2");

        assert_eq!(dlq.count().await.expect("count"), 2);

        // Reprocess only one (limit=1).
        let reprocessed = dlq_pipeline.reprocess_dlq_entries(1).await.expect("reprocess");
        assert_eq!(reprocessed, 1);

        // One should remain in the DLQ.
        assert_eq!(dlq.count().await.expect("count"), 1);

        // The remaining entry should still be intact with the correct data.
        let remaining = dlq.peek(1).await.expect("peek");
        assert_eq!(remaining.len(), 1);

        let letter = &remaining[0];
        let entry: EffectDeadLetterEntry =
            serde_json::from_value(letter.payload.clone()).expect("deserialize");

        // Verify the remaining entry has consistent data.
        assert_eq!(entry.intent_id.to_string(), letter.original_message_id);
        assert!(
            entry.retry_count > 0,
            "retry count should be preserved"
        );
        assert!(
            !entry.error_reason.is_empty(),
            "error reason should be preserved"
        );
        assert!(
            !letter.error_history.is_empty(),
            "error history should be preserved"
        );
    }

    // -------------------------------------------------------------------
    // Test 7: EffectDeadLetterEntry serde round-trip
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn effect_dead_letter_entry_serde_round_trip() {
        let entry = EffectDeadLetterEntry {
            intent_id: EffectId::new(),
            intent_payload: json!({"kind": "model_call", "params": {"model": "gpt-4"}}),
            effect_kind: "model_call".to_string(),
            error_reason: "rate limit exceeded".to_string(),
            retry_count: 3,
            dead_lettered_at: Utc::now(),
        };

        let json_str = serde_json::to_string(&entry).expect("serialize");
        let back: EffectDeadLetterEntry =
            serde_json::from_str(&json_str).expect("deserialize");

        assert_eq!(back.intent_id, entry.intent_id);
        assert_eq!(back.effect_kind, "model_call");
        assert_eq!(back.error_reason, "rate limit exceeded");
        assert_eq!(back.retry_count, 3);
    }

    // -------------------------------------------------------------------
    // Test 8: Multiple reprocess cycles work correctly
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn multiple_reprocess_cycles() {
        let (dlq_pipeline, store, dlq) = setup().await;
        let intent_id = propose_and_claim(dlq_pipeline.pipeline(), EffectKind::Simulation).await;

        // First failure -> DLQ.
        dlq_pipeline
            .dead_letter_failed_effect(intent_id, "timeout", 3)
            .await
            .expect("dl");
        assert_eq!(dlq.count().await.expect("count"), 1);

        // Reprocess -> back to pending.
        let n = dlq_pipeline.reprocess_dlq_entries(10).await.expect("rp1");
        assert_eq!(n, 1);
        assert_eq!(dlq.count().await.expect("count"), 0);

        let stored = store.get_intent(intent_id).await.expect("get");
        assert_eq!(stored.state, "pending");

        // Second failure -> DLQ again.
        dlq_pipeline
            .dead_letter_failed_effect(intent_id, "timeout again", 4)
            .await
            .expect("dl2");
        assert_eq!(dlq.count().await.expect("count"), 1);

        // The intent should now be dead-lettered again.
        let stored = store.get_intent(intent_id).await.expect("get");
        assert_eq!(stored.state, "dead_lettered");
    }
}
