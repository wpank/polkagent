//! Black-box HTTP coverage for the shared-runtime durable adapters.

#![allow(
    clippy::expect_used,
    reason = "integration tests fail immediately at controlled fixture boundaries"
)]

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use axum_test::TestServer;
use polkagent_api::{AgentStore, ApiServer, RunManagerTrait, RuntimeAgentStore, RuntimeRunManager};
use polkagent_runtime::{AdapterPolicy, PolkagentRuntime, RuntimeFactory, RuntimeOptions};
use polkagent_store_trait::EffectStore;

async fn runtime_at(root: &std::path::Path) -> PolkagentRuntime {
    let config_path = root.join("polkagent.toml");
    std::fs::write(&config_path, "").expect("write empty config");
    let mut options = RuntimeOptions::new(root);
    options.config_path = Some(config_path);
    options.database_path = Some(root.join("api.db"));
    options.disable_harness = true;
    options.adapter_policy = AdapterPolicy::AllowSimulated;
    options.discover_environment_providers = false;
    RuntimeFactory::build(options).await.expect("build runtime")
}

fn test_server(runtime: &PolkagentRuntime) -> TestServer {
    let agents: Arc<dyn AgentStore> = Arc::new(RuntimeAgentStore::from_runtime(runtime));
    let runs: Arc<dyn RunManagerTrait> = Arc::new(RuntimeRunManager::from_runtime(runtime));
    let effects: Arc<dyn EffectStore> = Arc::new(runtime.pool().clone());
    let server = ApiServer::new(
        runtime.config().as_ref().clone(),
        agents,
        runs,
        effects,
        runtime.event_bus().clone(),
    );
    TestServer::new(server.into_router())
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
