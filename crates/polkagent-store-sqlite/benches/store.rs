//! Criterion benchmarks for the SQLite store performance-critical paths.
//!
//! Covers all ten benchmark categories requested:
//!
//!  1. Run CRUD (create, get, update_state, list_by_agent, list_by_state)
//!  2. Effect pipeline (propose_intent, claim_intent, record_attempt, record_outcome)
//!  3. Artifact operations (store, get, get_body, verify, list_for_run)
//!  4. Event operations (append, list_for_run, list_by_kinds)
//!  5. Payment operations (record_charge, get_budget_usage)
//!  6. Conversation operations (create, append_message, get)
//!  7. Migration performance (full schema creation)
//!  8. Concurrent read/write (multiple threads reading while one writes)
//!  9. Bulk insert (100, 1_000, 10_000 records)
//! 10. Query with filters (by agent, by status, by time range)
//!
//! Uses file-backed SQLite databases via `tempfile` so that WAL mode is
//! exercised (in-memory SQLite does not support WAL).
//!
//! PRD-15 performance benchmarks (enhanced).

use chrono::{DateTime, Utc};
use criterion::{
    criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput,
};
use tempfile::NamedTempFile;
use uuid::Uuid;

use polkagent_store_sqlite::{
    migrations,
    pool::SqlitePool,
    store::{SqliteArtifactStore, SqliteEffectStore, SqliteEventStore, SqliteRunStore},
};

// ---------------------------------------------------------------------------
// DB setup helpers
// ---------------------------------------------------------------------------

/// Open a file-backed SQLite pool with migrations applied.
///
/// Returns the pool and the tempfile handle (which must be kept alive to
/// prevent the OS from deleting the file while the benchmark runs).
fn open_bench_db() -> (SqlitePool, NamedTempFile) {
    let tmpfile = NamedTempFile::new().expect("tempfile");
    let pool = SqlitePool::open(tmpfile.path()).expect("open db");
    {
        let writer = pool.writer();
        migrations::migrate(&writer).expect("migrate");
    }
    (pool, tmpfile)
}

/// Pre-insert a bench agent and return its ID.
fn seed_agent(run_store: &SqliteRunStore) -> String {
    run_store
        .create_agent(
            "bench-agent",
            Some("A benchmark test agent"),
            r#"{"model":"claude-sonnet-4","version":"1.0"}"#,
        )
        .expect("create agent")
        .id
}

/// Pre-insert a run for the given agent and return its ID.
fn seed_run(run_store: &SqliteRunStore, agent_id: &str) -> String {
    run_store
        .create_run(agent_id, None, r#"{"task":"benchmark","priority":"high"}"#)
        .expect("create run")
        .id
}

/// Build a tokio runtime suitable for blocking on async calls in benchmarks.
fn build_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

// ============================================================================
// 1. Run CRUD operations
// ============================================================================

fn bench_run_crud(c: &mut Criterion) {
    let mut group = c.benchmark_group("run_crud");
    group.sample_size(200);

    // --- create ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool);
        let agent_id = seed_agent(&run_store);

        group.bench_function("create_run", |b| {
            b.iter(|| {
                let row = run_store
                    .create_run(&agent_id, None, r#"{"bench":true}"#)
                    .expect("create run");
                criterion::black_box(row.id);
            });
        });
    }

    // --- get (point lookup) ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool);
        let agent_id = seed_agent(&run_store);
        let run_id = seed_run(&run_store, &agent_id);

        group.bench_function("get_run", |b| {
            b.iter(|| {
                let row = run_store.get_run(&run_id).expect("get run");
                criterion::black_box(row);
            });
        });
    }

    // --- update_state ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool);
        let agent_id = seed_agent(&run_store);
        let run_id = seed_run(&run_store, &agent_id);

        let states = ["running", "paused", "running", "completed"];
        let mut idx = 0usize;

        group.bench_function("update_run_state", |b| {
            b.iter(|| {
                let st = states[idx % states.len()];
                run_store
                    .update_run_state(&run_id, st, None)
                    .expect("update state");
                idx += 1;
            });
        });
    }

    // --- list_by_agent (10 pre-seeded runs) ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool);
        let agent_id = seed_agent(&run_store);
        for _ in 0..10 {
            seed_run(&run_store, &agent_id);
        }

        group.bench_function("runs_for_agent_10", |b| {
            b.iter(|| {
                let rows = run_store.runs_for_agent(&agent_id).expect("runs for agent");
                criterion::black_box(rows.len());
            });
        });
    }

    // --- list_agents_by_state ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool);
        for i in 0..20 {
            run_store
                .create_agent(
                    &format!("agent-{i}"),
                    Some("benchmarking agent"),
                    r#"{"model":"test"}"#,
                )
                .expect("create agent");
        }

        group.bench_function("list_agents_by_state", |b| {
            b.iter(|| {
                let rows = run_store
                    .list_agents(Some("active"), false)
                    .expect("list agents");
                criterion::black_box(rows.len());
            });
        });
    }

    group.finish();
}

