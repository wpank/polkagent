#![allow(
    clippy::expect_used,
    reason = "runtime integration fixtures fail fast with boundary-specific diagnostics"
)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use polkagent_core::event::EventKind;
use polkagent_core::{AgentId, RunId};
use polkagent_runtime::{
    AdapterPolicy, ComponentState, ConfigSource, RuntimeError, RuntimeFactory, RuntimeOptions,
    WarningCode,
};
use polkagent_store_sqlite::{migrations, SqlitePool, SqliteRunStore};
use polkagent_store_trait::event::EventStore;
use polkagent_store_trait::{RunStatus, RunStore};
use tempfile::TempDir;

fn write_config(root: &Path, contents: &str) -> PathBuf {
    let path = root.join("polkagent.toml");
    std::fs::write(&path, contents).expect("write config fixture");
    path
}

fn simulated_options(root: &Path, config_path: PathBuf, database_path: PathBuf) -> RuntimeOptions {
    let mut options = RuntimeOptions::new(root);
    options.config_path = Some(config_path);
    options.database_path = Some(database_path);
    options.disable_harness = true;
    options.adapter_policy = AdapterPolicy::AllowSimulated;
    options.discover_environment_providers = false;
    options
}

fn migrated_pool(path: &Path) -> SqlitePool {
    let pool = SqlitePool::open(path).expect("open fixture database");
    {
        let writer = pool.writer();
        migrations::migrate(&writer).expect("migrate fixture database");
    }
    pool
}

fn create_legacy_agent(store: &SqliteRunStore) -> polkagent_store_sqlite::AgentRow {
    let now = "2026-01-01T00:00:00Z";
    let spec = serde_json::json!({
        "name": "runtime-agent",
        "description": "runtime fixture",
        "model": "fake/default-model",
        "tools": [],
        "autonomy_level": "supervised",
        "created_at": now,
        "updated_at": now,
        "declared_capabilities": [],
        "policy_refs": [],
        "surface_bindings": []
    });
    store
        .create_agent("runtime-agent", Some("runtime fixture"), &spec.to_string())
        .expect("create legacy agent")
}

#[tokio::test]
async fn build_migrates_recovers_rehydrates_and_shares_durable_events() {
    let temp = TempDir::new().expect("tempdir");
    let config_path = write_config(temp.path(), "");
    let database_path = temp.path().join("nested/runtime.db");
    std::fs::create_dir_all(database_path.parent().expect("database parent"))
        .expect("create fixture database parent");

    let pool = migrated_pool(&database_path);
    let store = SqliteRunStore::new(pool.clone());
    let row = create_legacy_agent(&store);
    let abandoned_run = RunId::new();
    RunStore::create(&pool, abandoned_run, &row.id, RunStatus::new("running"))
        .await
        .expect("seed abandoned run");
    drop(store);
    drop(pool);

    let runtime = RuntimeFactory::build(simulated_options(
        temp.path(),
        config_path.clone(),
        database_path.clone(),
    ))
    .await
    .expect("build runtime");

    assert!(runtime.readiness().operational);
    assert_eq!(runtime.readiness().recovered_runs, 1);
    assert_eq!(runtime.readiness().rehydrated_agents, 1);
    assert_eq!(runtime.readiness().database_path, database_path);
    assert!(matches!(
        runtime.readiness().config_source,
        ConfigSource::Explicit { ref path } if path == &config_path
    ));
    assert!(runtime.app().has_timeout_enforcer());
    assert!(runtime
        .readiness()
        .warnings
        .iter()
        .any(|warning| { warning.code == WarningCode::LegacyAgentNormalized }));

    let recovered = RunStore::get(runtime.pool(), abandoned_run)
        .await
        .expect("read recovered run");
    assert_eq!(recovered.status, RunStatus::new("failed"));

    let mut events = runtime.subscribe_events();
    let agent_id: AgentId = row.id.parse().expect("typed agent id");
    let run_id = runtime
        .app()
        .start_run(agent_id, "verify shared runtime")
        .await
        .expect("start rehydrated agent");

    let observed_created = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.expect("shared event bus remains open");
            if event.run_id == run_id && matches!(event.kind, EventKind::RunCreated) {
                return true;
            }
        }
    })
    .await
    .expect("RunCreated event timeout");
    assert!(observed_created);

    let persisted_run = RunStore::get(runtime.pool(), run_id)
        .await
        .expect("run persisted in runtime pool");
    assert_eq!(persisted_run.id, run_id);
    let durable_events = EventStore::read_run_events(runtime.pool(), run_id)
        .await
        .expect("read durable run events");
    assert!(!durable_events.is_empty());
}

