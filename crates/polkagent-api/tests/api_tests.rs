//! Integration tests for the `polkagent-api` REST API.
//!
//! Each test builds an in-memory `AppState`, wraps it in the Axum router via
//! `ApiServer::into_router`, and drives it with `axum_test::TestServer`.
//! No real TCP sockets or database files are opened.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "this integration-test target intentionally fails fast on malformed fixture responses"
)]

use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use axum_test::TestServer;
use polkagent_core::{
    ArtifactId, EffectAttemptId, EffectId, EffectOutcomeId, RunId, StepId, Timestamp, WorkerId,
};
use polkagent_store_trait::event::{EventFilter, EventStore, EventStoreError, StoredEvent};
use polkagent_store_trait::{
    ArtifactStore, ArtifactSummary, EffectStore, StoreError, StoreRetryClass, StoredIntent,
    StoredOutcome,
};
use serde_json::json;
use tokio::sync::RwLock;

use polkagent_api::{server::ApiServer, InMemoryAgentStore, InMemoryRunManager};
use polkagent_config::{Config, ProviderConfig};
use polkagent_event::EventBus;
use sha2::{Digest, Sha256};

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

    async fn expired_leases(&self, _cutoff: Timestamp) -> Result<Vec<StoredIntent>, StoreError> {
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

    async fn unconsumed_outcomes(&self, _run_id: RunId) -> Result<Vec<StoredOutcome>, StoreError> {
        Ok(vec![])
    }

    async fn mark_outcomes_consumed(
        &self,
        _outcome_ids: &[EffectOutcomeId],
    ) -> Result<(), StoreError> {
        Ok(())
    }

    async fn update_intent_state(
        &self,
        intent_id: EffectId,
        _new_state: &str,
    ) -> Result<StoredIntent, StoreError> {
        // NoopEffectStore always returns NotFound — tests requiring real state
        // transitions must use `InMemoryEffectStore` (see below).
        Err(StoreError::NotFound {
            resource_type: "EffectIntent",
            id: intent_id.to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// InMemoryEffectStore — for approve/deny/state-transition tests
// ---------------------------------------------------------------------------

use tokio::sync::RwLock as TokioRwLock;

/// A fully-functional in-memory `EffectStore` for approve/deny tests.
struct InMemoryEffectStore {
    intents: TokioRwLock<HashMap<EffectId, StoredIntent>>,
}

impl InMemoryEffectStore {
    fn new() -> Self {
        Self {
            intents: TokioRwLock::new(HashMap::new()),
        }
    }

    async fn seed_intent(&self, intent: StoredIntent) {
        self.intents.write().await.insert(intent.id, intent);
    }
}

#[async_trait::async_trait]
impl EffectStore for InMemoryEffectStore {
    async fn propose_intent(&self, intent: StoredIntent) -> Result<(), StoreError> {
        self.intents.write().await.insert(intent.id, intent);
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
        self.intents
            .read()
            .await
            .get(&intent_id)
            .cloned()
            .ok_or(StoreError::NotFound {
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
        self.intents
            .read()
            .await
            .get(&intent_id)
            .cloned()
            .ok_or(StoreError::NotFound {
                resource_type: "EffectIntent",
                id: intent_id.to_string(),
            })
    }

    async fn get_by_run(&self, run_id: RunId) -> Result<Vec<StoredIntent>, StoreError> {
        let guard = self.intents.read().await;
        Ok(guard
            .values()
            .filter(|i| i.run_id == run_id)
            .cloned()
            .collect())
    }

    async fn expired_leases(&self, _cutoff: Timestamp) -> Result<Vec<StoredIntent>, StoreError> {
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

    async fn unconsumed_outcomes(&self, _run_id: RunId) -> Result<Vec<StoredOutcome>, StoreError> {
        Ok(vec![])
    }

    async fn mark_outcomes_consumed(
        &self,
        _outcome_ids: &[EffectOutcomeId],
    ) -> Result<(), StoreError> {
        Ok(())
    }

    async fn update_intent_state(
        &self,
        intent_id: EffectId,
        new_state: &str,
    ) -> Result<StoredIntent, StoreError> {
        let mut guard = self.intents.write().await;
        let intent = guard.get_mut(&intent_id).ok_or(StoreError::NotFound {
            resource_type: "EffectIntent",
            id: intent_id.to_string(),
        })?;

        // Only allow transitions from "awaiting_approval" (PRD-14 §4).
        let valid_source = ["awaiting_approval"];
        if !valid_source.contains(&intent.state.as_str()) {
            return Err(StoreError::InvalidTransition {
                message: format!("cannot transition from '{}' to '{new_state}'", intent.state),
            });
        }

        new_state.clone_into(&mut intent.state);
        Ok(intent.clone())
    }
}

/// Build a `StoredIntent` with the given state for seeding in tests.
fn make_stored_intent(run_id: RunId, state: &str) -> StoredIntent {
    StoredIntent {
        id: EffectId::new(),
        run_id,
        step_id: StepId::new(),
        state: state.to_owned(),
        lease_owner: None,
        lease_expires: None,
        retry_class: StoreRetryClass::Idempotent,
        payload: serde_json::Value::Null,
        idempotency_key: "test-key".to_owned(),
        created_at: chrono::Utc::now(),
    }
}

/// Build a `TestServer` backed by `InMemoryEffectStore` and return the store
/// for seeding intents before requests.
fn test_server_with_effect_store() -> (TestServer, Arc<InMemoryEffectStore>) {
    let agents = Arc::new(InMemoryAgentStore::new());
    let run_manager = Arc::new(InMemoryRunManager::new());
    let store = Arc::new(InMemoryEffectStore::new());
    let event_bus = EventBus::with_default_capacity();
    let state = polkagent_api::AppState::new(
        Config::default(),
        agents,
        run_manager,
        store.clone() as Arc<dyn EffectStore>,
        event_bus,
    );
    let server = ApiServer::from_state(state);
    (TestServer::new(server.into_router()), store)
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
    let state =
        polkagent_api::AppState::new(Config::default(), agents, run_manager, store, event_bus);
    let server = ApiServer::from_state(state);
    TestServer::new(server.into_router())
}

/// Build a `TestServer` with a custom `Config` (e.g. to inject providers).
fn test_server_with_config(config: Config) -> TestServer {
    let agents = Arc::new(InMemoryAgentStore::new());
    let run_manager = Arc::new(InMemoryRunManager::new());
    let store: Arc<dyn EffectStore> = Arc::new(NoopEffectStore);
    let event_bus = EventBus::with_default_capacity();
    let state = polkagent_api::AppState::new(config, agents, run_manager, store, event_bus);
    let server = ApiServer::from_state(state);
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
    let resp = server.get(&format!("/api/v1alpha1/agents/{fake_id}")).await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    let err = &body["error"];
    assert!(err["code"].is_string(), "error.code must be a string");
    assert!(err["message"].is_string(), "error.message must be a string");
    assert!(
        err["request_id"].is_string(),
        "error.request_id must be a string"
    );
    assert!(
        err["timestamp"].is_string(),
        "error.timestamp must be a string"
    );
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
    let resp = server.get(&format!("/api/v1alpha1/agents/{fake_id}")).await;
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
    let resp = server.get(&format!("/api/v1alpha1/runs/{fake_id}")).await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    let request_id = body["error"]["request_id"]
        .as_str()
        .expect("request_id must be a string");
    assert!(
        uuid::Uuid::parse_str(request_id).is_ok(),
        "request_id '{request_id}' should be a valid UUID"
    );
}

#[tokio::test]
async fn error_501_includes_all_required_fields() {
    // Skills endpoint returns 501 when no skill_registry is configured.
    let server = test_server();
    let resp = server.get("/api/v1alpha1/skills").await;
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
        .get(&format!("/api/v1alpha1/runs?limit=2&after={next_cursor2}"))
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
    assert!(body["spec"]["tools"]
        .as_array()
        .expect("tools")
        .contains(&json!("shell_exec")));
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
    let resp = server.get(&format!("/api/v1alpha1/agents/{fake_id}")).await;
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

    let resp = server.get("/api/v1alpha1/runs?state=running").await;
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
    assert!(
        cancelled_run.is_some(),
        "cancelled run should appear in unfiltered list"
    );
    assert_eq!(cancelled_run.unwrap()["status"]["state"], "cancelled");
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

    let get_resp = server.get(&format!("/api/v1alpha1/runs/{run_id}")).await;
    get_resp.assert_status_ok();
    let body: serde_json::Value = get_resp.json();

    // Required fields must all be present.
    assert_eq!(body["version"], "v1alpha1");
    assert_eq!(body["id"], run_id.as_str());
    assert_eq!(body["agent_id"], agent_id.as_str());
    assert!(body["status"].is_object(), "status must be an object");
    assert!(body["input"].is_object(), "input must be present");
    assert!(
        body["turns_completed"].is_number(),
        "turns_completed must be a number"
    );
    assert!(
        body["created_at"].is_string(),
        "created_at must be a string"
    );
    assert!(
        body["started_at"].is_string(),
        "started_at must be present for running run"
    );
}

#[tokio::test]
async fn get_run_not_found_returns_404() {
    let server = test_server();
    let fake_id = polkagent_core::RunId::new().to_string();
    let resp = server.get(&format!("/api/v1alpha1/runs/{fake_id}")).await;
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
async fn resume_run_existing_not_in_awaiting_returns_409() {
    // Previously this was 501; now the endpoint is implemented and returns
    // 409 Conflict when the run is not in AwaitingApproval state.
    let server = test_server();
    let agent_id = create_test_agent(&server).await;
    let run_id = create_test_run(&server, &agent_id).await;

    let resp = server
        .post(&format!("/api/v1alpha1/runs/{run_id}/resume"))
        .await;
    resp.assert_status(axum::http::StatusCode::CONFLICT);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "INVALID_STATE");
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
async fn approve_effect_nonexistent_returns_404_with_not_found_code() {
    // Previously this was 501 (stub); now it returns 404 for unknown effects.
    let server = test_server();
    let fake_id = polkagent_core::EffectId::new().to_string();
    let resp = server
        .post(&format!("/api/v1alpha1/effects/{fake_id}/approve"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_FOUND");
    let message = body["error"]["message"].as_str().unwrap_or("");
    assert!(
        message.contains(&fake_id),
        "404 message should reference the effect ID"
    );
}

#[tokio::test]
async fn deny_effect_nonexistent_returns_404_with_not_found_code() {
    // Previously this was 501 (stub); now it returns 404 for unknown effects.
    let server = test_server();
    let fake_id = polkagent_core::EffectId::new().to_string();
    let resp = server
        .post(&format!("/api/v1alpha1/effects/{fake_id}/deny"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_FOUND");
    let message = body["error"]["message"].as_str().unwrap_or("");
    assert!(
        message.contains(&fake_id),
        "404 message should reference the effect ID"
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
    assert_eq!(body["ready"], true);
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
    assert!(!pv.is_empty(), "platform_version must not be empty");
}

#[tokio::test]
async fn system_info_uptime_secs_is_non_negative() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/system/info").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    let uptime = body["uptime_secs"]
        .as_u64()
        .expect("uptime_secs must be a number");
    // Uptime should be >= 0 (realistically >= 0 since we just started).
    assert!(
        uptime < 300,
        "uptime should be reasonable (< 5 min for a test)"
    );
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
    let config = Config {
        providers: vec![
            ProviderConfig {
                id: "anthropic-default".to_owned(),
                provider_type: "anthropic".to_owned(),
                api_key_env: "ANTHROPIC_API_KEY".to_owned(),
                base_url: "https://api.anthropic.com".to_owned(),
                default_model: "claude-sonnet-4-6".to_owned(),
                timeout_secs: 120,
                max_retries: 3,
                ..Default::default()
            },
            ProviderConfig {
                id: "openai-compat".to_owned(),
                provider_type: "openai_compatible".to_owned(),
                api_key_env: "OPENAI_API_KEY".to_owned(),
                base_url: "https://api.openai.com/v1".to_owned(),
                default_model: "gpt-4o".to_owned(),
                timeout_secs: 60,
                max_retries: 2,
                ..Default::default()
            },
        ],
        ..Default::default()
    };

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
    let config = Config {
        providers: vec![ProviderConfig {
            id: "test-provider".to_owned(),
            provider_type: "anthropic".to_owned(),
            api_key_env: "TEST_KEY".to_owned(),
            base_url: "https://example.com".to_owned(),
            default_model: "test-model".to_owned(),
            timeout_secs: 30,
            max_retries: 1,
            ..Default::default()
        }],
        ..Default::default()
    };

    let server = test_server_with_config(config);
    let resp = server.get("/api/v1alpha1/providers/test-provider").await;
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
    let resp = server.get("/api/v1alpha1/providers/nonexistent").await;
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
    let config = Config {
        providers: vec![
            ProviderConfig {
                id: "anthropic-1".to_owned(),
                provider_type: "anthropic".to_owned(),
                api_key_env: "KEY".to_owned(),
                base_url: "https://api.anthropic.com".to_owned(),
                default_model: "claude-opus-4-6".to_owned(),
                timeout_secs: 60,
                max_retries: 2,
                ..Default::default()
            },
            ProviderConfig {
                id: "openai-1".to_owned(),
                provider_type: "openai_compatible".to_owned(),
                api_key_env: "KEY2".to_owned(),
                base_url: "https://api.openai.com/v1".to_owned(),
                default_model: "gpt-4o".to_owned(),
                timeout_secs: 60,
                max_retries: 2,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
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
    let config = Config {
        providers: vec![ProviderConfig {
            id: "anthropic-default".to_owned(),
            provider_type: "anthropic".to_owned(),
            api_key_env: "KEY".to_owned(),
            base_url: "https://api.anthropic.com".to_owned(),
            default_model: "claude-opus-4-6".to_owned(),
            timeout_secs: 60,
            max_retries: 2,
            ..Default::default()
        }],
        ..Default::default()
    };
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
    let config = Config {
        providers: vec![ProviderConfig {
            id: "test-provider".to_owned(),
            provider_type: "anthropic".to_owned(),
            api_key_env: "KEY".to_owned(),
            base_url: "https://api.anthropic.com".to_owned(),
            default_model: "claude-sonnet-4-6".to_owned(),
            timeout_secs: 60,
            max_retries: 2,
            ..Default::default()
        }],
        ..Default::default()
    };
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
    let resp = server.post("/api/v1alpha1/skills/my-skill/uninstall").await;
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
        .json(&json!({
            "entry_ids": [polkagent_memory::MemoryId::new().to_string()]
        }))
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
async fn resume_run_returns_409_when_not_in_awaiting_approval() {
    // A freshly-created run is in Running state, not AwaitingApproval —
    // so resume should return 409 Conflict.
    let server = test_server();
    let agent_id = create_test_agent(&server).await;
    let run_id = create_test_run(&server, &agent_id).await;

    let resp = server
        .post(&format!("/api/v1alpha1/runs/{run_id}/resume"))
        .await;
    resp.assert_status(axum::http::StatusCode::CONFLICT);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "INVALID_STATE");
    let message = body["error"]["message"].as_str().expect("message");
    assert!(
        !message.is_empty(),
        "409 message should explain the invalid transition"
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

// ===========================================================================
// In-memory EventStore for tests
// ===========================================================================

/// Minimal in-memory `EventStore` for testing the events REST endpoints.
struct InMemoryEventStore {
    events: RwLock<Vec<StoredEvent>>,
    next_global_seq: RwLock<u64>,
    cursor_reads: RwLock<Vec<(u64, usize)>>,
    fail_cursor_reads: AtomicBool,
}

impl InMemoryEventStore {
    fn new() -> Self {
        Self {
            events: RwLock::new(Vec::new()),
            next_global_seq: RwLock::new(1),
            cursor_reads: RwLock::new(Vec::new()),
            fail_cursor_reads: AtomicBool::new(false),
        }
    }

    /// Helper: store an event directly (bypasses monotonicity checks for tests).
    async fn insert(&self, mut event: StoredEvent) {
        let mut seq_guard = self.next_global_seq.write().await;
        event.global_sequence = *seq_guard;
        *seq_guard += 1;
        drop(seq_guard);
        self.events.write().await.push(event);
    }

    async fn insert_batch(&self, mut events: Vec<StoredEvent>) {
        let mut next_global_sequence = self.next_global_seq.write().await;
        for event in &mut events {
            event.global_sequence = *next_global_sequence;
            *next_global_sequence += 1;
        }
        self.events.write().await.extend(events);
    }

    async fn cursor_reads(&self) -> Vec<(u64, usize)> {
        self.cursor_reads.read().await.clone()
    }
}

#[async_trait::async_trait]
impl EventStore for InMemoryEventStore {
    async fn append_durable(&self, event: StoredEvent) -> Result<StoredEvent, EventStoreError> {
        let mut event = event;
        let mut seq_guard = self.next_global_seq.write().await;
        event.global_sequence = *seq_guard;
        *seq_guard += 1;
        drop(seq_guard);
        self.events.write().await.push(event.clone());
        Ok(event)
    }

    async fn append_diagnostic(
        &self,
        _event: StoredEvent,
        _expires_at: String,
    ) -> Result<(), EventStoreError> {
        Ok(())
    }

    async fn read_from_cursor(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        self.cursor_reads.write().await.push((cursor, limit));
        if self.fail_cursor_reads.load(Ordering::SeqCst) {
            return Err(EventStoreError::Backend(Box::new(std::io::Error::other(
                "PRIVATE_EVENT_STORE_BACKEND_DETAIL",
            ))));
        }
        let guard = self.events.read().await;
        let results: Vec<StoredEvent> = guard
            .iter()
            .filter(|e| e.global_sequence > cursor)
            .take(limit)
            .cloned()
            .collect();
        Ok(results)
    }

    async fn read_run_events(&self, run_id: RunId) -> Result<Vec<StoredEvent>, EventStoreError> {
        let guard = self.events.read().await;
        let run_id_str = run_id.to_string();
        Ok(guard
            .iter()
            .filter(|e| e.run_id == run_id_str)
            .cloned()
            .collect())
    }

    async fn query(&self, filter: EventFilter) -> Result<Vec<StoredEvent>, EventStoreError> {
        let guard = self.events.read().await;
        let mut results: Vec<StoredEvent> = guard
            .iter()
            .filter(|e| {
                if let Some(ref rid) = filter.run_id {
                    if e.run_id != rid.to_string() {
                        return false;
                    }
                }
                if !filter.event_types.is_empty() && !filter.event_types.contains(&e.event_type) {
                    return false;
                }
                if let Some(since) = filter.since_global_sequence {
                    if e.global_sequence < since {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();
        results.sort_by_key(|e| e.global_sequence);
        if let Some(limit) = filter.limit {
            results.truncate(limit);
        }
        Ok(results)
    }

    async fn max_sequence(&self, run_id: RunId) -> Result<u64, EventStoreError> {
        let guard = self.events.read().await;
        let run_id_str = run_id.to_string();
        let max = guard
            .iter()
            .filter(|e| e.run_id == run_id_str)
            .map(|e| e.sequence)
            .max()
            .unwrap_or(0);
        Ok(max)
    }

    async fn has_terminal_event(&self, _run_id: RunId) -> Result<bool, EventStoreError> {
        Ok(false)
    }
}

// ===========================================================================
// In-memory ArtifactStore for tests
// ===========================================================================

struct InMemoryArtifactStore {
    artifacts: RwLock<HashMap<ArtifactId, (ArtifactSummary, Vec<u8>)>>,
    /// `child_id` -> set of `parent_ids` (for lineage tracking)
    lineage: RwLock<HashMap<ArtifactId, HashSet<ArtifactId>>>,
}

impl InMemoryArtifactStore {
    fn new() -> Self {
        Self {
            artifacts: RwLock::new(HashMap::new()),
            lineage: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl ArtifactStore for InMemoryArtifactStore {
    async fn store(
        &self,
        artifact_id: ArtifactId,
        run_id: Option<RunId>,
        kind: &str,
        algorithm: &str,
        digest_hex: &str,
        classification: &str,
        body: &[u8],
    ) -> Result<(), StoreError> {
        let summary = ArtifactSummary {
            id: artifact_id,
            kind: kind.to_owned(),
            algorithm: algorithm.to_owned(),
            digest_hex: digest_hex.to_owned(),
            classification: classification.to_owned(),
            run_id,
            created_at: chrono::Utc::now(),
        };
        self.artifacts
            .write()
            .await
            .insert(artifact_id, (summary, body.to_vec()));
        Ok(())
    }

    async fn get(&self, id: ArtifactId) -> Result<ArtifactSummary, StoreError> {
        self.artifacts
            .read()
            .await
            .get(&id)
            .map(|(s, _)| s.clone())
            .ok_or_else(|| StoreError::NotFound {
                resource_type: "Artifact",
                id: id.to_string(),
            })
    }

    async fn get_body(&self, id: ArtifactId) -> Result<Vec<u8>, StoreError> {
        self.artifacts
            .read()
            .await
            .get(&id)
            .map(|(_, b)| b.clone())
            .ok_or_else(|| StoreError::NotFound {
                resource_type: "Artifact",
                id: id.to_string(),
            })
    }

    async fn verify(&self, id: ArtifactId) -> Result<bool, StoreError> {
        Ok(self.artifacts.read().await.contains_key(&id))
    }

    async fn list_for_run(&self, run_id: RunId) -> Result<Vec<ArtifactSummary>, StoreError> {
        let guard = self.artifacts.read().await;
        let results = guard
            .values()
            .filter(|(s, _)| s.run_id.as_ref() == Some(&run_id))
            .map(|(s, _)| s.clone())
            .collect();
        Ok(results)
    }

    async fn add_lineage(
        &self,
        child_id: ArtifactId,
        parent_id: ArtifactId,
    ) -> Result<(), StoreError> {
        self.lineage
            .write()
            .await
            .entry(child_id)
            .or_default()
            .insert(parent_id);
        Ok(())
    }

    async fn get_lineage(&self, id: ArtifactId) -> Result<Vec<ArtifactId>, StoreError> {
        let guard = self.lineage.read().await;
        let mut ancestors = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        visited.insert(id);
        queue.push_back(id);

        while let Some(current) = queue.pop_front() {
            if let Some(parents) = guard.get(&current) {
                for &parent in parents {
                    if visited.insert(parent) {
                        ancestors.push(parent);
                        queue.push_back(parent);
                    }
                }
            }
        }

        Ok(ancestors)
    }
}

// ===========================================================================
// Test helpers with optional stores
// ===========================================================================

/// Build a `TestServer` with an in-memory `EventStore` attached.
fn test_server_with_event_store(store: Arc<InMemoryEventStore>) -> TestServer {
    let agents = Arc::new(InMemoryAgentStore::new());
    let run_manager = Arc::new(InMemoryRunManager::new());
    let effect_store: Arc<dyn EffectStore> = Arc::new(NoopEffectStore);
    let event_bus = EventBus::with_default_capacity();
    let state = polkagent_api::AppState::new(
        Config::default(),
        agents,
        run_manager,
        effect_store,
        event_bus,
    );
    let state = state.with_event_store(store);
    let server = ApiServer::from_state(state);
    TestServer::new(server.into_router())
}

/// Build a `TestServer` with an in-memory `ArtifactStore` attached.
fn test_server_with_artifact_store(store: Arc<InMemoryArtifactStore>) -> TestServer {
    let agents = Arc::new(InMemoryAgentStore::new());
    let run_manager = Arc::new(InMemoryRunManager::new());
    let effect_store: Arc<dyn EffectStore> = Arc::new(NoopEffectStore);
    let event_bus = EventBus::with_default_capacity();
    let state = polkagent_api::AppState::new(
        Config::default(),
        agents,
        run_manager,
        effect_store,
        event_bus,
    );
    let state = state.with_artifact_store(store);
    let server = ApiServer::from_state(state);
    TestServer::new(server.into_router())
}

/// Make a minimal `StoredEvent` for testing.
fn make_stored_event(id: &str, run_id: RunId, event_type: &str, global_seq: u64) -> StoredEvent {
    StoredEvent {
        id: id.to_owned(),
        event_type: event_type.to_owned(),
        sequence: global_seq,
        global_sequence: global_seq,
        run_id: run_id.to_string(),
        turn_id: None,
        step_id: None,
        effect_intent_id: None,
        effect_attempt_id: None,
        conversation_id: None,
        correlation_id: run_id.to_string(),
        causation_id: None,
        scope_id: "test".to_owned(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        durability: "durable".to_owned(),
        payload: serde_json::Value::Null,
        trace_id: None,
        span_id: None,
        schema_version: 1,
    }
}

// ===========================================================================
// Events REST endpoint tests — with event store configured
// ===========================================================================

#[tokio::test]
async fn list_events_with_store_returns_events() {
    let store = Arc::new(InMemoryEventStore::new());
    let run_id = RunId::new();

    // Insert two events directly into the store.
    store
        .insert(make_stored_event("evt-1", run_id, "run_created", 1))
        .await;
    store
        .insert(make_stored_event("evt-2", run_id, "run_started", 2))
        .await;

    let server = test_server_with_event_store(store);
    let resp = server.get("/api/v1alpha1/events").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    let data = body["data"].as_array().expect("data array");
    assert_eq!(data.len(), 2);
    assert!(body["cursor"].is_object());
    assert_eq!(body["meta"]["page_size"], 2);
}

#[tokio::test]
async fn list_events_without_store_returns_501() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/events").await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

#[tokio::test]
async fn list_events_filters_by_run_id() {
    let store = Arc::new(InMemoryEventStore::new());
    let run_a = RunId::new();
    let run_b = RunId::new();

    store
        .insert(make_stored_event("evt-a1", run_a, "run_created", 1))
        .await;
    store
        .insert(make_stored_event("evt-a2", run_a, "run_started", 2))
        .await;
    store
        .insert(make_stored_event("evt-b1", run_b, "run_created", 3))
        .await;

    let server = test_server_with_event_store(store);
    let resp = server
        .get(&format!("/api/v1alpha1/events?run_id={run_a}"))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    let data = body["data"].as_array().expect("data array");
    // Only run_a events.
    assert_eq!(data.len(), 2);
    for evt in data {
        assert_eq!(evt["run_id"], run_a.to_string().as_str());
    }
}

#[tokio::test]
async fn list_events_filters_by_since() {
    let store = Arc::new(InMemoryEventStore::new());
    let run_id = RunId::new();

    store
        .insert(make_stored_event("evt-1", run_id, "run_created", 1))
        .await;
    store
        .insert(make_stored_event("evt-2", run_id, "run_started", 2))
        .await;
    store
        .insert(make_stored_event("evt-3", run_id, "run_completed", 3))
        .await;

    let server = test_server_with_event_store(store);
    // Request events with global_sequence >= 2.
    let resp = server.get("/api/v1alpha1/events?since=2").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    let data = body["data"].as_array().expect("data array");
    // Should return evt-2 and evt-3 (global_sequence 2 and 3).
    assert_eq!(data.len(), 2);
}

#[tokio::test]
async fn get_event_by_id_returns_single_event() {
    let store = Arc::new(InMemoryEventStore::new());
    let run_id = RunId::new();
    store
        .insert(make_stored_event(
            "target-event-id",
            run_id,
            "run_created",
            1,
        ))
        .await;

    let server = test_server_with_event_store(store);
    let resp = server.get("/api/v1alpha1/events/target-event-id").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    assert!(body["data"].is_object());
    assert_eq!(body["data"]["id"], "target-event-id");
}

#[tokio::test]
async fn get_event_by_id_finds_target_after_more_than_ten_thousand_events() {
    let store = Arc::new(InMemoryEventStore::new());
    let run_id = RunId::new();
    let mut events = Vec::with_capacity(10_002);
    for sequence in 1..=10_001_u64 {
        events.push(make_stored_event(
            &format!("earlier-{sequence}"),
            run_id,
            "turn_started",
            sequence,
        ));
    }
    events.push(make_stored_event(
        "target-after-ten-thousand",
        run_id,
        "run_completed",
        10_002,
    ));
    store.insert_batch(events).await;

    let server = test_server_with_event_store(store.clone());
    let response = server
        .get("/api/v1alpha1/events/target-after-ten-thousand")
        .await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["data"]["id"], "target-after-ten-thousand");
    let reads = store.cursor_reads().await;
    assert_eq!(reads.len(), 11);
    assert!(reads.iter().all(|(_, limit)| *limit == 1_000));
    assert_eq!(reads.last(), Some(&(10_000, 1_000)));
}

#[tokio::test]
async fn get_event_by_id_not_found_returns_404() {
    let store = Arc::new(InMemoryEventStore::new());
    let server = test_server_with_event_store(store);
    let resp = server.get("/api/v1alpha1/events/nonexistent-id").await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_FOUND");
}

#[tokio::test]
async fn get_event_by_id_does_not_expose_backend_details() {
    let store = Arc::new(InMemoryEventStore::new());
    store.fail_cursor_reads.store(true, Ordering::SeqCst);
    let server = test_server_with_event_store(store);
    let response = server.get("/api/v1alpha1/events/some-id").await;
    response.assert_status(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    let body: serde_json::Value = response.json();
    assert_eq!(body["error"]["code"], "INTERNAL_ERROR");
    let message = body["error"]["message"].as_str().expect("error message");
    assert!(!message.contains("PRIVATE_EVENT_STORE_BACKEND_DETAIL"));
    assert_eq!(message, "internal error: event lookup failed");
}

#[tokio::test]
async fn get_event_by_id_returns_501_error_code_without_store() {
    let server = test_server();
    let resp = server.get("/api/v1alpha1/events/some-id").await;
    resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
}

#[tokio::test]
async fn list_events_respects_limit() {
    let store = Arc::new(InMemoryEventStore::new());
    let run_id = RunId::new();

    for i in 1..=5u64 {
        store
            .insert(make_stored_event(
                &format!("evt-{i}"),
                run_id,
                "run_created",
                i,
            ))
            .await;
    }

    let server = test_server_with_event_store(store);
    let resp = server.get("/api/v1alpha1/events?limit=2").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    let data = body["data"].as_array().expect("data array");
    assert_eq!(data.len(), 2);
    assert!(body["cursor"]["has_more"].as_bool().unwrap_or(false));
}

// ===========================================================================
// Artifact endpoint tests — with artifact store configured
// ===========================================================================

#[tokio::test]
async fn get_artifact_content_returns_bytes_with_correct_content_type() {
    let store = Arc::new(InMemoryArtifactStore::new());
    let artifact_id = ArtifactId::new();
    let body_bytes = b"hello artifact content";

    store
        .store(
            artifact_id,
            None,
            "file",
            "blake3",
            "deadbeef",
            "public",
            body_bytes,
        )
        .await
        .expect("store artifact");

    let server = test_server_with_artifact_store(store);
    let resp = server
        .get(&format!("/api/v1alpha1/artifacts/{artifact_id}/content"))
        .await;
    resp.assert_status_ok();

    // The Content-Type header should be application/octet-stream.
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("application/octet-stream"),
        "expected application/octet-stream, got: {content_type}"
    );

    let bytes = resp.as_bytes();
    assert_eq!(bytes.as_ref(), body_bytes.as_ref());
}

#[tokio::test]
async fn get_artifact_content_not_found_returns_404() {
    let store = Arc::new(InMemoryArtifactStore::new());
    let server = test_server_with_artifact_store(store);
    let fake_id = ArtifactId::new();
    let resp = server
        .get(&format!("/api/v1alpha1/artifacts/{fake_id}/content"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_artifact_provenance_returns_chain() {
    let store = Arc::new(InMemoryArtifactStore::new());
    let artifact_id = ArtifactId::new();

    store
        .store(
            artifact_id,
            None,
            "code",
            "blake3",
            "cafebabe",
            "private",
            b"source code here",
        )
        .await
        .expect("store artifact");

    let server = test_server_with_artifact_store(store);
    let resp = server
        .get(&format!("/api/v1alpha1/artifacts/{artifact_id}/provenance"))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    let chain = body["chain"].as_array().expect("chain array");
    assert!(!chain.is_empty(), "provenance chain should not be empty");
    assert_eq!(chain[0]["id"], artifact_id.to_string().as_str());
    assert_eq!(chain[0]["kind"], "code");
}

#[tokio::test]
async fn get_artifact_provenance_not_found_returns_404() {
    let store = Arc::new(InMemoryArtifactStore::new());
    let server = test_server_with_artifact_store(store);
    let fake_id = ArtifactId::new();
    let resp = server
        .get(&format!("/api/v1alpha1/artifacts/{fake_id}/provenance"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
}

// ===========================================================================
// Event bus subscription tests
// ===========================================================================

#[tokio::test]
async fn event_bus_publishes_to_subscribers() {
    use polkagent_core::event::{EventCorrelation, EventKind, RunEvent};
    use polkagent_core::ids::EventId;

    let bus = EventBus::with_default_capacity();
    let mut receiver = bus.subscribe();

    let run_id = RunId::new();
    let event = RunEvent::new_durable(
        EventId::new(),
        run_id,
        1,
        EventKind::RunCreated,
        EventCorrelation {
            run_id,
            ..Default::default()
        },
    );
    let event_id = event.id;

    bus.publish(event);

    let delivered_event = receiver.recv().await.expect("should receive event");
    assert_eq!(delivered_event.id, event_id);
    assert_eq!(delivered_event.run_id, run_id);
}

#[tokio::test]
async fn event_bus_multiple_subscribers_each_receive_event() {
    use polkagent_core::event::{EventCorrelation, EventKind, RunEvent};
    use polkagent_core::ids::EventId;

    let bus = EventBus::with_default_capacity();
    let mut rx1 = bus.subscribe();
    let mut rx2 = bus.subscribe();

    let run_id = RunId::new();
    let event = RunEvent::new_durable(
        EventId::new(),
        run_id,
        1,
        EventKind::RunStarted,
        EventCorrelation {
            run_id,
            ..Default::default()
        },
    );
    let event_id = event.id;

    bus.publish(event);

    let r1 = rx1.recv().await.expect("rx1 should receive");
    let r2 = rx2.recv().await.expect("rx2 should receive");
    assert_eq!(r1.id, event_id);
    assert_eq!(r2.id, event_id);
}

#[tokio::test]
async fn event_bus_subscribe_before_publish_receives_event() {
    use polkagent_core::event::{EventCorrelation, EventKind, RunEvent};
    use polkagent_core::ids::EventId;

    let bus = EventBus::with_default_capacity();
    // Subscribe first, then publish.
    let mut receiver = bus.subscribe();

    let run_id = RunId::new();
    let event = RunEvent::new_durable(
        EventId::new(),
        run_id,
        1,
        EventKind::RunCompleted {
            output_artifact_id: None,
            input_tokens: 0,
            output_tokens: 0,
        },
        EventCorrelation {
            run_id,
            ..Default::default()
        },
    );

    bus.publish(event.clone());

    let delivered_event = receiver.recv().await.expect("should receive");
    assert_eq!(delivered_event.id, event.id);
}

#[tokio::test]
async fn event_bus_no_events_before_subscribe_are_replayed() {
    use polkagent_core::event::{EventCorrelation, EventKind, RunEvent};
    use polkagent_core::ids::EventId;

    let bus = EventBus::with_default_capacity();
    let run_id = RunId::new();

    // Publish before subscribing.
    let old_event = RunEvent::new_durable(
        EventId::new(),
        run_id,
        1,
        EventKind::RunCreated,
        EventCorrelation {
            run_id,
            ..Default::default()
        },
    );
    bus.publish(old_event);

    // Subscribe after the publish — old events should NOT be replayed.
    let mut receiver = bus.subscribe();

    // Now publish a new event.
    let new_event = RunEvent::new_durable(
        EventId::new(),
        run_id,
        2,
        EventKind::RunStarted,
        EventCorrelation {
            run_id,
            ..Default::default()
        },
    );
    let new_event_id = new_event.id;
    bus.publish(new_event);

    let delivered_event = receiver.recv().await.expect("should receive new event");
    assert_eq!(
        delivered_event.id, new_event_id,
        "should only receive the post-subscribe event"
    );
}

// ===========================================================================
// Effect approval / denial tests
// ===========================================================================

/// `POST /effects/:id/approve` on an existing `awaiting_approval` intent returns 200
/// with the updated state set to "approved" and an approval record.
#[tokio::test]
async fn approve_effect_pending_returns_200_with_approved_state() {
    let (server, store) = test_server_with_effect_store();
    let run_id = RunId::new();
    let intent = make_stored_intent(run_id, "awaiting_approval");
    let effect_id = intent.id.to_string();
    store.seed_intent(intent).await;

    let resp = server
        .post(&format!("/api/v1alpha1/effects/{effect_id}/approve"))
        .json(&json!({}))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    assert_eq!(body["effect_id"], effect_id.as_str());
    assert_eq!(body["new_state"], "approved");
    assert!(
        body["approved_at"].is_string(),
        "approved_at must be a timestamp string"
    );
    // PRD-14 §4: response must include an approval record.
    assert!(body["approval"]["id"].is_string());
    assert_eq!(body["approval"]["decision"], "approved");
}

/// POST /effects/:id/approve on a non-existent effect returns 404.
#[tokio::test]
async fn approve_effect_nonexistent_returns_404() {
    let server = test_server();
    let fake_id = polkagent_core::EffectId::new().to_string();
    let resp = server
        .post(&format!("/api/v1alpha1/effects/{fake_id}/approve"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_FOUND");
}

/// POST /effects/:id/approve on an already-resolved (non-pending) intent
/// returns 409 Conflict.
#[tokio::test]
async fn approve_effect_already_resolved_returns_409() {
    let (server, store) = test_server_with_effect_store();
    let run_id = RunId::new();
    // Seed an intent in "resolved" state — cannot be approved.
    let intent = make_stored_intent(run_id, "resolved");
    let effect_id = intent.id.to_string();
    store.seed_intent(intent).await;

    let resp = server
        .post(&format!("/api/v1alpha1/effects/{effect_id}/approve"))
        .await;
    resp.assert_status(axum::http::StatusCode::CONFLICT);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "INVALID_STATE");
}

/// `POST /effects/:id/approve` on an `awaiting_approval` intent with comment and conditions.
#[tokio::test]
async fn approve_effect_waiting_approval_returns_200() {
    let (server, store) = test_server_with_effect_store();
    let run_id = RunId::new();
    let intent = make_stored_intent(run_id, "awaiting_approval");
    let effect_id = intent.id.to_string();
    store.seed_intent(intent).await;

    let resp = server
        .post(&format!("/api/v1alpha1/effects/{effect_id}/approve"))
        .json(&json!({ "comment": "Approved per policy", "conditions": ["max_value:500"] }))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["new_state"], "approved");
    assert_eq!(body["approval"]["comment"], "Approved per policy");
    assert_eq!(body["approval"]["conditions"][0], "max_value:500");
}

/// `POST /effects/:id/deny` on an existing `awaiting_approval` intent returns 200
/// with the updated state set to "denied" and a denial record.
#[tokio::test]
async fn deny_effect_pending_returns_200_with_denied_state() {
    let (server, store) = test_server_with_effect_store();
    let run_id = RunId::new();
    let intent = make_stored_intent(run_id, "awaiting_approval");
    let effect_id = intent.id.to_string();
    store.seed_intent(intent).await;

    let resp = server
        .post(&format!("/api/v1alpha1/effects/{effect_id}/deny"))
        .json(&json!({ "comment": "Over budget", "reason": "budget exceeded" }))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    assert_eq!(body["effect_id"], effect_id.as_str());
    assert_eq!(body["new_state"], "denied");
    assert!(
        body["denied_at"].is_string(),
        "denied_at must be a timestamp string"
    );
    // The denial reason is recorded.
    assert_eq!(body["reason"], "budget exceeded");
    // PRD-14 §4: response must include a denial record.
    assert!(body["approval"]["id"].is_string());
    assert_eq!(body["approval"]["decision"], "denied");
    assert_eq!(body["approval"]["comment"], "Over budget");
}

/// POST /effects/:id/deny with no reason body records null/absent reason.
#[tokio::test]
async fn deny_effect_without_reason_omits_reason_field() {
    let (server, store) = test_server_with_effect_store();
    let run_id = RunId::new();
    let intent = make_stored_intent(run_id, "awaiting_approval");
    let effect_id = intent.id.to_string();
    store.seed_intent(intent).await;

    let resp = server
        .post(&format!("/api/v1alpha1/effects/{effect_id}/deny"))
        .json(&json!({}))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["new_state"], "denied");
    // reason is absent when not provided (skip_serializing_if = None).
    assert!(
        body["reason"].is_null() || body.get("reason").is_none(),
        "reason should be absent when not supplied"
    );
}

/// POST /effects/:id/deny on a non-existent effect returns 404.
#[tokio::test]
async fn deny_effect_nonexistent_returns_404() {
    let server = test_server();
    let fake_id = polkagent_core::EffectId::new().to_string();
    let resp = server
        .post(&format!("/api/v1alpha1/effects/{fake_id}/deny"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "NOT_FOUND");
}

/// POST /effects/:id/deny on an already-resolved intent returns 409.
#[tokio::test]
async fn deny_effect_already_resolved_returns_409() {
    let (server, store) = test_server_with_effect_store();
    let run_id = RunId::new();
    let intent = make_stored_intent(run_id, "resolved");
    let effect_id = intent.id.to_string();
    store.seed_intent(intent).await;

    let resp = server
        .post(&format!("/api/v1alpha1/effects/{effect_id}/deny"))
        .await;
    resp.assert_status(axum::http::StatusCode::CONFLICT);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "INVALID_STATE");
}

// ===========================================================================
// Run resume tests
// ===========================================================================

// The resume_run tests below cover:
//   1. 404 when the run doesn't exist.
//   2. 409 when the run exists but is not in AwaitingApproval.
//   3. 200 when the run is in AwaitingApproval (using force_awaiting_approval).

/// POST /runs/:id/resume on a non-existent run returns 404.
#[tokio::test]
async fn resume_run_nonexistent_returns_404() {
    let server = test_server();
    let fake_id = polkagent_core::RunId::new().to_string();
    let resp = server
        .post(&format!("/api/v1alpha1/runs/{fake_id}/resume"))
        .await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "RUN_NOT_FOUND");
}

/// `POST /runs/:id/resume` on a run that is not in `AwaitingApproval` returns
/// 409 Conflict with `INVALID_STATE` code.
#[tokio::test]
async fn resume_run_not_in_awaiting_approval_returns_409() {
    let server = test_server();
    let agent_id = create_test_agent(&server).await;
    let run_id = create_test_run(&server, &agent_id).await;

    // Newly-created run is in Running state — not AwaitingApproval.
    let resp = server
        .post(&format!("/api/v1alpha1/runs/{run_id}/resume"))
        .await;
    resp.assert_status(axum::http::StatusCode::CONFLICT);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["error"]["code"], "INVALID_STATE");
}

/// `POST /runs/:id/resume` returns the full `RunResponse` with correct version.
///
/// We verify the 200 path by using an `InMemoryRunManager` whose state we
/// set directly to `AwaitingApproval` before calling resume.
#[tokio::test]
async fn resume_run_awaiting_approval_returns_200_with_running_state() {
    use polkagent_api::InMemoryRunManager;
    use polkagent_core::RunId as CoreRunId;

    // Build a run manager that we can manipulate.
    let run_manager = Arc::new(InMemoryRunManager::new());

    // Create an agent first.
    let agents: Arc<dyn polkagent_api::state::AgentStore> = Arc::new(InMemoryAgentStore::new());
    let store: Arc<dyn EffectStore> = Arc::new(NoopEffectStore);
    let event_bus = EventBus::with_default_capacity();
    let state = polkagent_api::AppState::new(
        Config::default(),
        agents.clone(),
        run_manager.clone(),
        store,
        event_bus,
    );
    let server = ApiServer::from_state(state);
    let server = TestServer::new(server.into_router());

    // Create an agent and a run via the API.
    let resp = server
        .post("/api/v1alpha1/agents")
        .json(&json!({ "name": "Resume Test", "model": "anthropic/claude-opus-4-6" }))
        .await;
    resp.assert_status(axum::http::StatusCode::CREATED);
    let agent_body: serde_json::Value = resp.json();
    let agent_id = agent_body["id"].as_str().unwrap().to_owned();

    let resp = server
        .post(&format!("/api/v1alpha1/agents/{agent_id}/runs"))
        .json(&json!({ "input": "test" }))
        .await;
    resp.assert_status(axum::http::StatusCode::CREATED);
    let run_body: serde_json::Value = resp.json();
    let run_id_str = run_body["id"].as_str().unwrap().to_owned();
    let run_id: CoreRunId = run_id_str.parse().unwrap();

    // Force the run into AwaitingApproval state directly.
    run_manager.force_awaiting_approval(run_id).await;

    // Now resume should succeed.
    let resp = server
        .post(&format!("/api/v1alpha1/runs/{run_id_str}/resume"))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["version"], "v1alpha1");
    assert_eq!(body["id"], run_id_str.as_str());
    // After resume the state should be Running.
    assert_eq!(body["status"]["state"], "running");
}

// ===========================================================================
// PCA C1 Bridge tests (PRD-06)
// ===========================================================================

#[tokio::test]
async fn bridge_health_returns_identity_and_capabilities() {
    let server = test_server();
    let resp = server.get("/v1/compat/pca/health").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert!(body["identity"]["bot_id"].is_string());
    assert!(body["transport"]["connected"].as_bool().unwrap_or(false));
    assert_eq!(body["transport"]["protocol"], "polkagent-c1");
    assert!(body["capabilities"]["send"].as_bool().unwrap_or(false));
}

#[tokio::test]
async fn bridge_inbound_returns_empty_deliveries() {
    let server = test_server();
    let resp = server.get("/v1/compat/pca/inbound").await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    let deliveries = body["deliveries"].as_array().expect("deliveries array");
    assert!(deliveries.is_empty());
}

#[tokio::test]
async fn assembled_router_enforces_the_documented_operational_access_policy() {
    let token = "production-access-policy-token";
    let mut config = Config::default();
    config.auth.enabled = true;
    config.auth.api_keys = vec![format!("{:x}", Sha256::digest(token.as_bytes()))];
    config.api.read_only = true;
    config.server.rate_limit.enabled = true;
    config.server.rate_limit.requests_per_second = 0;
    config.server.rate_limit.burst = 1;
    let server = test_server_with_config(config);

    // Every public path is both authentication-free and outside the shared
    // anonymous token bucket. Repetition catches a route that bypasses auth
    // but accidentally remains rate-limited.
    for path in [
        "/openapi.json",
        "/health/live",
        "/health/ready",
        "/health/startup",
        "/v1/compat/pca/health",
    ] {
        server.get(path).await.assert_status_ok();
        server.get(path).await.assert_status_ok();
    }

    let missing_credentials = server.get("/metrics").await;
    missing_credentials.assert_status(axum::http::StatusCode::UNAUTHORIZED);
    assert_eq!(
        missing_credentials.json::<serde_json::Value>(),
        json!({
            "error": {
                "code": "UNAUTHORIZED",
                "message": "missing credentials"
            }
        })
    );

    let invalid_credentials = server
        .get("/v1/compat/pca/inbound")
        .add_header("X-Api-Key", "invalid-pca-key")
        .await;
    invalid_credentials.assert_status(axum::http::StatusCode::UNAUTHORIZED);
    assert_eq!(
        invalid_credentials.json::<serde_json::Value>(),
        json!({
            "error": {
                "code": "UNAUTHORIZED",
                "message": "invalid API key"
            }
        })
    );

    let first_protected = server
        .get("/api/v1alpha1/system/info")
        .add_header("X-Api-Key", token)
        .await;
    first_protected.assert_status_ok();
    first_protected.assert_header("x-ratelimit-limit", "1");
    first_protected.assert_header("x-ratelimit-remaining", "0");

    let rate_limited = server
        .get("/api/v1alpha1/system/info")
        .add_header("X-Api-Key", token)
        .await;
    rate_limited.assert_status(axum::http::StatusCode::TOO_MANY_REQUESTS);
    rate_limited.assert_header("retry-after", "3600");
    rate_limited.assert_header("x-ratelimit-limit", "1");
    rate_limited.assert_header("x-ratelimit-remaining", "0");
    assert_eq!(
        rate_limited.json::<serde_json::Value>(),
        json!({
            "error": {
                "code": "RATE_LIMIT_EXCEEDED",
                "message": "too many requests",
                "retry_after": 3600
            }
        })
    );

    // Axum's query extractor rejects malformed values before the handler and
    // uses a plain-text 400, matching the reusable OpenAPI response.
    let bad_query = server
        .get("/api/v1alpha1/agents?limit=not-an-integer")
        .authorization_bearer(token)
        .add_header("X-Api-Key", "query-client")
        .await;
    bad_query.assert_status(axum::http::StatusCode::BAD_REQUEST);
    bad_query.assert_header("content-type", "text/plain; charset=utf-8");
    assert!(bad_query
        .text()
        .contains("Failed to deserialize query string"));

    // Read-only remains an independent method policy. A unique rate key keeps
    // this assertion focused on the guard rather than token-bucket state.
    let read_only = server
        .post("/api/v1alpha1/agents")
        .authorization_bearer(token)
        .add_header("X-Api-Key", "read-only-client")
        .json(&json!({
            "name": "blocked",
            "model": "anthropic/claude-opus-4-6"
        }))
        .await;
    read_only.assert_status(axum::http::StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        read_only.json::<serde_json::Value>(),
        json!({"error": "server is in read-only mode"})
    );
}

#[tokio::test]
async fn bridge_ack_validates_required_fields() {
    let server = test_server();
    let resp = server
        .post("/v1/compat/pca/inbound/ack")
        .json(&json!({ "delivery_id": "", "lease_id": "" }))
        .await;
    resp.assert_status(axum::http::StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn bridge_ack_succeeds_with_valid_ids() {
    let server = test_server();
    let resp = server
        .post("/v1/compat/pca/inbound/ack")
        .json(&json!({ "delivery_id": "del-1", "lease_id": "lease-1" }))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert!(body["ok"].as_bool().unwrap_or(false));
}

#[tokio::test]
async fn bridge_renew_validates_required_fields() {
    let server = test_server();
    let resp = server
        .post("/v1/compat/pca/inbound/renew")
        .json(&json!({ "delivery_id": "", "lease_id": "" }))
        .await;
    resp.assert_status(axum::http::StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn bridge_renew_returns_new_lease() {
    let server = test_server();
    let resp = server
        .post("/v1/compat/pca/inbound/renew")
        .json(&json!({ "delivery_id": "del-1", "lease_id": "lease-1" }))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert!(body["ok"].as_bool().unwrap_or(false));
    assert!(body["new_lease_ms"].as_u64().unwrap_or(0) > 0);
}

#[tokio::test]
async fn bridge_send_validates_chat_id() {
    let server = test_server();
    let resp = server
        .post("/v1/compat/pca/send")
        .json(&json!({ "chat_id": "", "text": "hello" }))
        .await;
    resp.assert_status(axum::http::StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn bridge_send_validates_text() {
    let server = test_server();
    let resp = server
        .post("/v1/compat/pca/send")
        .json(&json!({ "chat_id": "chat-1", "text": "" }))
        .await;
    resp.assert_status(axum::http::StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn bridge_send_returns_message_id() {
    let server = test_server();
    let resp = server
        .post("/v1/compat/pca/send")
        .json(&json!({ "chat_id": "chat-1", "text": "Hello from harness" }))
        .await;
    resp.assert_status(axum::http::StatusCode::CREATED);
    let body: serde_json::Value = resp.json();
    assert!(body["ok"].as_bool().unwrap_or(false));
    assert!(body["message_id"].is_string());
    assert!(!body["message_id"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn bridge_send_with_optional_fields() {
    let server = test_server();
    let resp = server
        .post("/v1/compat/pca/send")
        .json(&json!({
            "chat_id": "chat-1",
            "text": "Reply",
            "delivery_id": "del-1",
            "lease_id": "lease-1",
            "reply_to": "msg-42"
        }))
        .await;
    resp.assert_status(axum::http::StatusCode::CREATED);
    let body: serde_json::Value = resp.json();
    assert!(body["ok"].as_bool().unwrap_or(false));
}
