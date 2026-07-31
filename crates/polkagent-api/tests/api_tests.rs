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

use polkagent_api::{InMemoryRunManager, InMemoryAgentStore, server::ApiServer};
use polkagent_config::{Config, ProviderConfig};
use polkagent_event::EventBus;

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

/// Build a `TestServer` backed by the full Axum router with in-memory stores.
fn test_server() -> TestServer {
    let agents = Arc::new(InMemoryAgentStore::new());
    let run_manager = Arc::new(InMemoryRunManager::new());
    let store: Arc<dyn EffectStore> = Arc::new(NoopEffectStore);
    let event_bus = EventBus::with_default_capacity();
    let server = ApiServer::new(Config::default(), agents, run_manager, store, event_bus);
    TestServer::new(server.into_router())
}

/// Build a `TestServer` with a custom `Config` (e.g. to inject providers).
fn test_server_with_config(config: Config) -> TestServer {
    let agents = Arc::new(InMemoryAgentStore::new());
    let run_manager = Arc::new(InMemoryRunManager::new());
    let store: Arc<dyn EffectStore> = Arc::new(NoopEffectStore);
    let event_bus = EventBus::with_default_capacity();
    let server = ApiServer::new(config, agents, run_manager, store, event_bus);
    TestServer::new(server.into_router())
}

/// Helper: create an agent and return its ID string.
async fn create_test_agent(server: &TestServer) -> String {
    let resp = server
        .post("/api/v1alpha1/agents")
        .json(&json!({ "name": "Run Test Agent", "model": "anthropic/claude-opus-4-6" }))
        .await;
    resp.assert_status(axum::http::StatusCode::CREATED);
    let body: serde_json::Value = resp.json();
    body["id"].as_str().expect("id").to_owned()
}

/// Helper: create a run for the given agent and return the run ID string.
async fn create_test_run(server: &TestServer, agent_id: &str) -> String {
    let resp = server
        .post(&format!("/api/v1alpha1/agents/{agent_id}/runs"))
        .json(&json!({ "input": "test task" }))
        .await;
    resp.assert_status(axum::http::StatusCode::CREATED);
    let body: serde_json::Value = resp.json();
    body["id"].as_str().expect("run_id").to_owned()
}

// ===========================================================================
// Error format tests
// ===========================================================================

