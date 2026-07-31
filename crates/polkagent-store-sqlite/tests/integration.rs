//! Integration tests for the polkagent-store-sqlite crate.
//!
//! Each test that touches the file system uses a `tempfile::TempDir` so that
//! the database is automatically cleaned up when the test ends.  Tests that
//! only need schema validation can use an in-memory database.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use chrono::Utc;
use tempfile::TempDir;

use polkagent_store_sqlite::{
    SqlitePool,
    migrations,
    SqliteRunStore, SqliteEffectStore, SqliteArtifactStore, SqliteEventStore,
};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Open a file-based database in a temp directory and run migrations.
/// Returns `(TempDir, SqlitePool)` — keep the `TempDir` alive for the test
/// duration or the file is deleted.
fn open_file_db() -> (TempDir, SqlitePool) {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("test.db");
    let pool = SqlitePool::open(&path).expect("open pool");
    {
        let writer = pool.writer();
        migrations::migrate(&writer).expect("migrate");
    }
    (dir, pool)
}

/// Open an in-memory database and run migrations.
fn open_mem_db() -> SqlitePool {
    let pool = SqlitePool::open_in_memory().expect("open in-memory pool");
    {
        let writer = pool.writer();
        migrations::migrate(&writer).expect("migrate");
    }
    pool
}

/// Open an in-memory database *without* running migrations (raw pool).
fn open_raw_mem_db() -> SqlitePool {
    SqlitePool::open_in_memory().expect("open in-memory pool")
}

// ---------------------------------------------------------------------------
// 1. Schema creation
// ---------------------------------------------------------------------------

#[test]
fn schema_creation_succeeds() {
    let pool = open_mem_db();
    // Verify all expected tables exist after migration.
    let writer = pool.writer();
    // Domain tables that must be empty after a fresh migration.
    let domain_tables = [
        "agents",
        "runs",
        "turns",
        "steps",
        "effect_intents",
        "effect_attempts",
        "effect_outcomes",
        "artifacts",
        "artifact_bodies",
        "artifact_lineage",
        "run_events",
    ];
    for table in &domain_tables {
        let count: i64 = writer
            .query_row(
                &format!("SELECT COUNT(*) FROM {table}"),
                [],
                |r| r.get(0),
            )
            .unwrap_or_else(|e| panic!("table '{table}' not queryable: {e}"));
        assert_eq!(count, 0, "table '{table}' should be empty initially");
    }
    // schema_migrations should have exactly one row (the v1 migration).
    let migration_count: i64 = writer
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
        .expect("schema_migrations query");
    assert_eq!(migration_count, 5, "all migrations should be recorded");
}

#[test]
fn schema_creation_via_raw_pool_and_explicit_migrate() {
    // Verifies that schema_migrations starts at 0 before migration.
    let pool = open_raw_mem_db();
    let version = migrations::current_version(&pool.writer()).expect("version");
    assert_eq!(version, 0);
    migrations::migrate(&pool.writer()).expect("migrate");
    let version_after = migrations::current_version(&pool.writer()).expect("version after");
    assert_eq!(version_after, 5);
}

// ---------------------------------------------------------------------------
// 2. WAL mode enabled (file-based only)
// ---------------------------------------------------------------------------

#[test]
fn wal_mode_is_enabled() {
    let (_dir, pool) = open_file_db();
    let writer = pool.writer();
    let mode: String = writer
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .expect("pragma journal_mode");
    assert_eq!(mode, "wal", "journal_mode must be WAL");
}

// ---------------------------------------------------------------------------
// 3. Agent CRUD
// ---------------------------------------------------------------------------

#[test]
fn agent_create_and_retrieve() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool);

    let agent = store
        .create_agent("test-agent", Some("A test agent"), r#"{"kind":"echo"}"#)
        .expect("create agent");

    assert_eq!(agent.name, "test-agent");
    assert_eq!(agent.description.as_deref(), Some("A test agent"));
    assert_eq!(agent.state, "active");

    let fetched = store.get_agent(&agent.id).expect("get agent");
    assert_eq!(fetched.id, agent.id);
    assert_eq!(fetched.name, "test-agent");
}

#[test]
fn agent_not_found_returns_error() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool);
    let result = store.get_agent("00000000-0000-0000-0000-000000000000");
    assert!(
        matches!(result, Err(polkagent_store_sqlite::StoreError::NotFound(_))),
        "expected NotFound, got {result:?}"
    );
}

