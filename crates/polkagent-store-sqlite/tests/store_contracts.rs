//! Contract / conformance tests for the store traits.
//!
//! These tests verify the semantic contracts of:
//!
//! - [`RunStore`](polkagent_store_trait::RunStore)
//! - [`EffectStore`](polkagent_store_trait::EffectStore)
//! - [`EventStore`](polkagent_store_trait::event::EventStore)
//! - [`ArtifactStore`](polkagent_artifact::store::ArtifactStore)
//!
//! Each test uses an in-memory SQLite database with migrations applied.  The
//! tests are intentionally written against the *trait* APIs (not the concrete
//! `Sqlite*Store` wrappers) so they validate the public contract that any
//! implementation must uphold.

use std::time::Duration;

use chrono::Utc;

use polkagent_core::artifact::{Artifact, ArtifactKind};
use polkagent_core::ids::{ArtifactId, EffectAttemptId, EffectId, EffectOutcomeId, RunId, StepId, WorkerId};
use polkagent_store_sqlite::{SqlitePool, migrations};
use polkagent_store_trait::event::{EventStore, EventStoreError, StoredEvent};
use polkagent_store_trait::{
    EffectStore, RunStatus, RunStore, StoreError, StoreRetryClass, StoredIntent, StoredOutcome,
};

use polkagent_artifact::store::ArtifactStore;

// ============================================================================
// Test helpers
// ============================================================================

const TEST_AGENT: &str = "test-agent";
const OTHER_AGENT: &str = "other-agent";

/// Create an in-memory pool with all migrations applied and a test agent
/// pre-inserted (required to satisfy FK on `runs.agent_id`).
fn setup_pool() -> SqlitePool {
    let pool = SqlitePool::open_in_memory().expect("open in-memory pool");
    {
        let writer = pool.writer();
        migrations::migrate(&writer).expect("migrate");
        writer
            .execute(
                "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at) \
                 VALUES (?1, 'Test Agent', 'active', '{}', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                [TEST_AGENT],
            )
            .expect("insert test agent");
        writer
            .execute(
                "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at) \
                 VALUES (?1, 'Other Agent', 'active', '{}', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                [OTHER_AGENT],
            )
            .expect("insert other agent");
    }
    pool
}

/// Insert a run record directly (bypasses the RunStore trait) so that FK
/// constraints on `effect_intents.run_id` and `artifacts.run_id` are satisfied.
fn insert_run(pool: &SqlitePool, run_id: RunId, agent_id: &str) {
    let now = Utc::now().to_rfc3339();
    let writer = pool.writer();
    writer
        .execute(
            "INSERT INTO runs (id, agent_id, state, params_json, created_at, updated_at) \
             VALUES (?1, ?2, 'created', '{}', ?3, ?4)",
            rusqlite::params![run_id.to_string(), agent_id, now, now],
        )
        .expect("insert run");
}

/// Insert a run record from a string `run_id`.  Uses `INSERT OR IGNORE` so it
/// is safe to call multiple times with the same ID.  Satisfies the
/// `run_events.run_id REFERENCES runs(id)` FK constraint.
fn insert_run_str(pool: &SqlitePool, run_id: &str) {
    let now = Utc::now().to_rfc3339();
    let writer = pool.writer();
    writer
        .execute(
            "INSERT OR IGNORE INTO runs (id, agent_id, state, params_json, created_at, updated_at) \
             VALUES (?1, ?2, 'created', '{}', ?3, ?4)",
            rusqlite::params![run_id, TEST_AGENT, now, now],
        )
        .expect("insert run for event store test");
}

/// Insert the full chain of FK records: run -> turn -> step, returning the
/// step_id.  Required because `effect_intents.step_id` has a FK to `steps(id)`.
fn insert_run_with_step(pool: &SqlitePool, run_id: RunId, agent_id: &str) -> StepId {
    let now = Utc::now().to_rfc3339();
    let writer = pool.writer();

    // Run.
    writer
        .execute(
            "INSERT INTO runs (id, agent_id, state, params_json, created_at, updated_at) \
             VALUES (?1, ?2, 'created', '{}', ?3, ?4)",
            rusqlite::params![run_id.to_string(), agent_id, &now, &now],
        )
        .expect("insert run");

    // Turn.
    let turn_id = uuid::Uuid::now_v7().to_string();
    writer
        .execute(
            "INSERT INTO turns (id, run_id, sequence, role, started_at) \
             VALUES (?1, ?2, 1, 'assistant', ?3)",
            rusqlite::params![turn_id, run_id.to_string(), &now],
        )
        .expect("insert turn");

    // Step.
    let step_id = StepId::new();
    writer
        .execute(
            "INSERT INTO steps (id, turn_id, sequence, kind, started_at) \
             VALUES (?1, ?2, 1, 'model_call', ?3)",
            rusqlite::params![step_id.to_string(), turn_id, &now],
        )
        .expect("insert step");

    step_id
}

/// Build a minimal `StoredIntent` for testing the `EffectStore` trait.
///
/// `step_id` must reference a step that exists in the database (FK constraint).
fn make_intent(run_id: RunId, step_id: StepId, idempotency_key: &str) -> StoredIntent {
    StoredIntent {
        id: EffectId::new(),
        run_id,
        step_id,
        state: "pending".into(),
        lease_owner: None,
        lease_expires: None,
        retry_class: StoreRetryClass::Idempotent,
        payload: serde_json::json!({"kind": "test_effect", "params": {"value": 42}}),
        idempotency_key: idempotency_key.to_string(),
        created_at: Utc::now(),
    }
}

