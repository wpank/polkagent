//! Integration tests for the `polkagent-api` REST API.
//!
//! Each test builds an in-memory `AppState`, wraps it in the Axum router via
//! `ApiServer::into_router`, and drives it with `axum_test::TestServer`.
//! No real TCP sockets or database files are opened.

use std::sync::Arc;
use std::time::Duration;

use axum_test::TestServer;
use polkagent_core::{EffectAttemptId, EffectId, EffectOutcomeId, RunId, Timestamp, WorkerId};
use polkagent_store_trait::{
    EffectStore, StoredIntent, StoredOutcome, StoreError,
};
use serde_json::json;

use polkagent_api::{RunManager, server::ApiServer};
use polkagent_config::Config;

// ---------------------------------------------------------------------------
// In-memory EffectStore for tests
// ---------------------------------------------------------------------------

/// Minimal `EffectStore` implementation that stores nothing and returns
/// `NotFound` for every read. This is sufficient to satisfy the health-check
/// readiness probe (which expects either `Ok` or `NotFound`).
struct NoopEffectStore;

#[async_trait::async_trait]
impl EffectStore for NoopEffectStore {
    async fn propose_intent(&self, _intent: StoredIntent) -> Result<(), StoreError> {
        Ok(())
    }

    async fn claim_intent(
        &self,
        _worker_id: WorkerId,
        _lease_duration: Duration,
    ) -> Result<Option<StoredIntent>, StoreError> {
        Ok(None)
    }

    async fn claim_intent_by_id(
        &self,
        intent_id: EffectId,
        _worker_id: WorkerId,
        _lease_duration: Duration,
    ) -> Result<StoredIntent, StoreError> {
        Err(StoreError::NotFound {
            resource_type: "EffectIntent",
            id: intent_id.to_string(),
        })
    }

    async fn release_claim(
        &self,
        _intent_id: EffectId,
        _worker_id: WorkerId,
    ) -> Result<(), StoreError> {
        Ok(())
    }

    async fn get_intent(&self, intent_id: EffectId) -> Result<StoredIntent, StoreError> {
        Err(StoreError::NotFound {
            resource_type: "EffectIntent",
            id: intent_id.to_string(),
        })
    }

    async fn get_by_run(&self, _run_id: RunId) -> Result<Vec<StoredIntent>, StoreError> {
        Ok(vec![])
    }

    async fn expired_leases(
        &self,
        _cutoff: Timestamp,
    ) -> Result<Vec<StoredIntent>, StoreError> {
        Ok(vec![])
    }

    async fn record_attempt_start(
        &self,
        _attempt_id: EffectAttemptId,
        _intent_id: EffectId,
        _worker_id: WorkerId,
        _payload: serde_json::Value,
    ) -> Result<(), StoreError> {
        Ok(())
    }

    async fn record_outcome(&self, _outcome: StoredOutcome) -> Result<(), StoreError> {
        Ok(())
    }

    async fn unconsumed_outcomes(
        &self,
        _run_id: RunId,
    ) -> Result<Vec<StoredOutcome>, StoreError> {
        Ok(vec![])
    }

    async fn mark_outcomes_consumed(
        &self,
        _outcome_ids: &[EffectOutcomeId],
    ) -> Result<(), StoreError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Build a `TestServer` backed by the full Axum router.
fn test_server() -> TestServer {
    let store: Arc<dyn EffectStore> = Arc::new(NoopEffectStore);
    let server = ApiServer::new(Config::default(), RunManager::new(), store);
    TestServer::new(server.into_router())
}

// ---------------------------------------------------------------------------
// Health endpoint tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_live_returns_200() {
    let server = test_server();
    let response = server.get("/health/live").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn health_ready_returns_200_when_store_up() {
    let server = test_server();
    let response = server.get("/health/ready").await;
    // Our NoopEffectStore returns NotFound which is the healthy response.
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn health_startup_returns_200() {
    let server = test_server();
    let response = server.get("/health/startup").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["status"], "ok");
}

// ---------------------------------------------------------------------------
// Agent CRUD tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_agent_returns_201() {
    let server = test_server();
    let response = server
        .post("/api/v1alpha1/agents")
        .json(&json!({
            "name": "Test Agent",
            "model": "anthropic/claude-opus-4-6"
        }))
        .await;
    response.assert_status(axum::http::StatusCode::CREATED);
    let body: serde_json::Value = response.json();
    assert_eq!(body["version"], "v1alpha1");
    assert_eq!(body["spec"]["name"], "Test Agent");
    assert!(body["id"].is_string());
}