// ============================================================================
// 2. Effect pipeline
// ============================================================================

fn bench_effect_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("effect_pipeline");
    group.sample_size(200);

    // --- propose_intent ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool.clone());
        let effect_store = SqliteEffectStore::new(pool);
        let agent_id = seed_agent(&run_store);
        let run_id = seed_run(&run_store, &agent_id);
        let mut counter = 0u64;

        group.bench_function("propose_intent", |b| {
            b.iter(|| {
                counter += 1;
                let intent = effect_store
                    .create_intent(
                        &run_id,
                        None,
                        None,
                        "model_call",
                        r#"{"input":"benchmark payload","temperature":0.7}"#,
                        &format!("bench-key-{counter}"),
                    )
                    .expect("create intent");
                criterion::black_box(&intent.id);
            });
        });
    }

    // --- claim_intent ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool.clone());
        let effect_store = SqliteEffectStore::new(pool);
        let agent_id = seed_agent(&run_store);
        let run_id = seed_run(&run_store, &agent_id);

        // Pre-create intents to claim.
        let mut intent_ids = Vec::new();
        for i in 0..500 {
            let intent = effect_store
                .create_intent(
                    &run_id,
                    None,
                    None,
                    "model_call",
                    r#"{"input":"claim test"}"#,
                    &format!("claim-key-{i}"),
                )
                .expect("create intent");
            intent_ids.push(intent.id);
        }
        let mut claim_idx = 0usize;

        group.bench_function("claim_intent", |b| {
            b.iter(|| {
                if claim_idx < intent_ids.len() {
                    let lease_until = Utc::now() + chrono::Duration::seconds(60);
                    effect_store
                        .claim_intent(&intent_ids[claim_idx], "bench-worker", lease_until)
                        .expect("claim intent");
                    claim_idx += 1;
                }
            });
        });
    }

    // --- propose + claim cycle (combined) ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool.clone());
        let effect_store = SqliteEffectStore::new(pool);
        let agent_id = seed_agent(&run_store);
        let run_id = seed_run(&run_store, &agent_id);
        let mut counter = 0u64;

        group.bench_function("propose_and_claim", |b| {
            b.iter(|| {
                counter += 1;
                let idem_key = format!("cycle-key-{counter}");
                let intent = effect_store
                    .create_intent(
                        &run_id,
                        None,
                        None,
                        "model_call",
                        r#"{"input":"benchmark payload"}"#,
                        &idem_key,
                    )
                    .expect("create intent");
                let lease_until = Utc::now() + chrono::Duration::seconds(60);
                effect_store
                    .claim_intent(&intent.id, "bench-worker", lease_until)
                    .expect("claim intent");
                criterion::black_box(&intent.id);
            });
        });
    }

    // --- record_attempt ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool.clone());
        let effect_store = SqliteEffectStore::new(pool);
        let agent_id = seed_agent(&run_store);
        let run_id = seed_run(&run_store, &agent_id);
        let intent = effect_store
            .create_intent(
                &run_id,
                None,
                None,
                "model_call",
                r#"{"input":"attempt test"}"#,
                "attempt-key",
            )
            .expect("create intent");
        let mut attempt_num = 0i64;

        group.bench_function("record_attempt", |b| {
            b.iter(|| {
                attempt_num += 1;
                let attempt = effect_store
                    .create_attempt(&intent.id, attempt_num)
                    .expect("create attempt");
                criterion::black_box(attempt.id);
            });
        });
    }

    // --- record_outcome ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool.clone());
        let effect_store = SqliteEffectStore::new(pool);
        let agent_id = seed_agent(&run_store);
        let run_id = seed_run(&run_store, &agent_id);

        // Pre-create intents for outcomes (each intent can have only one outcome).
        let mut intent_ids = Vec::new();
        for i in 0..500 {
            let intent = effect_store
                .create_intent(
                    &run_id,
                    None,
                    None,
                    "model_call",
                    r#"{"input":"outcome test"}"#,
                    &format!("outcome-key-{i}"),
                )
                .expect("create intent");
            intent_ids.push(intent.id);
        }
        let mut outcome_idx = 0usize;

        group.bench_function("record_outcome", |b| {
            b.iter(|| {
                if outcome_idx < intent_ids.len() {
                    let outcome = effect_store
                        .record_outcome(
                            &intent_ids[outcome_idx],
                            "success",
                            r#"{"output":"benchmark result","tokens":150}"#,
                        )
                        .expect("record outcome");
                    criterion::black_box(outcome.id);
                    outcome_idx += 1;
                }
            });
        });
    }

    group.finish();
}

// ============================================================================
// 3. Artifact operations
// ============================================================================