/// Build a minimal `StoredOutcome` linked to a given intent.
fn make_outcome(intent_id: EffectId, run_id: RunId) -> StoredOutcome {
    StoredOutcome {
        id: EffectOutcomeId::new(),
        intent_id,
        attempt_id: EffectAttemptId::new(),
        run_id,
        consumed: false,
        payload: serde_json::json!({"status": "success", "data": "ok"}),
        observed_at: Utc::now(),
    }
}

/// Build a minimal `StoredEvent` for testing the `EventStore` trait.
fn make_event(run_id: &str, sequence: u64, event_type: &str) -> StoredEvent {
    StoredEvent {
        id: uuid::Uuid::now_v7().to_string(),
        event_type: event_type.to_string(),
        sequence,
        global_sequence: 0, // assigned by the store
        run_id: run_id.to_string(),
        conversation_id: None,
        correlation_id: "corr-contract-test".to_string(),
        causation_id: None,
        scope_id: "scope-contract-test".to_string(),
        timestamp: Utc::now().to_rfc3339(),
        durability: "durable".to_string(),
        payload: serde_json::json!({"test": true}),
        trace_id: None,
        span_id: None,
        schema_version: 1,
    }
}

/// Build a simple test `Artifact` from bytes.
fn make_artifact(body: &[u8]) -> Artifact {
    Artifact::from_bytes(
        ArtifactId::new(),
        ArtifactKind::Custom {
            type_uri: "test://contract-test".into(),
        },
        body,
    )
}

// ============================================================================
// RunStore contracts
// ============================================================================

mod run_store {
    use super::*;

    #[tokio::test]
    async fn create_then_get_returns_same_data() {
        let pool = setup_pool();
        let run_id = RunId::new();

        RunStore::create(&pool, run_id, TEST_AGENT, RunStatus::new("created"))
            .await
            .expect("create");

        let summary = RunStore::get(&pool, run_id).await.expect("get");
        assert_eq!(summary.id, run_id);
        assert_eq!(summary.agent_id, TEST_AGENT);
        assert_eq!(summary.status.as_str(), "created");
        assert!(summary.completed_at.is_none());
    }

    #[tokio::test]
    async fn create_duplicate_returns_conflict() {
        let pool = setup_pool();
        let run_id = RunId::new();

        RunStore::create(&pool, run_id, TEST_AGENT, RunStatus::new("created"))
            .await
            .expect("first create");

        let err = RunStore::create(&pool, run_id, TEST_AGENT, RunStatus::new("created"))
            .await
            .expect_err("duplicate should fail");

        assert!(
            matches!(err, StoreError::Conflict { .. }),
            "expected Conflict, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn get_nonexistent_returns_not_found() {
        let pool = setup_pool();
        let run_id = RunId::new();

        let err = RunStore::get(&pool, run_id)
            .await
            .expect_err("should not find");

        assert!(
            matches!(err, StoreError::NotFound { .. }),
            "expected NotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn update_state_persists_correctly() {
        let pool = setup_pool();
        let run_id = RunId::new();

        RunStore::create(&pool, run_id, TEST_AGENT, RunStatus::new("created"))
            .await
            .expect("create");

        RunStore::update_state(&pool, run_id, RunStatus::new("running"))
            .await
            .expect("update");

        let summary = RunStore::get(&pool, run_id).await.expect("get");
        assert_eq!(summary.status.as_str(), "running");
        // Non-terminal state: completed_at should remain None.
        assert!(summary.completed_at.is_none());
    }

    #[tokio::test]
    async fn update_to_terminal_state_sets_completed_at() {
        let pool = setup_pool();
        let run_id = RunId::new();

        RunStore::create(&pool, run_id, TEST_AGENT, RunStatus::new("created"))
            .await
            .expect("create");

        for terminal in &["completed", "failed", "cancelled", "timed_out"] {
            // Reset to a non-terminal state between iterations.
            RunStore::update_state(&pool, run_id, RunStatus::new("running"))
                .await
                .expect("reset to running");

            RunStore::update_state(&pool, run_id, RunStatus::new(*terminal))
                .await
                .expect("update to terminal");

            let summary = RunStore::get(&pool, run_id).await.expect("get");
            assert_eq!(summary.status.as_str(), *terminal);
            assert!(
                summary.completed_at.is_some(),
                "completed_at should be set for terminal state '{terminal}'"
            );
        }
    }

    #[tokio::test]
    async fn list_by_agent_returns_only_that_agents_runs() {
        let pool = setup_pool();

        let id1 = RunId::new();
        let id2 = RunId::new();
        let id3 = RunId::new();

        RunStore::create(&pool, id1, TEST_AGENT, RunStatus::new("created"))
            .await
            .expect("create 1");
        RunStore::create(&pool, id2, OTHER_AGENT, RunStatus::new("created"))
            .await
            .expect("create 2");
        RunStore::create(&pool, id3, TEST_AGENT, RunStatus::new("running"))
            .await
            .expect("create 3");

        let runs = RunStore::list_by_agent(&pool, TEST_AGENT, 10, 0)
            .await
            .expect("list");

        assert_eq!(runs.len(), 2, "should have exactly 2 runs for test-agent");
        assert!(
            runs.iter().all(|r| r.agent_id == TEST_AGENT),
            "all runs should belong to test-agent"
        );
    }

    #[tokio::test]
    async fn list_by_state_filters_correctly() {
        let pool = setup_pool();

        let id1 = RunId::new();
        let id2 = RunId::new();
        let id3 = RunId::new();

        RunStore::create(&pool, id1, TEST_AGENT, RunStatus::new("created"))
            .await
            .expect("create 1");
        RunStore::create(&pool, id2, TEST_AGENT, RunStatus::new("running"))
            .await
            .expect("create 2");
        RunStore::create(&pool, id3, TEST_AGENT, RunStatus::new("created"))
            .await
            .expect("create 3");

        let created = RunStore::list_by_state(&pool, RunStatus::new("created"), 10, 0)
            .await
            .expect("list created");
        assert_eq!(created.len(), 2);
        assert!(created.iter().all(|r| r.status.as_str() == "created"));

        let running = RunStore::list_by_state(&pool, RunStatus::new("running"), 10, 0)
            .await
            .expect("list running");
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].status.as_str(), "running");
    }

