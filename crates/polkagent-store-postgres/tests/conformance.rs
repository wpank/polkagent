//! PRD-15 conformance tests for the `PostgreSQL` store adapter.
//!
//! These tests require a live `PostgreSQL` instance. Set the
//! `TEST_DATABASE_URL` environment variable to run them:
//!
//! ```text
//! TEST_DATABASE_URL=postgres://user:pass@localhost/polkagent_test cargo test -p polkagent-store-postgres
//! ```
//!
//! If `TEST_DATABASE_URL` is not set, these tests are skipped.

// This assertion-oriented conformance target uses `expect` to identify the
// exact live-database fixture step or shared store contract that failed.
#![allow(clippy::expect_used)]

use polkagent_core::ids::{RunId, StepId};
use polkagent_store_postgres::PgPool;
use polkagent_store_trait::conformance;
use polkagent_store_trait::event::{EventFilter, EventStore, EventStoreError, StoredEvent};

const TEST_AGENT: &str = "conformance-agent";

async fn maybe_pool() -> Option<PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        return None;
    };

    let tenant = uuid::Uuid::now_v7().to_string();
    let pool = PgPool::connect(&url, &tenant).await.ok()?;
    pool.migrate().await.ok()?;

    // Insert a test agent for FK constraints.
    let mut tx = pool.pool().begin().await.ok()?;
    pool.set_tenant(&mut tx).await.ok()?;
    sqlx::query(
        "INSERT INTO agents (id, tenant_id, name, state, spec_json)
         VALUES ($1, $2, 'Conformance Agent', 'active', '{}')",
    )
    .bind(TEST_AGENT)
    .bind(pool.tenant_id())
    .execute(&mut *tx)
    .await
    .ok()?;
    tx.commit().await.ok()?;

    Some(pool)
}

/// Wrapper that auto-inserts prerequisite run records for events.
struct EventStoreWithRunSetup {
    pool: PgPool,
}

impl EventStoreWithRunSetup {
    fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn ensure_run_exists(&self, run_id: &str) {
        let mut tx = self.pool.pool().begin().await.expect("begin tx");
        self.pool.set_tenant(&mut tx).await.expect("set tenant");
        sqlx::query(
            "INSERT INTO runs (id, tenant_id, agent_id, state, params_json)
             VALUES ($1, $2, $3, 'created', '{}')
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(run_id)
        .bind(self.pool.tenant_id())
        .bind(TEST_AGENT)
        .execute(&mut *tx)
        .await
        .expect("ensure run exists");
        tx.commit().await.expect("commit");
    }
}

#[async_trait::async_trait]
impl EventStore for EventStoreWithRunSetup {
    async fn append_durable(&self, event: StoredEvent) -> Result<StoredEvent, EventStoreError> {
        self.ensure_run_exists(&event.run_id).await;
        self.pool.append_durable(event).await
    }

    async fn append_diagnostic(
        &self,
        event: StoredEvent,
        expires_at: String,
    ) -> Result<(), EventStoreError> {
        self.ensure_run_exists(&event.run_id).await;
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

    async fn max_sequence(&self, run_id: polkagent_core::RunId) -> Result<u64, EventStoreError> {
        self.pool.max_sequence(run_id).await
    }

    async fn has_terminal_event(
        &self,
        run_id: polkagent_core::RunId,
    ) -> Result<bool, EventStoreError> {
        self.pool.has_terminal_event(run_id).await
    }
}

async fn insert_run_with_step(pool: &PgPool, run_id: RunId) -> StepId {
    let tenant = pool.tenant_id().to_string();
    let mut tx = pool.pool().begin().await.expect("begin tx");
    pool.set_tenant(&mut tx).await.expect("set tenant");

    sqlx::query(
        "INSERT INTO runs (id, tenant_id, agent_id, state, params_json)
         VALUES ($1, $2, $3, 'created', '{}')",
    )
    .bind(run_id.to_string())
    .bind(&tenant)
    .bind(TEST_AGENT)
    .execute(&mut *tx)
    .await
    .expect("insert run");

    let turn_id = uuid::Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT INTO turns (id, tenant_id, run_id, sequence, role, started_at)
         VALUES ($1, $2, $3, 1, 'assistant', now())",
    )
    .bind(&turn_id)
    .bind(&tenant)
    .bind(run_id.to_string())
    .execute(&mut *tx)
    .await
    .expect("insert turn");