fn bench_artifact_ops(c: &mut Criterion) {
    let mut group = c.benchmark_group("artifact_ops");
    group.sample_size(200);

    // --- create artifact (metadata) ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool.clone());
        let artifact_store = SqliteArtifactStore::new(pool);
        let agent_id = seed_agent(&run_store);
        let run_id = seed_run(&run_store, &agent_id);

        let body = b"benchmark artifact body content for testing";
        let digest = blake3::hash(body).to_hex().to_string();

        group.bench_function("create_artifact", |b| {
            b.iter(|| {
                let row = artifact_store
                    .create_artifact(
                        Some(&run_id),
                        "log",
                        &digest,
                        body.len() as i64,
                        r#"{"source":"benchmark","format":"text/plain"}"#,
                    )
                    .expect("create artifact");
                criterion::black_box(row.id);
            });
        });
    }

    // --- get artifact (point lookup) ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool.clone());
        let artifact_store = SqliteArtifactStore::new(pool);
        let agent_id = seed_agent(&run_store);
        let run_id = seed_run(&run_store, &agent_id);

        let body = b"benchmark artifact body";
        let digest = blake3::hash(body).to_hex().to_string();
        let art = artifact_store
            .create_artifact(
                Some(&run_id),
                "log",
                &digest,
                body.len() as i64,
                r#"{"source":"benchmark"}"#,
            )
            .expect("create artifact");

        group.bench_function("get_artifact", |b| {
            b.iter(|| {
                let row = artifact_store.get_artifact(&art.id).expect("get artifact");
                criterion::black_box(row);
            });
        });
    }

    // --- store_body + get_body ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let artifact_store = SqliteArtifactStore::new(pool);

        let body = b"This is the artifact body content used for benchmarking retrieval speed";
        let digest = blake3::hash(body).to_hex().to_string();
        artifact_store
            .store_body(&digest, body)
            .expect("store body");

        group.bench_function("get_body", |b| {
            b.iter(|| {
                let data = artifact_store.get_body(&digest).expect("get body");
                criterion::black_box(data.len());
            });
        });
    }

    // --- store_body (content-addressed, dedup on same digest) ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let artifact_store = SqliteArtifactStore::new(pool);
        let body = b"dedup benchmark body content";
        let digest = blake3::hash(body).to_hex().to_string();

        group.bench_function("store_body", |b| {
            b.iter(|| {
                artifact_store
                    .store_body(&digest, body)
                    .expect("store body");
            });
        });
    }

    // --- list_for_run (20 artifacts seeded) ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool.clone());
        let artifact_store = SqliteArtifactStore::new(pool);
        let agent_id = seed_agent(&run_store);
        let run_id = seed_run(&run_store, &agent_id);

        for i in 0..20 {
            let body = format!("artifact body number {i}");
            let digest = blake3::hash(body.as_bytes()).to_hex().to_string();
            artifact_store
                .create_artifact(
                    Some(&run_id),
                    "log",
                    &digest,
                    body.len() as i64,
                    &format!(r#"{{"index":{i}}}"#),
                )
                .expect("create artifact");
        }

        group.bench_function("artifacts_for_run_20", |b| {
            b.iter(|| {
                let rows = artifact_store
                    .artifacts_for_run(&run_id)
                    .expect("artifacts for run");
                criterion::black_box(rows.len());
            });
        });
    }

    group.finish();
}

// ============================================================================
// 4. Event operations
// ============================================================================