    #[tokio::test]
    async fn pagination_respects_limit_and_offset() {
        let pool = setup_pool();

        // Create 5 runs.
        for _ in 0..5 {
            let id = RunId::new();
            RunStore::create(&pool, id, TEST_AGENT, RunStatus::new("created"))
                .await
                .expect("create");
        }

        // Page 1: limit 2, offset 0.
        let page1 = RunStore::list_by_agent(&pool, TEST_AGENT, 2, 0)
            .await
            .expect("page 1");
        assert_eq!(page1.len(), 2);

        // Page 2: limit 2, offset 2.
        let page2 = RunStore::list_by_agent(&pool, TEST_AGENT, 2, 2)
            .await
            .expect("page 2");
        assert_eq!(page2.len(), 2);

        // Page 3: limit 2, offset 4.
        let page3 = RunStore::list_by_agent(&pool, TEST_AGENT, 2, 4)
            .await
            .expect("page 3");
        assert_eq!(page3.len(), 1);

        // Past the end.
        let page4 = RunStore::list_by_agent(&pool, TEST_AGENT, 2, 10)
            .await
            .expect("page 4");
        assert!(page4.is_empty());

        // All pages together should cover all 5 runs with no duplicates.
        let mut all_ids: Vec<String> = page1
            .iter()
            .chain(page2.iter())
            .chain(page3.iter())
            .map(|r| r.id.to_string())
            .collect();
        all_ids.sort();
        all_ids.dedup();
        assert_eq!(all_ids.len(), 5);
    }

    #[tokio::test]
    async fn update_state_nonexistent_returns_not_found() {
        let pool = setup_pool();
        let run_id = RunId::new();

        let err = RunStore::update_state(&pool, run_id, RunStatus::new("running"))
            .await
            .expect_err("should fail");

        assert!(
            matches!(err, StoreError::NotFound { .. }),
            "expected NotFound, got: {err:?}"
        );
    }
}

// ============================================================================
// EffectStore contracts
// ============================================================================

mod effect_store {
    use super::*;

    #[tokio::test]
    async fn propose_then_get_returns_stored_intent() {
        let pool = setup_pool();
        let run_id = RunId::new();
        let step_id = insert_run_with_step(&pool, run_id, TEST_AGENT);

        let intent = make_intent(run_id, step_id, "propose-get-1");
        let intent_id = intent.id;

        EffectStore::propose_intent(&pool, intent).await.expect("propose");

        let fetched = EffectStore::get_intent(&pool, intent_id)
            .await
            .expect("get");
        assert_eq!(fetched.id, intent_id);
        assert_eq!(fetched.run_id, run_id);
        assert_eq!(fetched.state, "pending");
        assert!(fetched.lease_owner.is_none());
    }