#[tokio::test]
async fn strict_mode_rejects_a_runtime_without_executor_or_harness() {
    let temp = TempDir::new().expect("tempdir");
    let config_path = write_config(temp.path(), "");
    let mut options = RuntimeOptions::new(temp.path());
    options.config_path = Some(config_path);
    options.database_path = Some(temp.path().join("strict.db"));
    options.disable_harness = true;
    options.discover_environment_providers = false;

    let error = RuntimeFactory::build(options)
        .await
        .expect_err("strict runtime should reject missing execution backend");
    assert!(matches!(error, RuntimeError::NoExecutionBackend));
}

#[tokio::test]
async fn explicit_missing_config_fails_closed() {
    let temp = TempDir::new().expect("tempdir");
    let mut options = RuntimeOptions::new(temp.path());
    options.config_path = Some(PathBuf::from("missing.toml"));
    options.database_path = Some(temp.path().join("unused.db"));
    options.disable_harness = true;
    options.adapter_policy = AdapterPolicy::AllowSimulated;
    options.discover_environment_providers = false;

    let error = RuntimeFactory::build(options)
        .await
        .expect_err("missing explicit config must fail");
    assert!(matches!(error, RuntimeError::ConfigLoad { .. }));
}

#[tokio::test]
async fn read_only_override_is_reported_without_modifying_source() {
    let temp = TempDir::new().expect("tempdir");
    let source = "[api]\nread_only = false\n";
    let config_path = write_config(temp.path(), source);
    let database_path = temp.path().join("readonly.db");
    let mut options = simulated_options(temp.path(), config_path.clone(), database_path);
    options.read_only = true;

    let runtime = RuntimeFactory::build(options)
        .await
        .expect("build read-only runtime");
    assert!(runtime.config().api.read_only);
    assert!(runtime.readiness().read_only_requested);
    assert!(runtime
        .readiness()
        .warnings
        .iter()
        .any(|warning| { warning.code == WarningCode::ReadOnlySurfaceOnly }));
    assert_eq!(
        std::fs::read_to_string(config_path).expect("read source config"),
        source
    );
}

#[tokio::test]
async fn unsupported_postgres_backend_is_explicit() {
    let temp = TempDir::new().expect("tempdir");
    let config_path = write_config(temp.path(), "[database]\nbackend = \"postgres\"\n");
    let mut options = simulated_options(temp.path(), config_path, temp.path().join("unused.db"));
    options.disable_harness = true;

    let error = RuntimeFactory::build(options)
        .await
        .expect_err("Postgres is not composed yet");
    assert!(matches!(error, RuntimeError::UnsupportedDatabase { .. }));
}

#[tokio::test]
async fn corrupt_active_agent_fails_runtime_construction() {
    let temp = TempDir::new().expect("tempdir");
    let config_path = write_config(temp.path(), "");
    let database_path = temp.path().join("corrupt.db");
    let pool = migrated_pool(&database_path);
    SqliteRunStore::new(pool.clone())
        .create_agent("corrupt", None, "[]")
        .expect("seed corrupt agent");
    drop(pool);

    let error = RuntimeFactory::build(simulated_options(temp.path(), config_path, database_path))
        .await
        .expect_err("corrupt active agent must fail closed");
    assert!(matches!(error, RuntimeError::AgentSpec { .. }));
}

#[test]
fn simulated_readiness_is_structured() {
    assert!(ComponentState::Degraded.is_operational());
}