fn bench_event_ops(c: &mut Criterion) {
    let mut group = c.benchmark_group("event_ops");
    group.sample_size(200);

    // --- append single event ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool.clone());
        let event_store = SqliteEventStore::new(pool);
        let agent_id = seed_agent(&run_store);
        let run_id = seed_run(&run_store, &agent_id);
        let mut seq = 0i64;

        group.bench_function("append_event", |b| {
            b.iter(|| {
                seq += 1;
                let row = event_store
                    .append_event(
                        &run_id,
                        seq,
                        "turn_started",
                        r#"{"turn_number":1,"model":"claude-sonnet-4"}"#,
                        Some("corr-bench-1"),
                        1,
                    )
                    .expect("append event");
                criterion::black_box(row.id);
            });
        });
    }

    // --- append batch of 100 events ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool.clone());
        let event_store = SqliteEventStore::new(pool);
        let agent_id = seed_agent(&run_store);
        let run_id = seed_run(&run_store, &agent_id);
        let mut base_seq = 0i64;

        group.throughput(Throughput::Elements(100));
        group.bench_function("append_100_events", |b| {
            b.iter(|| {
                for _ in 0..100 {
                    base_seq += 1;
                    event_store
                        .append_event(
                            &run_id,
                            base_seq,
                            "step_executed",
                            r#"{"bench":true,"step":"tool_call"}"#,
                            None,
                            1,
                        )
                        .expect("append event");
                }
            });
        });
    }

    // --- events_for_run (100 events seeded) ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool.clone());
        let event_store = SqliteEventStore::new(pool);
        let agent_id = seed_agent(&run_store);
        let run_id = seed_run(&run_store, &agent_id);

        for seq in 1..=100 {
            event_store
                .append_event(
                    &run_id,
                    seq,
                    if seq % 3 == 0 {
                        "tool_result"
                    } else if seq % 3 == 1 {
                        "turn_started"
                    } else {
                        "step_executed"
                    },
                    r#"{"bench":true}"#,
                    Some("corr-bench"),
                    1,
                )
                .expect("seed event");
        }

        group.bench_function("events_for_run_100", |b| {
            b.iter(|| {
                let rows = event_store
                    .events_for_run(&run_id)
                    .expect("events for run");
                criterion::black_box(rows.len());
            });
        });
    }

    // --- events_after (tail read, starting from seq 80 out of 100) ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool.clone());
        let event_store = SqliteEventStore::new(pool);
        let agent_id = seed_agent(&run_store);
        let run_id = seed_run(&run_store, &agent_id);

        for seq in 1..=100 {
            event_store
                .append_event(&run_id, seq, "turn_started", r#"{"bench":true}"#, None, 1)
                .expect("seed event");
        }

        group.bench_function("events_after_seq_80", |b| {
            b.iter(|| {
                let rows = event_store
                    .events_after(&run_id, 80)
                    .expect("events after");
                criterion::black_box(rows.len());
            });
        });
    }

    // --- next_sequence ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool.clone());
        let event_store = SqliteEventStore::new(pool);
        let agent_id = seed_agent(&run_store);
        let run_id = seed_run(&run_store, &agent_id);

        for seq in 1..=50 {
            event_store
                .append_event(&run_id, seq, "turn_started", r#"{"bench":true}"#, None, 1)
                .expect("seed event");
        }

        group.bench_function("next_sequence", |b| {
            b.iter(|| {
                let next = event_store.next_sequence(&run_id).expect("next seq");
                criterion::black_box(next);
            });
        });
    }

    group.finish();
}

// ============================================================================
// 5. Payment operations
// ============================================================================

fn bench_payment_ops(c: &mut Criterion) {
    use polkagent_payment::{
        PaymentStore,
        types::{Amount, AssetId, CostRecord, PaymentIntent, PaymentStatus},
    };

    let mut group = c.benchmark_group("payment_ops");
    group.sample_size(200);

    let rt = build_rt();

    // --- record_cost ---
    {
        let (pool, _tmpfile) = open_bench_db();

        // Seed agent + run via raw SQL (the payment trait operates on SqlitePool).
        {
            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at)
                     VALUES ('pay-agent', 'Pay Agent', 'active', '{}', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                    [],
                )
                .expect("insert agent");
            writer
                .execute(
                    "INSERT INTO runs (id, agent_id, state, params_json, created_at, updated_at)
                     VALUES ('pay-run', 'pay-agent', 'completed', '{}', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                    [],
                )
                .expect("insert run");
        }

        group.bench_function("record_cost", |b| {
            b.iter(|| {
                let record = CostRecord {
                    run_id: "pay-run".to_string(),
                    provider: "anthropic".to_string(),
                    model: "claude-sonnet-4".to_string(),
                    input_tokens: 2048,
                    output_tokens: 1024,
                    estimated_usd: 0.021,
                    recorded_at: Utc::now(),
                };
                rt.block_on(PaymentStore::record_cost(&pool, record))
                    .expect("record cost");
            });
        });
    }

    // --- get_usage ---
    {
        let (pool, _tmpfile) = open_bench_db();
        {
            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at)
                     VALUES ('usage-agent', 'Usage Agent', 'active', '{}', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                    [],
                )
                .expect("insert agent");
            writer
                .execute(
                    "INSERT INTO runs (id, agent_id, state, params_json, created_at, updated_at)
                     VALUES ('usage-run', 'usage-agent', 'completed', '{}', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                    [],
                )
                .expect("insert run");
        }

        // Seed 50 cost records.
        for _ in 0..50 {
            let record = CostRecord {
                run_id: "usage-run".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-sonnet-4".to_string(),
                input_tokens: 1500,
                output_tokens: 800,
                estimated_usd: 0.015,
                recorded_at: Utc::now(),
            };
            rt.block_on(PaymentStore::record_cost(&pool, record))
                .expect("seed cost");
        }

        let since = DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let until = DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        group.bench_function("get_usage", |b| {
            b.iter(|| {
                let summary = rt
                    .block_on(PaymentStore::get_usage(&pool, "usage-agent", since, until))
                    .expect("get usage");
                criterion::black_box(summary);
            });
        });
    }

    // --- create_intent ---
    {
        let (pool, _tmpfile) = open_bench_db();

        group.bench_function("create_payment_intent", |b| {
            b.iter(|| {
                let intent = PaymentIntent {
                    id: Uuid::now_v7(),
                    agent_id: "agent-bench".to_string(),
                    run_id: "run-bench".to_string(),
                    amount: Amount::new(1_000_000_000, AssetId::Native, 10),
                    recipient: "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"
                        .to_string(),
                    idempotency_key: Uuid::now_v7().to_string(),
                    created_at: Utc::now(),
                    status: PaymentStatus::Pending,
                    metadata: None,
                };
                rt.block_on(PaymentStore::create_intent(&pool, intent))
                    .expect("create intent");
            });
        });
    }

    group.finish();
}

