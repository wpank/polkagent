//! Cross-tenant access denial tests.
//!
//! These tests verify that row-level security enforces tenant isolation:
//! data written by tenant A must be invisible to tenant B.
//!
//! Requires a live PostgreSQL instance with `TEST_DATABASE_URL` set.

use polkagent_core::ids::{ArtifactId, RunId};
use polkagent_store_postgres::PgPool;
use polkagent_store_trait::{ArtifactStore, RunStatus, RunStore, StoreError};

const AGENT_ID: &str = "cross-tenant-agent";

async fn setup_tenants() -> Option<(PgPool, PgPool)> {
    let url = match std::env::var("TEST_DATABASE_URL") {
        Ok(u) => u,
        Err(_) => return None,
    };

    let tenant_a = format!("tenant-a-{}", uuid::Uuid::now_v7());
    let tenant_b = format!("tenant-b-{}", uuid::Uuid::now_v7());

    let pool_a = PgPool::connect(&url, &tenant_a).await.ok()?;
    pool_a.migrate().await.ok()?;

    let pool_b = PgPool::from_pool(pool_a.pool().clone(), &tenant_b);

    // Insert agent for each tenant.
    for pool in [&pool_a, &pool_b] {
        let mut tx = pool.pool().begin().await.ok()?;
        pool.set_tenant(&mut *tx).await.ok()?;
        sqlx::query(
            "INSERT INTO agents (id, tenant_id, name, state, spec_json)
             VALUES ($1, $2, 'Cross-Tenant Agent', 'active', '{}')",
        )
        .bind(AGENT_ID)
        .bind(pool.tenant_id())
        .execute(&mut *tx)
        .await
        .ok()?;
        tx.commit().await.ok()?;
    }

    Some((pool_a, pool_b))
}

// ---------------------------------------------------------------------------
// RunStore isolation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_created_by_tenant_a_invisible_to_tenant_b() {
    let Some((pool_a, pool_b)) = setup_tenants().await else {
        return;
    };

    let run_id = RunId::new();

    // Tenant A creates a run.
    RunStore::create(&pool_a, run_id, AGENT_ID, RunStatus::new("created"))
        .await
        .expect("tenant A create run");

    // Tenant A can see it.
    let summary = RunStore::get(&pool_a, run_id)
        .await
        .expect("tenant A get run");
    assert_eq!(summary.id, run_id);

    // Tenant B cannot see it.
    let err = RunStore::get(&pool_b, run_id)
        .await
        .expect_err("tenant B must not see tenant A's run");

    assert!(
        matches!(err, StoreError::NotFound { .. }),
        "expected NotFound, got: {err:?}"
    );
}

#[tokio::test]
async fn run_list_by_agent_tenant_isolated() {
    let Some((pool_a, pool_b)) = setup_tenants().await else {
        return;
    };

    let run_a = RunId::new();
    let run_b = RunId::new();

    RunStore::create(&pool_a, run_a, AGENT_ID, RunStatus::new("created"))
        .await
        .expect("tenant A create");
    RunStore::create(&pool_b, run_b, AGENT_ID, RunStatus::new("created"))
        .await
        .expect("tenant B create");

    // Each tenant only sees their own run.
    let listed_a = RunStore::list_by_agent(&pool_a, AGENT_ID, 100, 0)
        .await
        .expect("tenant A list");
    assert_eq!(listed_a.len(), 1);
    assert_eq!(listed_a[0].id, run_a);

    let listed_b = RunStore::list_by_agent(&pool_b, AGENT_ID, 100, 0)
        .await
        .expect("tenant B list");
    assert_eq!(listed_b.len(), 1);
    assert_eq!(listed_b[0].id, run_b);
}