    let step_id = StepId::new();
    sqlx::query(
        "INSERT INTO steps (id, tenant_id, turn_id, sequence, kind, started_at)
         VALUES ($1, $2, $3, 1, 'model_call', now())",
    )
    .bind(step_id.to_string())
    .bind(&tenant)
    .bind(&turn_id)
    .execute(&mut *tx)
    .await
    .expect("insert step");

    tx.commit().await.expect("commit");
    step_id
}

// ---------------------------------------------------------------------------
// RunStore conformance
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_store_crud() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    conformance::test_run_store_crud(&pool, TEST_AGENT).await;
}

#[tokio::test]
async fn run_store_duplicate_conflict() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    conformance::test_run_store_duplicate_conflict(&pool, TEST_AGENT).await;
}

#[tokio::test]
async fn run_store_get_not_found() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    conformance::test_run_store_get_not_found(&pool).await;
}

#[tokio::test]
async fn run_store_terminal_state_sets_completed_at() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    conformance::test_run_store_terminal_state_sets_completed_at(&pool, TEST_AGENT).await;
}

// ---------------------------------------------------------------------------
// EffectStore conformance
// ---------------------------------------------------------------------------

#[tokio::test]
async fn effect_store_crud() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    let run_id = RunId::new();
    let step_id = insert_run_with_step(&pool, run_id).await;
    conformance::test_effect_store_crud(&pool, run_id, step_id).await;
}

#[tokio::test]
async fn effect_store_propose_duplicate_conflict() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    let run_id = RunId::new();
    let step_id = insert_run_with_step(&pool, run_id).await;
    conformance::test_effect_store_propose_duplicate_conflict(&pool, run_id, step_id).await;
}

#[tokio::test]
async fn effect_store_claim_empty_returns_none() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    conformance::test_effect_store_claim_empty_returns_none(&pool).await;
}

#[tokio::test]
async fn effect_store_release_restores_pending() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    let run_id = RunId::new();
    let step_id = insert_run_with_step(&pool, run_id).await;
    conformance::test_effect_store_release_restores_pending(&pool, run_id, step_id).await;
}

// ---------------------------------------------------------------------------
// EventStore conformance
// ---------------------------------------------------------------------------

#[tokio::test]
async fn event_store_append_and_query() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    let store = EventStoreWithRunSetup::new(pool);
    conformance::test_event_store_append_and_query(&store).await;
}

#[tokio::test]
async fn event_store_non_monotonic_sequence_rejected() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    let store = EventStoreWithRunSetup::new(pool);
    conformance::test_event_store_non_monotonic_sequence_rejected(&store).await;
}

#[tokio::test]
async fn event_store_duplicate_terminal_rejected() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    let store = EventStoreWithRunSetup::new(pool);
    conformance::test_event_store_duplicate_terminal_rejected(&store).await;
}

#[tokio::test]
async fn event_store_cursor_pagination() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    let store = EventStoreWithRunSetup::new(pool);
    conformance::test_event_store_cursor_pagination(&store).await;
}

#[tokio::test]
async fn event_store_max_sequence() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    let store = EventStoreWithRunSetup::new(pool);
    conformance::test_event_store_max_sequence(&store).await;
}

#[tokio::test]
async fn event_store_has_terminal_event() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    let store = EventStoreWithRunSetup::new(pool);
    conformance::test_event_store_has_terminal_event(&store).await;
}

// ---------------------------------------------------------------------------
// ArtifactStore conformance
// ---------------------------------------------------------------------------

#[tokio::test]
async fn artifact_store_crud() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    conformance::test_artifact_store_crud(&pool).await;
}

#[tokio::test]
async fn artifact_store_get_not_found() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    conformance::test_artifact_store_get_not_found(&pool).await;
}

#[tokio::test]
async fn artifact_store_verify_missing_returns_false() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    conformance::test_artifact_store_verify_missing_returns_false(&pool).await;
}

#[tokio::test]
async fn artifact_store_list_for_run() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    conformance::test_artifact_store_list_for_run(&pool).await;
}