#[tokio::test]
async fn create_agent_validates_empty_name() {
    let server = test_server();
    let response = server
        .post("/api/v1alpha1/agents")
        .json(&json!({ "name": "   ", "model": "anthropic/claude-opus-4-6" }))
        .await;
    response.assert_status(axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = response.json();
    assert_eq!(body["error"]["code"], "VALIDATION_ERROR");
}

#[tokio::test]
async fn create_agent_validates_empty_model() {
    let server = test_server();
    let response = server
        .post("/api/v1alpha1/agents")
        .json(&json!({ "name": "My Agent", "model": "" }))
        .await;
    response.assert_status(axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = response.json();
    assert_eq!(body["error"]["code"], "VALIDATION_ERROR");
}

#[tokio::test]
async fn list_agents_empty() {
    let server = test_server();
    let response = server.get("/api/v1alpha1/agents").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["data"].as_array().expect("data array").len(), 0);
    assert!(!body["cursor"]["has_more"].as_bool().unwrap_or(true));
}

#[tokio::test]
async fn create_then_list_then_get_agent() {
    let server = test_server();

    // Create
    let create_resp = server
        .post("/api/v1alpha1/agents")
        .json(&json!({
            "name": "Builder Agent",
            "model": "anthropic/claude-opus-4-6",
            "description": "Polkadot SDK coding assistant"
        }))
        .await;
    create_resp.assert_status(axum::http::StatusCode::CREATED);
    let created: serde_json::Value = create_resp.json();
    let agent_id = created["id"].as_str().expect("id present").to_owned();

    // List — should contain the agent
    let list_resp = server.get("/api/v1alpha1/agents").await;
    list_resp.assert_status_ok();
    let list_body: serde_json::Value = list_resp.json();
    let agents = list_body["data"].as_array().expect("data array");
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0]["id"], agent_id.as_str());

    // Get by ID
    let get_resp = server
        .get(&format!("/api/v1alpha1/agents/{agent_id}"))
        .await;
    get_resp.assert_status_ok();
    let get_body: serde_json::Value = get_resp.json();
    assert_eq!(get_body["id"], agent_id.as_str());
    assert_eq!(get_body["spec"]["name"], "Builder Agent");
}

#[tokio::test]
async fn get_agent_not_found_returns_404() {
    let server = test_server();
    let fake_id = polkagent_core::AgentId::new().to_string();
    let response = server
        .get(&format!("/api/v1alpha1/agents/{fake_id}"))
        .await;
    response.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = response.json();
    assert_eq!(body["error"]["code"], "AGENT_NOT_FOUND");
    assert!(body["error"]["request_id"].is_string());
}

