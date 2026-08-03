//! PRD-15 conformance tests for the SQLite store adapter.
//!
//! These tests call the shared conformance suite from `polkagent-store-trait`
//! and exercise the SQLite-backed implementations.  Each test sets up an
//! in-memory SQLite pool with full migrations and calls the appropriate
//! shared conformance function.
//!
//! Note: `SqlitePool` implements `polkagent_store_trait::{RunStore, EffectStore,
//! EventStore}` and separately implements `polkagent_artifact::store::ArtifactStore`
//! (a different trait).  The `ArtifactStore` conformance tests from store-trait
//! use the store-trait version of the interface and are exercised by
//! implementations that use the store-trait API directly.

use chrono::Utc;

use polkagent_core::ids::{RunId, StepId};
use polkagent_store_sqlite::{migrations, SqlitePool};
use polkagent_store_trait::conformance;
use polkagent_store_trait::event::{EventFilter, EventStore, EventStoreError, StoredEvent};

// ---------------------------------------------------------------------------
// Setup helpers
// ---------------------------------------------------------------------------

const TEST_AGENT: &str = "conformance-agent";

/// Create an in-memory SQLite pool with all migrations applied and one
/// test agent pre-inserted (FK required by `runs.agent_id`).
fn setup_pool() -> SqlitePool {
    let pool = SqlitePool::open_in_memory().expect("open in-memory pool");
    {
        let w = pool.writer();
        migrations::migrate(&w).expect("migrate");
        w.execute(
            "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at) \
             VALUES (?1, 'Conformance Agent', 'active', '{}', \
             '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
            [TEST_AGENT],
        )
        .expect("insert conformance agent");
    }
    pool
}

/// Wrapper around `SqlitePool` that auto-inserts prerequisite run records
/// when `append_durable` is called with a `run_id` not yet in the `runs`
/// table.  This satisfies the `run_events.run_id REFERENCES runs(id)` FK
/// constraint without modifying the shared conformance test functions.
struct EventStoreWithRunSetup {
    pool: SqlitePool,
}

impl EventStoreWithRunSetup {
    fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Ensure a run with the given `run_id` exists in the `runs` table.
    /// Uses `INSERT OR IGNORE` so it is safe to call multiple times.
    fn ensure_run_exists(&self, run_id: &str) {
        let now = Utc::now().to_rfc3339();
        let w = self.pool.writer();
        w.execute(
            "INSERT OR IGNORE INTO runs (id, agent_id, state, params_json, created_at, updated_at) \
             VALUES (?1, ?2, 'created', '{}', ?3, ?4)",
            rusqlite::params![run_id, TEST_AGENT, &now, &now],
        )
        .expect("ensure run exists for event store conformance test");
    }
}

#[async_trait::async_trait]
impl EventStore for EventStoreWithRunSetup {
    async fn append_durable(&self, event: StoredEvent) -> Result<StoredEvent, EventStoreError> {
        self.ensure_run_exists(&event.run_id);
        self.pool.append_durable(event).await
    }

    async fn append_diagnostic(
        &self,
        event: StoredEvent,
        expires_at: String,
    ) -> Result<(), EventStoreError> {
        self.ensure_run_exists(&event.run_id);
        self.pool.append_diagnostic(event, expires_at).await
    }

    async fn read_from_cursor(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        self.pool.read_from_cursor(cursor, limit).await
    }

    async fn read_run_events(
        &self,
        run_id: polkagent_core::RunId,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        self.pool.read_run_events(run_id).await
    }

    async fn query(&self, filter: EventFilter) -> Result<Vec<StoredEvent>, EventStoreError> {
        self.pool.query(filter).await
    }

    async fn max_sequence(
        &self,
        run_id: polkagent_core::RunId,
    ) -> Result<u64, EventStoreError> {
        self.pool.max_sequence(run_id).await
    }

    async fn has_terminal_event(
        &self,
        run_id: polkagent_core::RunId,
    ) -> Result<bool, EventStoreError> {
        self.pool.has_terminal_event(run_id).await
    }
}

