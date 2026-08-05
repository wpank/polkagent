//! Shared helpers for the integration test suite.
//!
//! This module provides in-memory store implementations and factory functions
//! used by the integration test files under `tests/`.

// This dedicated test-support library uses `expect` on fixture mutexes and
// setup invariants so failures identify the exact integration harness boundary.
#![allow(clippy::expect_used)]

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;

use polkagent_core::{EffectAttemptId, EffectId, EffectOutcomeId, RunId, TurnId, WorkerId};
use polkagent_event::{EventBus, EventRecorder};
use polkagent_run::RunManager;
use polkagent_store_trait::{
    event::{EventFilter, EventStore, EventStoreError, StoredEvent},
    EffectStore, RunStatus, RunStore, RunSummary, StoreError, StoredIntent, StoredOutcome,
};

// ---------------------------------------------------------------------------
// In-memory RunStore
// ---------------------------------------------------------------------------

/// A minimal in-memory `RunStore` suitable for integration tests.
#[derive(Debug, Default)]
pub struct MemRunStore {
    runs: Mutex<HashMap<String, RunSummary>>,
}

#[async_trait::async_trait]
impl RunStore for MemRunStore {
    async fn create(
        &self,
        run_id: RunId,
        agent_id: &str,
        status: RunStatus,
    ) -> Result<(), StoreError> {
        let mut guard = self.runs.lock().expect("lock");
        let key = run_id.to_string();
        if guard.contains_key(&key) {
            return Err(StoreError::Conflict {
                resource_type: "Run",
                id: key,
            });
        }
        guard.insert(
            key,
            RunSummary {
                id: run_id,
                agent_id: agent_id.to_owned(),
                status,
                created_at: Utc::now(),
                started_at: None,
                completed_at: None,
                deadline_at: None,
            },
        );
        Ok(())
    }

    async fn get(&self, run_id: RunId) -> Result<RunSummary, StoreError> {
        let guard = self.runs.lock().expect("lock");
        guard
            .get(&run_id.to_string())
            .cloned()
            .ok_or_else(|| StoreError::NotFound {
                resource_type: "Run",
                id: run_id.to_string(),
            })
    }

    async fn update_state(&self, run_id: RunId, new_status: RunStatus) -> Result<(), StoreError> {
        let mut guard = self.runs.lock().expect("lock");
        guard
            .get_mut(&run_id.to_string())
            .ok_or_else(|| StoreError::NotFound {
                resource_type: "Run",
                id: run_id.to_string(),
            })?
            .status = new_status;
        Ok(())
    }

    async fn list_by_agent(
        &self,
        agent_id: &str,
        limit: u32,
        _offset: u32,
    ) -> Result<Vec<RunSummary>, StoreError> {
        let guard = self.runs.lock().expect("lock");
        let mut runs: Vec<RunSummary> = guard
            .values()
            .filter(|r| r.agent_id == agent_id)
            .cloned()
            .collect();
        runs.sort_by_key(|run| Reverse(run.created_at));
        runs.truncate(limit as usize);
        Ok(runs)
    }

    async fn list_by_state(
        &self,
        status: RunStatus,
        limit: u32,
        _offset: u32,
    ) -> Result<Vec<RunSummary>, StoreError> {
        let guard = self.runs.lock().expect("lock");
        let mut runs: Vec<RunSummary> = guard
            .values()
            .filter(|r| r.status == status)
            .cloned()
            .collect();
        runs.sort_by_key(|run| Reverse(run.created_at));
        runs.truncate(limit as usize);
        Ok(runs)
    }

    async fn insert_turn(
        &self,
        _turn_id: TurnId,
        _run_id: RunId,
        _sequence: u32,
        _role: &str,
        _started_at: &str,
        _completed_at: Option<&str>,
        _input_tokens: u32,
        _output_tokens: u32,
    ) -> Result<(), StoreError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// In-memory EventStore
// ---------------------------------------------------------------------------

const TERMINAL_TYPES: &[&str] = &[
    "run_completed",
    "run_failed",
    "run_cancelled",
    "run_timed_out",
];

/// A minimal in-memory `EventStore` suitable for integration tests.
#[derive(Debug, Default)]
pub struct MemEventStore {
    pub durable: Mutex<Vec<StoredEvent>>,
    pub sequences: Mutex<HashMap<String, u64>>,
    pub terminal: Mutex<HashSet<String>>,
}

#[async_trait::async_trait]
impl EventStore for MemEventStore {
    async fn append_durable(&self, mut event: StoredEvent) -> Result<StoredEvent, EventStoreError> {
        let mut seqs = self.sequences.lock().expect("lock");
        let current = seqs.get(&event.run_id).copied().unwrap_or(0);
        if event.sequence <= current {
            return Err(EventStoreError::NonMonotonicSequence {
                run_id: event.run_id.clone(),
                current,
                proposed: event.sequence,
            });
        }
        seqs.insert(event.run_id.clone(), event.sequence);

        let mut term = self.terminal.lock().expect("lock");
        if TERMINAL_TYPES.contains(&event.event_type.as_str()) && !term.insert(event.run_id.clone())
        {
            return Err(EventStoreError::DuplicateTerminalEvent {
                run_id: event.run_id.clone(),
            });
        }

        let mut durable = self.durable.lock().expect("lock");
        event.global_sequence = (durable.len() + 1) as u64;
        durable.push(event.clone());
        Ok(event)
    }