#[tokio::test]
async fn error_404_includes_all_required_fields() {
    let server = test_server();
    let fake_id = polkagent_core::AgentId::new().to_string();
    let resp = server
        .get(&format!("/api/v1alpha1/agents/{fake_id}"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    let err = &body["error"];
    assert!(err["code"].is_string(), "error.code must be a string");
    assert!(err["message"].is_string(), "error.message must be a string");
    assert!(err["request_id"].is_string(), "error.request_id must be a string");
    assert!(err["timestamp"].is_string(), "error.timestamp must be a string");
}

#[tokio::test]
async fn error_422_includes_validation_details() {
    let server = test_server();
    let resp = server
        .post("/api/v1alpha1/agents")
        .json(&json!({ "name": "   ", "model": "anthropic/claude-opus-4-6" }))
        .await;
    resp.assert_status(axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = resp.json();
    let err = &body["error"];
    assert_eq!(err["code"], "VALIDATION_ERROR");
    // The message should contain validation-relevant information.
    let message = err["message"].as_str().expect("message string");
    assert!(
        message.contains("name"),
        "422 error message should reference the invalid field"
    );
    assert!(err["request_id"].is_string());
    assert!(err["timestamp"].is_string());
}

#[tokio::test]
async fn error_409_includes_all_required_fields() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;
    let run_id = create_test_run(&server, &agent_id).await;

    // Cancel once to make the run terminal.
    server
        .post(&format!("/api/v1alpha1/runs/{run_id}/cancel"))
        .await
        .assert_status_ok();

    // Cancel again to get a 409.
    let resp = server
        .post(&format!("/api/v1alpha1/runs/{run_id}/cancel"))
        .await;
    resp.assert_status(axum::http::StatusCode::CONFLICT);
    let body: serde_json::Value = resp.json();
    let err = &body["error"];
    assert_eq!(err["code"], "INVALID_STATE");
    assert!(err["message"].is_string());
    assert!(err["request_id"].is_string());
    assert!(err["timestamp"].is_string());
}

#[tokio::test]
async fn error_timestamp_is_valid_iso8601() {
    let server = test_server();
    let fake_id = polkagent_core::AgentId::new().to_string();
    let resp = server
        .get(&format!("/api/v1alpha1/agents/{fake_id}"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    let timestamp = body["error"]["timestamp"]
        .as_str()
        .expect("timestamp must be a string");
    // chrono::DateTime::parse_from_rfc3339 validates ISO 8601.
    let parsed = chrono::DateTime::parse_from_rfc3339(timestamp);
    assert!(
        parsed.is_ok(),
        "error timestamp '{}' is not valid ISO-8601/RFC-3339: {:?}",
        timestamp,
        parsed.err()
    );
}

#[tokio::test]
async fn error_request_id_is_valid_uuid() {
    let server = test_server();
    let fake_id = polkagent_core::RunId::new().to_string();
    let resp = server
        .get(&format!("/api/v1alpha1/runs/{fake_id}"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    let request_id = body["error"]["request_id"]
        .as_str()
        .expect("request_id must be a string");
    assert!(
        uuid::Uuid::parse_str(request_id).is_ok(),
        "request_id '{}' should be a valid UUID",
        request_id
    );
}

#[tokio::test]
async fn error_501_includes_all_required_fields() {
    let server = test_server();
    let fake_id = polkagent_core::EffectId::new().to_string();
    // Approve effect is a stub that returns 501.
    let resp = server
        .post(&format!("/api/v1alpha1/effects/{fake_id}/approve"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    let err = &body["error"];
    assert_eq!(err["code"], "NOT_IMPLEMENTED");
    assert!(err["message"].is_string());
    assert!(err["request_id"].is_string());
    assert!(err["timestamp"].is_string());
}

// ===========================================================================
// Pagination tests
// ===========================================================================

#[tokio::test]
async fn pagination_cursor_with_after_and_limit() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;

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
    assert!(page2_body["cursor"]["has_more"].as_bool().unwrap_or(false));

    // Third page should have exactly 1 item and has_more = false.
    let next_cursor2 = page2_body["cursor"]["next"].as_str().expect("next cursor");
    let page3 = server
        .get(&format!(
            "/api/v1alpha1/runs?limit=2&after={next_cursor2}"
        ))
        .await;
    page3.assert_status_ok();
    let page3_body: serde_json::Value = page3.json();
    assert_eq!(page3_body["data"].as_array().expect("data").len(), 1);
    assert!(!page3_body["cursor"]["has_more"].as_bool().unwrap_or(true));
    assert!(page3_body["cursor"]["next"].is_null());
}

#[tokio::test]
async fn pagination_has_more_correct_at_boundary() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;

    // Create exactly 3 runs.
    for _ in 0..3 {
        server
            .post(&format!("/api/v1alpha1/agents/{agent_id}/runs"))
            .json(&json!({ "input": "task" }))
            .await;
    }

    // Request with limit=3 (exact boundary) -- should get all 3, has_more = false.
    let resp = server.get("/api/v1alpha1/runs?limit=3").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["data"].as_array().expect("data").len(), 3);
    assert!(!body["cursor"]["has_more"].as_bool().unwrap_or(true));
    assert!(body["cursor"]["next"].is_null());
}

#[tokio::test]
async fn pagination_page_size_matches_requested_limit() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;

    // Create 10 runs.
    for _ in 0..10 {
        server
            .post(&format!("/api/v1alpha1/agents/{agent_id}/runs"))
            .json(&json!({ "input": "task" }))
            .await;
    }

    // Request page of 7.
    let resp = server.get("/api/v1alpha1/runs?limit=7").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    let page_size = body["meta"]["page_size"].as_u64().expect("page_size");
    assert_eq!(page_size, 7);
    assert_eq!(body["data"].as_array().expect("data").len(), 7);
}

#[tokio::test]
async fn pagination_default_limit_when_not_specified() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;

    // Create 3 runs.
    for _ in 0..3 {
        server
            .post(&format!("/api/v1alpha1/agents/{agent_id}/runs"))
            .json(&json!({ "input": "task" }))
            .await;
    }

    // Default limit is 50; with only 3 runs we should get all 3.
    let resp = server.get("/api/v1alpha1/runs").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["data"].as_array().expect("data").len(), 3);
    assert!(!body["cursor"]["has_more"].as_bool().unwrap_or(true));
}

#[tokio::test]
async fn pagination_agents_cursor_works() {
    let server = test_server();

    // Create 4 agents.
    for i in 0..4 {
        server
            .post("/api/v1alpha1/agents")
            .json(&json!({
                "name": format!("Agent {i}"),
                "model": "anthropic/claude-opus-4-6"
            }))
            .await;
    }

    // Request page of 2.
    let resp = server.get("/api/v1alpha1/agents?limit=2").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["data"].as_array().expect("data").len(), 2);
    assert!(body["cursor"]["has_more"].as_bool().unwrap_or(false));
    assert_eq!(body["meta"]["page_size"].as_u64().unwrap(), 2);

    // Fetch second page.
    let next_cursor = body["cursor"]["next"].as_str().expect("next cursor");
    let page2 = server
        .get(&format!("/api/v1alpha1/agents?limit=2&after={next_cursor}"))
        .await;
    page2.assert_status_ok();
    let page2_body: serde_json::Value = page2.json();
    assert_eq!(page2_body["data"].as_array().expect("data").len(), 2);
    assert!(!page2_body["cursor"]["has_more"].as_bool().unwrap_or(true));
}

// ===========================================================================
// Agent endpoint tests
// ===========================================================================

#[tokio::test]
async fn create_agent_with_minimum_fields() {
    let server = test_server();
    let resp = server
        .post("/api/v1alpha1/agents")
        .json(&json!({
            "name": "Minimal Agent",
            "model": "anthropic/claude-opus-4-6"
        }))
        .await;
    resp.assert_status(axum::http::StatusCode::CREATED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    assert_eq!(body["spec"]["name"], "Minimal Agent");
    assert_eq!(body["spec"]["model"], "anthropic/claude-opus-4-6");
    assert!(body["id"].is_string());
    assert!(body["created_at"].is_string());
    assert!(body["updated_at"].is_string());
}

#[tokio::test]
async fn create_agent_with_all_fields() {
    let server = test_server();
    let resp = server
        .post("/api/v1alpha1/agents")
        .json(&json!({
            "name": "Full Agent",
            "model": "anthropic/claude-opus-4-6",
            "description": "A fully-configured agent for testing",
            "tools": ["file_read", "shell_exec", "web_search"],
            "system_prompt": "You are a helpful Polkadot developer assistant."
        }))
        .await;
    resp.assert_status(axum::http::StatusCode::CREATED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["spec"]["name"], "Full Agent");
    assert_eq!(body["spec"]["model"], "anthropic/claude-opus-4-6");
    assert_eq!(
        body["spec"]["description"],
        "A fully-configured agent for testing"
    );
    let tools = body["spec"]["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 3);
    assert!(tools.contains(&json!("file_read")));
    assert!(tools.contains(&json!("shell_exec")));
    assert!(tools.contains(&json!("web_search")));
    assert_eq!(
        body["spec"]["system_prompt"],
        "You are a helpful Polkadot developer assistant."
    );
}

#[tokio::test]
async fn create_agent_with_empty_name_returns_422() {
    let server = test_server();
    let resp = server
        .post("/api/v1alpha1/agents")
        .json(&json!({ "name": "   ", "model": "anthropic/claude-opus-4-6" }))
        .await;
    resp.assert_status(axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "VALIDATION_ERROR");
}

#[tokio::test]
async fn create_agent_with_empty_model_returns_422() {
    let server = test_server();
    let resp = server
        .post("/api/v1alpha1/agents")
        .json(&json!({ "name": "Good Name", "model": "" }))
        .await;
    resp.assert_status(axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "VALIDATION_ERROR");
}

#[tokio::test]
async fn create_agent_with_whitespace_model_returns_422() {
    let server = test_server();
    let resp = server
        .post("/api/v1alpha1/agents")
        .json(&json!({ "name": "Good Name", "model": "   " }))
        .await;
    resp.assert_status(axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "VALIDATION_ERROR");
}

#[tokio::test]
async fn list_agents_returns_correct_count() {
    let server = test_server();

    // Create 3 agents.
    for i in 0..3 {
        server
            .post("/api/v1alpha1/agents")
            .json(&json!({
                "name": format!("Agent {i}"),
                "model": "anthropic/claude-opus-4-6"
            }))
            .await
            .assert_status(axum::http::StatusCode::CREATED);
    }

    let resp = server.get("/api/v1alpha1/agents").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["data"].as_array().expect("data").len(), 3);
    assert_eq!(body["meta"]["page_size"].as_u64().unwrap(), 3);
}

#[tokio::test]
async fn list_agents_empty_returns_zero() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/agents").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["data"].as_array().expect("data").len(), 0);
    assert!(!body["cursor"]["has_more"].as_bool().unwrap_or(true));
    assert_eq!(body["meta"]["page_size"].as_u64().unwrap(), 0);
}

#[tokio::test]
async fn show_agent_returns_full_details() {
    let server = test_server();
    let create_resp = server
        .post("/api/v1alpha1/agents")
        .json(&json!({
            "name": "Detail Agent",
            "model": "anthropic/claude-opus-4-6",
            "description": "Testing full detail view",
            "tools": ["shell_exec"],
            "system_prompt": "Be precise."
        }))
        .await;
    create_resp.assert_status(axum::http::StatusCode::CREATED);
    let created: serde_json::Value = create_resp.json();
    let agent_id = created["id"].as_str().expect("id").to_owned();

    let get_resp = server
        .get(&format!("/api/v1alpha1/agents/{agent_id}"))
        .await;
    get_resp.assert_status_ok();
    let body: serde_json::Value = get_resp.json();
    assert_eq!(body["id"], agent_id.as_str());
    assert_eq!(body["version"], "v1alpha1");
    assert_eq!(body["spec"]["name"], "Detail Agent");
    assert_eq!(body["spec"]["model"], "anthropic/claude-opus-4-6");
    assert_eq!(body["spec"]["description"], "Testing full detail view");
    assert_eq!(body["spec"]["system_prompt"], "Be precise.");
    assert!(body["spec"]["tools"].as_array().expect("tools").contains(&json!("shell_exec")));
    assert!(body["created_at"].is_string());
    assert!(body["updated_at"].is_string());
}

#[tokio::test]
async fn delete_agent_returns_204_and_removes_from_list() {
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
    let resp = server
        .delete(&format!("/api/v1alpha1/agents/{fake_id}"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "AGENT_NOT_FOUND");
}

#[tokio::test]
async fn list_agents_after_delete_does_not_include_deleted() {
    let server = test_server();

    // Create two agents.
    let resp_a = server
        .post("/api/v1alpha1/agents")
        .json(&json!({ "name": "Agent A", "model": "m1" }))
        .await;
    resp_a.assert_status(axum::http::StatusCode::CREATED);
    let id_a = resp_a.json::<serde_json::Value>()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let resp_b = server
        .post("/api/v1alpha1/agents")
        .json(&json!({ "name": "Agent B", "model": "m2" }))
        .await;
    resp_b.assert_status(axum::http::StatusCode::CREATED);
    let id_b = resp_b.json::<serde_json::Value>()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Delete A.
    server
        .delete(&format!("/api/v1alpha1/agents/{id_a}"))
        .await
        .assert_status(axum::http::StatusCode::NO_CONTENT);

    // List should only contain B.
    let list_resp = server.get("/api/v1alpha1/agents").await;
    list_resp.assert_status_ok();
    let body: serde_json::Value = list_resp.json();
    let agents = body["data"].as_array().expect("data");
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0]["id"], id_b.as_str());
}