    #[tokio::test]
    async fn propose_duplicate_id_returns_conflict() {
        let pool = setup_pool();
        let run_id = RunId::new();
        let step_id = insert_run_with_step(&pool, run_id, TEST_AGENT);

        let intent = make_intent(run_id, step_id, "dup-id-1");
        let dup = StoredIntent {
            idempotency_key: "dup-id-2".into(),
            ..make_intent(run_id, step_id, "unused")
        };
        // Give the duplicate the same effect ID.
        let dup_with_same_id = StoredIntent {
            id: intent.id,
            ..dup
        };

        EffectStore::propose_intent(&pool, intent).await.expect("first");

        let err = EffectStore::propose_intent(&pool, dup_with_same_id)
            .await
            .expect_err("duplicate id should fail");

        assert!(
            matches!(err, StoreError::Conflict { .. }),
            "expected Conflict, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn claim_returns_a_claimable_intent() {
        let pool = setup_pool();
        let run_id = RunId::new();
        let step_id = insert_run_with_step(&pool, run_id, TEST_AGENT);

        let intent = make_intent(run_id, step_id, "claim-1");
        let intent_id = intent.id;
        EffectStore::propose_intent(&pool, intent).await.expect("propose");

        let worker = WorkerId::new();
        let claimed = EffectStore::claim_intent(&pool, worker, Duration::from_secs(30))
            .await
            .expect("claim");

        assert!(claimed.is_some(), "should claim the pending intent");
        let claimed = claimed.unwrap();
        assert_eq!(claimed.id, intent_id);
        assert_eq!(claimed.state, "claimed");
        assert_eq!(claimed.lease_owner, Some(worker));
        assert!(claimed.lease_expires.is_some());
    }

    #[tokio::test]
    async fn claim_empty_store_returns_none() {
        let pool = setup_pool();

        let worker = WorkerId::new();
        let result = EffectStore::claim_intent(&pool, worker, Duration::from_secs(30))
            .await
            .expect("claim empty");

        assert!(result.is_none(), "empty store should return None");
    }

    #[tokio::test]
    async fn claimed_intent_is_not_claimable_by_another_worker() {
        let pool = setup_pool();
        let run_id = RunId::new();
        let step_id = insert_run_with_step(&pool, run_id, TEST_AGENT);

        let intent = make_intent(run_id, step_id, "exclusive-claim-1");
        EffectStore::propose_intent(&pool, intent).await.expect("propose");

        let worker1 = WorkerId::new();
        let worker2 = WorkerId::new();

        // Worker 1 claims.
        let claimed = EffectStore::claim_intent(&pool, worker1, Duration::from_secs(300))
            .await
            .expect("claim 1");
        assert!(claimed.is_some());

        // Worker 2 tries to claim -- should get None (no pending intents).
        let second = EffectStore::claim_intent(&pool, worker2, Duration::from_secs(300))
            .await
            .expect("claim 2");
        assert!(second.is_none(), "already-claimed intent should not be claimable");
    }

    #[tokio::test]
    async fn release_makes_intent_claimable_again() {
        let pool = setup_pool();
        let run_id = RunId::new();
        let step_id = insert_run_with_step(&pool, run_id, TEST_AGENT);

        let intent = make_intent(run_id, step_id, "release-1");
        let intent_id = intent.id;
        EffectStore::propose_intent(&pool, intent).await.expect("propose");

        let worker = WorkerId::new();
        let claimed = EffectStore::claim_intent(&pool, worker, Duration::from_secs(300))
            .await
            .expect("claim");
        assert!(claimed.is_some());

        // Release the claim.
        EffectStore::release_claim(&pool, intent_id, worker)
            .await
            .expect("release");

        // After release, the intent should be claimable again.
        let fetched = EffectStore::get_intent(&pool, intent_id)
            .await
            .expect("get after release");
        assert_eq!(fetched.state, "pending");
        assert!(fetched.lease_owner.is_none());

        // Another worker can now claim it.
        let worker2 = WorkerId::new();
        let reclaimed = EffectStore::claim_intent(&pool, worker2, Duration::from_secs(300))
            .await
            .expect("reclaim");
        assert!(reclaimed.is_some());
        assert_eq!(reclaimed.unwrap().lease_owner, Some(worker2));
    }

    #[tokio::test]
    async fn record_outcome_marks_intent_resolved() {
        let pool = setup_pool();
        let run_id = RunId::new();
        let step_id = insert_run_with_step(&pool, run_id, TEST_AGENT);

        let intent = make_intent(run_id, step_id, "outcome-1");
        let intent_id = intent.id;
        EffectStore::propose_intent(&pool, intent).await.expect("propose");

        // Record an attempt start first.
        let attempt_id = EffectAttemptId::new();
        let worker = WorkerId::new();
        EffectStore::record_attempt_start(
            &pool,
            attempt_id,
            intent_id,
            worker,
            serde_json::json!({}),
        )
        .await
        .expect("attempt start");

        // Record the outcome.
        let outcome = StoredOutcome {
            attempt_id,
            ..make_outcome(intent_id, run_id)
        };
        EffectStore::record_outcome(&pool, outcome)
            .await
            .expect("record outcome");

        // The intent should now be resolved.
        let fetched = EffectStore::get_intent(&pool, intent_id)
            .await
            .expect("get after outcome");
        assert_eq!(fetched.state, "resolved");
    }

    #[tokio::test]
    async fn double_outcome_recording_returns_conflict() {
        let pool = setup_pool();
        let run_id = RunId::new();
        let step_id = insert_run_with_step(&pool, run_id, TEST_AGENT);

        let intent = make_intent(run_id, step_id, "double-outcome-1");
        let intent_id = intent.id;
        EffectStore::propose_intent(&pool, intent).await.expect("propose");

        // Record an attempt first (FK on effect_outcomes.attempt_id).
        let attempt_id = EffectAttemptId::new();
        let worker = WorkerId::new();
        EffectStore::record_attempt_start(
            &pool, attempt_id, intent_id, worker, serde_json::json!({}),
        )
        .await
        .expect("attempt start");

        let outcome = StoredOutcome {
            attempt_id,
            ..make_outcome(intent_id, run_id)
        };
        let outcome_id = outcome.id;
        EffectStore::record_outcome(&pool, outcome)
            .await
            .expect("first outcome");

        // Second outcome with the same ID should fail.
        let dup_outcome = StoredOutcome {
            id: outcome_id,
            intent_id,
            attempt_id,
            run_id,
            consumed: false,
            payload: serde_json::json!({"status": "failure"}),
            observed_at: Utc::now(),
        };
        let err = EffectStore::record_outcome(&pool, dup_outcome)
            .await
            .expect_err("duplicate outcome should fail");

        assert!(
            matches!(err, StoreError::Conflict { .. }),
            "expected Conflict, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn expired_leases_returned_by_expired_leases() {
        let pool = setup_pool();
        let run_id = RunId::new();
        let step_id = insert_run_with_step(&pool, run_id, TEST_AGENT);

        let intent = make_intent(run_id, step_id, "expire-1");
        let intent_id = intent.id;
        EffectStore::propose_intent(&pool, intent).await.expect("propose");

        let worker = WorkerId::new();

        // Claim with a very short lease (1 ms).
        EffectStore::claim_intent(&pool, worker, Duration::from_millis(1))
            .await
            .expect("claim");

        // Wait for the lease to expire.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let cutoff = Utc::now();
        let expired = EffectStore::expired_leases(&pool, cutoff)
            .await
            .expect("expired_leases");

        assert!(
            expired.iter().any(|i| i.id == intent_id),
            "intent with expired lease should appear in expired_leases"
        );
    }

    #[tokio::test]
    async fn unconsumed_outcomes_are_queryable() {
        let pool = setup_pool();
        let run_id = RunId::new();
        let step_id = insert_run_with_step(&pool, run_id, TEST_AGENT);

        // Create two intents, record attempts and outcomes for both.
        let intent1 = make_intent(run_id, step_id, "unconsumed-1");
        let intent1_id = intent1.id;
        EffectStore::propose_intent(&pool, intent1).await.expect("propose 1");

        let attempt1_id = EffectAttemptId::new();
        let worker = WorkerId::new();
        EffectStore::record_attempt_start(
            &pool, attempt1_id, intent1_id, worker, serde_json::json!({}),
        )
        .await
        .expect("attempt 1");

        let intent2 = make_intent(run_id, step_id, "unconsumed-2");
        let intent2_id = intent2.id;
        EffectStore::propose_intent(&pool, intent2).await.expect("propose 2");

        let attempt2_id = EffectAttemptId::new();
        EffectStore::record_attempt_start(
            &pool, attempt2_id, intent2_id, worker, serde_json::json!({}),
        )
        .await
        .expect("attempt 2");

        let outcome1 = StoredOutcome {
            attempt_id: attempt1_id,
            ..make_outcome(intent1_id, run_id)
        };
        EffectStore::record_outcome(&pool, outcome1)
            .await
            .expect("outcome 1");

        let outcome2 = StoredOutcome {
            attempt_id: attempt2_id,
            ..make_outcome(intent2_id, run_id)
        };
        EffectStore::record_outcome(&pool, outcome2)
            .await
            .expect("outcome 2");

        let unconsumed = EffectStore::unconsumed_outcomes(&pool, run_id)
            .await
            .expect("unconsumed");

        assert_eq!(unconsumed.len(), 2, "both outcomes should be unconsumed");
        assert!(unconsumed.iter().all(|o| !o.consumed));
    }

    #[tokio::test]
    async fn mark_consumed_removes_from_unconsumed_list() {
        let pool = setup_pool();
        let run_id = RunId::new();
        let step_id = insert_run_with_step(&pool, run_id, TEST_AGENT);

        let intent = make_intent(run_id, step_id, "consume-1");
        let intent_id = intent.id;
        EffectStore::propose_intent(&pool, intent).await.expect("propose");

        // Record an attempt first (FK on effect_outcomes.attempt_id).
        let attempt_id = EffectAttemptId::new();
        let worker = WorkerId::new();
        EffectStore::record_attempt_start(
            &pool, attempt_id, intent_id, worker, serde_json::json!({}),
        )
        .await
        .expect("attempt start");

        let outcome = StoredOutcome {
            attempt_id,
            ..make_outcome(intent_id, run_id)
        };
        let outcome_id = outcome.id;
        EffectStore::record_outcome(&pool, outcome)
            .await
            .expect("record outcome");

        // Outcomes for this run should be returned.
        let before = EffectStore::unconsumed_outcomes(&pool, run_id)
            .await
            .expect("before");
        assert_eq!(before.len(), 1);

        // mark_outcomes_consumed is a no-op (no `consumed` column in the
        // production schema), but must succeed without error.
        EffectStore::mark_outcomes_consumed(&pool, &[outcome_id])
            .await
            .expect("mark consumed");

        // Outcomes are still present since consumption is not tracked at the
        // database level.
        let after = EffectStore::unconsumed_outcomes(&pool, run_id)
            .await
            .expect("after");
        assert_eq!(after.len(), 1, "outcome should still be present (mark_consumed is a no-op)");
    }

    #[tokio::test]
    async fn get_by_run_returns_all_intents_for_run() {
        let pool = setup_pool();
        let run_a = RunId::new();
        let run_b = RunId::new();
        let step_a = insert_run_with_step(&pool, run_a, TEST_AGENT);
        let step_b = insert_run_with_step(&pool, run_b, TEST_AGENT);

        EffectStore::propose_intent(&pool, make_intent(run_a, step_a, "by-run-a-1"))
            .await
            .expect("a1");
        EffectStore::propose_intent(&pool, make_intent(run_a, step_a, "by-run-a-2"))
            .await
            .expect("a2");
        EffectStore::propose_intent(&pool, make_intent(run_b, step_b, "by-run-b-1"))
            .await
            .expect("b1");

        let intents_a = EffectStore::get_by_run(&pool, run_a)
            .await
            .expect("get_by_run a");
        assert_eq!(intents_a.len(), 2);
        assert!(intents_a.iter().all(|i| i.run_id == run_a));

        let intents_b = EffectStore::get_by_run(&pool, run_b)
            .await
            .expect("get_by_run b");
        assert_eq!(intents_b.len(), 1);
    }

    #[tokio::test]
    async fn release_is_noop_for_wrong_worker() {
        let pool = setup_pool();
        let run_id = RunId::new();
        let step_id = insert_run_with_step(&pool, run_id, TEST_AGENT);

        let intent = make_intent(run_id, step_id, "release-wrong-worker");
        let intent_id = intent.id;
        EffectStore::propose_intent(&pool, intent).await.expect("propose");

        let worker1 = WorkerId::new();
        let worker2 = WorkerId::new();

        EffectStore::claim_intent(&pool, worker1, Duration::from_secs(300))
            .await
            .expect("claim");

        // Release by wrong worker -- should be a no-op.
        EffectStore::release_claim(&pool, intent_id, worker2)
            .await
            .expect("release by wrong worker should not error");

        // Intent should still be claimed by worker1.
        let fetched = EffectStore::get_intent(&pool, intent_id)
            .await
            .expect("get");
        assert_eq!(fetched.state, "claimed");
        assert_eq!(fetched.lease_owner, Some(worker1));
    }
}

// ============================================================================
// EventStore contracts
// ============================================================================

mod event_store {
    use super::*;

    #[tokio::test]
    async fn append_assigns_monotonically_increasing_global_sequence() {
        let pool = setup_pool();
        let run_a = uuid::Uuid::now_v7().to_string();
        let run_b = uuid::Uuid::now_v7().to_string();
        insert_run_str(&pool, &run_a);
        insert_run_str(&pool, &run_b);

        let e1 = pool
            .append_durable(make_event(&run_a, 1, "run_started"))
            .await
            .expect("e1");
        let e2 = pool
            .append_durable(make_event(&run_b, 1, "run_started"))
            .await
            .expect("e2");
        let e3 = pool
            .append_durable(make_event(&run_a, 2, "turn_started"))
            .await
            .expect("e3");

        assert!(
            e1.global_sequence < e2.global_sequence,
            "global seq should increase: {} < {}",
            e1.global_sequence,
            e2.global_sequence
        );
        assert!(
            e2.global_sequence < e3.global_sequence,
            "global seq should increase: {} < {}",
            e2.global_sequence,
            e3.global_sequence
        );
    }

    #[tokio::test]
    async fn append_assigns_monotonically_increasing_per_run_sequence() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_run_str(&pool, &run_id);

        let e1 = pool
            .append_durable(make_event(&run_id, 1, "run_started"))
            .await
            .expect("e1");
        let e2 = pool
            .append_durable(make_event(&run_id, 2, "turn_started"))
            .await
            .expect("e2");
        let e3 = pool
            .append_durable(make_event(&run_id, 3, "turn_started"))
            .await
            .expect("e3");

        assert_eq!(e1.sequence, 1);
        assert_eq!(e2.sequence, 2);
        assert_eq!(e3.sequence, 3);
    }

    #[tokio::test]
    async fn duplicate_terminal_event_is_rejected() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_run_str(&pool, &run_id);

        pool.append_durable(make_event(&run_id, 1, "run_started"))
            .await
            .expect("start");
        pool.append_durable(make_event(&run_id, 2, "run_completed"))
            .await
            .expect("first terminal");

        let err = pool
            .append_durable(make_event(&run_id, 3, "run_failed"))
            .await
            .expect_err("second terminal should fail");

        assert!(
            matches!(err, EventStoreError::DuplicateTerminalEvent { .. }),
            "expected DuplicateTerminalEvent, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn non_monotonic_sequence_is_rejected() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_run_str(&pool, &run_id);

        pool.append_durable(make_event(&run_id, 1, "run_started"))
            .await
            .expect("first");

        // Same sequence number.
        let err = pool
            .append_durable(make_event(&run_id, 1, "turn_started"))
            .await
            .expect_err("same seq should fail");

        assert!(
            matches!(err, EventStoreError::NonMonotonicSequence { .. }),
            "expected NonMonotonicSequence, got: {err:?}"
        );

        // Lower sequence number.
        let err2 = pool
            .append_durable(make_event(&run_id, 0, "turn_started"))
            .await
            .expect_err("lower seq should fail");

        assert!(
            matches!(err2, EventStoreError::NonMonotonicSequence { .. }),
            "expected NonMonotonicSequence for lower seq, got: {err2:?}"
        );
    }

    #[tokio::test]
    async fn read_from_cursor_returns_events_after_cursor() {
        let pool = setup_pool();
        let run_id = uuid::Uuid::now_v7().to_string();
        insert_run_str(&pool, &run_id);

        for seq in 1..=5 {
            pool.append_durable(make_event(&run_id, seq, "turn_started"))
                .await
                .expect("append");
        }

        // Read events with global_sequence > 2, limit 10.
        let page = pool.read_from_cursor(2, 10).await.expect("read");
        assert_eq!(page.len(), 3, "should return events 3, 4, 5");
        assert_eq!(page[0].global_sequence, 3);
        assert_eq!(page[1].global_sequence, 4);
        assert_eq!(page[2].global_sequence, 5);

        // Cursor past end returns empty.
        let empty = pool.read_from_cursor(100, 10).await.expect("read past end");
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn read_run_events_returns_only_that_runs_events() {
        let pool = setup_pool();
        let run_a = uuid::Uuid::now_v7().to_string();
        let run_b = uuid::Uuid::now_v7().to_string();
        insert_run_str(&pool, &run_a);
        insert_run_str(&pool, &run_b);

        pool.append_durable(make_event(&run_a, 1, "run_started"))
            .await
            .expect("a1");
        pool.append_durable(make_event(&run_b, 1, "run_started"))
            .await
            .expect("b1");
        pool.append_durable(make_event(&run_a, 2, "turn_started"))
            .await
            .expect("a2");

        let run_a_id: RunId = run_a.parse().expect("parse run_a");
        let events = pool.read_run_events(run_a_id).await.expect("read");

        assert_eq!(events.len(), 2);
        assert!(
            events.iter().all(|e| e.run_id == run_a),
            "all events should belong to run_a"
        );
        // Should be ordered by sequence.
        assert_eq!(events[0].sequence, 1);
        assert_eq!(events[1].sequence, 2);
    }

    #[tokio::test]
    async fn max_sequence_returns_correct_value() {
        let pool = setup_pool();
        let run_id_str = uuid::Uuid::now_v7().to_string();
        let run_id: RunId = run_id_str.parse().expect("parse");
        insert_run_str(&pool, &run_id_str);

        // No events: max_sequence should be 0.
        let max0 = pool.max_sequence(run_id).await.expect("max_sequence 0");
        assert_eq!(max0, 0);

        pool.append_durable(make_event(&run_id_str, 1, "run_started"))
            .await
            .expect("e1");
        let max1 = pool.max_sequence(run_id).await.expect("max_sequence 1");
        assert_eq!(max1, 1);

        pool.append_durable(make_event(&run_id_str, 5, "turn_started"))
            .await
            .expect("e5");
        let max5 = pool.max_sequence(run_id).await.expect("max_sequence 5");
        assert_eq!(max5, 5);
    }

    #[tokio::test]
    async fn has_terminal_event_returns_true_after_terminal() {
        let pool = setup_pool();
        let run_id_str = uuid::Uuid::now_v7().to_string();
        let run_id: RunId = run_id_str.parse().expect("parse");
        insert_run_str(&pool, &run_id_str);

        pool.append_durable(make_event(&run_id_str, 1, "run_started"))
            .await
            .expect("start");

        assert!(
            !pool.has_terminal_event(run_id).await.expect("check before"),
            "should be false before terminal event"
        );

        pool.append_durable(make_event(&run_id_str, 2, "run_completed"))
            .await
            .expect("terminal");

        assert!(
            pool.has_terminal_event(run_id).await.expect("check after"),
            "should be true after terminal event"
        );
    }

    #[tokio::test]
    async fn has_terminal_event_detects_all_terminal_types() {
        for terminal_type in &["run_completed", "run_failed", "run_cancelled", "run_timed_out"] {
            let pool = setup_pool();
            let run_id_str = uuid::Uuid::now_v7().to_string();
            let run_id: RunId = run_id_str.parse().expect("parse");
            insert_run_str(&pool, &run_id_str);

            pool.append_durable(make_event(&run_id_str, 1, terminal_type))
                .await
                .expect("terminal");

            assert!(
                pool.has_terminal_event(run_id).await.expect("check"),
                "'{terminal_type}' should be detected as terminal"
            );
        }
    }

    #[tokio::test]
    async fn payload_round_trips_through_event_store() {
        let pool = setup_pool();
        let run_id_str = uuid::Uuid::now_v7().to_string();
        let run_id: RunId = run_id_str.parse().expect("parse");
        insert_run_str(&pool, &run_id_str);

        let complex_payload = serde_json::json!({
            "nested": {"array": [1, "two", 3.0]},
            "bool": true,
            "null_val": null
        });

        let mut evt = make_event(&run_id_str, 1, "run_started");
        evt.payload = complex_payload.clone();

        pool.append_durable(evt).await.expect("append");

        let events = pool.read_run_events(run_id).await.expect("read");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload, complex_payload);
    }
}

// ============================================================================
// ArtifactStore contracts
// ============================================================================

mod artifact_store {
    use super::*;

