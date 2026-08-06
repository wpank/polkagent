//! Black-box HTTP coverage for the shared-runtime durable adapters.

#![allow(
    clippy::expect_used,
    reason = "integration tests fail immediately at controlled fixture boundaries"
)]

use std::{sync::Arc, time::Duration};

use axum::http::StatusCode;
use axum_test::TestServer;
use polkagent_api::{
    app_state_from_runtime, ApiServer, AppState, InMemoryAgentStore, InMemoryRunManager,
    RuntimeArtifactStore, RuntimeRunManager, RuntimeToolRegistryStore, RUNTIME_UNAVAILABLE_ROUTES,
};
use polkagent_config::{Config, ModelOverrideConfig};
use polkagent_core::{AgentId, AgentSpec, ArtifactId, BlobRef, RunId};
use polkagent_event::{EventBus, EventRecorder};
use polkagent_executor_fake::FakeExecutor;
use polkagent_interaction::{InteractionService, InteractionStore};
use polkagent_memory::{classification::Classification, MemoryEntry, MemoryId, MemoryType};
use polkagent_runtime::{
    AdapterPolicy, ComponentState, DurableInteractionService, PolkagentRuntime, RuntimeFactory,
    RuntimeOptions,
};
use polkagent_service::AppService;
use polkagent_store_sqlite::{migrations, SqliteInteractionStore, SqlitePool};
use polkagent_store_trait::{ArtifactStore, RunStatus, RunStore};
use sha2::{Digest, Sha256};

async fn runtime_at(root: &std::path::Path) -> PolkagentRuntime {
    runtime_with_config(root, "").await
}

async fn runtime_with_config(root: &std::path::Path, config: &str) -> PolkagentRuntime {
    let config_path = root.join("polkagent.toml");
    std::fs::write(&config_path, config).expect("write runtime config");
    let mut options = RuntimeOptions::new(root);
    options.config_path = Some(config_path);
    options.database_path = Some(root.join("api.db"));
    options.disable_harness = true;
    options.adapter_policy = AdapterPolicy::AllowSimulated;
    options.discover_environment_providers = false;
    RuntimeFactory::build(options).await.expect("build runtime")
}

async fn memory_runtime_at(root: &std::path::Path) -> PolkagentRuntime {
    runtime_with_config(
        root,
        r#"
[memory]
enabled = true
backend = "sqlite"
"#,
    )
    .await
}

fn write_skill(root: &std::path::Path, directory: &str, manifest: &str) {
    let skill_directory = root.join("skills").join(directory);
    std::fs::create_dir_all(&skill_directory).expect("create skill directory");
    std::fs::write(skill_directory.join("skill.toml"), manifest).expect("write skill manifest");
}

fn memory_entry(
    agent_id: AgentId,
    memory_type: MemoryType,
    content: &str,
    created_at: chrono::DateTime<chrono::Utc>,
    relevance_score: f64,
) -> MemoryEntry {
    MemoryEntry {
        id: MemoryId::new(),
        agent_id,
        episode_id: None,
        memory_type,
        content: content.to_owned(),
        embedding: None,
        metadata: serde_json::json!({"source": "runtime-test"}),
        provenance: None,
        created_at,
        accessed_at: created_at,
        access_count: 0,
        relevance_score,
        confidence: 0.8,
        classification: Classification::Internal,
    }
}

fn test_server(runtime: &PolkagentRuntime) -> TestServer {
    let state = app_state_from_runtime(runtime, runtime.config().as_ref().clone());
    let server = ApiServer::from_state(state);
    TestServer::new(server.into_router())
}

fn projection_server(pool: SqlitePool) -> TestServer {
    let event_bus = EventBus::with_default_capacity();
    let shared_pool = Arc::new(pool.clone());
    let recorder = EventRecorder::new(shared_pool.clone(), event_bus.clone());
    let app = Arc::new(
        AppService::builder()
            .with_config(Config::default())
            .with_run_store(shared_pool.clone())
            .with_effect_store(shared_pool.clone())
            .with_event_bus(event_bus.clone())
            .with_event_recorder(recorder)
            .build()
            .expect("build projection service"),
    );
    let state = AppState::new(
        Config::default(),
        Arc::new(InMemoryAgentStore::new()),
        Arc::new(RuntimeRunManager::new(pool, app)),
        shared_pool,
        event_bus,
    );
    TestServer::new(ApiServer::from_state(state).into_router())
}

fn cancellable_interaction_server() -> (TestServer, AgentId) {
    let pool = SqlitePool::open_in_memory().expect("open SQLite fixture");
    migrations::migrate(&pool.writer()).expect("migrate SQLite fixture");
    let agent_id = AgentId::new();
    let spec = AgentSpec::new(agent_id, "queued-http-agent", "fake/model");
    let now = chrono::Utc::now().to_rfc3339();
    pool.writer()
        .execute(
            "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at)
             VALUES (?1, ?2, 'active', ?3, ?4, ?4)",
            rusqlite::params![
                agent_id.to_string(),
                &spec.name,
                serde_json::to_string(&spec).expect("encode agent"),
                now
            ],
        )
        .expect("seed durable agent");

    let event_bus = EventBus::new(32);
    let shared_pool = Arc::new(pool.clone());
    let recorder = EventRecorder::new(shared_pool.clone(), event_bus.clone());
    let app = Arc::new(
        AppService::builder()
            .with_config(Config::default())
            .with_run_store(shared_pool.clone())
            .with_effect_store(shared_pool.clone())
            .with_conversation_store(shared_pool.clone())
            .with_payment_store(shared_pool.clone())
            .with_event_bus(event_bus.clone())
            .with_event_recorder(recorder)
            .build()
            .expect("build queued app service"),
    );
    app.create_agent(spec).expect("register live agent");
    let interaction_service: Arc<dyn InteractionService> =
        Arc::new(DurableInteractionService::new(app, pool.clone()));
    let interaction_store: Arc<dyn InteractionStore> =
        Arc::new(SqliteInteractionStore::new(pool.clone()));
    let state = AppState::new(
        Config::default(),
        Arc::new(InMemoryAgentStore::new()),
        Arc::new(InMemoryRunManager::new()),
        shared_pool.clone(),
        event_bus,
    )
    .with_conversation_store(shared_pool)
    .with_interaction_service(interaction_service)
    .with_interaction_store(interaction_store);
    (
        TestServer::new(ApiServer::from_state(state).into_router()),
        agent_id,
    )
}

struct ConfigInteractionFixture {
    app: Arc<AppService>,
    pool: SqlitePool,
    event_bus: EventBus,
    config: Config,
    first_agent: AgentId,
    second_agent: AgentId,
}

impl ConfigInteractionFixture {
    fn new() -> Self {
        let pool = SqlitePool::open_in_memory().expect("open configuration SQLite fixture");
        migrations::migrate(&pool.writer()).expect("migrate configuration SQLite fixture");
        let first_agent = AgentId::new();
        let second_agent = AgentId::new();
        let specs = [
            AgentSpec::new(first_agent, "config-agent-a", "fake/default-model"),
            AgentSpec::new(second_agent, "config-agent-b", "fake/default-model"),
        ];
        let now = chrono::Utc::now().to_rfc3339();
        for spec in &specs {
            pool.writer()
                .execute(
                    "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at)
                     VALUES (?1, ?2, 'active', ?3, ?4, ?4)",
                    rusqlite::params![
                        spec.id.to_string(),
                        &spec.name,
                        serde_json::to_string(spec).expect("encode configuration agent"),
                        &now
                    ],
                )
                .expect("seed configuration agent");
        }
        let config = Config {
            models: ["model-a", "model-b"]
                .into_iter()
                .map(|slug| ModelOverrideConfig {
                    slug: slug.to_owned(),
                    provider: "fake".to_owned(),
                    ..ModelOverrideConfig::default()
                })
                .collect(),
            ..Config::default()
        };
        let event_bus = EventBus::new(32);
        let shared_pool = Arc::new(pool.clone());
        let recorder = EventRecorder::new(shared_pool.clone(), event_bus.clone());
        let app = Arc::new(
            AppService::builder()
                .with_config(config.clone())
                .with_executor(FakeExecutor::new())
                .with_run_store(shared_pool.clone())
                .with_effect_store(shared_pool.clone())
                .with_conversation_store(shared_pool.clone())
                .with_payment_store(shared_pool)
                .with_event_bus(event_bus.clone())
                .with_event_recorder(recorder)
                .build()
                .expect("build configuration app service"),
        );
        for spec in specs {
            app.create_agent(spec)
                .expect("register configuration agent");
        }
        Self {
            app,
            pool,
            event_bus,
            config,
            first_agent,
            second_agent,
        }
    }