// ============================================================================
// 6. Conversation operations
// ============================================================================

fn bench_conversation_ops(c: &mut Criterion) {
    use polkagent_conversation::{
        ConversationStore,
        types::{Conversation, Message, MessageContent, MessageRole},
    };
    use polkagent_core::ids::{AgentId, ConversationId};

    let mut group = c.benchmark_group("conversation_ops");
    group.sample_size(200);

    let rt = build_rt();

    // --- create conversation ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let agent_id = AgentId::new();

        group.bench_function("create_conversation", |b| {
            b.iter(|| {
                let conv = Conversation::new(ConversationId::new(), agent_id);
                let id = rt
                    .block_on(ConversationStore::create(&pool, conv))
                    .expect("create conversation");
                criterion::black_box(id);
            });
        });
    }

    // --- append_message ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let agent_id = AgentId::new();
        let conv = Conversation::new(ConversationId::new(), agent_id);
        let conv_id = conv.id;
        rt.block_on(ConversationStore::create(&pool, conv))
            .expect("create conversation");

        group.bench_function("append_message", |b| {
            b.iter(|| {
                let msg = Message {
                    id: Uuid::now_v7(),
                    conversation_id: conv_id,
                    role: MessageRole::User,
                    content: MessageContent::Text {
                        text: "Hello, how can you help me with my Polkadot staking setup?"
                            .to_string(),
                    },
                    created_at: Utc::now(),
                    token_count: Some(15),
                };
                let id = rt
                    .block_on(ConversationStore::add_message(&pool, conv_id, msg))
                    .expect("add message");
                criterion::black_box(id);
            });
        });
    }

    // --- get conversation ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let agent_id = AgentId::new();
        let conv = Conversation::new(ConversationId::new(), agent_id);
        let conv_id = conv.id;
        rt.block_on(ConversationStore::create(&pool, conv))
            .expect("create conversation");

        // Seed 20 messages.
        let base = Utc::now();
        for i in 0..20i64 {
            let role = if i % 2 == 0 {
                MessageRole::User
            } else {
                MessageRole::Assistant
            };
            let msg = Message {
                id: Uuid::now_v7(),
                conversation_id: conv_id,
                role,
                content: MessageContent::Text {
                    text: format!("Message number {i} in the conversation"),
                },
                created_at: base + chrono::Duration::milliseconds(i),
                token_count: Some(10),
            };
            rt.block_on(ConversationStore::add_message(&pool, conv_id, msg))
                .expect("seed message");
        }

        group.bench_function("get_conversation", |b| {
            b.iter(|| {
                let fetched = rt
                    .block_on(ConversationStore::get(&pool, conv_id))
                    .expect("get conversation");
                criterion::black_box(fetched);
            });
        });
    }

    // --- get_messages (paginated, 20 messages seeded) ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let agent_id = AgentId::new();
        let conv = Conversation::new(ConversationId::new(), agent_id);
        let conv_id = conv.id;
        rt.block_on(ConversationStore::create(&pool, conv))
            .expect("create conversation");

        let base = Utc::now();
        for i in 0..20i64 {
            let msg = Message {
                id: Uuid::now_v7(),
                conversation_id: conv_id,
                role: MessageRole::User,
                content: MessageContent::Text {
                    text: format!("message {i}"),
                },
                created_at: base + chrono::Duration::milliseconds(i),
                token_count: None,
            };
            rt.block_on(ConversationStore::add_message(&pool, conv_id, msg))
                .expect("seed message");
        }

        group.bench_function("get_messages_page", |b| {
            b.iter(|| {
                let msgs = rt
                    .block_on(ConversationStore::get_messages(&pool, conv_id, 10, 0))
                    .expect("get messages");
                criterion::black_box(msgs.len());
            });
        });
    }

    // --- get_recent_messages ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let agent_id = AgentId::new();
        let conv = Conversation::new(ConversationId::new(), agent_id);
        let conv_id = conv.id;
        rt.block_on(ConversationStore::create(&pool, conv))
            .expect("create conversation");

        let base = Utc::now();
        for i in 0..50i64 {
            let msg = Message {
                id: Uuid::now_v7(),
                conversation_id: conv_id,
                role: if i % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                },
                content: MessageContent::Text {
                    text: format!("conversation turn {i}"),
                },
                created_at: base + chrono::Duration::milliseconds(i),
                token_count: Some(12),
            };
            rt.block_on(ConversationStore::add_message(&pool, conv_id, msg))
                .expect("seed message");
        }

        group.bench_function("get_recent_messages_10", |b| {
            b.iter(|| {
                let msgs = rt
                    .block_on(ConversationStore::get_recent_messages(&pool, conv_id, 10))
                    .expect("get recent");
                criterion::black_box(msgs.len());
            });
        });
    }

    group.finish();
}