    async fn append_diagnostic(
        &self,
        _event: StoredEvent,
        _expires_at: String,
    ) -> Result<(), EventStoreError> {
        Ok(())
    }

    async fn read_from_cursor(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        let durable = self.durable.lock().expect("lock");
        Ok(durable
            .iter()
            .filter(|e| e.global_sequence > cursor)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn read_run_events(&self, run_id: RunId) -> Result<Vec<StoredEvent>, EventStoreError> {
        let durable = self.durable.lock().expect("lock");
        Ok(durable
            .iter()
            .filter(|e| e.run_id == run_id.to_string())
            .cloned()
            .collect())
    }

    async fn query(&self, filter: EventFilter) -> Result<Vec<StoredEvent>, EventStoreError> {
        let durable = self.durable.lock().expect("lock");
        Ok(durable
            .iter()
            .filter(|e| {
                filter
                    .run_id
                    .as_ref()
                    .is_none_or(|rid| e.run_id == rid.to_string())
            })
            .cloned()
            .collect())
    }

    async fn max_sequence(&self, run_id: RunId) -> Result<u64, EventStoreError> {
        let seqs = self.sequences.lock().expect("lock");
        Ok(seqs.get(&run_id.to_string()).copied().unwrap_or(0))
    }

    async fn has_terminal_event(&self, run_id: RunId) -> Result<bool, EventStoreError> {
        let term = self.terminal.lock().expect("lock");
        Ok(term.contains(&run_id.to_string()))
    }
}

// ---------------------------------------------------------------------------
// In-memory EffectStore
// ---------------------------------------------------------------------------

/// A minimal in-memory `EffectStore` suitable for integration tests.
#[derive(Debug, Default)]
pub struct MemEffectStore {
    pub intents: Mutex<HashMap<EffectId, StoredIntent>>,
    pub attempts: Mutex<Vec<serde_json::Value>>,
    pub outcomes: Mutex<Vec<StoredOutcome>>,
}

#[async_trait::async_trait]
impl EffectStore for MemEffectStore {
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
        let Some(intent) = intents.get_mut(&intent_id) else {
            return Ok(());
        };
        if intent.lease_owner == Some(worker_id) {
            intent.state = "pending".to_string();
            intent.lease_owner = None;
            intent.lease_expires = None;
        }
        Ok(())
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
                    && i.lease_expires.is_some_and(|expires| expires < cutoff)
            })
            .cloned()
            .collect())
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

    async fn unconsumed_outcomes(&self, run_id: RunId) -> Result<Vec<StoredOutcome>, StoreError> {
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
// Factory helpers
// ---------------------------------------------------------------------------

/// Build a `RunManager` backed by in-memory stores, returning both the manager
/// and the event store for verification.
pub fn make_run_manager() -> (RunManager, Arc<MemEventStore>) {
    let run_store: Arc<dyn RunStore> = Arc::new(MemRunStore::default());
    let event_store = Arc::new(MemEventStore::default());
    let event_store_dyn = Arc::clone(&event_store) as Arc<dyn EventStore>;
    let bus = EventBus::new(64);
    let recorder = EventRecorder::new(event_store_dyn, bus);
    let manager = RunManager::new(run_store, recorder);
    (manager, event_store)
}

/// Build an `EffectPipeline` backed by an in-memory store.
pub fn make_effect_pipeline() -> (polkagent_effect::EffectPipeline, Arc<MemEffectStore>) {
    let store = Arc::new(MemEffectStore::default());
    let store_dyn: Arc<dyn EffectStore> = Arc::clone(&store) as Arc<dyn EffectStore>;
    let pipeline = polkagent_effect::EffectPipeline::new(store_dyn, WorkerId::new());
    (pipeline, store)
}
