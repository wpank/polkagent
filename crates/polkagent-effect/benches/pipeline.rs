//! Criterion benchmarks for the polkagent-effect crate.
//!
//! Covers:
//! 1. EffectIntent creation with different effect kinds
//! 2. Idempotency key generation (BLAKE3 hashing)
//! 3. Effect state machine transitions (Pending -> Claimed -> Executing -> Completed)
//! 4. Full pipeline cycle: propose + claim + execute + record outcome
//! 5. Concurrent effect claiming (multiple workers)
//! 6. Effect serialization/deserialization (serde_json)
//! 7. EffectOutcome construction
//! 8. Priority queue ordering
//! 9. Bulk effect creation (100, 1000 intents)
//! 10. Effect filtering by run_id, by kind, by state

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

use polkagent_core::{
    EffectAttemptId, EffectId, EffectOutcomeId, RunId, StepId, TurnId, WorkerId,
};
use polkagent_effect::{
    idempotency::IdempotencyKey,
    pipeline::{EffectIntentSpec, EffectPipeline},
    types::{
        CancellationReason, EffectAttempt, EffectIntent, EffectIntentState, EffectKind,
        EffectOutcome, EffectPriority, ErrorClass, OutcomeResult, ResolutionHint,
    },
};
use polkagent_store_trait::{EffectStore, StoredIntent, StoredOutcome, StoreError};

// ---------------------------------------------------------------------------
// Minimal in-memory EffectStore for benchmarks
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct InMemoryStore {
    intents: Mutex<HashMap<EffectId, StoredIntent>>,
    attempts: Mutex<Vec<serde_json::Value>>,
    outcomes: Mutex<Vec<StoredOutcome>>,
}

impl InMemoryStore {
    fn new_arc() -> (Arc<Self>, Arc<dyn EffectStore>) {
        let inner = Arc::new(Self::default());
        let dyn_store: Arc<dyn EffectStore> = Arc::clone(&inner) as Arc<dyn EffectStore>;
        (inner, dyn_store)
    }
}

#[async_trait]
impl EffectStore for InMemoryStore {
    async fn propose_intent(&self, intent: StoredIntent) -> Result<(), StoreError> {
        let mut intents = self.intents.lock().expect("lock");
        if intents.contains_key(&intent.id) {
            return Err(StoreError::Conflict {
                resource_type: "EffectIntent",
                id: intent.id.to_string(),
            });
        }
        intents.insert(intent.id, intent);
        Ok(())
    }

    async fn claim_intent(
        &self,
        worker_id: WorkerId,
        lease_duration: Duration,
    ) -> Result<Option<StoredIntent>, StoreError> {
        let mut intents = self.intents.lock().expect("lock");
        let now = Utc::now();
        let pending_id = intents
            .values()
            .find(|i| i.state.eq_ignore_ascii_case("pending"))
            .map(|i| i.id);
        match pending_id {
            None => Ok(None),
            Some(id) => {
                let intent = intents.get_mut(&id).expect("exists");
                let expires = now
                    + chrono::Duration::from_std(lease_duration)
                        .unwrap_or_else(|_| chrono::Duration::seconds(60));
                intent.state = "claimed".to_string();
                intent.lease_owner = Some(worker_id);
                intent.lease_expires = Some(expires);
                Ok(Some(intent.clone()))
            }
        }
    }

    async fn claim_intent_by_id(
        &self,
        intent_id: EffectId,
        worker_id: WorkerId,
        lease_duration: Duration,
    ) -> Result<StoredIntent, StoreError> {
        let mut intents = self.intents.lock().expect("lock");
        let now = Utc::now();
        match intents.get_mut(&intent_id) {
            None => Err(StoreError::NotFound {
                resource_type: "EffectIntent",
                id: intent_id.to_string(),
            }),
            Some(intent) => {
                let expires = now
                    + chrono::Duration::from_std(lease_duration)
                        .unwrap_or_else(|_| chrono::Duration::seconds(60));
                intent.state = "claimed".to_string();
                intent.lease_owner = Some(worker_id);
                intent.lease_expires = Some(expires);
                Ok(intent.clone())
            }
        }
    }

    async fn release_claim(
        &self,
        intent_id: EffectId,
        worker_id: WorkerId,
    ) -> Result<(), StoreError> {
        let mut intents = self.intents.lock().expect("lock");
        if let Some(intent) = intents.get_mut(&intent_id) {
            if intent.lease_owner == Some(worker_id) {
                intent.state = "pending".to_string();
                intent.lease_owner = None;
                intent.lease_expires = None;
            }
        }
        Ok(())
    }