#[tokio::test]
async fn delete_agent_returns_204() {
    let server = test_server();

    // Create
    let create_resp = server
        .post("/api/v1alpha1/agents")
        .json(&json!({ "name": "To Delete", "model": "anthropic/claude-opus-4-6" }))
        .await;
    create_resp.assert_status(axum::http::StatusCode::CREATED);
    let created: serde_json::Value = create_resp.json();
    let agent_id = created["id"].as_str().expect("id").to_owned();

    // Delete
    let delete_resp = server
        .delete(&format!("/api/v1alpha1/agents/{agent_id}"))
        .await;
    delete_resp.assert_status(axum::http::StatusCode::NO_CONTENT);

    // Get should now be 404
    let get_resp = server
        .get(&format!("/api/v1alpha1/agents/{agent_id}"))
        .await;
    get_resp.assert_status(axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_nonexistent_agent_returns_404() {
    let server = test_server();
    let fake_id = polkagent_core::AgentId::new().to_string();
    let response = server
        .delete(&format!("/api/v1alpha1/agents/{fake_id}"))
        .await;
    response.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = response.json();
    assert_eq!(body["error"]["code"], "AGENT_NOT_FOUND");
}

// ---------------------------------------------------------------------------
// Run lifecycle tests
// ---------------------------------------------------------------------------

/// Helper: create an agent and return its ID string.
async fn create_agent(server: &TestServer) -> String {
    let resp = server
        .post("/api/v1alpha1/agents")
        .json(&json!({ "name": "Run Test Agent", "model": "anthropic/claude-opus-4-6" }))
        .await;
    resp.assert_status(axum::http::StatusCode::CREATED);
    let body: serde_json::Value = resp.json();
    body["id"].as_str().expect("id").to_owned()
}

#[tokio::test]
async fn create_run_returns_201() {
    let server = test_server();
    let agent_id = create_agent(&server).await;

    let resp = server
        .post(&format!("/api/v1alpha1/agents/{agent_id}/runs"))
        .json(&json!({ "input": { "type": "user_message", "content": "Hello" } }))
        .await;
    resp.assert_status(axum::http::StatusCode::CREATED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    assert_eq!(body["agent_id"], agent_id.as_str());
    assert!(body["id"].is_string());
    assert_eq!(body["status"]["state"], "running");
}

#[tokio::test]
async fn create_run_for_missing_agent_returns_404() {
    let server = test_server();
    let fake_id = polkagent_core::AgentId::new().to_string();
    let resp = server
        .post(&format!("/api/v1alpha1/agents/{fake_id}/runs"))
        .json(&json!({ "input": "hello" }))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "AGENT_NOT_FOUND");
}

#[tokio::test]
async fn get_run_returns_run_detail() {
    let server = test_server();
    let agent_id = create_agent(&server).await;

    let create_resp = server
        .post(&format!("/api/v1alpha1/agents/{agent_id}/runs"))
        .json(&json!({ "input": "ping" }))
        .await;
    create_resp.assert_status(axum::http::StatusCode::CREATED);
    let run: serde_json::Value = create_resp.json();
    let run_id = run["id"].as_str().expect("run_id").to_owned();

    let get_resp = server
        .get(&format!("/api/v1alpha1/runs/{run_id}"))
        .await;
    get_resp.assert_status_ok();
    let body: serde_json::Value = get_resp.json();
    assert_eq!(body["id"], run_id.as_str());
    assert_eq!(body["agent_id"], agent_id.as_str());
}

#[tokio::test]
async fn get_run_not_found_returns_404() {
    let server = test_server();
    let fake_id = polkagent_core::RunId::new().to_string();
    let resp = server
        .get(&format!("/api/v1alpha1/runs/{fake_id}"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "RUN_NOT_FOUND");
}

#[tokio::test]
async fn list_runs_returns_all_runs() {
    let server = test_server();
    let agent_id = create_agent(&server).await;

    // Create two runs.
    for _ in 0..2 {
        server
            .post(&format!("/api/v1alpha1/agents/{agent_id}/runs"))
            .json(&json!({ "input": "task" }))
            .await
            .assert_status(axum::http::StatusCode::CREATED);
    }

    let resp = server.get("/api/v1alpha1/runs").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["data"].as_array().expect("data").len(), 2);
}

#[tokio::test]
async fn list_runs_filter_by_agent_id() {
    let server = test_server();
    let agent_a = create_agent(&server).await;
    let agent_b = {
        let resp = server
            .post("/api/v1alpha1/agents")
            .json(&json!({ "name": "Agent B", "model": "anthropic/claude-opus-4-6" }))
            .await;
        let body: serde_json::Value = resp.json();
        body["id"].as_str().expect("id").to_owned()
    };

    // One run for A, two runs for B.
    server
        .post(&format!("/api/v1alpha1/agents/{agent_a}/runs"))
        .json(&json!({ "input": "a" }))
        .await;
    for _ in 0..2 {
        server
            .post(&format!("/api/v1alpha1/agents/{agent_b}/runs"))
            .json(&json!({ "input": "b" }))
            .await;
    }

    let resp = server
        .get(&format!("/api/v1alpha1/runs?agent_id={agent_a}"))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    let runs = body["data"].as_array().expect("data");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["agent_id"], agent_a.as_str());
}

#[tokio::test]
async fn cancel_run_returns_updated_run() {
    let server = test_server();
    let agent_id = create_agent(&server).await;

    let create_resp = server
        .post(&format!("/api/v1alpha1/agents/{agent_id}/runs"))
        .json(&json!({ "input": "long task" }))
        .await;
    let run: serde_json::Value = create_resp.json();
    let run_id = run["id"].as_str().expect("run_id").to_owned();

    let cancel_resp = server
        .post(&format!("/api/v1alpha1/runs/{run_id}/cancel"))
        .await;
    cancel_resp.assert_status_ok();
    let body: serde_json::Value = cancel_resp.json();
    // The state is a tagged enum; the "state" tag value should be "cancelled".
    assert_eq!(body["status"]["state"], "cancelled");
    assert!(body["completed_at"].is_string());
}

#[tokio::test]
async fn cancel_already_cancelled_run_returns_409() {
    let server = test_server();
    let agent_id = create_agent(&server).await;

    let create_resp = server
        .post(&format!("/api/v1alpha1/agents/{agent_id}/runs"))
        .json(&json!({ "input": "task" }))
        .await;
    let run: serde_json::Value = create_resp.json();
    let run_id = run["id"].as_str().expect("run_id").to_owned();

    // First cancel — should succeed.
    server
        .post(&format!("/api/v1alpha1/runs/{run_id}/cancel"))
        .await
        .assert_status_ok();

    // Second cancel — run is terminal, should be 409.
    let second_cancel = server
        .post(&format!("/api/v1alpha1/runs/{run_id}/cancel"))
        .await;
    second_cancel.assert_status(axum::http::StatusCode::CONFLICT);
    let body: serde_json::Value = second_cancel.json();
    assert_eq!(body["error"]["code"], "INVALID_STATE");
}

// ---------------------------------------------------------------------------
// Pagination tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pagination_limit_respected() {
    let server = test_server();
    let agent_id = create_agent(&server).await;

    // Create 5 runs.
    for _ in 0..5 {
        server
            .post(&format!("/api/v1alpha1/agents/{agent_id}/runs"))
            .json(&json!({ "input": "task" }))
            .await;
    }

    // Request page of 2.
    let resp = server.get("/api/v1alpha1/runs?limit=2").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["data"].as_array().expect("data").len(), 2);
    assert!(body["cursor"]["has_more"].as_bool().unwrap_or(false));
    let next_cursor = body["cursor"]["next"].as_str().expect("next cursor");

    // Fetch next page using the cursor.
    let page2 = server
        .get(&format!("/api/v1alpha1/runs?limit=2&after={next_cursor}"))
        .await;
    page2.assert_status_ok();
    let page2_body: serde_json::Value = page2.json();
    assert_eq!(page2_body["data"].as_array().expect("data").len(), 2);
}