#[tokio::test]
async fn get_agent_not_found_returns_404() {
    let server = test_server();
    let fake_id = polkagent_core::AgentId::new().to_string();
    let resp = server
        .get(&format!("/api/v1alpha1/agents/{fake_id}"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "AGENT_NOT_FOUND");
    assert!(body["error"]["request_id"].is_string());
}

// ===========================================================================
// Run endpoint tests
// ===========================================================================

#[tokio::test]
async fn create_run_for_valid_agent() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;

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
    assert!(body["created_at"].is_string());
    assert!(body["started_at"].is_string());
}

#[tokio::test]
async fn create_run_for_nonexistent_agent_returns_404() {
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
async fn list_runs_with_agent_id_filter() {
    let server = test_server();

    let agent_a = create_test_agent(&server).await;
    let agent_b = {
        let resp = server
            .post("/api/v1alpha1/agents")
            .json(&json!({ "name": "Agent B", "model": "anthropic/claude-opus-4-6" }))
            .await;
        resp.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    // Two runs for A, one run for B.
    create_test_run(&server, &agent_a).await;
    create_test_run(&server, &agent_a).await;
    create_test_run(&server, &agent_b).await;

    // Filter by agent A.
    let resp = server
        .get(&format!("/api/v1alpha1/runs?agent_id={agent_a}"))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    let runs = body["data"].as_array().expect("data");
    assert_eq!(runs.len(), 2);
    for run in runs {
        assert_eq!(run["agent_id"], agent_a.as_str());
    }

    // Filter by agent B.
    let resp_b = server
        .get(&format!("/api/v1alpha1/runs?agent_id={agent_b}"))
        .await;
    resp_b.assert_status_ok();
    let body_b: serde_json::Value = resp_b.json();
    let runs_b = body_b["data"].as_array().expect("data");
    assert_eq!(runs_b.len(), 1);
    assert_eq!(runs_b[0]["agent_id"], agent_b.as_str());
}

#[tokio::test]
async fn list_runs_with_state_filter_returns_400_for_tagged_enum() {
    // RunState uses `#[serde(tag = "state")]` (internally-tagged enum), which
    // cannot be deserialized from a flat query-string value like `state=running`.
    // Axum correctly returns 400 Bad Request. This test documents the current
    // behaviour; a future improvement could add a dedicated query-string
    // deserializable state filter type.
    let server = test_server();
    let agent_id = create_test_agent(&server).await;
    create_test_run(&server, &agent_id).await;

    let resp = server
        .get("/api/v1alpha1/runs?state=running")
        .await;
    resp.assert_status(axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn list_runs_without_state_filter_returns_all() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;

    // Create 3 runs (all start as "running").
    let run_1 = create_test_run(&server, &agent_id).await;
    let _run_2 = create_test_run(&server, &agent_id).await;
    let _run_3 = create_test_run(&server, &agent_id).await;

    // Cancel run_1 to make it terminal.
    server
        .post(&format!("/api/v1alpha1/runs/{run_1}/cancel"))
        .await
        .assert_status_ok();

    // Without a state filter, all 3 runs (mixed states) are returned.
    let resp = server.get("/api/v1alpha1/runs").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    let all_runs = body["data"].as_array().expect("data");
    assert_eq!(all_runs.len(), 3);

    // Verify the cancelled run appears in the list with the correct state.
    let cancelled_run = all_runs.iter().find(|r| r["id"] == run_1.as_str());
    assert!(cancelled_run.is_some(), "cancelled run should appear in unfiltered list");
    assert_eq!(
        cancelled_run.unwrap()["status"]["state"],
        "cancelled"
    );
}

#[tokio::test]
async fn cancel_active_run_returns_200() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;
    let run_id = create_test_run(&server, &agent_id).await;

    let cancel_resp = server
        .post(&format!("/api/v1alpha1/runs/{run_id}/cancel"))
        .await;
    cancel_resp.assert_status_ok();
    let body: serde_json::Value = cancel_resp.json();
    assert_eq!(body["status"]["state"], "cancelled");
    assert!(body["completed_at"].is_string());
    assert!(body["terminal_reason"].is_string());
}

#[tokio::test]
async fn cancel_completed_run_returns_409() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;
    let run_id = create_test_run(&server, &agent_id).await;

    // Cancel to make it terminal.
    server
        .post(&format!("/api/v1alpha1/runs/{run_id}/cancel"))
        .await
        .assert_status_ok();

    // Second cancel -- run is already terminal, should be 409.
    let second_cancel = server
        .post(&format!("/api/v1alpha1/runs/{run_id}/cancel"))
        .await;
    second_cancel.assert_status(axum::http::StatusCode::CONFLICT);
    let body: serde_json::Value = second_cancel.json();
    assert_eq!(body["error"]["code"], "INVALID_STATE");
}

#[tokio::test]
async fn cancel_nonexistent_run_returns_404() {
    let server = test_server();
    let fake_id = polkagent_core::RunId::new().to_string();
    let resp = server
        .post(&format!("/api/v1alpha1/runs/{fake_id}/cancel"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "RUN_NOT_FOUND");
}

#[tokio::test]
async fn get_run_detail_includes_all_fields() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;

    let create_resp = server
        .post(&format!("/api/v1alpha1/agents/{agent_id}/runs"))
        .json(&json!({ "input": { "type": "user_message", "content": "detailed" } }))
        .await;
    create_resp.assert_status(axum::http::StatusCode::CREATED);
    let run: serde_json::Value = create_resp.json();
    let run_id = run["id"].as_str().expect("run_id").to_owned();

    let get_resp = server
        .get(&format!("/api/v1alpha1/runs/{run_id}"))
        .await;
    get_resp.assert_status_ok();
    let body: serde_json::Value = get_resp.json();

    // Required fields must all be present.
    assert_eq!(body["version"], "v1alpha1");
    assert_eq!(body["id"], run_id.as_str());
    assert_eq!(body["agent_id"], agent_id.as_str());
    assert!(body["status"].is_object(), "status must be an object");
    assert!(body["input"].is_object(), "input must be present");
    assert!(body["turns_completed"].is_number(), "turns_completed must be a number");
    assert!(body["created_at"].is_string(), "created_at must be a string");
    assert!(body["started_at"].is_string(), "started_at must be present for running run");
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
    let agent_id = create_test_agent(&server).await;

    for _ in 0..4 {
        create_test_run(&server, &agent_id).await;
    }

    let resp = server.get("/api/v1alpha1/runs").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["data"].as_array().expect("data").len(), 4);
    assert_eq!(body["version"], "v1alpha1");
}