    fn server(&self) -> TestServer {
        let shared_pool = Arc::new(self.pool.clone());
        let interaction_service: Arc<dyn InteractionService> = Arc::new(
            DurableInteractionService::new(Arc::clone(&self.app), self.pool.clone()),
        );
        let interaction_store: Arc<dyn InteractionStore> =
            Arc::new(SqliteInteractionStore::new(self.pool.clone()));
        let state = AppState::new(
            self.config.clone(),
            Arc::new(InMemoryAgentStore::new()),
            Arc::new(InMemoryRunManager::new()),
            shared_pool.clone(),
            self.event_bus.clone(),
        )
        .with_conversation_store(shared_pool)
        .with_interaction_service(interaction_service)
        .with_interaction_store(interaction_store);
        TestServer::new(ApiServer::from_state(state).into_router())
    }
}

async fn create_config_interaction(server: &TestServer, agent_id: AgentId) -> String {
    let response = server
        .post("/api/v1alpha1/interactions")
        .json(&serde_json::json!({
            "target": {"kind": "agent", "id": agent_id},
            "working_directory": "/tmp"
        }))
        .await;
    response.assert_status(StatusCode::CREATED);
    response.json::<serde_json::Value>()["interaction"]["conversation_id"]
        .as_str()
        .expect("configuration interaction id")
        .to_owned()
}

async fn read_config(server: &TestServer, conversation_id: &str) -> serde_json::Value {
    let response = server
        .get(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/config"
        ))
        .await;
    response.assert_status_ok();
    let body = response.json::<serde_json::Value>();
    assert_eq!(body["version"], "v1alpha1");
    assert_eq!(
        body["config"]
            .as_object()
            .expect("supported configuration object")
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        ["model", "target"].into_iter().collect()
    );
    body["config"].clone()
}

fn assert_http_error(response: &axum_test::TestResponse, status: StatusCode, code: &str) {
    response.assert_status(status);
    let body = response.json::<serde_json::Value>();
    assert_eq!(body["error"]["code"], code);
    assert!(body["error"]["request_id"].is_string());
    assert!(body["error"]["timestamp"].is_string());
}

async fn store_runtime_artifact(
    store: &RuntimeArtifactStore,
    id: ArtifactId,
    run_id: RunId,
    kind: &str,
    classification: &str,
    body: &[u8],
) {
    let digest = BlobRef::from_bytes(body).blake3_hex;
    store
        .store(
            id,
            Some(run_id),
            kind,
            "blake3",
            &digest,
            classification,
            body,
        )
        .await
        .expect("store runtime artifact");
}

#[tokio::test]
async fn http_create_execute_and_read_survive_runtime_restart() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let runtime = runtime_at(temp.path()).await;
    let server = test_server(&runtime);
    let create_agent = server
        .post("/api/v1alpha1/agents")
        .json(&serde_json::json!({
            "name": "durable-http-agent",
            "model": "fake/default-model",
            "description": "full DTO survives",
            "tools": ["governance.list_referenda"],
            "declared_capabilities": ["chain.query"],
            "policy_refs": ["read-only.toml"],
            "surface_bindings": ["http"]
        }))
        .await;
    create_agent.assert_status(StatusCode::CREATED);
    let agent = create_agent.json::<serde_json::Value>();
    let agent_id = agent["id"].as_str().expect("agent id");
    assert_eq!(agent["spec"]["description"], "full DTO survives");
    assert_eq!(agent["spec"]["declared_capabilities"][0], "chain.query");

    let input = serde_json::json!({
        "prompt": "inspect treasury",
        "context": {"network": "polkadot"},
    });
    let create_run = server
        .post(&format!("/api/v1alpha1/agents/{agent_id}/runs"))
        .json(&serde_json::json!({"input": input}))
        .await;
    create_run.assert_status(StatusCode::CREATED);
    let created = create_run.json::<serde_json::Value>();
    let run_id = created["id"].as_str().expect("run id").to_owned();
    assert_eq!(created["input"], input);

    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let response = server.get(&format!("/api/v1alpha1/runs/{run_id}")).await;
            response.assert_status_ok();
            let body = response.json::<serde_json::Value>();
            if body["status"]["state"] == "completed" {
                assert_eq!(body["input"], input);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fake run should complete");

    drop(server);
    drop(runtime);
    let restarted = runtime_at(temp.path()).await;
    let restarted_server = test_server(&restarted);
    let persisted_agent = restarted_server
        .get(&format!("/api/v1alpha1/agents/{agent_id}"))
        .await;
    persisted_agent.assert_status_ok();
    assert_eq!(
        persisted_agent.json::<serde_json::Value>()["spec"]["surface_bindings"][0],
        "http"
    );
    let persisted_run = restarted_server
        .get(&format!("/api/v1alpha1/runs/{run_id}"))
        .await;
    persisted_run.assert_status_ok();
    let persisted_run = persisted_run.json::<serde_json::Value>();
    assert_eq!(persisted_run["input"], input);
    assert_eq!(persisted_run["status"]["state"], "completed");
}