#[test]
fn agent_state_update() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool);
    let agent = store.create_agent("a", None, "{}").expect("create");
    store.update_agent_state(&agent.id, "disabled").expect("update");
    let fetched = store.get_agent(&agent.id).expect("get");
    assert_eq!(fetched.state, "disabled");
}

// ---------------------------------------------------------------------------
// 4. Run CRUD
// ---------------------------------------------------------------------------

#[test]
fn run_create_and_retrieve() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool);

    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store
        .create_run(&agent.id, Some("conv-1"), r#"{"input":"hello"}"#)
        .expect("create run");

    assert_eq!(run.agent_id, agent.id);
    assert_eq!(run.conversation_id.as_deref(), Some("conv-1"));
    assert_eq!(run.state, "created");
    assert!(run.completed_at.is_none());

    let fetched = store.get_run(&run.id).expect("get run");
    assert_eq!(fetched.id, run.id);
    assert_eq!(fetched.agent_id, agent.id);
}

#[test]
fn run_state_transition() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool);
    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");

    store
        .update_run_state(&run.id, "working", None)
        .expect("update to working");
    let r = store.get_run(&run.id).expect("get");
    assert_eq!(r.state, "working");

    let completed_at = Utc::now().to_rfc3339();
    store
        .update_run_state(&run.id, "completed", Some(&completed_at))
        .expect("update to completed");
    let r = store.get_run(&run.id).expect("get again");
    assert_eq!(r.state, "completed");
    assert!(r.completed_at.is_some());
}

#[test]
fn runs_for_agent_lists_all_runs() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool);
    let agent = store.create_agent("ag", None, "{}").expect("agent");

    for _ in 0..3 {
        store.create_run(&agent.id, None, "{}").expect("run");
    }

    let runs = store.runs_for_agent(&agent.id).expect("list runs");
    assert_eq!(runs.len(), 3);
}

// ---------------------------------------------------------------------------
// 5. Turn and step CRUD
// ---------------------------------------------------------------------------

#[test]
fn turn_create_and_complete() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool);

    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");
    let turn = store
        .create_turn(&run.id, 1, "user")
        .expect("create turn");

    assert_eq!(turn.sequence, 1);
    assert_eq!(turn.role, "user");
    assert!(turn.completed_at.is_none());

    store
        .complete_turn(&turn.id, 100, 250)
        .expect("complete turn");
    let fetched = store.get_turn(&turn.id).expect("get turn");
    assert!(fetched.completed_at.is_some());
    assert_eq!(fetched.input_tokens, 100);
    assert_eq!(fetched.output_tokens, 250);
}

#[test]
fn step_create_and_complete() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool);

    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");
    let turn = store.create_turn(&run.id, 1, "assistant").expect("turn");
    let step = store
        .create_step(&turn.id, 1, "model_call")
        .expect("create step");

    assert_eq!(step.kind, "model_call");
    assert_eq!(step.sequence, 1);
    assert!(step.completed_at.is_none());

    store.complete_step(&step.id).expect("complete step");
}

#[test]
fn turns_for_run_ordered_by_sequence() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool);

    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");

    // Insert in reverse sequence order to verify ordering.
    store.create_turn(&run.id, 3, "assistant").expect("t3");
    store.create_turn(&run.id, 1, "user").expect("t1");
    store.create_turn(&run.id, 2, "assistant").expect("t2");

    let turns = store.turns_for_run(&run.id).expect("list turns");
    assert_eq!(turns.len(), 3);
    assert_eq!(turns[0].sequence, 1);
    assert_eq!(turns[1].sequence, 2);
    assert_eq!(turns[2].sequence, 3);
}

// ---------------------------------------------------------------------------
// 6. Effect intent CRUD and lease management
// ---------------------------------------------------------------------------

#[test]
fn effect_intent_create_and_retrieve() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool.clone());
    let effect = SqliteEffectStore::new(pool);

    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");

    let intent = effect
        .create_intent(&run.id, None, None, "model", "{}", "idem-key-1")
        .expect("create intent");

    assert_eq!(intent.kind, "model");
    assert_eq!(intent.idempotency_key, "idem-key-1");
    assert!(intent.claimed_by.is_none());
    assert!(intent.claimed_until.is_none());

    let fetched = effect.get_intent(&intent.id).expect("get intent");
    assert_eq!(fetched.id, intent.id);
}