    async fn get_intent(&self, intent_id: EffectId) -> Result<StoredIntent, StoreError> {
        let intents = self.intents.lock().expect("lock");
        intents
            .get(&intent_id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound {
                resource_type: "EffectIntent",
                id: intent_id.to_string(),
            })
    }

    async fn get_by_run(&self, run_id: RunId) -> Result<Vec<StoredIntent>, StoreError> {
        let intents = self.intents.lock().expect("lock");
        Ok(intents
            .values()
            .filter(|i| i.run_id == run_id)
            .cloned()
            .collect())
    }

    async fn expired_leases(
        &self,
        cutoff: polkagent_core::Timestamp,
    ) -> Result<Vec<StoredIntent>, StoreError> {
        let intents = self.intents.lock().expect("lock");
        Ok(intents
            .values()
            .filter(|i| {
                i.state.eq_ignore_ascii_case("claimed")
                    && i.lease_expires
                        .map(|exp| exp < cutoff)
                        .unwrap_or(false)
            })
            .cloned()
            .collect())
    }

    async fn update_intent_state(
        &self,
        intent_id: EffectId,
        new_state: &str,
    ) -> Result<StoredIntent, StoreError> {
        let mut intents = self.intents.lock().expect("lock");
        match intents.get_mut(&intent_id) {
            None => Err(StoreError::NotFound {
                resource_type: "EffectIntent",
                id: intent_id.to_string(),
            }),
            Some(intent) => {
                intent.state = new_state.to_string();
                Ok(intent.clone())
            }
        }
    }

    async fn record_attempt_start(
        &self,
        _attempt_id: EffectAttemptId,
        _intent_id: EffectId,
        _worker_id: WorkerId,
        payload: serde_json::Value,
    ) -> Result<(), StoreError> {
        self.attempts.lock().expect("lock").push(payload);
        Ok(())
    }

    async fn record_outcome(&self, outcome: StoredOutcome) -> Result<(), StoreError> {
        let mut outcomes = self.outcomes.lock().expect("lock");
        if outcomes.iter().any(|o| o.id == outcome.id) {
            return Err(StoreError::Conflict {
                resource_type: "EffectOutcome",
                id: outcome.id.to_string(),
            });
        }
        let mut intents = self.intents.lock().expect("lock");
        if let Some(intent) = intents.get_mut(&outcome.intent_id) {
            intent.state = "resolved".to_string();
            intent.lease_owner = None;
            intent.lease_expires = None;
        }
        outcomes.push(outcome);
        Ok(())
    }

    async fn unconsumed_outcomes(
        &self,
        run_id: RunId,
    ) -> Result<Vec<StoredOutcome>, StoreError> {
        let outcomes = self.outcomes.lock().expect("lock");
        Ok(outcomes
            .iter()
            .filter(|o| o.run_id == run_id && !o.consumed)
            .cloned()
            .collect())
    }