#[tokio::test]
async fn handler_reports_conflicts_and_corrupt_projection_as_distinct_errors() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let runtime = runtime_at(temp.path()).await;
    let server = test_server(&runtime);
    let request = serde_json::json!({
        "name": "unique-http-agent",
        "model": "fake/default-model"
    });
    let created = server.post("/api/v1alpha1/agents").json(&request).await;
    created.assert_status(StatusCode::CREATED);
    let agent_id = created.json::<serde_json::Value>()["id"]
        .as_str()
        .expect("agent id")
        .to_owned();

    let conflict = server.post("/api/v1alpha1/agents").json(&request).await;
    conflict.assert_status(StatusCode::CONFLICT);
    assert_eq!(
        conflict.json::<serde_json::Value>()["error"]["code"],
        "INVALID_STATE"
    );

    runtime
        .pool()
        .writer()
        .execute(
            "UPDATE agents SET spec_json = '{}' WHERE id = ?1",
            [&agent_id],
        )
        .expect("corrupt agent projection");
    let corrupt = server
        .get(&format!("/api/v1alpha1/agents/{agent_id}"))
        .await;
    corrupt.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        corrupt.json::<serde_json::Value>()["error"]["code"],
        "INTERNAL_ERROR"
    );
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "the table keeps the complete persisted state grammar visible in one restart contract"
)]
async fn durable_run_state_grammar_survives_projection_restart_and_sanitizes_corruption() {
    struct StateFixture {
        id: RunId,
        stored: String,
        expected_status: serde_json::Value,
        terminal_reason: Option<&'static str>,
        started: bool,
        completed: bool,
    }

    let temp = tempfile::TempDir::new().expect("tempdir");
    let database_path = temp.path().join("projection-restart.db");
    let pool = SqlitePool::open(&database_path).expect("open state fixture database");
    migrations::migrate(&pool.writer()).expect("migrate state fixture database");
    let agent_id = AgentId::new();
    let agent = AgentSpec::new(agent_id, "state-projection-agent", "fake/default-model");
    pool.writer()
        .execute(
            "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at)
             VALUES (?1, ?2, 'active', ?3, ?4, ?4)",
            rusqlite::params![
                agent_id.to_string(),
                &agent.name,
                serde_json::to_string(&agent).expect("encode state fixture agent"),
                agent.created_at.to_rfc3339(),
            ],
        )
        .expect("seed state fixture agent");
    let first_effect = polkagent_core::EffectId::new();
    let second_effect = polkagent_core::EffectId::new();
    let legacy_reason = "legacy persisted state did not include a reason";
    let fixtures = vec![
        StateFixture {
            id: RunId::new(),
            stored: "created".to_owned(),
            expected_status: serde_json::json!({"state": "created"}),
            terminal_reason: None,
            started: false,
            completed: false,
        },
        StateFixture {
            id: RunId::new(),
            stored: "queued".to_owned(),
            expected_status: serde_json::json!({"state": "queued"}),
            terminal_reason: None,
            started: false,
            completed: false,
        },
        StateFixture {
            id: RunId::new(),
            stored: "running".to_owned(),
            expected_status: serde_json::json!({"state": "running"}),
            terminal_reason: None,
            started: true,
            completed: false,
        },
        StateFixture {
            id: RunId::new(),
            stored: "awaiting_approval:approval-7".to_owned(),
            expected_status: serde_json::json!({
                "state": "awaiting_approval",
                "request_id": "approval-7"
            }),
            terminal_reason: None,
            started: true,
            completed: false,
        },
        StateFixture {
            id: RunId::new(),
            stored: format!("waiting_effect:{first_effect},{second_effect}"),
            expected_status: serde_json::json!({
                "state": "waiting_effect",
                "pending_intent_ids": [first_effect, second_effect]
            }),
            terminal_reason: None,
            started: true,
            completed: false,
        },
        StateFixture {
            id: RunId::new(),
            stored: "completing".to_owned(),
            expected_status: serde_json::json!({"state": "completing"}),
            terminal_reason: None,
            started: true,
            completed: false,
        },
        StateFixture {
            id: RunId::new(),
            stored: "completed".to_owned(),
            expected_status: serde_json::json!({"state": "completed"}),
            terminal_reason: None,
            started: true,
            completed: true,
        },
        StateFixture {
            id: RunId::new(),
            stored: "failed:provider: timeout".to_owned(),
            expected_status: serde_json::json!({
                "state": "failed",
                "reason": "provider: timeout"
            }),
            terminal_reason: Some("provider: timeout"),
            started: true,
            completed: true,
        },
        StateFixture {
            id: RunId::new(),
            stored: "cancelled:user request".to_owned(),
            expected_status: serde_json::json!({
                "state": "cancelled",
                "reason": "user request"
            }),
            terminal_reason: Some("user request"),
            started: true,
            completed: true,
        },
        StateFixture {
            id: RunId::new(),
            stored: "failed".to_owned(),
            expected_status: serde_json::json!({
                "state": "failed",
                "reason": legacy_reason
            }),
            terminal_reason: Some(legacy_reason),
            started: true,
            completed: true,
        },
        StateFixture {
            id: RunId::new(),
            stored: "cancelled".to_owned(),
            expected_status: serde_json::json!({
                "state": "cancelled",
                "reason": legacy_reason
            }),
            terminal_reason: Some(legacy_reason),
            started: true,
            completed: true,
        },
        StateFixture {
            id: RunId::new(),
            stored: "timed_out".to_owned(),
            expected_status: serde_json::json!({"state": "timed_out"}),
            terminal_reason: None,
            started: true,
            completed: true,
        },
    ];
    let created_at = "2026-03-01T10:00:00Z";
    let started_at = "2026-03-01T10:01:00Z";
    let completed_at = "2026-03-01T10:02:00Z";

    for fixture in &fixtures {
        RunStore::create(
            &pool,
            fixture.id,
            &agent_id.to_string(),
            RunStatus::new(fixture.stored.clone()),
        )
        .await
        .expect("seed persisted run state");
        let input = serde_json::json!({"stored_state": fixture.stored});
        pool.writer()
            .execute(
                "UPDATE runs
                 SET params_json = ?1, created_at = ?2, updated_at = ?2,
                     started_at = ?3, completed_at = ?4
                 WHERE id = ?5",
                rusqlite::params![
                    serde_json::to_string(&input).expect("encode run input"),
                    created_at,
                    fixture.started.then_some(started_at),
                    fixture.completed.then_some(completed_at),
                    fixture.id.to_string(),
                ],
            )
            .expect("set persisted run projection fields");
    }

    let malformed = [
        (RunId::new(), "timed_out:private-reason"),
        (RunId::new(), "waiting_effect:private-invalid-effect-id"),
        (RunId::new(), "invented-state-private-payload"),
    ];
    for (run_id, state) in malformed {
        RunStore::create(&pool, run_id, &agent_id.to_string(), RunStatus::new(state))
            .await
            .expect("seed malformed persisted state");
    }

    drop(pool);
    let reopened = SqlitePool::open(&database_path).expect("reopen state fixture database");
    let server = projection_server(reopened);

    for fixture in &fixtures {
        let response = server
            .get(&format!("/api/v1alpha1/runs/{}", fixture.id))
            .await;
        response.assert_status_ok();
        let body = response.json::<serde_json::Value>();
        assert_eq!(
            body["status"], fixture.expected_status,
            "{}",
            fixture.stored
        );
        assert_eq!(
            body["input"],
            serde_json::json!({"stored_state": fixture.stored}),
            "{}",
            fixture.stored
        );
        assert_eq!(body["created_at"], created_at, "{}", fixture.stored);
        assert_eq!(
            body.get("started_at").and_then(serde_json::Value::as_str),
            fixture.started.then_some(started_at),
            "{}",
            fixture.stored
        );
        assert_eq!(
            body.get("completed_at").and_then(serde_json::Value::as_str),
            fixture.completed.then_some(completed_at),
            "{}",
            fixture.stored
        );
        assert_eq!(
            body.get("terminal_reason")
                .and_then(serde_json::Value::as_str),
            fixture.terminal_reason,
            "{}",
            fixture.stored
        );
    }

    for (run_id, state) in malformed {
        let response = server.get(&format!("/api/v1alpha1/runs/{run_id}")).await;
        response.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
        let body = response.json::<serde_json::Value>();
        assert_eq!(body["error"]["code"], "INTERNAL_ERROR");
        assert_eq!(
            body["error"]["message"],
            "internal error: stored run state is invalid"
        );
        assert!(
            !serde_json::to_string(&body)
                .expect("encode error response")
                .contains(state),
            "malformed persisted state must not leak over HTTP"
        );
    }
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one black-box scenario proves the loaded catalog, refusal boundary, and restart snapshot together"
)]
async fn runtime_skill_catalog_is_authoritative_read_only_and_restart_stable() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    write_skill(
        temp.path(),
        "zeta-directory",
        r#"
[skill]
name = "zeta-skill"
version = "2.0.0"
description = "Loaded second after deterministic sorting"

[capabilities]
tools = ["polkagent.zeta"]
"#,
    );
    write_skill(
        temp.path(),
        "alpha-directory",
        r#"
[skill]
name = "alpha-skill"
version = "1.2.3"
description = "Loaded first after deterministic sorting"

[capabilities]
required_grants = ["memory.read"]
tools = ["polkagent.alpha"]
"#,
    );
    let config = r#"