#[tokio::test]
async fn tenant_b_cannot_update_tenant_a_run() {
    let Some((pool_a, pool_b)) = setup_tenants().await else {
        return;
    };

    let run_id = RunId::new();

    RunStore::create(&pool_a, run_id, AGENT_ID, RunStatus::new("created"))
        .await
        .expect("tenant A create");

    // Tenant B update_state should fail (row not visible via RLS).
    let err = RunStore::update_state(&pool_b, run_id, RunStatus::new("running"))
        .await
        .expect_err("tenant B must not update tenant A's run");

    assert!(
        matches!(err, StoreError::NotFound { .. }),
        "expected NotFound, got: {err:?}"
    );

    // Verify tenant A's run is still "created".
    let summary = RunStore::get(&pool_a, run_id).await.expect("tenant A get");
    assert_eq!(summary.status.as_str(), "created");
}

// ---------------------------------------------------------------------------
// ArtifactStore isolation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn artifact_created_by_tenant_a_invisible_to_tenant_b() {
    let Some((pool_a, pool_b)) = setup_tenants().await else {
        return;
    };

    let artifact_id = ArtifactId::new();
    let body = b"tenant A secret data";
    let digest = format!("{:064x}", 0xdead_beef_u64);

    ArtifactStore::store(
        &pool_a,
        artifact_id,
        None,
        "test",
        "blake3",
        &digest,
        "public",
        body,
    )
    .await
    .expect("tenant A store artifact");

    // Tenant A can see it.
    let summary = ArtifactStore::get(&pool_a, artifact_id)
        .await
        .expect("tenant A get artifact");
    assert_eq!(summary.id, artifact_id);

    // Tenant B cannot see it.
    let err = ArtifactStore::get(&pool_b, artifact_id)
        .await
        .expect_err("tenant B must not see tenant A's artifact");

    assert!(
        matches!(err, StoreError::NotFound { .. }),
        "expected NotFound, got: {err:?}"
    );
}

#[tokio::test]
async fn artifact_list_for_run_tenant_isolated() {
    let Some((pool_a, pool_b)) = setup_tenants().await else {
        return;
    };

    let run_a = RunId::new();
    RunStore::create(&pool_a, run_a, AGENT_ID, RunStatus::new("created"))
        .await
        .expect("tenant A create run");

    let artifact_id = ArtifactId::new();
    let digest = format!("{:064x}", 0xface_u64);

    ArtifactStore::store(
        &pool_a,
        artifact_id,
        Some(run_a),
        "test",
        "blake3",
        &digest,
        "public",
        b"body",
    )
    .await
    .expect("tenant A store artifact");

    // Tenant A sees the artifact.
    let list_a = ArtifactStore::list_for_run(&pool_a, run_a)
        .await
        .expect("tenant A list");
    assert_eq!(list_a.len(), 1);

    // Tenant B sees nothing for the same run_id.
    let list_b = ArtifactStore::list_for_run(&pool_b, run_a)
        .await
        .expect("tenant B list");
    assert!(list_b.is_empty());
}

// ---------------------------------------------------------------------------
// EventStore isolation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn events_tenant_isolated() {
    use polkagent_store_trait::event::EventStore;

    let Some((pool_a, pool_b)) = setup_tenants().await else {
        return;
    };

    let run_id = RunId::new();
    let run_str = run_id.to_string();

    // Create a run in tenant A.
    RunStore::create(&pool_a, run_id, AGENT_ID, RunStatus::new("created"))
        .await
        .expect("tenant A create run");

    // Append an event in tenant A.
    let event = polkagent_store_trait::event::StoredEvent {
        id: uuid::Uuid::now_v7().to_string(),
        event_type: "run_started".to_string(),
        sequence: 1,
        global_sequence: 0,
        run_id: run_str.clone(),
        conversation_id: None,
        correlation_id: "corr".to_string(),
        causation_id: None,
        scope_id: "scope".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        durability: "durable".to_string(),
        payload: serde_json::json!({"test": true}),
        trace_id: None,
        span_id: None,
        schema_version: 1,
    };

    pool_a
        .append_durable(event)
        .await
        .expect("tenant A append event");

    // Tenant A sees the event.
    let events_a = pool_a
        .read_run_events(run_id)
        .await
        .expect("tenant A read events");
    assert_eq!(events_a.len(), 1);

    // Tenant B sees nothing.
    let events_b = pool_b
        .read_run_events(run_id)
        .await
        .expect("tenant B read events");
    assert!(events_b.is_empty());
}