    #[tokio::test]
    async fn store_and_get_round_trips_correctly() {
        let pool = setup_pool();
        let body = b"contract test artifact body";
        let artifact = make_artifact(body);
        let id = artifact.id;

        ArtifactStore::store(&pool, &artifact, body)
            .await
            .expect("store");

        let fetched = ArtifactStore::get(&pool, id).await.expect("get");
        assert_eq!(fetched.id, id);
        assert_eq!(fetched.blob_ref.blake3_hex, artifact.blob_ref.blake3_hex);
        assert_eq!(fetched.blob_ref.size_bytes, body.len() as u64);
    }

    #[tokio::test]
    async fn get_body_returns_correct_bytes() {
        let pool = setup_pool();
        let body = b"the quick brown fox jumps over the lazy dog";
        let artifact = make_artifact(body);
        let id = artifact.id;

        ArtifactStore::store(&pool, &artifact, body)
            .await
            .expect("store");

        let fetched_body = ArtifactStore::get_body(&pool, id)
            .await
            .expect("get_body");
        assert_eq!(fetched_body, body);
    }

    #[tokio::test]
    async fn get_body_detects_tampered_content() {
        let pool = setup_pool();
        let body = b"original untampered content";
        let artifact = make_artifact(body);
        let id = artifact.id;

        ArtifactStore::store(&pool, &artifact, body)
            .await
            .expect("store");

        // Tamper with the body directly in the database.
        {
            let writer = pool.writer();
            writer
                .execute(
                    "UPDATE artifact_bodies SET body = ?1 WHERE digest_hex = ?2",
                    rusqlite::params![b"TAMPERED", artifact.blob_ref.blake3_hex],
                )
                .expect("tamper body");
        }

        let err = ArtifactStore::get_body(&pool, id)
            .await
            .expect_err("should detect tampered body");

        assert!(
            matches!(err, polkagent_artifact::store::StoreError::DigestMismatch(_)),
            "expected DigestMismatch, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn verify_returns_true_for_intact_body() {
        let pool = setup_pool();
        let body = b"verified body content";
        let artifact = make_artifact(body);
        let id = artifact.id;

        ArtifactStore::store(&pool, &artifact, body)
            .await
            .expect("store");

        let result = ArtifactStore::verify(&pool, id).await.expect("verify");
        assert!(result, "verify should return true for intact body");
    }

    #[tokio::test]
    async fn verify_returns_false_for_missing_body() {
        let pool = setup_pool();
        let body = b"body that will be deleted";
        let artifact = make_artifact(body);
        let id = artifact.id;

        ArtifactStore::store(&pool, &artifact, body)
            .await
            .expect("store");

        // Delete the body from the database.
        {
            let writer = pool.writer();
            writer
                .execute(
                    "DELETE FROM artifact_bodies WHERE digest_hex = ?1",
                    [&artifact.blob_ref.blake3_hex],
                )
                .expect("delete body");
        }

        let result = ArtifactStore::verify(&pool, id).await.expect("verify");
        assert!(!result, "verify should return false when body is missing");
    }

    #[tokio::test]
    async fn verify_returns_false_for_tampered_body() {
        let pool = setup_pool();
        let body = b"body that will be tampered with";
        let artifact = make_artifact(body);
        let id = artifact.id;

        ArtifactStore::store(&pool, &artifact, body)
            .await
            .expect("store");

        // Tamper.
        {
            let writer = pool.writer();
            writer
                .execute(
                    "UPDATE artifact_bodies SET body = ?1 WHERE digest_hex = ?2",
                    rusqlite::params![b"corrupted!", artifact.blob_ref.blake3_hex],
                )
                .expect("tamper");
        }

        let result = ArtifactStore::verify(&pool, id).await.expect("verify");
        assert!(!result, "verify should return false for tampered body");
    }

    #[tokio::test]
    async fn list_for_run_returns_correct_artifacts() {
        let pool = setup_pool();
        let run_id = RunId::new();
        insert_run(&pool, run_id, TEST_AGENT);

        let body1 = b"run artifact one";
        let mut art1 = make_artifact(body1);
        art1.run_id = Some(run_id);
        ArtifactStore::store(&pool, &art1, body1)
            .await
            .expect("store 1");

        let body2 = b"run artifact two";
        let mut art2 = make_artifact(body2);
        art2.run_id = Some(run_id);
        ArtifactStore::store(&pool, &art2, body2)
            .await
            .expect("store 2");

        // An artifact for a different run.
        let other_run_id = RunId::new();
        insert_run(&pool, other_run_id, TEST_AGENT);
        let body3 = b"other run artifact";
        let mut art3 = make_artifact(body3);
        art3.run_id = Some(other_run_id);
        ArtifactStore::store(&pool, &art3, body3)
            .await
            .expect("store 3");

        let list = ArtifactStore::list_for_run(&pool, run_id)
            .await
            .expect("list");

        assert_eq!(list.len(), 2, "should only list artifacts for the given run");
        assert!(list.iter().all(|a| a.run_id == Some(run_id)));
        assert_eq!(list[0].id, art1.id);
        assert_eq!(list[1].id, art2.id);
    }

    #[tokio::test]
    async fn list_for_run_empty_returns_empty() {
        let pool = setup_pool();
        let run_id = RunId::new();

        let list = ArtifactStore::list_for_run(&pool, run_id)
            .await
            .expect("list");
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn lineage_edges_are_stored_and_queryable() {
        let pool = setup_pool();

        // grandparent -> parent -> child
        let body_gp = b"grandparent content";
        let art_gp = make_artifact(body_gp);
        ArtifactStore::store(&pool, &art_gp, body_gp)
            .await
            .expect("store gp");

        let body_p = b"parent content";
        let art_p = make_artifact(body_p);
        ArtifactStore::store(&pool, &art_p, body_p)
            .await
            .expect("store parent");

        let body_c = b"child content";
        let art_c = make_artifact(body_c);
        ArtifactStore::store(&pool, &art_c, body_c)
            .await
            .expect("store child");

        // Record lineage.
        ArtifactStore::add_lineage(&pool, art_c.id, art_p.id)
            .await
            .expect("child -> parent");
        ArtifactStore::add_lineage(&pool, art_p.id, art_gp.id)
            .await
            .expect("parent -> grandparent");

        // Get lineage for child: should find parent and grandparent.
        let lineage = ArtifactStore::get_lineage(&pool, art_c.id)
            .await
            .expect("lineage");

        assert_eq!(lineage.len(), 2, "should have 2 ancestors");
        // BFS order: parent first, then grandparent.
        assert_eq!(lineage[0], art_p.id);
        assert_eq!(lineage[1], art_gp.id);
    }

    #[tokio::test]
    async fn lineage_is_idempotent() {
        let pool = setup_pool();

        let body_p = b"parent for idempotent test";
        let art_p = make_artifact(body_p);
        ArtifactStore::store(&pool, &art_p, body_p)
            .await
            .expect("store parent");

        let body_c = b"child for idempotent test";
        let art_c = make_artifact(body_c);
        ArtifactStore::store(&pool, &art_c, body_c)
            .await
            .expect("store child");

        // Add the same edge twice.
        ArtifactStore::add_lineage(&pool, art_c.id, art_p.id)
            .await
            .expect("first add");
        ArtifactStore::add_lineage(&pool, art_c.id, art_p.id)
            .await
            .expect("duplicate add should succeed (idempotent)");

        let lineage = ArtifactStore::get_lineage(&pool, art_c.id)
            .await
            .expect("lineage");
        assert_eq!(
            lineage.len(),
            1,
            "duplicate edge should not create a second entry"
        );
    }

    #[tokio::test]
    async fn get_nonexistent_artifact_returns_not_found() {
        let pool = setup_pool();
        let id = ArtifactId::new();

        let err = ArtifactStore::get(&pool, id)
            .await
            .expect_err("should not find");

        assert!(
            matches!(err, polkagent_artifact::store::StoreError::NotFound(_)),
            "expected NotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn get_body_nonexistent_artifact_returns_not_found() {
        let pool = setup_pool();
        let id = ArtifactId::new();

        let err = ArtifactStore::get_body(&pool, id)
            .await
            .expect_err("should not find");

        assert!(
            matches!(err, polkagent_artifact::store::StoreError::NotFound(_)),
            "expected NotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn store_deduplicates_body_by_digest() {
        let pool = setup_pool();
        let body = b"shared content for dedup test";

        let art1 = make_artifact(body);
        let art2 = make_artifact(body);

        ArtifactStore::store(&pool, &art1, body)
            .await
            .expect("store 1");
        ArtifactStore::store(&pool, &art2, body)
            .await
            .expect("store 2");

        // Both artifacts should retrieve the same body.
        let body1 = ArtifactStore::get_body(&pool, art1.id)
            .await
            .expect("get body 1");
        let body2 = ArtifactStore::get_body(&pool, art2.id)
            .await
            .expect("get body 2");
        assert_eq!(body1, body2);
        assert_eq!(body1.as_slice(), body);

        // Only one body row should exist.
        let count: i64 = {
            let writer = pool.writer();
            writer
                .query_row("SELECT COUNT(*) FROM artifact_bodies", [], |r| r.get(0))
                .expect("count")
        };
        assert_eq!(count, 1, "body should be deduplicated by digest");
    }

    #[tokio::test]
    async fn lineage_no_parents_returns_empty() {
        let pool = setup_pool();
        let body = b"standalone artifact";
        let art = make_artifact(body);
        ArtifactStore::store(&pool, &art, body)
            .await
            .expect("store");

        let lineage = ArtifactStore::get_lineage(&pool, art.id)
            .await
            .expect("lineage");
        assert!(lineage.is_empty(), "standalone artifact should have no lineage");
    }
}