[skills]
directories = ["skills"]
auto_load = true
"#;

    let runtime = runtime_with_config(temp.path(), config).await;
    assert_eq!(runtime.readiness().skills.state, ComponentState::Ready);
    assert!(runtime.readiness().skills.detail.contains("loaded 2"));
    assert!(runtime.app().skill_runner().is_some());
    assert_eq!(
        runtime
            .app()
            .skill_manifests()
            .iter()
            .map(|manifest| manifest.skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha-skill", "zeta-skill"]
    );

    let server = test_server(&runtime);
    let listed = server.get("/api/v1alpha1/skills").await;
    listed.assert_status_ok();
    let first_snapshot = listed.json::<serde_json::Value>();
    assert_eq!(
        first_snapshot,
        serde_json::json!({
            "version": "v1alpha1",
            "data": [
                {
                    "version": "v1alpha1",
                    "id": "alpha-skill@1.2.3",
                    "name": "alpha-skill",
                    "skill_version": "1.2.3",
                    "description": "Loaded first after deterministic sorting",
                    "required_grants": ["memory.read"],
                    "tools": ["polkagent.alpha"],
                    "active": true
                },
                {
                    "version": "v1alpha1",
                    "id": "zeta-skill@2.0.0",
                    "name": "zeta-skill",
                    "skill_version": "2.0.0",
                    "description": "Loaded second after deterministic sorting",
                    "required_grants": [],
                    "tools": ["polkagent.zeta"],
                    "active": true
                }
            ]
        })
    );

    let alpha = server.get("/api/v1alpha1/skills/alpha-skill").await;
    alpha.assert_status_ok();
    assert_eq!(alpha.json::<serde_json::Value>(), first_snapshot["data"][0]);

    let missing = server.get("/api/v1alpha1/skills/missing-skill").await;
    assert_http_error(&missing, StatusCode::NOT_FOUND, "NOT_FOUND");
    assert_eq!(
        missing.json::<serde_json::Value>()["error"]["message"],
        "not found: skill 'missing-skill'"
    );

    let mutation_responses = [
        server
            .post("/api/v1alpha1/skills/install")
            .json(&serde_json::json!({"path": "/untrusted/package"}))
            .await,
        server
            .post("/api/v1alpha1/skills/alpha-skill/uninstall")
            .await,
        server
            .put("/api/v1alpha1/skills/alpha-skill/config")
            .json(&serde_json::json!({"config": {"mode": "changed"}}))
            .await,
    ];
    for response in mutation_responses {
        assert_http_error(&response, StatusCode::NOT_IMPLEMENTED, "NOT_IMPLEMENTED");
        assert_eq!(
            response.json::<serde_json::Value>()["error"]["message"],
            "not implemented: runtime skill catalog is read-only"
        );
    }

    let mut read_only_config = runtime.config().as_ref().clone();
    read_only_config.api.read_only = true;
    let read_only = TestServer::new(
        ApiServer::from_state(app_state_from_runtime(&runtime, read_only_config)).into_router(),
    );
    assert_eq!(
        read_only
            .get("/api/v1alpha1/skills")
            .await
            .json::<serde_json::Value>(),
        first_snapshot
    );

    drop(read_only);
    drop(server);
    drop(runtime);
    let restarted = runtime_with_config(temp.path(), config).await;
    assert_eq!(restarted.readiness().skills.state, ComponentState::Ready);
    assert_eq!(
        test_server(&restarted)
            .get("/api/v1alpha1/skills")
            .await
            .json::<serde_json::Value>(),
        first_snapshot
    );

    drop(restarted);
    let disabled = runtime_with_config(
        temp.path(),
        r#"
[skills]
directories = ["skills"]
auto_load = false
"#,
    )
    .await;
    assert_eq!(disabled.readiness().skills.state, ComponentState::Disabled);
    assert!(disabled.app().skill_runner().is_none());
    assert!(disabled.app().skill_manifests().is_empty());
    assert_eq!(
        test_server(&disabled)
            .get("/api/v1alpha1/skills")
            .await
            .json::<serde_json::Value>()["data"],
        serde_json::json!([])
    );
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one black-box scenario keeps exact-store stats, deletion, restart, auth, and read-only evidence contiguous"
)]
async fn runtime_memory_operations_use_exact_durable_store_and_survive_restart() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let runtime = memory_runtime_at(temp.path()).await;
    assert_eq!(runtime.readiness().memory.state, ComponentState::Ready);
    let agent_id = AgentId::new();
    let now = chrono::Utc::now();
    let semantic = memory_entry(
        agent_id,
        MemoryType::Semantic,
        "durable rust memory",
        now,
        0.9,
    );
    let procedural = memory_entry(
        agent_id,
        MemoryType::Procedural,
        "durable café release procedure",
        now + chrono::Duration::seconds(1),
        0.4,
    );
    runtime
        .app()
        .store_memory(&agent_id, semantic.clone())
        .await
        .expect("store semantic memory through runtime service");
    runtime
        .app()
        .store_memory(&agent_id, procedural.clone())
        .await
        .expect("store procedural memory through runtime service");

    let server = test_server(&runtime);
    let queried = server
        .post("/api/v1alpha1/memory/query")
        .json(&serde_json::json!({
            "query": "durable",
            "limit": 10,
            "namespace": "semantic"
        }))
        .await;
    queried.assert_status_ok();
    let queried = queried.json::<serde_json::Value>();
    assert_eq!(queried["data"].as_array().expect("query data").len(), 1);
    assert_eq!(queried["data"][0]["id"], semantic.id.to_string());
    assert_eq!(queried["data"][0]["content"], semantic.content);
    assert_eq!(queried["data"][0]["score"], 0.9);
    assert_eq!(queried["data"][0]["namespace"], "semantic");

    let exact_path = format!("/api/v1alpha1/memory/entries/{}", semantic.id);
    let exact = server.get(&exact_path).await;
    exact.assert_status_ok();
    let exact = exact.json::<serde_json::Value>();
    assert_eq!(exact["data"], queried["data"][0]);
    let inspected = runtime
        .app()
        .memory_store()
        .expect("runtime memory store")
        .peek_memory(semantic.id)
        .await
        .expect("inspect access metadata");
    assert_eq!(inspected.access_count, 0);
    assert_eq!(inspected.accessed_at, semantic.accessed_at);

    let stats = server.get("/api/v1alpha1/memory/stats").await;
    stats.assert_status_ok();
    let stats = stats.json::<serde_json::Value>();
    assert_eq!(stats["total_memories"], 2);
    assert_eq!(
        stats["total_bytes"],
        u64::try_from(semantic.content.len() + procedural.content.len())
            .expect("fixture byte count fits u64")
    );
    assert_eq!(stats["namespaces"], 2);
    let inspected_after_stats = runtime
        .app()
        .memory_store()
        .expect("runtime memory store")
        .peek_memory(semantic.id)
        .await
        .expect("inspect access metadata after stats");
    assert_eq!(inspected_after_stats.access_count, inspected.access_count);
    assert_eq!(inspected_after_stats.accessed_at, inspected.accessed_at);

    assert_http_error(
        &server
            .post("/api/v1alpha1/memory/query")
            .json(&serde_json::json!({
                "query": "durable",
                "namespace": "private-project"
            }))
            .await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_http_error(
        &server
            .get("/api/v1alpha1/memory/entries/not-a-memory-id")
            .await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_http_error(
        &server
            .get(&format!("/api/v1alpha1/memory/entries/{}", MemoryId::new()))
            .await,
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
    );
    assert_http_error(
        &server
            .post("/api/v1alpha1/memory/forget")
            .json(&serde_json::json!({"entry_ids": []}))
            .await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_http_error(
        &server
            .post("/api/v1alpha1/memory/forget")
            .json(&serde_json::json!({
                "entry_ids": [semantic.id.to_string(), "not-a-memory-id"]
            }))
            .await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    server.get(&exact_path).await.assert_status_ok();

    let unknown_id = MemoryId::new();
    let unknown_delete = server
        .post("/api/v1alpha1/memory/forget")
        .json(&serde_json::json!({"entry_ids": [unknown_id.to_string()]}))
        .await;
    unknown_delete.assert_status_ok();
    assert_eq!(unknown_delete.json::<serde_json::Value>()["deleted"], 0);

    let deleted = server
        .post("/api/v1alpha1/memory/forget")
        .json(&serde_json::json!({
            "entry_ids": [
                semantic.id.to_string(),
                semantic.id.to_string(),
                unknown_id.to_string()
            ]
        }))
        .await;
    deleted.assert_status_ok();
    assert_eq!(deleted.json::<serde_json::Value>()["deleted"], 1);
    assert_http_error(
        &server.get(&exact_path).await,
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
    );
    let post_delete_stats = server.get("/api/v1alpha1/memory/stats").await;
    post_delete_stats.assert_status_ok();
    let post_delete_stats = post_delete_stats.json::<serde_json::Value>();
    assert_eq!(post_delete_stats["total_memories"], 1);
    assert_eq!(
        post_delete_stats["total_bytes"],
        u64::try_from(procedural.content.len()).expect("fixture byte count fits u64")
    );
    assert_eq!(post_delete_stats["namespaces"], 1);

    drop(server);
    drop(runtime);
    let restarted = memory_runtime_at(temp.path()).await;
    let restarted_server = test_server(&restarted);
    let restarted_query = restarted_server
        .post("/api/v1alpha1/memory/query")
        .json(&serde_json::json!({
            "query": "durable",
            "limit": 10,
            "namespace": "procedural"
        }))
        .await;
    restarted_query.assert_status_ok();
    let restarted_query = restarted_query.json::<serde_json::Value>();
    assert_eq!(
        restarted_query["data"]
            .as_array()
            .expect("restarted query data")
            .len(),
        1
    );
    assert_eq!(restarted_query["data"][0]["id"], procedural.id.to_string());
    assert_http_error(
        &restarted_server.get(&exact_path).await,
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
    );
    let procedural_path = format!("/api/v1alpha1/memory/entries/{}", procedural.id);
    let restarted_exact = restarted_server.get(&procedural_path).await;
    restarted_exact.assert_status_ok();
    assert_eq!(
        restarted_exact.json::<serde_json::Value>()["data"]["id"],
        procedural.id.to_string()
    );
    assert_eq!(
        restarted_server
            .get("/api/v1alpha1/memory/stats")
            .await
            .json::<serde_json::Value>(),
        post_delete_stats
    );

    let token = "memory-reader-token";
    let mut protected_config = restarted.config().as_ref().clone();
    protected_config.auth.enabled = true;
    protected_config.auth.api_keys = vec![format!("{:x}", Sha256::digest(token.as_bytes()))];
    let protected = TestServer::new(
        ApiServer::from_state(app_state_from_runtime(&restarted, protected_config.clone()))
            .into_router(),
    );
    protected
        .get("/api/v1alpha1/memory/stats")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    protected
        .get("/api/v1alpha1/memory/stats")
        .authorization_bearer(token)
        .await
        .assert_status_ok();
    protected
        .post("/api/v1alpha1/memory/forget")
        .json(&serde_json::json!({"entry_ids": [procedural.id.to_string()]}))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    protected_config.api.read_only = true;
    let read_only = TestServer::new(
        ApiServer::from_state(app_state_from_runtime(&restarted, protected_config)).into_router(),
    );
    read_only
        .get("/api/v1alpha1/memory/stats")
        .authorization_bearer(token)
        .await
        .assert_status_ok();
    read_only
        .post("/api/v1alpha1/memory/forget")
        .authorization_bearer(token)
        .json(&serde_json::json!({"entry_ids": [procedural.id.to_string()]}))
        .await
        .assert_status(StatusCode::METHOD_NOT_ALLOWED);
    read_only
        .post("/api/v1alpha1/memory/query")
        .authorization_bearer(token)
        .json(&serde_json::json!({"query": "durable"}))
        .await
        .assert_status(StatusCode::METHOD_NOT_ALLOWED);
    restarted_server
        .get(&procedural_path)
        .await
        .assert_status_ok();
}

#[tokio::test]
async fn disabled_runtime_memory_exposes_truthful_empty_read_view() {
    let disabled_root = tempfile::TempDir::new().expect("disabled tempdir");
    let disabled = runtime_at(disabled_root.path()).await;
    assert_eq!(disabled.readiness().memory.state, ComponentState::Disabled);
    let disabled_state = app_state_from_runtime(&disabled, disabled.config().as_ref().clone());
    assert!(disabled_state.memory_store.is_some());
    assert!(disabled_state.memory_stats_available);
    assert!(disabled_state.memory_store_mutable);
    let disabled_server = TestServer::new(ApiServer::from_state(disabled_state).into_router());
    let disabled_query = disabled_server
        .post("/api/v1alpha1/memory/query")
        .json(&serde_json::json!({"query": "anything"}))
        .await;
    disabled_query.assert_status_ok();
    assert_eq!(
        disabled_query.json::<serde_json::Value>()["data"],
        serde_json::json!([])
    );
    assert_http_error(
        &disabled_server
            .get(&format!("/api/v1alpha1/memory/entries/{}", MemoryId::new()))
            .await,
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
    );
    let zero_stats = disabled_server.get("/api/v1alpha1/memory/stats").await;
    zero_stats.assert_status_ok();
    assert_eq!(
        zero_stats.json::<serde_json::Value>(),
        serde_json::json!({
            "version": "v1alpha1",
            "total_memories": 0,
            "total_bytes": 0,
            "namespaces": 0
        })
    );
    let disabled_delete = disabled_server
        .post("/api/v1alpha1/memory/forget")
        .json(&serde_json::json!({"entry_ids": [MemoryId::new().to_string()]}))
        .await;
    disabled_delete.assert_status_ok();
    assert_eq!(disabled_delete.json::<serde_json::Value>()["deleted"], 0);
}

#[tokio::test]
async fn runtime_server_composes_real_optional_stores_and_publishes_501_boundary() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let runtime = runtime_at(temp.path()).await;
    let state = app_state_from_runtime(&runtime, runtime.config().as_ref().clone());
    let runtime_interactions: Arc<dyn InteractionService> = runtime.interactions().clone();

    assert!(state.event_store.is_some());
    assert!(state.payment_store.is_some());
    assert!(state.conversation_store.is_some());
    assert!(state.artifact_store.is_some());
    assert!(state.skill_registry.is_some());
    assert!(!state.skill_registry_mutable);
    assert!(state.tool_registry.is_some());
    assert!(state.memory_store.is_some());
    assert!(state.memory_stats_available);
    assert!(state.memory_store_mutable);
    assert!(state.audit_store.is_none());
    assert!(state.service_registry_store.is_none());
    assert!(Arc::ptr_eq(
        state
            .interaction_service
            .as_ref()
            .expect("interaction service"),
        &runtime_interactions
    ));
    assert!(state.interaction_store.is_some());

    let server = TestServer::new(ApiServer::from_state(state).into_router());
    server.get("/api/v1alpha1/events").await.assert_status_ok();
    server
        .get("/api/v1alpha1/payments/balance")
        .await
        .assert_status_ok();
    server
        .get("/api/v1alpha1/conversations?agent_id=00000000-0000-0000-0000-000000000000")
        .await
        .assert_status_ok();
    let disabled_skills = server.get("/api/v1alpha1/skills").await;
    disabled_skills.assert_status_ok();
    assert_eq!(
        disabled_skills.json::<serde_json::Value>()["data"],
        serde_json::json!([])
    );
    assert_eq!(runtime.readiness().skills.state, ComponentState::Disabled);
    assert!(runtime.app().skill_runner().is_none());
    assert_http_error(
        &server.get("/api/v1alpha1/skills/missing-skill").await,
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
    );

    assert_eq!(RUNTIME_UNAVAILABLE_ROUTES.len(), 9);
    for route in RUNTIME_UNAVAILABLE_ROUTES {
        let path = route
            .path
            .replace("{id}", "00000000-0000-0000-0000-000000000000")
            .replace("{skill_id}", "missing-skill")
            .replace("{tool_id}", "missing-tool")
            .replace("{entry_id}", "missing-entry");
        let response = match route.method {
            "GET" => server.get(&path).await,
            "POST" => {
                let body = match route.path {
                    "/api/v1alpha1/skills/install" => {
                        serde_json::json!({"path": "/missing-skill"})
                    }
                    "/api/v1alpha1/registry/listings" => serde_json::json!({
                        "name": "unavailable",
                        "description": "boundary probe",
                        "author": "test",
                        "version": "0.1.0",
                        "pricing": {"model": "free"}
                    }),
                    _ => serde_json::json!({}),
                };
                server.post(&path).json(&body).await
            }
            "PUT" => {
                server
                    .put(&path)
                    .json(&serde_json::json!({"config": {}}))
                    .await
            }
            method => panic!("unexpected unavailable route method {method}"),
        };
        response.assert_status(StatusCode::NOT_IMPLEMENTED);
        assert_eq!(
            response.json::<serde_json::Value>()["error"]["code"],
            "NOT_IMPLEMENTED",
            "{} {}",
            route.method,
            route.path
        );
    }

    assert!(RUNTIME_UNAVAILABLE_ROUTES.iter().all(|route| {
        route.path.starts_with("/api/v1alpha1/")
            && !route.method.is_empty()
            && !route.reason.is_empty()
    }));
    assert!(RUNTIME_UNAVAILABLE_ROUTES
        .iter()
        .all(|route| route.dependency != "memory"));
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one black-box scenario keeps atomic refusal, isolation, and restart evidence contiguous"
)]
async fn durable_http_config_is_atomic_isolated_and_persistent() {
    let fixture = ConfigInteractionFixture::new();
    let server = fixture.server();
    let first_id = create_config_interaction(&server, fixture.first_agent).await;
    let second_id = create_config_interaction(&server, fixture.second_agent).await;

    let initial_first = read_config(&server, &first_id).await;
    assert_eq!(
        initial_first["target"]["id"],
        fixture.first_agent.to_string()
    );
    assert!(initial_first["model"].is_null());

    let changed_target = server
        .put(&format!("/api/v1alpha1/interactions/{first_id}/config"))
        .json(&serde_json::json!({
            "option": "target",
            "value": {"kind": "agent", "id": fixture.second_agent}
        }))
        .await;
    changed_target.assert_status_ok();
    assert_eq!(
        changed_target.json::<serde_json::Value>()["config"]["target"]["id"],
        fixture.second_agent.to_string()
    );
    let target_snapshot = read_config(&server, &first_id).await;

    let unknown_target = server
        .put(&format!("/api/v1alpha1/interactions/{first_id}/config"))
        .json(&serde_json::json!({
            "option": "target",
            "value": {"kind": "agent", "id": AgentId::new()}
        }))
        .await;
    assert_http_error(&unknown_target, StatusCode::NOT_FOUND, "NOT_FOUND");
    assert_eq!(read_config(&server, &first_id).await, target_snapshot);

    let strict_compatibility = server
        .put(&format!("/api/v1alpha1/interactions/{first_id}/target"))
        .json(&serde_json::json!({
            "target": {"kind": "agent", "id": fixture.first_agent},
            "model": "must-not-be-ignored"
        }))
        .await;
    assert_http_error(
        &strict_compatibility,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_eq!(read_config(&server, &first_id).await, target_snapshot);

    let first_model = server
        .put(&format!("/api/v1alpha1/interactions/{first_id}/config"))
        .json(&serde_json::json!({"option": "model", "value": "model-a"}))
        .await;
    first_model.assert_status_ok();
    assert_eq!(
        first_model.json::<serde_json::Value>()["config"]["model"],
        "fake/model-a"
    );
    let second_model = server
        .put(&format!("/api/v1alpha1/interactions/{second_id}/config"))
        .json(&serde_json::json!({"option": "model", "value": "model-b"}))
        .await;
    second_model.assert_status_ok();
    assert_eq!(
        second_model.json::<serde_json::Value>()["config"]["model"],
        "fake/model-b"
    );
    let first_model_snapshot = read_config(&server, &first_id).await;
    let second_model_snapshot = read_config(&server, &second_id).await;
    assert_eq!(first_model_snapshot["model"], "fake/model-a");
    assert_eq!(second_model_snapshot["model"], "fake/model-b");

    let unknown_model = server
        .put(&format!("/api/v1alpha1/interactions/{first_id}/config"))
        .json(&serde_json::json!({
            "option": "model",
            "value": "not-in-the-model-catalog"
        }))
        .await;
    assert_http_error(
        &unknown_model,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    assert_eq!(read_config(&server, &first_id).await, first_model_snapshot);

    for invalid in [
        serde_json::json!({"option": "model", "value": ""}),
        serde_json::json!({"option": "provider", "value": "fake"}),
        serde_json::json!({
            "option": "model",
            "value": "model-b",
            "provider": "must-not-be-ignored"
        }),
    ] {
        let rejected = server
            .put(&format!("/api/v1alpha1/interactions/{first_id}/config"))
            .json(&invalid)
            .await;
        assert_http_error(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_eq!(read_config(&server, &first_id).await, first_model_snapshot);
    }

    let cleared = server
        .put(&format!("/api/v1alpha1/interactions/{second_id}/config"))
        .json(&serde_json::json!({"option": "model", "value": null}))
        .await;
    cleared.assert_status_ok();
    assert!(cleared.json::<serde_json::Value>()["config"]["model"].is_null());
    server
        .put(&format!("/api/v1alpha1/interactions/{second_id}/config"))
        .json(&serde_json::json!({"option": "model", "value": "model-b"}))
        .await
        .assert_status_ok();

    let compatibility = server
        .put(&format!("/api/v1alpha1/interactions/{first_id}/target"))
        .json(&serde_json::json!({
            "target": {"kind": "agent", "id": fixture.first_agent}
        }))
        .await;
    compatibility.assert_status_ok();
    let compatibility = compatibility.json::<serde_json::Value>();
    assert_eq!(
        compatibility["config"]["target"]["id"],
        fixture.first_agent.to_string()
    );
    assert_eq!(compatibility["config"]["model"], "fake/model-a");
    assert!(compatibility["config"].get("provider").is_some());

    {
        let writer = fixture.pool.writer();
        let turn_count: i64 = writer
            .query_row("SELECT COUNT(*) FROM interaction_turns", [], |row| {
                row.get(0)
            })
            .expect("count interaction turns");
        let run_count: i64 = writer
            .query_row("SELECT COUNT(*) FROM runs", [], |row| row.get(0))
            .expect("count interaction runs");
        assert_eq!(turn_count, 0, "configuration changes must not create turns");
        assert_eq!(run_count, 0, "configuration changes must not create runs");
    }

    drop(server);
    let restarted = fixture.server();
    let restarted_first = read_config(&restarted, &first_id).await;
    let restarted_second = read_config(&restarted, &second_id).await;
    assert_eq!(
        restarted_first["target"]["id"],
        fixture.first_agent.to_string()
    );
    assert_eq!(restarted_first["model"], "fake/model-a");
    assert_eq!(
        restarted_second["target"]["id"],
        fixture.second_agent.to_string()
    );
    assert_eq!(restarted_second["model"], "fake/model-b");
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one black-box scenario keeps the restart and idempotency lifecycle contiguous"
)]
async fn durable_http_interactions_execute_retry_conflict_and_replay_after_restart() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let runtime = runtime_at(temp.path()).await;
    let server = test_server(&runtime);
    let created_agent = server
        .post("/api/v1alpha1/agents")
        .json(&serde_json::json!({
            "name": "interaction-http-agent",
            "model": "fake/default-model"
        }))
        .await;
    created_agent.assert_status(StatusCode::CREATED);
    let agent_id = created_agent.json::<serde_json::Value>()["id"]
        .as_str()
        .expect("agent id")
        .to_owned();
    let created = server
        .post("/api/v1alpha1/interactions")
        .json(&serde_json::json!({
            "title": "HTTP agent session",
            "target": {"kind": "agent", "id": agent_id},
            "working_directory": temp.path(),
            "client_name": "api-test"
        }))
        .await;
    created.assert_status(StatusCode::CREATED);
    let conversation_id = created.json::<serde_json::Value>()["interaction"]["conversation_id"]
        .as_str()
        .expect("interaction id")
        .to_owned();
    server
        .get(&format!("/api/v1alpha1/interactions/{conversation_id}"))
        .await
        .assert_status_ok();
    let target = server
        .put(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/target"
        ))
        .json(&serde_json::json!({
            "target": {"kind": "agent", "id": agent_id}
        }))
        .await;
    target.assert_status_ok();
    assert_eq!(
        target.json::<serde_json::Value>()["config"]["target"]["id"],
        agent_id
    );
    let unsupported_config = server
        .post("/api/v1alpha1/interactions")
        .json(&serde_json::json!({
            "title": "unsupported provider override",
            "target": {"kind": "agent", "id": agent_id},
            "working_directory": temp.path(),
            "model": "must-not-be-ignored"
        }))
        .await;
    unsupported_config.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    server
        .post(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/approve"
        ))
        .json(&serde_json::json!({"approval_id": uuid::Uuid::now_v7()}))
        .await
        .assert_status(StatusCode::NOT_FOUND);
    let turn_id = uuid::Uuid::now_v7().to_string();
    let prompt = serde_json::json!({
        "turn_id": turn_id,
        "prompt": "respond through the durable interaction API",
        "working_directory": temp.path(),
        "client_name": "api-test"
    });
    let started = server
        .post(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/prompt"
        ))
        .json(&prompt)
        .await;
    started.assert_status(StatusCode::ACCEPTED);
    let handle = started.json::<serde_json::Value>()["handle"].clone();
    assert_eq!(handle["turn_id"], turn_id);

    let replay = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let response = server
                .get(&format!(
                    "/api/v1alpha1/interactions/{conversation_id}/events?after_sequence=0&limit=100"
                ))
                .await;
            response.assert_status_ok();
            let body = response.json::<serde_json::Value>();
            if body["data"]
                .as_array()
                .expect("event page")
                .iter()
                .any(|event| event["event"]["type"] == "turn_completed")
            {
                return body;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("interaction should complete");
    let events = replay["data"].as_array().expect("events");
    assert_eq!(events[0]["event"]["type"], "turn_started");
    assert_eq!(events[0]["sequence"], 1);
    assert_eq!(
        events.last().expect("terminal event")["event"]["type"],
        "turn_completed"
    );
    let first_sequence = events[0]["sequence"].as_u64().expect("first sequence");
    let final_checkpoint = replay["checkpoint"]["next_after_sequence"]
        .as_u64()
        .expect("final checkpoint");
    let first_page = server
        .get(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/events?after_sequence=0&limit=1"
        ))
        .await;
    first_page.assert_status_ok();
    let first_page = first_page.json::<serde_json::Value>();
    assert_eq!(first_page["data"].as_array().expect("first page").len(), 1);
    assert_eq!(first_page["checkpoint"]["next_after_sequence"], 1);
    assert_eq!(first_page["checkpoint"]["has_more"], true);

    let transcript = server
        .get(&format!("/api/v1alpha1/conversations/{conversation_id}"))
        .await;
    transcript.assert_status_ok();
    let transcript = transcript.json::<serde_json::Value>();
    assert_eq!(transcript["message_count"], 2);
    assert_eq!(transcript["messages"][0]["role"], "user");
    assert_eq!(
        transcript["messages"][0]["content"],
        "respond through the durable interaction API"
    );
    assert_eq!(transcript["messages"][1]["role"], "assistant");
    assert!(!transcript["messages"][1]["content"]
        .as_str()
        .expect("assistant text")
        .is_empty());

    let retry = server
        .post(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/prompt"
        ))
        .json(&prompt)
        .await;
    retry.assert_status(StatusCode::ACCEPTED);
    assert_eq!(retry.json::<serde_json::Value>()["handle"], handle);
    let conflict = server
        .post(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/prompt"
        ))
        .json(&serde_json::json!({
            "turn_id": turn_id,
            "prompt": "different prompt",
            "working_directory": temp.path()
        }))
        .await;
    conflict.assert_status(StatusCode::CONFLICT);
    assert_eq!(
        conflict.json::<serde_json::Value>()["error"]["code"],
        "INVALID_STATE"
    );

    let second_turn_id = uuid::Uuid::now_v7().to_string();
    let second = server
        .post(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/prompt"
        ))
        .json(&serde_json::json!({
            "turn_id": second_turn_id,
            "prompt": "verify turn-filtered replay",
            "working_directory": temp.path()
        }))
        .await;
    second.assert_status(StatusCode::ACCEPTED);
    let second_replay = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let response = server
                .get(&format!(
                    "/api/v1alpha1/interactions/{conversation_id}/events?after_sequence=0&turn_id={second_turn_id}&limit=100"
                ))
                .await;
            response.assert_status_ok();
            let body = response.json::<serde_json::Value>();
            if body["data"]
                .as_array()
                .expect("filtered events")
                .iter()
                .any(|event| event["event"]["type"] == "turn_completed")
            {
                return body;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("second interaction turn should complete");
    let second_events = second_replay["data"].as_array().expect("second events");
    assert!(second_events
        .iter()
        .all(|event| event["turn_id"] == second_turn_id));
    assert!(
        second_events[0]["sequence"]
            .as_u64()
            .expect("second sequence")
            > final_checkpoint
    );
    let second_checkpoint = second_replay["checkpoint"]["next_after_sequence"]
        .as_u64()
        .expect("second checkpoint");
    let filtered_page = server
        .get(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/events?after_sequence=0&turn_id={second_turn_id}&limit=1"
        ))
        .await;
    filtered_page.assert_status_ok();
    let filtered_page = filtered_page.json::<serde_json::Value>();
    assert_eq!(
        filtered_page["data"]
            .as_array()
            .expect("filtered page")
            .len(),
        1
    );
    assert_eq!(filtered_page["checkpoint"]["has_more"], true);
    assert!(
        filtered_page["checkpoint"]["next_after_sequence"]
            .as_u64()
            .expect("filtered checkpoint")
            > final_checkpoint
    );

    server
        .get("/api/v1alpha1/interactions")
        .await
        .assert_status_ok();
    let turns = server
        .get(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/turns"
        ))
        .await;
    turns.assert_status_ok();
    assert_eq!(
        turns.json::<serde_json::Value>()["data"]
            .as_array()
            .expect("turns")
            .len(),
        2
    );
    drop(server);
    drop(runtime);

    let restarted = runtime_at(temp.path()).await;
    let restarted_server = test_server(&restarted);
    let resumed = restarted_server
        .get(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/events?after_sequence={first_sequence}&limit=100"
        ))
        .await;
    resumed.assert_status_ok();
    let resumed = resumed.json::<serde_json::Value>();
    assert_eq!(
        resumed["checkpoint"]["next_after_sequence"],
        second_checkpoint
    );
    assert!(resumed["data"]
        .as_array()
        .expect("resumed events")
        .iter()
        .any(|event| event["event"]["type"] == "turn_completed"));
    let restarted_transcript = restarted_server
        .get(&format!("/api/v1alpha1/conversations/{conversation_id}"))
        .await;
    restarted_transcript.assert_status_ok();
    assert_eq!(
        restarted_transcript.json::<serde_json::Value>()["message_count"],
        4
    );

    let token = "interaction-reader-token";
    let mut protected_config = restarted.config().as_ref().clone();
    protected_config.auth.enabled = true;
    protected_config.auth.api_keys = vec![format!("{:x}", Sha256::digest(token.as_bytes()))];
    protected_config.api.read_only = true;
    let protected = TestServer::new(
        ApiServer::from_state(app_state_from_runtime(&restarted, protected_config)).into_router(),
    );
    protected
        .get("/api/v1alpha1/interactions")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    protected
        .get(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/events?after_sequence=0"
        ))
        .authorization_bearer(token)
        .await
        .assert_status_ok();
    protected
        .post("/api/v1alpha1/interactions")
        .authorization_bearer(token)
        .json(&serde_json::json!({
            "target": {"kind": "agent", "id": agent_id},
            "working_directory": temp.path()
        }))
        .await
        .assert_status(StatusCode::METHOD_NOT_ALLOWED);

    restarted_server
        .delete(&format!("/api/v1alpha1/interactions/{conversation_id}"))
        .await
        .assert_status(StatusCode::NO_CONTENT);
    let archived_prompt = restarted_server
        .post(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/prompt"
        ))
        .json(&serde_json::json!({
            "prompt": "must not execute",
            "working_directory": temp.path()
        }))
        .await;
    archived_prompt.assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn durable_http_interaction_cancellation_is_idempotent_and_replayable() {
    let (server, agent_id) = cancellable_interaction_server();
    let created = server
        .post("/api/v1alpha1/interactions")
        .json(&serde_json::json!({
            "target": {"kind": "agent", "id": agent_id},
            "working_directory": "/tmp"
        }))
        .await;
    created.assert_status(StatusCode::CREATED);
    let conversation_id = created.json::<serde_json::Value>()["interaction"]["conversation_id"]
        .as_str()
        .expect("interaction id")
        .to_owned();
    let started = server
        .post(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/prompt"
        ))
        .json(&serde_json::json!({
            "turn_id": uuid::Uuid::now_v7(),
            "prompt": "remain queued until cancelled",
            "working_directory": "/tmp"
        }))
        .await;
    started.assert_status(StatusCode::ACCEPTED);
    let turn_id = started.json::<serde_json::Value>()["handle"]["turn_id"]
        .as_str()
        .expect("turn id")
        .to_owned();
    let cancel_path =
        format!("/api/v1alpha1/interactions/{conversation_id}/turns/{turn_id}/cancel");
    let cancelled = server.post(&cancel_path).await;
    cancelled.assert_status_ok();
    assert_eq!(
        cancelled.json::<serde_json::Value>()["turn"]["state"],
        "cancelled"
    );
    let repeated = server.post(&cancel_path).await;
    repeated.assert_status_ok();
    assert_eq!(
        repeated.json::<serde_json::Value>()["turn"]["state"],
        "cancelled"
    );

    let replay = server
        .get(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/events?after_sequence=0&turn_id={turn_id}"
        ))
        .await;
    replay.assert_status_ok();
    let replay = replay.json::<serde_json::Value>();
    let cancellation_count = replay["data"]
        .as_array()
        .expect("cancel replay")
        .iter()
        .filter(|event| event["event"]["type"] == "turn_cancelled")
        .count();
    assert_eq!(cancellation_count, 1);
    assert_eq!(replay["checkpoint"]["has_more"], false);

    let transcript = server
        .get(&format!("/api/v1alpha1/conversations/{conversation_id}"))
        .await;
    transcript.assert_status_ok();
    let transcript = transcript.json::<serde_json::Value>();
    assert_eq!(transcript["message_count"], 2);
    assert_eq!(transcript["messages"][0]["role"], "user");
    assert_eq!(transcript["messages"][1]["role"], "assistant");
}

