//! Criterion benchmarks for the EventBus publish/subscribe performance.
//!
//! Covers:
//! - `publish` with 1 subscriber
//! - `publish` with 10 subscribers
//! - Event recording to in-memory EventStore via EventRecorder
//!
//! PRD-15 performance benchmarks.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

use polkagent_core::{
    event::{EventCorrelation, EventKind, RunEvent},
    ids::{EventId, RunId},
};
use polkagent_event::{bus::EventBus, recorder::EventRecorder};
use polkagent_store_trait::event::{EventFilter, EventStore, EventStoreError, StoredEvent};

// ---------------------------------------------------------------------------
// Minimal in-memory EventStore for benchmarks
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct InMemoryEventStore {
    durable: Mutex<Vec<StoredEvent>>,
    sequences: Mutex<HashMap<String, u64>>,
    terminals: Mutex<HashSet<String>>,
}

impl InMemoryEventStore {
    fn new_arc() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

#[async_trait]
impl EventStore for InMemoryEventStore {
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

        let mut durable = self.durable.lock().expect("lock");
        let global_seq = (durable.len() + 1) as u64;
        event.global_sequence = global_seq;
        durable.push(event.clone());
        Ok(event)
    }

    async fn append_diagnostic(
        &self,
        event: StoredEvent,
        _expires_at: String,
    ) -> Result<(), EventStoreError> {
        let mut durable = self.durable.lock().expect("lock");
        durable.push(event);
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
                    .map_or(true, |rid| e.run_id == rid.to_string())
            })
            .cloned()
            .collect())
    }

    async fn max_sequence(&self, run_id: RunId) -> Result<u64, EventStoreError> {
        let seqs = self.sequences.lock().expect("lock");
        Ok(seqs.get(&run_id.to_string()).copied().unwrap_or(0))
    }

    async fn has_terminal_event(&self, run_id: RunId) -> Result<bool, EventStoreError> {
        let terminals = self.terminals.lock().expect("lock");
        Ok(terminals.contains(&run_id.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_event(run_id: RunId, sequence: u64) -> RunEvent {
    RunEvent::new_durable(
        EventId::new(),
        run_id.clone(),
        sequence,
        EventKind::RunCreated,
        EventCorrelation {
            run_id,
            ..Default::default()
        },
    )
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

fn bench_bus_publish(c: &mut Criterion) {
    let mut group = c.benchmark_group("event_bus_publish");

    // 1 subscriber
    group.bench_function(BenchmarkId::new("subscribers", 1), |b| {
        let bus = EventBus::new(1024);
        let _rx = bus.subscribe();
        let run_id = RunId::new();
        let mut seq = 0u64;
        b.iter(|| {
            seq += 1;
            let event = make_event(run_id.clone(), seq);
            let n = bus.publish(event);
            criterion::black_box(n);
        });
    });

    // 10 subscribers
    group.bench_function(BenchmarkId::new("subscribers", 10), |b| {
        let bus = EventBus::new(4096);
        // Hold 10 receivers to keep them alive.
        let _receivers: Vec<_> = (0..10).map(|_| bus.subscribe()).collect();
        let run_id = RunId::new();
        let mut seq = 0u64;
        b.iter(|| {
            seq += 1;
            let event = make_event(run_id.clone(), seq);
            let n = bus.publish(event);
            criterion::black_box(n);
        });
    });

    group.finish();
}

fn bench_event_recording(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let store = InMemoryEventStore::new_arc();
    let store_dyn: Arc<dyn EventStore> = store;
    let bus = EventBus::new(1024);
    let recorder = EventRecorder::new(store_dyn, bus);

    c.bench_function("event_recorder_record_durable", |b| {
        let run_id = RunId::new();
        b.iter(|| {
            rt.block_on(async {
                let event = make_event(run_id.clone(), 0 /* pre-assign */);
                let recorded = recorder.record(event).await.expect("record");
                criterion::black_box(recorded);
            });
        });
    });

    // Batch: record 100 events in a single bench iteration.
    c.bench_function("event_recorder_batch_100_events", |b| {
        b.iter(|| {
            rt.block_on(async {
                let run_id = RunId::new();
                let recorder2 = {
                    let store2 = InMemoryEventStore::new_arc();
                    let store2_dyn: Arc<dyn EventStore> = store2;
                    EventRecorder::new(store2_dyn, EventBus::new(512))
                };

                for _ in 0..100 {
                    let event = make_event(run_id.clone(), 0);
                    recorder2.record(event).await.expect("record");
                }
                criterion::black_box(run_id);
            });
        });
    });
}

criterion_group!(benches, bench_bus_publish, bench_event_recording,);
criterion_main!(benches);