#[test]
fn effect_intent_duplicate_idempotency_key_rejected() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool.clone());
    let effect = SqliteEffectStore::new(pool);

    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");

    effect
        .create_intent(&run.id, None, None, "model", "{}", "same-key")
        .expect("first insert");

    let result = effect.create_intent(&run.id, None, None, "model", "{}", "same-key");
    assert!(
        matches!(result, Err(polkagent_store_sqlite::StoreError::Duplicate(_))),
        "expected Duplicate, got {result:?}"
    );
}

#[test]
fn effect_claim_with_lease() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool.clone());
    let effect = SqliteEffectStore::new(pool);

    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");
    let intent = effect
        .create_intent(&run.id, None, None, "tool", "{}", "tool-key-1")
        .expect("create");

    let lease_until = Utc::now() + chrono::Duration::seconds(30);
    let claimed = effect
        .claim_intent(&intent.id, "worker-1", lease_until)
        .expect("claim");

    assert_eq!(claimed.claimed_by.as_deref(), Some("worker-1"));
    assert!(claimed.claimed_until.is_some());
}

#[test]
fn effect_claim_already_claimed_fails() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool.clone());
    let effect = SqliteEffectStore::new(pool);

    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");
    let intent = effect
        .create_intent(&run.id, None, None, "tool", "{}", "tool-key-2")
        .expect("create");

    let lease_until = Utc::now() + chrono::Duration::seconds(60);
    effect.claim_intent(&intent.id, "worker-1", lease_until).expect("first claim");

    // Second claim by a different worker while lease is active must fail.
    let result = effect.claim_intent(&intent.id, "worker-2", lease_until);
    assert!(
        matches!(result, Err(polkagent_store_sqlite::StoreError::NotFound(_))),
        "expected NotFound (intent already claimed), got {result:?}"
    );
}

#[test]
fn expired_lease_can_be_reclaimed() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool.clone());
    let effect = SqliteEffectStore::new(pool);

    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");
    let intent = effect
        .create_intent(&run.id, None, None, "tool", "{}", "tool-key-3")
        .expect("create");

    // Claim with an expired lease (in the past).
    let expired_at = Utc::now() - chrono::Duration::seconds(10);
    // Directly set with an expired lease by claiming with past time.
    // We use a future time for the API then verify expired_leases returns it.
    let nearly_past = Utc::now() + chrono::Duration::milliseconds(1);
    effect.claim_intent(&intent.id, "worker-1", nearly_past).expect("first claim");

    // Wait a tiny bit so the lease is expired.
    thread::sleep(Duration::from_millis(10));

    // Now expired_leases should return this intent.
    let cutoff = Utc::now();
    let expired = effect.expired_leases(cutoff).expect("expired leases");
    assert!(
        expired.iter().any(|i| i.id == intent.id),
        "intent should appear in expired leases"
    );

    // Reclaim with a fresh lease.
    let new_lease = Utc::now() + chrono::Duration::seconds(30);
    let reclaimed = effect.claim_intent(&intent.id, "worker-2", new_lease).expect("reclaim");
    assert_eq!(reclaimed.claimed_by.as_deref(), Some("worker-2"));
    let _ = expired_at; // suppress unused warning
}

#[test]
fn effect_release_claim() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool.clone());
    let effect = SqliteEffectStore::new(pool);

    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");
    let intent = effect
        .create_intent(&run.id, None, None, "tool", "{}", "release-key")
        .expect("create");

    let lease = Utc::now() + chrono::Duration::seconds(30);
    effect.claim_intent(&intent.id, "worker-1", lease).expect("claim");
    effect.release_intent(&intent.id, "worker-1").expect("release");

    let fetched = effect.get_intent(&intent.id).expect("get");
    assert!(fetched.claimed_by.is_none());
    assert!(fetched.claimed_until.is_none());
}

// ---------------------------------------------------------------------------
// 7. Effect attempts
// ---------------------------------------------------------------------------

