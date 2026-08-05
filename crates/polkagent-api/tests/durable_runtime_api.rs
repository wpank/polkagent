//! Black-box HTTP coverage for the shared-runtime durable adapters.

#![allow(
    clippy::expect_used,
    reason = "integration tests fail immediately at controlled fixture boundaries"
)]

use std::{sync::Arc, time::Duration};

use axum::http::StatusCode;
use axum_test::TestServer;
use polkagent_api::{
    app_state_from_runtime, ApiServer, RuntimeArtifactStore, RuntimeToolRegistryStore,
    RUNTIME_UNAVAILABLE_ROUTES,
};
use polkagent_core::{ArtifactId, BlobRef, RunId};
use polkagent_runtime::{AdapterPolicy, PolkagentRuntime, RuntimeFactory, RuntimeOptions};
use polkagent_store_trait::ArtifactStore;
use sha2::{Digest, Sha256};

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
    let state = app_state_from_runtime(runtime, runtime.config().as_ref().clone());
    let server = ApiServer::from_state(state);
    TestServer::new(server.into_router())
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
async fn runtime_server_composes_real_optional_stores_and_publishes_501_boundary() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let runtime = runtime_at(temp.path()).await;
    let state = app_state_from_runtime(&runtime, runtime.config().as_ref().clone());

    assert!(state.event_store.is_some());
    assert!(state.payment_store.is_some());
    assert!(state.conversation_store.is_some());
    assert!(state.artifact_store.is_some());
    assert!(state.skill_registry.is_none());
    assert!(state.tool_registry.is_some());
    assert!(state.memory_store.is_none());
    assert!(state.audit_store.is_none());
    assert!(state.service_registry_store.is_none());

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

    assert_eq!(RUNTIME_UNAVAILABLE_ROUTES.len(), 15);
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
                    "/api/v1alpha1/memory/query" => serde_json::json!({"query": "missing"}),
                    "/api/v1alpha1/memory/forget" => {
                        serde_json::json!({"entry_ids": ["missing-entry"]})
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