#[tokio::test]
async fn pagination_last_page_has_more_false() {
    let server = test_server();
    let agent_id = create_agent(&server).await;

    // Create 3 runs.
    for _ in 0..3 {
        server
            .post(&format!("/api/v1alpha1/agents/{agent_id}/runs"))
            .json(&json!({ "input": "task" }))
            .await;
    }

    // Request all 3 (limit=10).
    let resp = server.get("/api/v1alpha1/runs?limit=10").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["data"].as_array().expect("data").len(), 3);
    assert!(!body["cursor"]["has_more"].as_bool().unwrap_or(true));
    assert!(body["cursor"]["next"].is_null());
}

// ---------------------------------------------------------------------------
// System info test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn system_info_returns_version_and_uptime() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/system/info").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    assert!(body["platform_version"].is_string());
    assert!(body["uptime_secs"].as_u64().is_some());
    assert!(body["config_summary"]["bind_address"].is_string());
    assert!(body["config_summary"]["database_backend"].is_string());
}

// ---------------------------------------------------------------------------
// Error response format tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn error_response_includes_request_id_and_timestamp() {
    let server = test_server();
    let fake_id = polkagent_core::AgentId::new().to_string();
    let resp = server
        .get(&format!("/api/v1alpha1/agents/{fake_id}"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    let err = &body["error"];
    assert!(err["code"].is_string());
    assert!(err["message"].is_string());
    assert!(err["request_id"].is_string());
    assert!(err["timestamp"].is_string());
}