#[test]
fn effect_attempt_create_and_complete() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool.clone());
    let effect = SqliteEffectStore::new(pool);

    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");
    let intent = effect
        .create_intent(&run.id, None, None, "model", "{}", "att-key-1")
        .expect("intent");

    let attempt = effect.create_attempt(&intent.id, 1).expect("attempt");
    assert_eq!(attempt.attempt_number, 1);
    assert!(attempt.completed_at.is_none());

    effect.complete_attempt(&attempt.id).expect("complete");
    let attempts = effect.attempts_for_intent(&intent.id).expect("list");
    assert_eq!(attempts.len(), 1);
    assert!(attempts[0].completed_at.is_some());
}

// ---------------------------------------------------------------------------
// 8. Effect outcomes (immutability)
// ---------------------------------------------------------------------------

#[test]
fn effect_outcome_recorded_and_immutable() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool.clone());
    let effect = SqliteEffectStore::new(pool);

    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");
    let intent = effect
        .create_intent(&run.id, None, None, "model", "{}", "outcome-key-1")
        .expect("intent");

    let outcome = effect
        .record_outcome(&intent.id, "success", r#"{"hash":"0xabc"}"#)
        .expect("record outcome");

    assert_eq!(outcome.status, "success");

    // A second outcome for the same intent must be rejected.
    let dup = effect.record_outcome(&intent.id, "success", "{}");
    assert!(
        matches!(dup, Err(polkagent_store_sqlite::StoreError::Duplicate(_))),
        "expected Duplicate on second outcome, got {dup:?}"
    );
}

#[test]
fn effect_outcome_immutability_trigger_fires() {
    // The SQL trigger prevents any UPDATE on effect_outcomes.
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool.clone());
    let effect = SqliteEffectStore::new(pool.clone());

    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");
    let intent = effect
        .create_intent(&run.id, None, None, "model", "{}", "trigger-key")
        .expect("intent");
    let outcome = effect
        .record_outcome(&intent.id, "success", "{}")
        .expect("outcome");

    // Attempt a direct UPDATE — the trigger must abort this.
    let writer = pool.writer();
    let result = writer.execute(
        "UPDATE effect_outcomes SET status = 'failure' WHERE id = ?1",
        rusqlite::params![outcome.id],
    );
    assert!(
        result.is_err(),
        "UPDATE on effect_outcomes should be rejected by the immutability trigger"
    );
}

// ---------------------------------------------------------------------------
// 9. Artifact CRUD
// ---------------------------------------------------------------------------

#[test]
fn artifact_create_and_retrieve() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool.clone());
    let art = SqliteArtifactStore::new(pool);

    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");

    let artifact = art
        .create_artifact(
            Some(&run.id),
            "DecodedCall",
            "abc123def456",
            1024,
            r#"{"chain":"polkadot"}"#,
        )
        .expect("create artifact");

    assert_eq!(artifact.kind, "DecodedCall");
    assert_eq!(artifact.digest_hex, "abc123def456");
    assert_eq!(artifact.size_bytes, 1024);

    let fetched = art.get_artifact(&artifact.id).expect("get artifact");
    assert_eq!(fetched.id, artifact.id);
    assert_eq!(fetched.digest_hex, artifact.digest_hex);
}

#[test]
fn artifact_immutability_trigger_fires() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool.clone());
    let art = SqliteArtifactStore::new(pool.clone());

    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");
    let artifact = art
        .create_artifact(Some(&run.id), "File", "deadbeef", 512, "{}")
        .expect("artifact");

    let writer = pool.writer();
    let result = writer.execute(
        "UPDATE artifacts SET kind = 'SimulationResult' WHERE id = ?1",
        rusqlite::params![artifact.id],
    );
    assert!(
        result.is_err(),
        "UPDATE on artifacts should be rejected by the immutability trigger"
    );
}

#[test]
fn artifact_body_store_and_retrieve() {
    let pool = open_mem_db();
    let art = SqliteArtifactStore::new(pool);

    let body = b"hello polkagent artifact body";
    let digest = "sha256hexdigest0001";
    art.store_body(digest, body).expect("store body");

    let retrieved = art.get_body(digest).expect("get body");
    assert_eq!(retrieved, body);
}

#[test]
fn artifact_body_insert_ignored_on_duplicate_digest() {
    let pool = open_mem_db();
    let art = SqliteArtifactStore::new(pool);

    let body = b"same content";
    let digest = "dupekey";
    art.store_body(digest, body).expect("first store");
    // INSERT OR IGNORE — must not error.
    art.store_body(digest, body).expect("second store (no-op)");
}