#[tokio::test]
async fn list_run_turns_for_existing_run() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;
    let run_id = create_test_run(&server, &agent_id).await;

    let resp = server
        .get(&format!("/api/v1alpha1/runs/{run_id}/turns"))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    // Stub returns empty list.
    assert!(body["data"].as_array().expect("data").is_empty());
}

#[tokio::test]
async fn list_run_turns_for_nonexistent_run_returns_404() {
    let server = test_server();
    let fake_id = polkagent_core::RunId::new().to_string();
    let resp = server
        .get(&format!("/api/v1alpha1/runs/{fake_id}/turns"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn resume_run_returns_501() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;
    let run_id = create_test_run(&server, &agent_id).await;

    let resp = server
        .post(&format!("/api/v1alpha1/runs/{run_id}/resume"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

// ===========================================================================
// Effect endpoint tests
// ===========================================================================

#[tokio::test]
async fn list_effects_for_a_run() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;
    let run_id = create_test_run(&server, &agent_id).await;

    // Our NoopEffectStore returns an empty list for get_by_run.
    let resp = server
        .get(&format!("/api/v1alpha1/runs/{run_id}/effects"))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    assert!(body["data"].as_array().expect("data").is_empty());
}

#[tokio::test]
async fn get_effect_detail_not_found() {
    let server = test_server();
    let fake_id = polkagent_core::EffectId::new().to_string();
    let resp = server
        .get(&format!("/api/v1alpha1/effects/{fake_id}"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_FOUND");
}

#[tokio::test]
async fn approve_effect_returns_501() {
    let server = test_server();
    let fake_id = polkagent_core::EffectId::new().to_string();
    let resp = server
        .post(&format!("/api/v1alpha1/effects/{fake_id}/approve"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("approve"),
        "501 message should mention 'approve'"
    );
}

#[tokio::test]
async fn deny_effect_returns_501() {
    let server = test_server();
    let fake_id = polkagent_core::EffectId::new().to_string();
    let resp = server
        .post(&format!("/api/v1alpha1/effects/{fake_id}/deny"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("deny"),
        "501 message should mention 'deny'"
    );
}

#[tokio::test]
async fn list_effects_via_runs_effects_alias() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;
    let run_id = create_test_run(&server, &agent_id).await;

    // The runs/:id/effects endpoint is an alias.
    let resp = server
        .get(&format!("/api/v1alpha1/runs/{run_id}/effects"))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    assert!(body["data"].is_array());
}

// ===========================================================================
// Health endpoint tests
// ===========================================================================

#[tokio::test]
async fn health_liveness_always_returns_200() {
    let server = test_server();
    let resp = server.get("/health/live").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn health_readiness_returns_200_when_store_healthy() {
    let server = test_server();
    let resp = server.get("/health/ready").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "ok");
    // The readiness check probes the effect store.
    assert_eq!(body["checks"]["effect_store"], "ok");
}

#[tokio::test]
async fn health_startup_returns_200() {
    let server = test_server();
    let resp = server.get("/health/startup").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["detail"], "initialisation complete");
}

#[tokio::test]
async fn health_endpoints_are_not_under_api_prefix() {
    // Health endpoints must be at /health/*, not /api/v1alpha1/health/*.
    let server = test_server();

    // Root-level health endpoints should work.
    server.get("/health/live").await.assert_status_ok();
    server.get("/health/ready").await.assert_status_ok();
    server.get("/health/startup").await.assert_status_ok();
}

// ===========================================================================
// System info tests
// ===========================================================================

#[tokio::test]
async fn system_info_response_includes_version() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/system/info").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    assert!(
        body["platform_version"].is_string(),
        "platform_version must be present"
    );
    let pv = body["platform_version"].as_str().unwrap();
    assert!(
        !pv.is_empty(),
        "platform_version must not be empty"
    );
}

#[tokio::test]
async fn system_info_uptime_secs_is_non_negative() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/system/info").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    let uptime = body["uptime_secs"].as_u64().expect("uptime_secs must be a number");
    // Uptime should be >= 0 (realistically >= 0 since we just started).
    assert!(uptime < 300, "uptime should be reasonable (< 5 min for a test)");
}

#[tokio::test]
async fn system_info_config_summary_does_not_leak_secrets() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/system/info").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    let config_summary = &body["config_summary"];

    // Should include safe fields.
    assert!(
        config_summary["bind_address"].is_string(),
        "bind_address should be present"
    );
    assert!(
        config_summary["database_backend"].is_string(),
        "database_backend should be present"
    );
    assert!(
        config_summary["max_concurrent_runs"].is_number(),
        "max_concurrent_runs should be present"
    );

    // Convert the entire response to a string and check no secret-related
    // fields leaked through.
    let body_str = serde_json::to_string(&body).expect("serialize");
    assert!(
        !body_str.contains("api_key"),
        "config summary must not contain 'api_key'"
    );
    assert!(
        !body_str.contains("ANTHROPIC_API_KEY"),
        "config summary must not contain env var names for secrets"
    );
    assert!(
        !body_str.contains("password"),
        "config summary must not contain 'password'"
    );
    assert!(
        !body_str.contains("postgres_url"),
        "config summary must not contain database URLs"
    );
}

#[tokio::test]
async fn system_info_config_summary_has_expected_values() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/system/info").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    let config_summary = &body["config_summary"];

    // Default config values.
    assert_eq!(config_summary["bind_address"], "127.0.0.1:4840");
    assert_eq!(config_summary["database_backend"], "sqlite");
    assert_eq!(config_summary["max_concurrent_runs"], 10);
}

// ===========================================================================
// Provider endpoint tests
// ===========================================================================

#[tokio::test]
async fn list_providers_with_no_providers_configured() {
    // Default Config has no providers.
    let server = test_server();
    let resp = server.get("/api/v1alpha1/providers").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    assert!(body["data"].as_array().expect("data").is_empty());
}

#[tokio::test]
async fn list_providers_returns_configured_providers() {
    let mut config = Config::default();
    config.providers = vec![
        ProviderConfig {
            id: "anthropic-default".to_owned(),
            provider_type: "anthropic".to_owned(),
            api_key_env: "ANTHROPIC_API_KEY".to_owned(),
            base_url: "https://api.anthropic.com".to_owned(),
            default_model: "claude-sonnet-4-6".to_owned(),
            timeout_secs: 120,
            max_retries: 3,
        },
        ProviderConfig {
            id: "openai-compat".to_owned(),
            provider_type: "openai_compatible".to_owned(),
            api_key_env: "OPENAI_API_KEY".to_owned(),
            base_url: "https://api.openai.com/v1".to_owned(),
            default_model: "gpt-4o".to_owned(),
            timeout_secs: 60,
            max_retries: 2,
        },
    ];

    let server = test_server_with_config(config);
    let resp = server.get("/api/v1alpha1/providers").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    let providers = body["data"].as_array().expect("data");
    assert_eq!(providers.len(), 2);
    assert_eq!(providers[0]["id"], "anthropic-default");
    assert_eq!(providers[0]["provider_type"], "anthropic");
    assert_eq!(providers[0]["base_url"], "https://api.anthropic.com");
    assert_eq!(providers[1]["id"], "openai-compat");

    // Provider response must not include the api_key_env field.
    let provider_str = serde_json::to_string(&providers[0]).expect("serialize");
    assert!(
        !provider_str.contains("api_key_env"),
        "provider response must not include api_key_env"
    );
    assert!(
        !provider_str.contains("ANTHROPIC_API_KEY"),
        "provider response must not leak API key env var name"
    );
}

#[tokio::test]
async fn get_provider_by_id() {
    let mut config = Config::default();
    config.providers = vec![ProviderConfig {
        id: "test-provider".to_owned(),
        provider_type: "anthropic".to_owned(),
        api_key_env: "TEST_KEY".to_owned(),
        base_url: "https://example.com".to_owned(),
        default_model: "test-model".to_owned(),
        timeout_secs: 30,
        max_retries: 1,
    }];

    let server = test_server_with_config(config);
    let resp = server
        .get("/api/v1alpha1/providers/test-provider")
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    assert_eq!(body["id"], "test-provider");
    assert_eq!(body["provider_type"], "anthropic");
    assert_eq!(body["base_url"], "https://example.com");
    assert_eq!(body["default_model"], "test-model");
    assert_eq!(body["timeout_secs"], 30);
    assert_eq!(body["max_retries"], 1);
}

#[tokio::test]
async fn get_provider_not_found_returns_404() {
    let server = test_server();
    let resp = server
        .get("/api/v1alpha1/providers/nonexistent")
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_FOUND");
}

// ===========================================================================
// Memory endpoint tests (stubs)
// ===========================================================================

#[tokio::test]
async fn memory_query_returns_501() {
    let server = test_server();
    let resp = server
        .post("/api/v1alpha1/memory/query")
        .json(&json!({ "query": "test search" }))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

#[tokio::test]
async fn memory_stats_returns_501() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/memory/stats").await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

// ===========================================================================
// Artifact endpoint tests (no artifact store configured)
// ===========================================================================

#[tokio::test]
async fn get_artifact_returns_501_without_store() {
    let server = test_server();
    let fake_id = polkagent_core::ArtifactId::new().to_string();
    let resp = server
        .get(&format!("/api/v1alpha1/artifacts/{fake_id}"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

#[tokio::test]
async fn get_artifact_content_returns_501_without_store() {
    let server = test_server();
    let fake_id = polkagent_core::ArtifactId::new().to_string();
    let resp = server
        .get(&format!("/api/v1alpha1/artifacts/{fake_id}/content"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn get_artifact_provenance_returns_501_without_store() {
    let server = test_server();
    let fake_id = polkagent_core::ArtifactId::new().to_string();
    let resp = server
        .get(&format!("/api/v1alpha1/artifacts/{fake_id}/provenance"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn list_run_artifacts_returns_501_without_store() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;
    let run_id = create_test_run(&server, &agent_id).await;
    let resp = server
        .get(&format!("/api/v1alpha1/runs/{run_id}/artifacts"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
}

// ===========================================================================
// Events REST endpoint tests (no event store configured)
// ===========================================================================

#[tokio::test]
async fn list_events_returns_501_without_store() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/events").await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

#[tokio::test]
async fn get_event_by_id_returns_501_without_store() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/events/some-event-id").await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn list_run_events_returns_501_without_store() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;
    let run_id = create_test_run(&server, &agent_id).await;
    let resp = server
        .get(&format!("/api/v1alpha1/runs/{run_id}/events"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
}

// ===========================================================================
// Cross-cutting: response version field
// ===========================================================================

#[tokio::test]
async fn all_list_responses_include_version() {
    let server = test_server();

    // Agents list
    let resp = server.get("/api/v1alpha1/agents").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");

    // Runs list
    let resp = server.get("/api/v1alpha1/runs").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");

    // Providers list
    let resp = server.get("/api/v1alpha1/providers").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");

    // System info
    let resp = server.get("/api/v1alpha1/system/info").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
}

// ===========================================================================
// Run detail sub-resources
// ===========================================================================

#[tokio::test]
async fn run_sub_resources_return_404_for_nonexistent_run() {
    let server = test_server();
    let fake_id = polkagent_core::RunId::new().to_string();

    // turns
    let resp = server
        .get(&format!("/api/v1alpha1/runs/{fake_id}/turns"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);

    // effects (via run effects endpoint, backed by the effect store -- may vary)
    // The runs/:id/effects handler calls effect_store.get_by_run which does not
    // check run existence first, so it may return 200 with empty data.
    // We verify at least that the endpoint is reachable.
    let resp = server
        .get(&format!("/api/v1alpha1/runs/{fake_id}/effects"))
        .await;
    // This may be 200 (empty list) since the effect store does not verify run existence.
    assert!(
        resp.status_code() == axum::http::StatusCode::OK
            || resp.status_code() == axum::http::StatusCode::NOT_FOUND
    );
}

// ===========================================================================
// Multiple agent interaction tests
// ===========================================================================

#[tokio::test]
async fn multiple_agents_independent_run_counts() {
    let server = test_server();

    let agent_a = create_test_agent(&server).await;
    let agent_b = {
        let resp = server
            .post("/api/v1alpha1/agents")
            .json(&json!({ "name": "Agent B", "model": "model-b" }))
            .await;
        resp.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    // 3 runs for A, 1 for B.
    for _ in 0..3 {
        create_test_run(&server, &agent_a).await;
    }
    create_test_run(&server, &agent_b).await;

    // Total runs should be 4.
    let resp = server.get("/api/v1alpha1/runs").await;
    let body: serde_json::Value = resp.json();
    assert_eq!(body["data"].as_array().expect("data").len(), 4);

    // Filtered by A should be 3.
    let resp_a = server
        .get(&format!("/api/v1alpha1/runs?agent_id={agent_a}"))
        .await;
    let body_a: serde_json::Value = resp_a.json();
    assert_eq!(body_a["data"].as_array().expect("data").len(), 3);

    // Filtered by B should be 1.
    let resp_b = server
        .get(&format!("/api/v1alpha1/runs?agent_id={agent_b}"))
        .await;
    let body_b: serde_json::Value = resp_b.json();
    assert_eq!(body_b["data"].as_array().expect("data").len(), 1);
}

// ===========================================================================
// Models endpoint tests
// ===========================================================================

#[tokio::test]
async fn list_all_models_empty_without_providers() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/models").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    assert!(body["data"].as_array().expect("data").is_empty());
}

#[tokio::test]
async fn list_all_models_returns_one_per_provider() {
    let mut config = Config::default();
    config.providers = vec![
        ProviderConfig {
            id: "anthropic-1".to_owned(),
            provider_type: "anthropic".to_owned(),
            api_key_env: "KEY".to_owned(),
            base_url: "https://api.anthropic.com".to_owned(),
            default_model: "claude-opus-4-6".to_owned(),
            timeout_secs: 60,
            max_retries: 2,
        },
        ProviderConfig {
            id: "openai-1".to_owned(),
            provider_type: "openai_compatible".to_owned(),
            api_key_env: "KEY2".to_owned(),
            base_url: "https://api.openai.com/v1".to_owned(),
            default_model: "gpt-4o".to_owned(),
            timeout_secs: 60,
            max_retries: 2,
        },
    ];
    let server = test_server_with_config(config);
    let resp = server.get("/api/v1alpha1/models").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    let models = body["data"].as_array().expect("data");
    assert_eq!(models.len(), 2);

    // All models should have the expected fields.
    for model in models {
        assert!(model["id"].is_string());
        assert!(model["name"].is_string());
        assert!(model["provider"].is_string());
        assert!(model["context_window"].is_number());
        assert!(model["capabilities"].is_object());
        assert_eq!(model["version"], "v1alpha1");
    }
}

#[tokio::test]
async fn get_model_by_id_returns_correct_details() {
    let mut config = Config::default();
    config.providers = vec![ProviderConfig {
        id: "anthropic-default".to_owned(),
        provider_type: "anthropic".to_owned(),
        api_key_env: "KEY".to_owned(),
        base_url: "https://api.anthropic.com".to_owned(),
        default_model: "claude-opus-4-6".to_owned(),
        timeout_secs: 60,
        max_retries: 2,
    }];
    let server = test_server_with_config(config);
    let resp = server.get("/api/v1alpha1/models/claude-opus-4-6").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["id"], "claude-opus-4-6");
    assert_eq!(body["provider"], "anthropic-default");
    assert_eq!(body["version"], "v1alpha1");
    // Claude opus should have a large context window.
    let ctx = body["context_window"].as_u64().expect("context_window");
    assert!(ctx >= 100_000, "claude should have >= 100k context window");
    // Claude supports vision and tool use.
    assert_eq!(body["capabilities"]["tool_use"], true);
    assert_eq!(body["capabilities"]["streaming"], true);
}

#[tokio::test]
async fn get_model_not_found_returns_404() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/models/nonexistent-model").await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_FOUND");
}

#[tokio::test]
async fn list_provider_models_returns_models_for_provider() {
    let mut config = Config::default();
    config.providers = vec![ProviderConfig {
        id: "test-provider".to_owned(),
        provider_type: "anthropic".to_owned(),
        api_key_env: "KEY".to_owned(),
        base_url: "https://api.anthropic.com".to_owned(),
        default_model: "claude-sonnet-4-6".to_owned(),
        timeout_secs: 60,
        max_retries: 2,
    }];
    let server = test_server_with_config(config);
    let resp = server
        .get("/api/v1alpha1/providers/test-provider/models")
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    let models = body["data"].as_array().expect("data");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["id"], "claude-sonnet-4-6");
    assert_eq!(models[0]["provider"], "test-provider");
}

#[tokio::test]
async fn list_provider_models_returns_404_for_unknown_provider() {
    let server = test_server();
    let resp = server
        .get("/api/v1alpha1/providers/nonexistent/models")
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_FOUND");
}

// ===========================================================================
// Skills endpoint tests
// ===========================================================================

#[tokio::test]
async fn list_skills_returns_501_without_registry() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/skills").await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

#[tokio::test]
async fn get_skill_returns_501_without_registry() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/skills/my-skill").await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

#[tokio::test]
async fn install_skill_returns_501() {
    let server = test_server();
    let resp = server
        .post("/api/v1alpha1/skills/install")
        .json(&json!({ "path": "/tmp/my-skill" }))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

#[tokio::test]
async fn uninstall_skill_returns_501() {
    let server = test_server();
    let resp = server
        .post("/api/v1alpha1/skills/my-skill/uninstall")
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

#[tokio::test]
async fn update_skill_config_returns_501() {
    let server = test_server();
    let resp = server
        .put("/api/v1alpha1/skills/my-skill/config")
        .json(&json!({ "config": { "chain": "polkadot" } }))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

// ===========================================================================
// Tools endpoint tests
// ===========================================================================

#[tokio::test]
async fn list_tools_returns_501_without_registry() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/tools").await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

#[tokio::test]
async fn get_tool_returns_501_without_registry() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/tools/polkagent.file.read").await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

#[tokio::test]
async fn get_tool_grants_returns_501_without_registry() {
    let server = test_server();
    let resp = server
        .get("/api/v1alpha1/tools/polkagent.file.read/grants")
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

// ===========================================================================
// Payments endpoint tests
// ===========================================================================

#[tokio::test]
async fn payment_balance_returns_501_without_store() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/payments/balance").await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

#[tokio::test]
async fn payment_usage_returns_501_without_store() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/payments/usage").await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

#[tokio::test]
async fn payment_receipts_returns_501_without_store() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/payments/receipts").await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

#[tokio::test]
async fn payment_receipt_by_id_returns_501_without_store() {
    let server = test_server();
    let resp = server
        .get("/api/v1alpha1/payments/receipts/some-receipt-id")
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

// ===========================================================================
// Memory new endpoint tests
// ===========================================================================

#[tokio::test]
async fn memory_forget_returns_501_without_store() {
    let server = test_server();
    let resp = server
        .post("/api/v1alpha1/memory/forget")
        .json(&json!({ "entry_ids": ["id-1", "id-2"] }))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

#[tokio::test]
async fn memory_get_entry_returns_501_without_store() {
    let server = test_server();
    let resp = server
        .get("/api/v1alpha1/memory/entries/some-entry-id")
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

// ===========================================================================
// Agent lifecycle endpoint tests
// ===========================================================================

#[tokio::test]
async fn agent_start_returns_200_with_lifecycle_response() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;
    let resp = server
        .post(&format!("/api/v1alpha1/agents/{agent_id}/start"))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    assert_eq!(body["agent_id"], agent_id.as_str());
    assert_eq!(body["action"], "start");
    assert!(body["status"].is_string());
}

#[tokio::test]
async fn agent_stop_returns_200_for_existing_agent() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;
    let resp = server
        .post(&format!("/api/v1alpha1/agents/{agent_id}/stop"))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    assert_eq!(body["action"], "stop");
}

#[tokio::test]
async fn agent_stop_returns_404_for_nonexistent_agent() {
    let server = test_server();
    let fake_id = polkagent_core::AgentId::new().to_string();
    let resp = server
        .post(&format!("/api/v1alpha1/agents/{fake_id}/stop"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "AGENT_NOT_FOUND");
}

#[tokio::test]
async fn agent_pause_returns_200_for_existing_agent() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;
    let resp = server
        .post(&format!("/api/v1alpha1/agents/{agent_id}/pause"))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    assert_eq!(body["action"], "pause");
}

#[tokio::test]
async fn agent_resume_returns_200_for_existing_agent() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;
    let resp = server
        .post(&format!("/api/v1alpha1/agents/{agent_id}/resume"))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    assert_eq!(body["action"], "resume");
}

#[tokio::test]
async fn resume_run_returns_501_with_run_state_in_message() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;
    let run_id = create_test_run(&server, &agent_id).await;

    let resp = server
        .post(&format!("/api/v1alpha1/runs/{run_id}/resume"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
    // Message should reference the run ID.
    let message = body["error"]["message"].as_str().expect("message");
    assert!(
        message.contains(&run_id),
        "501 message should reference the run ID"
    );
}

#[tokio::test]
async fn resume_run_returns_404_for_nonexistent_run() {
    let server = test_server();
    let fake_id = polkagent_core::RunId::new().to_string();
    let resp = server
        .post(&format!("/api/v1alpha1/runs/{fake_id}/resume"))
        .await;
    // Should be 404 since the run doesn't exist.
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "RUN_NOT_FOUND");
}