// ============================================================================
// 7. Migration performance
// ============================================================================

fn bench_migration(c: &mut Criterion) {
    let mut group = c.benchmark_group("migration");
    // Migrations can be slower; reduce sample size.
    group.sample_size(50);

    group.bench_function("full_schema_creation", |b| {
        b.iter_batched(
            || NamedTempFile::new().expect("tempfile"),
            |tmpfile| {
                let pool = SqlitePool::open(tmpfile.path()).expect("open db");
                let writer = pool.writer();
                migrations::migrate(&writer).expect("migrate");
                criterion::black_box(
                    migrations::current_version(&writer).expect("version"),
                );
            },
            BatchSize::SmallInput,
        );
    });

    // Also benchmark idempotent re-migration on an already-migrated DB.
    group.bench_function("idempotent_remigrate", |b| {
        let tmpfile = NamedTempFile::new().expect("tempfile");
        let pool = SqlitePool::open(tmpfile.path()).expect("open db");
        {
            let writer = pool.writer();
            migrations::migrate(&writer).expect("initial migrate");
        }

        b.iter(|| {
            let writer = pool.writer();
            migrations::migrate(&writer).expect("re-migrate");
        });
    });

    group.finish();
}

// ============================================================================
// 8. Concurrent read/write
// ============================================================================