#[test]
fn artifact_lineage_edges() {
    let pool = open_mem_db();
    let art = SqliteArtifactStore::new(pool);

    let parent = art
        .create_artifact(None, "MetadataSnapshot", "p-digest", 100, "{}")
        .expect("parent");
    let child = art
        .create_artifact(None, "DecodedCall", "c-digest", 200, "{}")
        .expect("child");

    art.add_lineage_edge(&child.id, &parent.id).expect("edge");

    let parents = art.parents_of(&child.id).expect("parents");
    assert_eq!(parents, vec![parent.id.clone()]);

    let children = art.children_of(&parent.id).expect("children");
    assert_eq!(children, vec![child.id.clone()]);
}

#[test]
fn artifacts_for_run_lists_all() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool.clone());
    let art = SqliteArtifactStore::new(pool);

    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");

    for i in 0..4 {
        art.create_artifact(
            Some(&run.id),
            "File",
            &format!("digest-{i}"),
            i * 100,
            "{}",
        )
        .expect("artifact");
    }

    let list = art.artifacts_for_run(&run.id).expect("list");
    assert_eq!(list.len(), 4);
}

// ---------------------------------------------------------------------------
// 10. Event store: ordering and uniqueness
// ---------------------------------------------------------------------------

#[test]
fn event_append_and_retrieve() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool.clone());
    let events = SqliteEventStore::new(pool);

    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");

    let ev1 = events
        .append_event(&run.id, 1, "RunCreated", "{}", Some("corr-1"), 1)
        .expect("event 1");
    let ev2 = events
        .append_event(&run.id, 2, "TurnStarted", "{}", Some("corr-1"), 1)
        .expect("event 2");

    assert_eq!(ev1.sequence, 1);
    assert_eq!(ev2.sequence, 2);

    let list = events.events_for_run(&run.id).expect("list events");
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].kind, "RunCreated");
    assert_eq!(list[1].kind, "TurnStarted");
}

#[test]
fn event_sequence_uniqueness_duplicate_rejected() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool.clone());
    let events = SqliteEventStore::new(pool);

    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");

    events
        .append_event(&run.id, 1, "RunCreated", "{}", None, 1)
        .expect("first");

    // Same (run_id, sequence=1) — must be rejected.
    let result = events.append_event(&run.id, 1, "RunCreated", "{}", None, 1);
    assert!(
        matches!(result, Err(polkagent_store_sqlite::StoreError::Duplicate(_))),
        "expected Duplicate, got {result:?}"
    );
}

#[test]
fn event_next_sequence_increments() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool.clone());
    let events = SqliteEventStore::new(pool);

    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");

    assert_eq!(events.next_sequence(&run.id).expect("next"), 1);

    events
        .append_event(&run.id, 1, "RunCreated", "{}", None, 1)
        .expect("append");
    assert_eq!(events.next_sequence(&run.id).expect("next"), 2);

    events
        .append_event(&run.id, 2, "TurnStarted", "{}", None, 1)
        .expect("append");
    assert_eq!(events.next_sequence(&run.id).expect("next"), 3);
}

#[test]
fn events_after_cursor() {
    let pool = open_mem_db();
    let store = SqliteRunStore::new(pool.clone());
    let events = SqliteEventStore::new(pool);

    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");

    for seq in 1..=5 {
        events
            .append_event(&run.id, seq, "Ping", "{}", None, 1)
            .expect("append");
    }

    // Events after sequence 2 = sequences 3, 4, 5.
    let after = events.events_after(&run.id, 2).expect("events_after");
    assert_eq!(after.len(), 3);
    assert_eq!(after[0].sequence, 3);
    assert_eq!(after[1].sequence, 4);
    assert_eq!(after[2].sequence, 5);
}

// ---------------------------------------------------------------------------
// 11. Concurrent readers don't block the writer (WAL)
// ---------------------------------------------------------------------------