    async fn mark_outcomes_consumed(
        &self,
        outcome_ids: &[EffectOutcomeId],
    ) -> Result<(), StoreError> {
        let mut outcomes = self.outcomes.lock().expect("lock");
        for o in outcomes.iter_mut() {
            if outcome_ids.contains(&o.id) {
                o.consumed = true;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helper constructors
// ---------------------------------------------------------------------------

/// All EffectKind variants for iteration in benchmarks.
const ALL_KINDS: [EffectKind; 9] = [
    EffectKind::ModelCall,
    EffectKind::ToolCall,
    EffectKind::SignatureRequest,
    EffectKind::Broadcast,
    EffectKind::FinalityWatch,
    EffectKind::Delivery,
    EffectKind::ChainRead,
    EffectKind::Simulation,
    EffectKind::HarnessOperation,
];

fn make_spec(run_id: RunId, kind: EffectKind, sequence: u64) -> EffectIntentSpec {
    EffectIntentSpec {
        run_id,
        turn_id: TurnId::new(),
        step_id: StepId::new(),
        kind,
        sequence,
        idempotency_key: None,
        payload: serde_json::json!({"bench": true, "seq": sequence}),
        retry_class: None,
        priority: None,
        max_attempts: None,
        action_card: None,
    }
}

fn make_spec_with_priority(
    run_id: RunId,
    kind: EffectKind,
    sequence: u64,
    priority: EffectPriority,
) -> EffectIntentSpec {
    EffectIntentSpec {
        run_id,
        turn_id: TurnId::new(),
        step_id: StepId::new(),
        kind,
        sequence,
        idempotency_key: None,
        payload: serde_json::json!({"bench": true, "seq": sequence}),
        retry_class: None,
        priority: Some(priority),
        max_attempts: None,
        action_card: None,
    }
}

fn make_effect_intent(kind: EffectKind) -> EffectIntent {
    let run_id = RunId::new();
    let params_hash = IdempotencyKey::hash_params(b"bench-intent");
    EffectIntent {
        id: EffectId::new(),
        run_id,
        turn_id: TurnId::new(),
        step_id: StepId::new(),
        kind,
        idempotency_key: IdempotencyKey::generate(run_id, 1, 0, kind, params_hash),
        sequence: 0,
        state: EffectIntentState::Pending,
        deadline: None,
        max_attempts: 3,
        attempt_count: 0,
        retry_class: kind.default_retry_class(),
        priority: EffectPriority::Normal,
        payload: serde_json::json!({"bench": true}),
        created_at: Utc::now(),
        resolved_at: None,
        action_card: None,
    }
}

fn make_outcome(intent_id: EffectId, run_id: RunId) -> EffectOutcome {
    EffectOutcome {
        id: EffectOutcomeId::new(),
        attempt_id: EffectAttemptId::new(),
        intent_id,
        run_id,
        result: OutcomeResult::Success {
            data: serde_json::json!({"ok": true}),
        },
        observed_at: Utc::now(),
        digest: [0u8; 32],
    }
}

fn make_effect_attempt(intent_id: EffectId, run_id: RunId) -> EffectAttempt {
    let worker_id = WorkerId::new();
    EffectAttempt {
        id: EffectAttemptId::new(),
        intent_id,
        run_id,
        attempt_number: 1,
        idempotency_key: IdempotencyKey::generate(
            run_id,
            1,
            0,
            EffectKind::ModelCall,
            IdempotencyKey::hash_params(b"bench"),
        ),
        worker_id,
        lease_expires: Utc::now() + chrono::Duration::seconds(60),
        retry_class: polkagent_core::RetryClass::Idempotent,
        state: polkagent_effect::types::AttemptState::Leased,
        created_at: Utc::now(),
        claimed_at: Utc::now(),
        started_at: None,
        completed_at: None,
    }
}

// ---------------------------------------------------------------------------
// 1. EffectIntent creation with different effect kinds
// ---------------------------------------------------------------------------

fn bench_intent_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("intent_creation");
    group.sample_size(500);

    for &kind in &ALL_KINDS {
        group.bench_with_input(
            BenchmarkId::new("EffectIntent::new", kind.discriminant_str()),
            &kind,
            |b, &kind| {
                b.iter(|| {
                    let intent = make_effect_intent(kind);
                    criterion::black_box(intent.id);
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 2. Idempotency key generation (BLAKE3 hashing)
// ---------------------------------------------------------------------------

fn bench_idempotency_key(c: &mut Criterion) {
    let mut group = c.benchmark_group("idempotency_key");
    group.sample_size(1000);

    let run_id = RunId::new();

    // hash_params with varying payload sizes
    for &size in &[32usize, 256, 1024, 4096] {
        let payload = vec![0xABu8; size];
        group.bench_with_input(
            BenchmarkId::new("hash_params", format!("{size}B")),
            &payload,
            |b, payload| {
                b.iter(|| {
                    let hash = IdempotencyKey::hash_params(payload);
                    criterion::black_box(hash);
                });
            },
        );
    }

    // Key generation per kind
    let params_hash = IdempotencyKey::hash_params(b"bench-payload");
    for &kind in &ALL_KINDS {
        group.bench_with_input(
            BenchmarkId::new("generate", kind.discriminant_str()),
            &kind,
            |b, &kind| {
                b.iter(|| {
                    let key = IdempotencyKey::generate(run_id, 1, 0, kind, params_hash);
                    criterion::black_box(key);
                });
            },
        );
    }

    // Hex encoding
    let key = IdempotencyKey::generate(run_id, 1, 0, EffectKind::ModelCall, params_hash);
    group.bench_function("to_hex", |b| {
        b.iter(|| {
            let hex = key.to_hex();
            criterion::black_box(hex);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 3. Effect state machine transitions
// ---------------------------------------------------------------------------

fn bench_state_transitions(c: &mut Criterion) {
    let mut group = c.benchmark_group("state_transitions");
    group.sample_size(500);

    let worker_id = WorkerId::new();
    let now = Utc::now();
    let outcome_id = EffectOutcomeId::new();

    // Pending -> Claimed
    group.bench_function("pending_to_claimed", |b| {
        b.iter(|| {
            let mut intent = make_effect_intent(EffectKind::ModelCall);
            intent.state = EffectIntentState::Claimed {
                worker_id,
                lease_expires: now + chrono::Duration::seconds(60),
            };
            criterion::black_box(&intent.state);
        });
    });

    // Claimed -> Executing
    group.bench_function("claimed_to_executing", |b| {
        b.iter(|| {
            let mut intent = make_effect_intent(EffectKind::ModelCall);
            intent.state = EffectIntentState::Claimed {
                worker_id,
                lease_expires: now + chrono::Duration::seconds(60),
            };
            intent.state = EffectIntentState::Executing {
                worker_id,
                started_at: now,
            };
            criterion::black_box(&intent.state);
        });
    });

    // Executing -> Resolved
    group.bench_function("executing_to_resolved", |b| {
        b.iter(|| {
            let mut intent = make_effect_intent(EffectKind::ModelCall);
            intent.state = EffectIntentState::Executing {
                worker_id,
                started_at: now,
            };
            intent.state = EffectIntentState::Resolved { outcome_id };
            criterion::black_box(&intent.state);
        });
    });

    // Full transition chain: Pending -> Claimed -> Executing -> Resolved
    group.bench_function("full_chain", |b| {
        b.iter(|| {
            let mut intent = make_effect_intent(EffectKind::ToolCall);
            assert!(intent.is_claimable());

            intent.state = EffectIntentState::Claimed {
                worker_id,
                lease_expires: now + chrono::Duration::seconds(60),
            };
            assert!(!intent.is_claimable());
            assert!(intent.current_lease().is_some());

            intent.state = EffectIntentState::Executing {
                worker_id,
                started_at: now,
            };
            assert!(!intent.is_terminal());

            intent.state = EffectIntentState::Resolved { outcome_id };
            assert!(intent.is_terminal());

            criterion::black_box(&intent.state);
        });
    });

    // Retrying transition
    group.bench_function("executing_to_retrying", |b| {
        b.iter(|| {
            let mut intent = make_effect_intent(EffectKind::ChainRead);
            intent.state = EffectIntentState::Executing {
                worker_id,
                started_at: now,
            };
            intent.state = EffectIntentState::Retrying {
                next_attempt_after: now + chrono::Duration::seconds(5),
                last_error: "connection timeout".to_string(),
            };
            criterion::black_box(&intent.state);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 4. Full pipeline cycle: propose + claim + execute + record outcome
// ---------------------------------------------------------------------------

fn bench_full_pipeline_cycle(c: &mut Criterion) {
    let mut group = c.benchmark_group("pipeline_full_cycle");
    group.sample_size(200);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    // Propose only
    group.bench_function("propose", |b| {
        b.iter(|| {
            rt.block_on(async {
                let (_, dyn_store) = InMemoryStore::new_arc();
                let pipeline = EffectPipeline::new(dyn_store, WorkerId::new());
                let run_id = RunId::new();
                let id = pipeline
                    .propose(make_spec(run_id, EffectKind::ModelCall, 1))
                    .await
                    .expect("propose");
                criterion::black_box(id);
            });
        });
    });

    // Propose + claim
    group.bench_function("propose_and_claim", |b| {
        b.iter(|| {
            rt.block_on(async {
                let (_, dyn_store) = InMemoryStore::new_arc();
                let pipeline = EffectPipeline::new(dyn_store, WorkerId::new());
                let run_id = RunId::new();
                pipeline
                    .propose(make_spec(run_id, EffectKind::ModelCall, 1))
                    .await
                    .expect("propose");
                let guard = pipeline
                    .claim_with_duration(Duration::from_secs(60))
                    .await
                    .expect("claim")
                    .expect("guard");
                guard.complete();
            });
        });
    });

    // Full cycle: propose + claim + record_outcome
    group.bench_function("propose_claim_outcome", |b| {
        b.iter(|| {
            rt.block_on(async {
                let (_, dyn_store) = InMemoryStore::new_arc();
                let pipeline = EffectPipeline::new(dyn_store, WorkerId::new());
                let run_id = RunId::new();

                let intent_id = pipeline
                    .propose(make_spec(run_id, EffectKind::ModelCall, 1))
                    .await
                    .expect("propose");

                let guard = pipeline
                    .claim_with_duration(Duration::from_secs(60))
                    .await
                    .expect("claim")
                    .expect("guard");

                let outcome = make_outcome(intent_id, run_id);
                pipeline
                    .record_outcome(intent_id, &outcome)
                    .await
                    .expect("record_outcome");
                guard.complete();

                criterion::black_box(outcome.id);
            });
        });
    });

    // Full cycle per effect kind
    for &kind in &[
        EffectKind::ModelCall,
        EffectKind::ToolCall,
        EffectKind::ChainRead,
        EffectKind::Broadcast,
    ] {
        group.bench_with_input(
            BenchmarkId::new("full_cycle_by_kind", kind.discriminant_str()),
            &kind,
            |b, &kind| {
                b.iter(|| {
                    rt.block_on(async {
                        let (_, dyn_store) = InMemoryStore::new_arc();
                        let pipeline = EffectPipeline::new(dyn_store, WorkerId::new());
                        let run_id = RunId::new();

                        let intent_id = pipeline
                            .propose(make_spec(run_id, kind, 1))
                            .await
                            .expect("propose");

                        let guard = pipeline
                            .claim_with_duration(Duration::from_secs(60))
                            .await
                            .expect("claim")
                            .expect("guard");

                        let outcome = make_outcome(intent_id, run_id);
                        pipeline
                            .record_outcome(intent_id, &outcome)
                            .await
                            .expect("record_outcome");
                        guard.complete();

                        criterion::black_box(outcome.id);
                    });
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 5. Concurrent effect claiming (multiple workers)
// ---------------------------------------------------------------------------

fn bench_concurrent_claiming(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_claiming");
    group.sample_size(100);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("tokio runtime");

    for &worker_count in &[2u32, 4, 8] {
        group.bench_with_input(
            BenchmarkId::new("claim_race", format!("{worker_count}_workers")),
            &worker_count,
            |b, &worker_count| {
                b.iter(|| {
                    rt.block_on(async {
                        let (_, dyn_store) = InMemoryStore::new_arc();
                        let pipeline = EffectPipeline::new(Arc::clone(&dyn_store), WorkerId::new());
                        pipeline
                            .propose(make_spec(RunId::new(), EffectKind::ToolCall, 1))
                            .await
                            .expect("propose");

                        let mut set = tokio::task::JoinSet::new();
                        for _ in 0..worker_count {
                            let store = Arc::clone(&dyn_store);
                            set.spawn(async move {
                                EffectPipeline::new(store, WorkerId::new())
                                    .claim_with_duration(Duration::from_secs(30))
                                    .await
                                    .expect("claim")
                            });
                        }

                        let mut successes = 0u32;
                        while let Some(r) = set.join_next().await {
                            if r.expect("join").is_some() {
                                successes += 1;
                            }
                        }
                        criterion::black_box(successes);
                    });
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 6. Effect serialization/deserialization (serde_json)
// ---------------------------------------------------------------------------

fn bench_serde(c: &mut Criterion) {
    let mut group = c.benchmark_group("serde");
    group.sample_size(500);

    // EffectIntent round-trip
    let intent = make_effect_intent(EffectKind::ModelCall);
    let intent_json = serde_json::to_string(&intent).expect("serialize intent");

    group.bench_function("EffectIntent/serialize", |b| {
        b.iter(|| {
            let json = serde_json::to_string(&intent).expect("serialize");
            criterion::black_box(json);
        });
    });

    group.bench_function("EffectIntent/deserialize", |b| {
        b.iter(|| {
            let back: EffectIntent =
                serde_json::from_str(&intent_json).expect("deserialize");
            criterion::black_box(back.id);
        });
    });

    // EffectOutcome round-trip
    let outcome = make_outcome(EffectId::new(), RunId::new());
    let outcome_json = serde_json::to_string(&outcome).expect("serialize outcome");

    group.bench_function("EffectOutcome/serialize", |b| {
        b.iter(|| {
            let json = serde_json::to_string(&outcome).expect("serialize");
            criterion::black_box(json);
        });
    });

    group.bench_function("EffectOutcome/deserialize", |b| {
        b.iter(|| {
            let back: EffectOutcome =
                serde_json::from_str(&outcome_json).expect("deserialize");
            criterion::black_box(back.id);
        });
    });

    // EffectAttempt round-trip
    let attempt = make_effect_attempt(EffectId::new(), RunId::new());
    let attempt_json = serde_json::to_string(&attempt).expect("serialize attempt");

    group.bench_function("EffectAttempt/serialize", |b| {
        b.iter(|| {
            let json = serde_json::to_string(&attempt).expect("serialize");
            criterion::black_box(json);
        });
    });

    group.bench_function("EffectAttempt/deserialize", |b| {
        b.iter(|| {
            let back: EffectAttempt =
                serde_json::from_str(&attempt_json).expect("deserialize");
            criterion::black_box(back.id);
        });
    });

    // OutcomeResult variants
    let variants: Vec<(&str, OutcomeResult)> = vec![
        (
            "Success",
            OutcomeResult::Success {
                data: serde_json::json!({"tx_hash": "0xdeadbeef", "block": 42}),
            },
        ),
        (
            "Failure",
            OutcomeResult::Failure {
                error_class: ErrorClass::ServerError,
                message: "internal server error".to_string(),
                retriable: true,
            },
        ),
        (
            "Timeout",
            OutcomeResult::Timeout {
                waited_secs: 120,
                partial_work_possible: true,
            },
        ),
        (
            "Cancelled",
            OutcomeResult::Cancelled {
                partial_work_possible: false,
                reason: CancellationReason::RunCancelled,
            },
        ),
        (
            "Unknown",
            OutcomeResult::Unknown {
                context: "finality not observed within window".to_string(),
                resolution_hint: ResolutionHint::CheckChain,
            },
        ),
    ];

    for (name, variant) in &variants {
        let json_str = serde_json::to_string(variant).expect("serialize variant");
        group.bench_with_input(
            BenchmarkId::new("OutcomeResult/serialize", *name),
            variant,
            |b, v| {
                b.iter(|| {
                    let json = serde_json::to_string(v).expect("serialize");
                    criterion::black_box(json);
                });
            },
        );
        group.bench_with_input(
            BenchmarkId::new("OutcomeResult/deserialize", *name),
            &json_str,
            |b, json| {
                b.iter(|| {
                    let back: OutcomeResult =
                        serde_json::from_str(json).expect("deserialize");
                    criterion::black_box(&back);
                });
            },
        );
    }

    // IdempotencyKey serde round-trip
    let key = IdempotencyKey::generate(
        RunId::new(),
        1,
        0,
        EffectKind::ModelCall,
        IdempotencyKey::hash_params(b"bench"),
    );
    let key_json = serde_json::to_string(&key).expect("serialize key");

    group.bench_function("IdempotencyKey/serialize", |b| {
        b.iter(|| {
            let json = serde_json::to_string(&key).expect("serialize");
            criterion::black_box(json);
        });
    });

    group.bench_function("IdempotencyKey/deserialize", |b| {
        b.iter(|| {
            let back: IdempotencyKey =
                serde_json::from_str(&key_json).expect("deserialize");
            criterion::black_box(back);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 7. EffectOutcome construction
// ---------------------------------------------------------------------------

fn bench_outcome_construction(c: &mut Criterion) {
    let mut group = c.benchmark_group("outcome_construction");
    group.sample_size(500);

    let intent_id = EffectId::new();
    let run_id = RunId::new();

    group.bench_function("Success", |b| {
        b.iter(|| {
            let outcome = EffectOutcome {
                id: EffectOutcomeId::new(),
                attempt_id: EffectAttemptId::new(),
                intent_id,
                run_id,
                result: OutcomeResult::Success {
                    data: serde_json::json!({"ok": true}),
                },
                observed_at: Utc::now(),
                digest: [0u8; 32],
            };
            criterion::black_box(outcome.id);
        });
    });

    group.bench_function("Failure", |b| {
        b.iter(|| {
            let outcome = EffectOutcome {
                id: EffectOutcomeId::new(),
                attempt_id: EffectAttemptId::new(),
                intent_id,
                run_id,
                result: OutcomeResult::Failure {
                    error_class: ErrorClass::NetworkError,
                    message: "connection refused".to_string(),
                    retriable: true,
                },
                observed_at: Utc::now(),
                digest: [0u8; 32],
            };
            criterion::black_box(outcome.id);
        });
    });

    group.bench_function("Timeout", |b| {
        b.iter(|| {
            let outcome = EffectOutcome {
                id: EffectOutcomeId::new(),
                attempt_id: EffectAttemptId::new(),
                intent_id,
                run_id,
                result: OutcomeResult::Timeout {
                    waited_secs: 120,
                    partial_work_possible: true,
                },
                observed_at: Utc::now(),
                digest: [0u8; 32],
            };
            criterion::black_box(outcome.id);
        });
    });

    group.bench_function("Cancelled", |b| {
        b.iter(|| {
            let outcome = EffectOutcome {
                id: EffectOutcomeId::new(),
                attempt_id: EffectAttemptId::new(),
                intent_id,
                run_id,
                result: OutcomeResult::Cancelled {
                    partial_work_possible: false,
                    reason: CancellationReason::LeaseExpired,
                },
                observed_at: Utc::now(),
                digest: [0u8; 32],
            };
            criterion::black_box(outcome.id);
        });
    });

    group.bench_function("Unknown", |b| {
        b.iter(|| {
            let outcome = EffectOutcome {
                id: EffectOutcomeId::new(),
                attempt_id: EffectAttemptId::new(),
                intent_id,
                run_id,
                result: OutcomeResult::Unknown {
                    context: "tx broadcast; finality unknown".to_string(),
                    resolution_hint: ResolutionHint::CheckChain,
                },
                observed_at: Utc::now(),
                digest: [0u8; 32],
            };
            criterion::black_box(outcome.id);
        });
    });

    // Outcome with BLAKE3 digest computation
    group.bench_function("with_digest_computation", |b| {
        b.iter(|| {
            let result = OutcomeResult::Success {
                data: serde_json::json!({"tx_hash": "0xdeadbeef"}),
            };
            let result_bytes = serde_json::to_vec(&result).expect("serialize");
            let digest = *blake3::hash(&result_bytes).as_bytes();
            let outcome = EffectOutcome {
                id: EffectOutcomeId::new(),
                attempt_id: EffectAttemptId::new(),
                intent_id,
                run_id,
                result,
                observed_at: Utc::now(),
                digest,
            };
            criterion::black_box(outcome.id);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 8. Priority queue ordering
// ---------------------------------------------------------------------------

fn bench_priority_ordering(c: &mut Criterion) {
    let mut group = c.benchmark_group("priority_ordering");
    group.sample_size(200);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    // EffectPriority comparison
    group.bench_function("priority_comparison", |b| {
        let priorities = [
            EffectPriority::Low,
            EffectPriority::Normal,
            EffectPriority::High,
            EffectPriority::Critical,
        ];
        b.iter(|| {
            let mut sorted = priorities;
            sorted.sort();
            criterion::black_box(sorted);
        });
    });

    // Sort a batch of intents by priority
    group.bench_function("sort_100_intents_by_priority", |b| {
        let priorities = [
            EffectPriority::Low,
            EffectPriority::Normal,
            EffectPriority::High,
            EffectPriority::Critical,
        ];
        b.iter(|| {
            let mut intents: Vec<EffectIntent> = (0..100)
                .map(|i| {
                    let mut intent = make_effect_intent(EffectKind::ModelCall);
                    intent.priority = priorities[i % 4];
                    intent.sequence = i as u64;
                    intent
                })
                .collect();
            intents.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.sequence.cmp(&b.sequence)));
            criterion::black_box(intents[0].priority);
        });
    });

    // Propose intents with different priorities and measure claiming order
    group.bench_function("propose_mixed_priorities", |b| {
        b.iter(|| {
            rt.block_on(async {
                let (_, dyn_store) = InMemoryStore::new_arc();
                let pipeline = EffectPipeline::new(dyn_store, WorkerId::new());
                let run_id = RunId::new();

                for (i, &priority) in [
                    EffectPriority::Low,
                    EffectPriority::Critical,
                    EffectPriority::Normal,
                    EffectPriority::High,
                ]
                .iter()
                .enumerate()
                {
                    pipeline
                        .propose(make_spec_with_priority(
                            run_id,
                            EffectKind::ModelCall,
                            i as u64 + 1,
                            priority,
                        ))
                        .await
                        .expect("propose");
                }

                criterion::black_box(run_id);
            });
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 9. Bulk effect creation (100, 1000 intents)
// ---------------------------------------------------------------------------

fn bench_bulk_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("bulk_creation");
    group.sample_size(50);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    for &count in &[100u64, 1000] {
        group.bench_with_input(
            BenchmarkId::new("propose_n_intents", count),
            &count,
            |b, &count| {
                b.iter(|| {
                    rt.block_on(async {
                        let (_, dyn_store) = InMemoryStore::new_arc();
                        let pipeline = EffectPipeline::new(dyn_store, WorkerId::new());
                        let run_id = RunId::new();

                        for seq in 1..=count {
                            pipeline
                                .propose(make_spec(run_id, EffectKind::ModelCall, seq))
                                .await
                                .expect("propose");
                        }

                        criterion::black_box(run_id);
                    });
                });
            },
        );

        // Bulk in-memory intent construction (no pipeline, pure struct creation)
        group.bench_with_input(
            BenchmarkId::new("construct_n_intents", count),
            &count,
            |b, &count| {
                b.iter(|| {
                    let intents: Vec<EffectIntent> = (0..count)
                        .map(|_| make_effect_intent(EffectKind::ToolCall))
                        .collect();
                    criterion::black_box(intents.len());
                });
            },
        );

        // Bulk propose + claim cycle
        group.bench_with_input(
            BenchmarkId::new("propose_and_claim_n", count),
            &count,
            |b, &count| {
                b.iter(|| {
                    rt.block_on(async {
                        let (_, dyn_store) = InMemoryStore::new_arc();
                        let pipeline = EffectPipeline::new(dyn_store, WorkerId::new());
                        let run_id = RunId::new();

                        // Propose all
                        let mut intent_ids = Vec::with_capacity(count as usize);
                        for seq in 1..=count {
                            let id = pipeline
                                .propose(make_spec(run_id, EffectKind::ChainRead, seq))
                                .await
                                .expect("propose");
                            intent_ids.push(id);
                        }

                        // Claim and complete all
                        for _ in 0..count {
                            if let Some(guard) = pipeline
                                .claim_with_duration(Duration::from_secs(60))
                                .await
                                .expect("claim")
                            {
                                let outcome = make_outcome(guard.intent_id, run_id);
                                pipeline
                                    .record_outcome(guard.intent_id, &outcome)
                                    .await
                                    .expect("record_outcome");
                                guard.complete();
                            }
                        }

                        criterion::black_box(intent_ids.len());
                    });
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 10. Effect filtering by run_id, by kind, by state
// ---------------------------------------------------------------------------

fn bench_filtering(c: &mut Criterion) {
    let mut group = c.benchmark_group("filtering");
    group.sample_size(200);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    // Prepare a store with 200 intents across 4 runs, mixed kinds and states
    let run_ids: Vec<RunId> = (0..4).map(|_| RunId::new()).collect();
    let kinds = [
        EffectKind::ModelCall,
        EffectKind::ToolCall,
        EffectKind::ChainRead,
        EffectKind::Broadcast,
        EffectKind::Simulation,
    ];

    let setup_store = || {
        rt.block_on(async {
            let (inner, dyn_store) = InMemoryStore::new_arc();
            let pipeline = EffectPipeline::new(Arc::clone(&dyn_store), WorkerId::new());

            for (i, run_id) in run_ids.iter().enumerate() {
                for seq in 0..50u64 {
                    let kind = kinds[(i + seq as usize) % kinds.len()];
                    pipeline
                        .propose(make_spec(*run_id, kind, (i as u64) * 100 + seq + 1))
                        .await
                        .expect("propose");
                }
            }

            // Claim some intents to create mixed states
            for _ in 0..50 {
                let _ = pipeline
                    .claim_with_duration(Duration::from_secs(600))
                    .await;
            }

            (inner, dyn_store)
        })
    };

    // Filter by run_id (via EffectStore::get_by_run)
    let target_run_id = run_ids[0];
    group.bench_function("filter_by_run_id", |b| {
        let (_, dyn_store) = setup_store();
        b.iter(|| {
            rt.block_on(async {
                let results = dyn_store.get_by_run(target_run_id).await.expect("get_by_run");
                criterion::black_box(results.len());
            });
        });
    });

    // Filter by kind (in-memory scan of all intents)
    group.bench_function("filter_by_kind_model_call", |b| {
        let (inner, _) = setup_store();
        b.iter(|| {
            let intents = inner.intents.lock().expect("lock");
            let matching: Vec<_> = intents
                .values()
                .filter(|i| {
                    i.payload
                        .get("kind")
                        .and_then(|v| v.as_str())
                        .map(|k| k == "model_call")
                        .unwrap_or(false)
                })
                .collect();
            criterion::black_box(matching.len());
        });
    });

    // Filter by state (pending)
    group.bench_function("filter_by_state_pending", |b| {
        let (inner, _) = setup_store();
        b.iter(|| {
            let intents = inner.intents.lock().expect("lock");
            let matching: Vec<_> = intents
                .values()
                .filter(|i| i.state.eq_ignore_ascii_case("pending"))
                .collect();
            criterion::black_box(matching.len());
        });
    });

    // Filter by state (claimed)
    group.bench_function("filter_by_state_claimed", |b| {
        let (inner, _) = setup_store();
        b.iter(|| {
            let intents = inner.intents.lock().expect("lock");
            let matching: Vec<_> = intents
                .values()
                .filter(|i| i.state.eq_ignore_ascii_case("claimed"))
                .collect();
            criterion::black_box(matching.len());
        });
    });

    // Combined filter: by run_id AND kind
    group.bench_function("filter_by_run_id_and_kind", |b| {
        let (inner, _) = setup_store();
        b.iter(|| {
            let intents = inner.intents.lock().expect("lock");
            let matching: Vec<_> = intents
                .values()
                .filter(|i| {
                    i.run_id == target_run_id
                        && i.payload
                            .get("kind")
                            .and_then(|v| v.as_str())
                            .map(|k| k == "tool_call")
                            .unwrap_or(false)
                })
                .collect();
            criterion::black_box(matching.len());
        });
    });

    // Combined filter: by run_id AND state
    group.bench_function("filter_by_run_id_and_state", |b| {
        let (inner, _) = setup_store();
        b.iter(|| {
            let intents = inner.intents.lock().expect("lock");
            let matching: Vec<_> = intents
                .values()
                .filter(|i| {
                    i.run_id == target_run_id
                        && i.state.eq_ignore_ascii_case("pending")
                })
                .collect();
            criterion::black_box(matching.len());
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Criterion harness
// ---------------------------------------------------------------------------

criterion_group!(
    benches,
    bench_intent_creation,
    bench_idempotency_key,
    bench_state_transitions,
    bench_full_pipeline_cycle,
    bench_concurrent_claiming,
    bench_serde,
    bench_outcome_construction,
    bench_priority_ordering,
    bench_bulk_creation,
    bench_filtering,
);
criterion_main!(benches);