fn bench_concurrent_rw(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_rw");
    group.sample_size(50);

    // Multiple reader threads reading while the main thread writes.
    group.bench_function("readers_while_writing", |b| {
        b.iter_batched(
            || {
                // Setup: fresh DB with seeded data.
                let tmpfile = NamedTempFile::new().expect("tempfile");
                let pool = SqlitePool::open(tmpfile.path()).expect("open db");
                {
                    let writer = pool.writer();
                    migrations::migrate(&writer).expect("migrate");
                }
                let run_store = SqliteRunStore::new(pool.clone());
                let agent_id = seed_agent(&run_store);
                for _ in 0..20 {
                    seed_run(&run_store, &agent_id);
                }
                (pool, tmpfile, agent_id)
            },
            |(pool, _tmpfile, agent_id)| {
                use std::sync::{Arc, Barrier};
                use std::thread;

                let barrier = Arc::new(Barrier::new(5)); // 4 readers + 1 writer
                let mut handles = Vec::new();

                // Spawn 4 reader threads.
                for _ in 0..4 {
                    let pool_clone = pool.clone();
                    let barrier_clone = barrier.clone();
                    let agent_id_clone = agent_id.clone();
                    handles.push(thread::spawn(move || {
                        barrier_clone.wait();
                        let reader = pool_clone.reader().expect("open reader");
                        for _ in 0..50 {
                            let mut stmt = reader
                                .prepare(
                                    "SELECT id, agent_id, conversation_id, state, params_json, \
                                     created_at, updated_at, completed_at \
                                     FROM runs WHERE agent_id = ?1",
                                )
                                .expect("prepare");
                            let rows: Vec<String> = stmt
                                .query_map([&agent_id_clone], |r| r.get(0))
                                .expect("query")
                                .collect::<Result<Vec<_>, _>>()
                                .expect("collect");
                            criterion::black_box(rows.len());
                        }
                    }));
                }

                // Writer thread (main): insert while readers are reading.
                barrier.wait();
                let run_store = SqliteRunStore::new(pool.clone());
                for _ in 0..50 {
                    run_store
                        .create_run(&agent_id, None, r#"{"concurrent":"write"}"#)
                        .expect("write during reads");
                }

                for h in handles {
                    h.join().expect("reader thread");
                }
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ============================================================================
// 9. Bulk insert
// ============================================================================

fn bench_bulk_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("bulk_insert");
    // Bulk inserts are slow; reduce sample size.
    group.sample_size(10);

    for count in [100u64, 1_000, 10_000] {
        group.throughput(Throughput::Elements(count));

        // --- bulk insert runs ---
        group.bench_with_input(
            BenchmarkId::new("runs", count),
            &count,
            |b, &count| {
                b.iter_batched(
                    || {
                        let tmpfile = NamedTempFile::new().expect("tempfile");
                        let pool = SqlitePool::open(tmpfile.path()).expect("open db");
                        {
                            let writer = pool.writer();
                            migrations::migrate(&writer).expect("migrate");
                        }
                        let run_store = SqliteRunStore::new(pool);
                        let agent_id = seed_agent(&run_store);
                        (run_store, tmpfile, agent_id)
                    },
                    |(run_store, _tmpfile, agent_id)| {
                        for _ in 0..count {
                            run_store
                                .create_run(
                                    &agent_id,
                                    None,
                                    r#"{"bulk":"insert","data":"realistic payload"}"#,
                                )
                                .expect("bulk insert run");
                        }
                    },
                    BatchSize::SmallInput,
                );
            },
        );

        // --- bulk insert events ---
        group.bench_with_input(
            BenchmarkId::new("events", count),
            &count,
            |b, &count| {
                b.iter_batched(
                    || {
                        let tmpfile = NamedTempFile::new().expect("tempfile");
                        let pool = SqlitePool::open(tmpfile.path()).expect("open db");
                        {
                            let writer = pool.writer();
                            migrations::migrate(&writer).expect("migrate");
                        }
                        let run_store = SqliteRunStore::new(pool.clone());
                        let event_store = SqliteEventStore::new(pool);
                        let agent_id = seed_agent(&run_store);
                        let run_id = seed_run(&run_store, &agent_id);
                        (event_store, tmpfile, run_id)
                    },
                    |(event_store, _tmpfile, run_id)| {
                        for seq in 1..=count as i64 {
                            event_store
                                .append_event(
                                    &run_id,
                                    seq,
                                    "step_executed",
                                    r#"{"bulk":"event","step":"tool_use"}"#,
                                    None,
                                    1,
                                )
                                .expect("bulk insert event");
                        }
                    },
                    BatchSize::SmallInput,
                );
            },
        );

        // --- bulk insert effect intents ---
        group.bench_with_input(
            BenchmarkId::new("intents", count),
            &count,
            |b, &count| {
                b.iter_batched(
                    || {
                        let tmpfile = NamedTempFile::new().expect("tempfile");
                        let pool = SqlitePool::open(tmpfile.path()).expect("open db");
                        {
                            let writer = pool.writer();
                            migrations::migrate(&writer).expect("migrate");
                        }
                        let run_store = SqliteRunStore::new(pool.clone());
                        let effect_store = SqliteEffectStore::new(pool);
                        let agent_id = seed_agent(&run_store);
                        let run_id = seed_run(&run_store, &agent_id);
                        (effect_store, tmpfile, run_id)
                    },
                    |(effect_store, _tmpfile, run_id)| {
                        for i in 0..count {
                            effect_store
                                .create_intent(
                                    &run_id,
                                    None,
                                    None,
                                    "model_call",
                                    r#"{"bulk":"intent","tokens":100}"#,
                                    &format!("bulk-intent-{i}"),
                                )
                                .expect("bulk insert intent");
                        }
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

// ============================================================================
// 10. Query with filters
// ============================================================================

fn bench_query_filters(c: &mut Criterion) {
    let mut group = c.benchmark_group("query_filters");
    group.sample_size(100);

    // --- filter runs by agent (among many agents) ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool);

        // Create 10 agents, each with 20 runs.
        let mut agent_ids = Vec::new();
        for i in 0..10 {
            let agent = run_store
                .create_agent(
                    &format!("filter-agent-{i}"),
                    Some("agent for filter benchmarks"),
                    &format!(r#"{{"model":"model-{i}"}}"#),
                )
                .expect("create agent");
            for _ in 0..20 {
                run_store
                    .create_run(&agent.id, None, r#"{"filter":"test"}"#)
                    .expect("create run");
            }
            agent_ids.push(agent.id);
        }

        group.bench_function("runs_by_agent_among_200", |b| {
            let mut idx = 0usize;
            b.iter(|| {
                let agent_id = &agent_ids[idx % agent_ids.len()];
                let rows = run_store.runs_for_agent(agent_id).expect("runs for agent");
                criterion::black_box(rows.len());
                idx += 1;
            });
        });
    }

    // --- filter agents by state ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool);

        // Create agents in various states.
        for i in 0..50 {
            let agent = run_store
                .create_agent(
                    &format!("state-agent-{i}"),
                    None,
                    r#"{"model":"test"}"#,
                )
                .expect("create agent");
            if i % 3 == 0 {
                run_store
                    .update_agent_state(&agent.id, "paused")
                    .expect("pause agent");
            } else if i % 5 == 0 {
                run_store
                    .update_agent_state(&agent.id, "archived")
                    .expect("archive agent");
            }
        }

        group.bench_function("list_agents_by_state_active", |b| {
            b.iter(|| {
                let rows = run_store
                    .list_agents(Some("active"), false)
                    .expect("list active");
                criterion::black_box(rows.len());
            });
        });

        group.bench_function("list_agents_by_state_paused", |b| {
            b.iter(|| {
                let rows = run_store
                    .list_agents(Some("paused"), false)
                    .expect("list paused");
                criterion::black_box(rows.len());
            });
        });

        group.bench_function("list_agents_all_including_archived", |b| {
            b.iter(|| {
                let rows = run_store
                    .list_agents(None, true)
                    .expect("list all");
                criterion::black_box(rows.len());
            });
        });
    }

    // --- filter events by time range via events_after ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool.clone());
        let event_store = SqliteEventStore::new(pool);
        let agent_id = seed_agent(&run_store);
        let run_id = seed_run(&run_store, &agent_id);

        // Seed 500 events.
        for seq in 1..=500 {
            event_store
                .append_event(
                    &run_id,
                    seq,
                    match seq % 4 {
                        0 => "turn_started",
                        1 => "step_executed",
                        2 => "tool_result",
                        _ => "model_response",
                    },
                    r#"{"bench":true}"#,
                    Some(&format!("corr-{}", seq % 10)),
                    1,
                )
                .expect("seed event");
        }

        // Read last 10% of events (after seq 450).
        group.bench_function("events_after_90pct", |b| {
            b.iter(|| {
                let rows = event_store.events_after(&run_id, 450).expect("events after");
                criterion::black_box(rows.len());
            });
        });

        // Read last 50% of events (after seq 250).
        group.bench_function("events_after_50pct", |b| {
            b.iter(|| {
                let rows = event_store.events_after(&run_id, 250).expect("events after");
                criterion::black_box(rows.len());
            });
        });
    }

    // --- filter unclaimed intents for a run ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool.clone());
        let effect_store = SqliteEffectStore::new(pool);
        let agent_id = seed_agent(&run_store);
        let run_id = seed_run(&run_store, &agent_id);

        // Create 100 intents; claim half of them.
        for i in 0..100 {
            let intent = effect_store
                .create_intent(
                    &run_id,
                    None,
                    None,
                    "model_call",
                    r#"{"filter":"unclaimed_test"}"#,
                    &format!("filter-key-{i}"),
                )
                .expect("create intent");
            if i % 2 == 0 {
                let lease_until = Utc::now() + chrono::Duration::seconds(600);
                effect_store
                    .claim_intent(&intent.id, "worker-1", lease_until)
                    .expect("claim");
            }
        }

        group.bench_function("unclaimed_intents_among_100", |b| {
            b.iter(|| {
                let rows = effect_store
                    .unclaimed_intents_for_run(&run_id)
                    .expect("unclaimed intents");
                criterion::black_box(rows.len());
            });
        });
    }

    // --- filter expired leases ---
    {
        let (pool, _tmpfile) = open_bench_db();
        let run_store = SqliteRunStore::new(pool.clone());
        let effect_store = SqliteEffectStore::new(pool.clone());
        let agent_id = seed_agent(&run_store);
        let run_id = seed_run(&run_store, &agent_id);

        // Create 100 intents with expired leases.
        let expired_lease = (Utc::now() - chrono::Duration::seconds(60)).to_rfc3339();
        for i in 0..100 {
            let intent = effect_store
                .create_intent(
                    &run_id,
                    None,
                    None,
                    "model_call",
                    r#"{"filter":"expired_test"}"#,
                    &format!("expired-key-{i}"),
                )
                .expect("create intent");
            // Directly set expired lease via raw SQL (claim_intent won't allow
            // claiming with a past date on already-expired leases cleanly).
            {
                let writer = pool.writer();
                writer
                    .execute(
                        "UPDATE effect_intents SET claimed_by = 'worker-old', claimed_until = ?1 WHERE id = ?2",
                        rusqlite::params![expired_lease, intent.id],
                    )
                    .expect("set expired lease");
            }
        }

        group.bench_function("expired_leases_100", |b| {
            b.iter(|| {
                let rows = effect_store
                    .expired_leases(Utc::now())
                    .expect("expired leases");
                criterion::black_box(rows.len());
            });
        });
    }

    // --- query payment costs by run ---
    {
        use polkagent_payment::{PaymentStore, types::CostRecord};

        let (pool, _tmpfile) = open_bench_db();
        let rt = build_rt();

        {
            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at)
                     VALUES ('q-agent', 'Query Agent', 'active', '{}', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                    [],
                )
                .expect("insert agent");
            writer
                .execute(
                    "INSERT INTO runs (id, agent_id, state, params_json, created_at, updated_at)
                     VALUES ('q-run', 'q-agent', 'completed', '{}', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                    [],
                )
                .expect("insert run");
        }

        // Seed 100 cost records.
        for _ in 0..100 {
            let record = CostRecord {
                run_id: "q-run".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-sonnet-4".to_string(),
                input_tokens: 2000,
                output_tokens: 1000,
                estimated_usd: 0.02,
                recorded_at: Utc::now(),
            };
            rt.block_on(PaymentStore::record_cost(&pool, record))
                .expect("seed cost");
        }

        group.bench_function("get_costs_for_run_100", |b| {
            b.iter(|| {
                let costs = rt
                    .block_on(PaymentStore::get_costs(&pool, "q-run"))
                    .expect("get costs");
                criterion::black_box(costs.len());
            });
        });
    }

    group.finish();
}

// ============================================================================
// Criterion harness
// ============================================================================

criterion_group!(
    benches,
    bench_run_crud,
    bench_effect_pipeline,
    bench_artifact_ops,
    bench_event_ops,
    bench_payment_ops,
    bench_conversation_ops,
    bench_migration,
    bench_concurrent_rw,
    bench_bulk_insert,
    bench_query_filters,
);
criterion_main!(benches);