/// Insert run + turn + step into `pool`, returning the `StepId`.
///
/// Required because `effect_intents.step_id` has a FK to `steps(id)`.
fn insert_run_with_step(pool: &SqlitePool, run_id: RunId) -> StepId {
    let now = Utc::now().to_rfc3339();
    let w = pool.writer();

    w.execute(
        "INSERT INTO runs (id, agent_id, state, params_json, created_at, updated_at) \
         VALUES (?1, ?2, 'created', '{}', ?3, ?4)",
        rusqlite::params![run_id.to_string(), TEST_AGENT, &now, &now],
    )
    .expect("insert run");

    let turn_id = uuid::Uuid::now_v7().to_string();
    w.execute(
        "INSERT INTO turns (id, run_id, sequence, role, started_at) \
         VALUES (?1, ?2, 1, 'assistant', ?3)",
        rusqlite::params![turn_id, run_id.to_string(), &now],
    )
    .expect("insert turn");

    let step_id = StepId::new();
    w.execute(
        "INSERT INTO steps (id, turn_id, sequence, kind, started_at) \
         VALUES (?1, ?2, 1, 'model_call', ?3)",
        rusqlite::params![step_id.to_string(), turn_id, &now],
    )
    .expect("insert step");

    step_id
}

// ---------------------------------------------------------------------------
// RunStore conformance
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_store_crud() {
    let pool = setup_pool();
    conformance::test_run_store_crud(&pool, TEST_AGENT).await;
}

#[tokio::test]
async fn run_store_duplicate_conflict() {
    let pool = setup_pool();
    conformance::test_run_store_duplicate_conflict(&pool, TEST_AGENT).await;
}

#[tokio::test]
async fn run_store_get_not_found() {
    let pool = setup_pool();
    conformance::test_run_store_get_not_found(&pool).await;
}

#[tokio::test]
async fn run_store_terminal_state_sets_completed_at() {
    let pool = setup_pool();
    conformance::test_run_store_terminal_state_sets_completed_at(&pool, TEST_AGENT).await;
}

// ---------------------------------------------------------------------------
// EffectStore conformance
// ---------------------------------------------------------------------------

#[tokio::test]
async fn effect_store_crud() {
    let pool = setup_pool();
    let run_id = RunId::new();
    let step_id = insert_run_with_step(&pool, run_id);
    conformance::test_effect_store_crud(&pool, run_id, step_id).await;
}

#[tokio::test]
async fn effect_store_propose_duplicate_conflict() {
    let pool = setup_pool();
    let run_id = RunId::new();
    let step_id = insert_run_with_step(&pool, run_id);
    conformance::test_effect_store_propose_duplicate_conflict(&pool, run_id, step_id).await;
}

#[tokio::test]
async fn effect_store_claim_empty_returns_none() {
    let pool = setup_pool();
    conformance::test_effect_store_claim_empty_returns_none(&pool).await;
}

#[tokio::test]
async fn effect_store_release_restores_pending() {
    let pool = setup_pool();
    let run_id = RunId::new();
    let step_id = insert_run_with_step(&pool, run_id);
    conformance::test_effect_store_release_restores_pending(&pool, run_id, step_id).await;
}

// ---------------------------------------------------------------------------
// EventStore conformance
// ---------------------------------------------------------------------------

#[tokio::test]
async fn event_store_append_and_query() {
    let store = EventStoreWithRunSetup::new(setup_pool());
    conformance::test_event_store_append_and_query(&store).await;
}

#[tokio::test]
async fn event_store_non_monotonic_sequence_rejected() {
    let store = EventStoreWithRunSetup::new(setup_pool());
    conformance::test_event_store_non_monotonic_sequence_rejected(&store).await;
}

#[tokio::test]
async fn event_store_duplicate_terminal_rejected() {
    let store = EventStoreWithRunSetup::new(setup_pool());
    conformance::test_event_store_duplicate_terminal_rejected(&store).await;
}

#[tokio::test]
async fn event_store_cursor_pagination() {
    let store = EventStoreWithRunSetup::new(setup_pool());
    conformance::test_event_store_cursor_pagination(&store).await;
}

#[tokio::test]
async fn event_store_max_sequence() {
    let store = EventStoreWithRunSetup::new(setup_pool());
    conformance::test_event_store_max_sequence(&store).await;
}

#[tokio::test]
async fn event_store_has_terminal_event() {
    let store = EventStoreWithRunSetup::new(setup_pool());
    conformance::test_event_store_has_terminal_event(&store).await;
}

// Note: ArtifactStore conformance tests require an implementor of
// `polkagent_store_trait::ArtifactStore`.  `SqlitePool` implements
// `polkagent_artifact::store::ArtifactStore` (a separate trait).
// Those tests are exercised by the `polkagent-artifact` crate's own test
// suite and by future adapters that implement the store-trait interface.