#[tokio::test]
async fn uncomposed_interaction_surface_reports_explicit_unavailability() {
    let pool = Arc::new(SqlitePool::open_in_memory().expect("open SQLite fixture"));
    let state = AppState::new(
        Config::default(),
        Arc::new(InMemoryAgentStore::new()),
        Arc::new(InMemoryRunManager::new()),
        pool,
        EventBus::new(8),
    );
    let server = TestServer::new(ApiServer::from_state(state).into_router());
    let unavailable = server.get("/api/v1alpha1/interactions").await;
    unavailable.assert_status(StatusCode::NOT_IMPLEMENTED);
    assert_eq!(
        unavailable.json::<serde_json::Value>()["error"]["code"],
        "NOT_IMPLEMENTED"
    );
}

#[tokio::test]
async fn configured_runtime_tools_preserve_specs_order_and_read_policy() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let runtime = runtime_at(temp.path()).await;
    let server = test_server(&runtime);

    let first = server.get("/api/v1alpha1/tools").await;
    first.assert_status_ok();
    let first = first.json::<serde_json::Value>();
    let tools = first["data"].as_array().expect("tool list");
    assert_eq!(tools.len(), 10);
    let names = tools
        .iter()
        .map(|tool| tool["id"].as_str().expect("tool id"))
        .collect::<Vec<_>>();
    assert!(names.windows(2).all(|pair| pair[0] < pair[1]));

    let second = server
        .get("/api/v1alpha1/tools")
        .await
        .json::<serde_json::Value>();
    assert_eq!(first, second);

    let referendum = server
        .get("/api/v1alpha1/tools/polkagent.governance.referendum_lookup")
        .await;
    referendum.assert_status_ok();
    let referendum = referendum.json::<serde_json::Value>();
    assert_eq!(referendum["required_grant"], "chain.query");
    assert_eq!(referendum["output_classification"], "public");
    assert_eq!(
        referendum["input_schema"]["properties"]["index"]["type"],
        "integer"
    );
    assert!(referendum["description"]
        .as_str()
        .is_some_and(|description| !description.is_empty()));

    let treasury = server
        .get("/api/v1alpha1/tools/polkagent.treasury.balance_query")
        .await;
    treasury.assert_status_ok();
    assert_eq!(
        treasury.json::<serde_json::Value>()["output_classification"],
        "internal"
    );
    let grants = server
        .get("/api/v1alpha1/tools/polkagent.governance.referendum_lookup/grants")
        .await;
    grants.assert_status_ok();
    let grants = grants.json::<serde_json::Value>();
    assert_eq!(grants["tool_id"], "polkagent.governance.referendum_lookup");
    assert_eq!(grants["required_grant"], "chain.query");

    let missing = server.get("/api/v1alpha1/tools/missing-tool").await;
    missing.assert_status(StatusCode::NOT_FOUND);
    assert_eq!(
        missing.json::<serde_json::Value>()["error"]["code"],
        "NOT_FOUND"
    );

    let token = "tool-reader-token";
    let mut protected_config = runtime.config().as_ref().clone();
    protected_config.auth.enabled = true;
    protected_config.auth.api_keys = vec![format!("{:x}", Sha256::digest(token.as_bytes()))];
    protected_config.api.read_only = true;
    let protected = TestServer::new(
        ApiServer::from_state(app_state_from_runtime(&runtime, protected_config)).into_router(),
    );
    protected
        .get("/api/v1alpha1/tools")
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    protected
        .get("/api/v1alpha1/tools")
        .authorization_bearer(token)
        .await
        .assert_status_ok();
}