#[test]
fn concurrent_readers_do_not_block_writer() {
    let (_dir, pool) = open_file_db();

    // Set up some data first.
    let store = SqliteRunStore::new(pool.clone());
    let agent = store.create_agent("ag", None, "{}").expect("agent");
    let run = store.create_run(&agent.id, None, "{}").expect("run");

    let pool_arc = Arc::new(pool);

    // Spawn multiple reader threads.
    let mut handles = vec![];
    for _ in 0..4 {
        let p = Arc::clone(&pool_arc);
        let run_id = run.id.clone();
        handles.push(thread::spawn(move || {
            let reader = p.reader().expect("reader");
            for _ in 0..20 {
                let _count: i64 = reader
                    .query_row(
                        "SELECT COUNT(*) FROM runs WHERE id = ?1",
                        rusqlite::params![run_id],
                        |r| r.get(0),
                    )
                    .expect("count query");
                // Small yield to interleave with writer.
                thread::yield_now();
            }
        }));
    }

    // Writer thread concurrently creates agents.
    let writer_pool = Arc::clone(&pool_arc);
    let writer_handle = thread::spawn(move || {
        let ws = SqliteRunStore::new((*writer_pool).clone());
        for i in 0..10 {
            ws.create_agent(&format!("concurrent-agent-{i}"), None, "{}")
                .expect("create in writer");
        }
    });

    writer_handle.join().expect("writer joined");
    for h in handles {
        h.join().expect("reader joined");
    }
}

// ---------------------------------------------------------------------------
// 12. End-to-end lifecycle: run → turn → step → intent → attempt → outcome → event
// ---------------------------------------------------------------------------

#[test]
fn full_lifecycle_smoke_test() {
    let pool = open_mem_db();

    let run_store = SqliteRunStore::new(pool.clone());
    let effect_store = SqliteEffectStore::new(pool.clone());
    let artifact_store = SqliteArtifactStore::new(pool.clone());
    let event_store = SqliteEventStore::new(pool.clone());

    // Agent + run.
    let agent = run_store.create_agent("lifecycle-agent", None, "{}").expect("agent");
    let run = run_store
        .create_run(&agent.id, Some("conv-abc"), "{}")
        .expect("run");

    // Emit RunCreated event.
    event_store
        .append_event(&run.id, 1, "RunCreated", "{}", Some("corr"), 1)
        .expect("event 1");

    // Turn 1.
    let turn = run_store.create_turn(&run.id, 1, "user").expect("turn");
    event_store
        .append_event(&run.id, 2, "TurnStarted", "{}", Some("corr"), 1)
        .expect("event 2");

    // Step 1 within turn.
    let step = run_store.create_step(&turn.id, 1, "model_call").expect("step");

    // Effect intent.
    let intent = effect_store
        .create_intent(
            &run.id,
            Some(&turn.id),
            Some(&step.id),
            "model",
            r#"{"model":"claude-3-5-sonnet"}"#,
            "lifecycle-idem-key",
        )
        .expect("intent");

    // Claim the intent.
    let lease = Utc::now() + chrono::Duration::seconds(30);
    effect_store
        .claim_intent(&intent.id, "worker-001", lease)
        .expect("claim");

    // Attempt.
    let attempt = effect_store.create_attempt(&intent.id, 1).expect("attempt");

    // Record outcome.
    let outcome = effect_store
        .record_outcome(&intent.id, "success", r#"{"completion":"I can help!"}"#)
        .expect("outcome");
    assert_eq!(outcome.status, "success");

    // Complete attempt.
    effect_store.complete_attempt(&attempt.id).expect("complete attempt");

    // Store an artifact.
    let artifact = artifact_store
        .create_artifact(
            Some(&run.id),
            "ModelResponse",
            "sha256abcdef",
            512,
            "{}",
        )
        .expect("artifact");

    // Store its body.
    artifact_store
        .store_body("sha256abcdef", b"I can help!")
        .expect("body");

    // Complete the turn.
    run_store.complete_turn(&turn.id, 50, 120).expect("complete turn");
    run_store.complete_step(&step.id).expect("complete step");

    // Transition run to completed.
    let completed_at = Utc::now().to_rfc3339();
    run_store
        .update_run_state(&run.id, "completed", Some(&completed_at))
        .expect("complete run");

    // Emit RunCompleted event.
    event_store
        .append_event(&run.id, 3, "RunCompleted", "{}", Some("corr"), 1)
        .expect("event 3");

    // Verify final state.
    let final_run = run_store.get_run(&run.id).expect("final run");
    assert_eq!(final_run.state, "completed");

    let all_events = event_store.events_for_run(&run.id).expect("all events");
    assert_eq!(all_events.len(), 3);

    let body = artifact_store
        .get_body(&artifact.digest_hex)
        .expect("body");
    assert_eq!(body, b"I can help!");
}