#[tokio::test]
async fn disabled_tool_registration_exposes_truthful_empty_read_view() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let runtime = runtime_at(temp.path()).await;
    let mut state = app_state_from_runtime(&runtime, runtime.config().as_ref().clone());
    state.tool_registry = Some(Arc::new(RuntimeToolRegistryStore::new(None)));
    let server = TestServer::new(ApiServer::from_state(state).into_router());

    let listed = server.get("/api/v1alpha1/tools").await;
    listed.assert_status_ok();
    assert_eq!(
        listed.json::<serde_json::Value>()["data"],
        serde_json::json!([])
    );

    server
        .get("/api/v1alpha1/tools/missing-tool")
        .await
        .assert_status(StatusCode::NOT_FOUND);
    server
        .get("/api/v1alpha1/tools/missing-tool/grants")
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn artifact_content_provenance_and_policy_survive_runtime_restart() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let runtime = runtime_at(temp.path()).await;
    let server = test_server(&runtime);
    let created_agent = server
        .post("/api/v1alpha1/agents")
        .json(&serde_json::json!({
            "name": "artifact-runtime-agent",
            "model": "fake/default-model"
        }))
        .await;
    created_agent.assert_status(StatusCode::CREATED);
    let agent_id = created_agent.json::<serde_json::Value>()["id"]
        .as_str()
        .expect("agent id")
        .to_owned();
    let created_run = server
        .post(&format!("/api/v1alpha1/agents/{agent_id}/runs"))
        .json(&serde_json::json!({"input": "produce evidence"}))
        .await;
    created_run.assert_status(StatusCode::CREATED);
    let run_id: RunId = created_run.json::<serde_json::Value>()["id"]
        .as_str()
        .expect("run id")
        .parse()
        .expect("typed run id");

    let artifacts = RuntimeArtifactStore::new(runtime.pool().clone());
    let root = ArtifactId::new();
    let parent = ArtifactId::new();
    let child = ArtifactId::new();
    store_runtime_artifact(&artifacts, root, run_id, "evidence/root", "public", b"root").await;
    store_runtime_artifact(
        &artifacts,
        parent,
        run_id,
        "evidence/parent",
        "internal",
        b"parent",
    )
    .await;
    store_runtime_artifact(
        &artifacts,
        child,
        run_id,
        "evidence/child",
        "private",
        b"durable child content",
    )
    .await;
    artifacts
        .add_lineage(child, parent)
        .await
        .expect("child lineage");
    artifacts
        .add_lineage(parent, root)
        .await
        .expect("parent lineage");

    let metadata = server
        .get(&format!("/api/v1alpha1/artifacts/{child}"))
        .await;
    metadata.assert_status_ok();
    let metadata = metadata.json::<serde_json::Value>();
    assert_eq!(metadata["kind"], "evidence/child");
    assert_eq!(metadata["classification"], "private");
    let content = server
        .get(&format!("/api/v1alpha1/artifacts/{child}/content"))
        .await;
    content.assert_status_ok();
    assert_eq!(content.as_bytes().as_ref(), b"durable child content");

    drop(server);
    drop(runtime);
    let restarted = runtime_at(temp.path()).await;
    let restarted_server = test_server(&restarted);
    let listed = restarted_server
        .get(&format!("/api/v1alpha1/runs/{run_id}/artifacts"))
        .await;
    listed.assert_status_ok();
    assert_eq!(
        listed.json::<serde_json::Value>()["data"]
            .as_array()
            .expect("artifact list")
            .len(),
        3
    );
    let provenance = restarted_server
        .get(&format!("/api/v1alpha1/artifacts/{child}/provenance"))
        .await;
    provenance.assert_status_ok();
    let provenance = provenance.json::<serde_json::Value>();
    let chain = provenance["chain"].as_array().expect("provenance chain");
    assert_eq!(chain.len(), 3);
    assert_eq!(chain[0]["id"], root.to_string());
    assert_eq!(chain[1]["id"], parent.to_string());
    assert_eq!(chain[2]["id"], child.to_string());
    let restarted_content = restarted_server
        .get(&format!("/api/v1alpha1/artifacts/{child}/content"))
        .await;
    restarted_content.assert_status_ok();
    assert_eq!(
        restarted_content.as_bytes().as_ref(),
        b"durable child content"
    );

    let token = "artifact-reader-token";
    let mut protected_config = restarted.config().as_ref().clone();
    protected_config.auth.enabled = true;
    protected_config.auth.api_keys = vec![format!("{:x}", Sha256::digest(token.as_bytes()))];
    protected_config.api.read_only = true;
    let protected = TestServer::new(
        ApiServer::from_state(app_state_from_runtime(&restarted, protected_config)).into_router(),
    );
    protected
        .get(&format!("/api/v1alpha1/artifacts/{child}"))
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    protected
        .get(&format!("/api/v1alpha1/artifacts/{child}"))
        .authorization_bearer(token)
        .await
        .assert_status_ok();
    protected
        .post("/api/v1alpha1/agents")
        .authorization_bearer(token)
        .json(&serde_json::json!({
            "name": "blocked-by-read-only",
            "model": "fake/default-model"
        }))
        .await
        .assert_status(StatusCode::METHOD_NOT_ALLOWED);
}
