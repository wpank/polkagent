//! Effect pipeline REST endpoints.
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `GET` | `/runs/:run_id/effects` | [`list_effects`] |
//! | `GET` | `/effects/:id` | [`get_effect`] |
//! | `POST` | `/effects/:id/approve` | [`approve_effect`] |
//! | `POST` | `/effects/:id/deny` | [`deny_effect`] |

use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Json,
};
use chrono::Utc;
use polkagent_core::{
    event::{EventKind, RunEvent},
    EffectId, EventId, RunId,
};
use polkagent_store_trait::StoreError;
use serde::Serialize;
use tracing::{info, instrument};
use uuid::Uuid;

use crate::{
    dto::{
        ApprovalRecordDto, ApproveEffectRequest, ApproveEffectResponse, DenyEffectRequest,
        DenyEffectResponse, API_VERSION,
    },
    error::ApiError,
    state::AppState,
};

// ---------------------------------------------------------------------------
// Constants — approval state strings
// ---------------------------------------------------------------------------

/// The effect state that permits approval or denial (PRD-14 §4).
const AWAITING_APPROVAL: &str = "awaiting_approval";

/// States that indicate a final decision has already been recorded.
const DECIDED_STATES: &[&str] = &["approved", "denied"];

// ---------------------------------------------------------------------------
// Response DTOs
// ---------------------------------------------------------------------------

/// Response body for a single stored effect intent.
#[derive(Debug, Clone, Serialize)]
pub struct EffectResponse {
    /// API version.
    pub version: String,
    /// The stored intent serialised from the effect store.
    pub data: serde_json::Value,
}

/// Response body for a list of stored effect intents.
#[derive(Debug, Clone, Serialize)]
pub struct ListEffectsResponse {
    /// API version.
    pub version: String,
    /// List of stored intents.
    pub data: Vec<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// GET /runs/:run_id/effects
// ---------------------------------------------------------------------------

/// List all effect intents associated with a run.
///
/// Returns the raw stored intent payloads from the effect store.
#[instrument(skip(state), fields(run_id = %run_id))]
pub async fn list_effects(
    State(state): State<AppState>,
    Path(run_id): Path<RunId>,
) -> Result<impl IntoResponse, ApiError> {
    let intents = state
        .effect_store
        .get_by_run(run_id)
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    let data: Vec<serde_json::Value> = intents
        .into_iter()
        .map(|intent| serde_json::to_value(&intent).unwrap_or_else(|_| serde_json::Value::Null))
        .collect();

    Ok(Json(ListEffectsResponse {
        version: API_VERSION.to_owned(),
        data,
    }))
}

// ---------------------------------------------------------------------------
// GET /effects/:id
// ---------------------------------------------------------------------------

/// Get detailed information about a single effect intent.
///
/// Returns 404 if no intent with the given ID exists.
#[instrument(skip(state), fields(effect_id = %id))]
pub async fn get_effect(
    State(state): State<AppState>,
    Path(id): Path<EffectId>,
) -> Result<impl IntoResponse, ApiError> {
    let intent = state
        .effect_store
        .get_intent(id)
        .await
        .map_err(|e| match e {
            polkagent_store_trait::StoreError::NotFound { .. } => {
                ApiError::NotFound(format!("effect intent {id}"))
            }
            other => ApiError::InternalError(other.to_string()),
        })?;

    let data = serde_json::to_value(&intent).map_err(|e| ApiError::InternalError(e.to_string()))?;

    Ok(Json(EffectResponse {
        version: API_VERSION.to_owned(),
        data,
    }))
}

// ---------------------------------------------------------------------------
// POST /effects/:id/approve
// ---------------------------------------------------------------------------

/// Approve a pending effect intent (PRD-14 §4).
///
/// - Looks up the effect intent in the store.
/// - Verifies it is in `"awaiting_approval"` state — returns 409 for any
///   other state, including already-decided (`"approved"` / `"denied"`).
/// - Transitions the intent to `"approved"` via the store.
/// - Creates an `ApprovalRecord` and
///   emits an [`EventKind::ApprovalGranted`] event on the bus.
/// - Returns 200 with the updated effect state and the approval record.
///
/// # Errors
///
/// - 404 if no intent with `id` exists.
/// - 409 if the intent is not in `"awaiting_approval"` state.
#[instrument(skip(state, body), fields(effect_id = %id))]
pub async fn approve_effect(
    State(state): State<AppState>,
    Path(id): Path<EffectId>,
    body: Option<Json<ApproveEffectRequest>>,
) -> Result<impl IntoResponse, ApiError> {
    let body = body.map(|b| b.0).unwrap_or_default();

    // Look up the intent, returning 404 if it doesn't exist.
    let intent = state
        .effect_store
        .get_intent(id)
        .await
        .map_err(|e| match e {
            StoreError::NotFound { .. } => ApiError::NotFound(format!("effect intent {id}")),
            other => ApiError::InternalError(other.to_string()),
        })?;

    // Return 409 Conflict if already decided.
    if DECIDED_STATES.contains(&intent.state.as_str()) {
        return Err(ApiError::InvalidState(format!(
            "effect intent {id} has already been decided (state: '{}')",
            intent.state
        )));
    }

    // Validate that the intent is awaiting approval.
    if intent.state.as_str() != AWAITING_APPROVAL {
        return Err(ApiError::InvalidState(format!(
            "effect intent {id} is in state '{}'; only '{}' intents may be approved",
            intent.state, AWAITING_APPROVAL
        )));
    }

    // Transition to "approved" via the store.
    let updated = state
        .effect_store
        .update_intent_state(id, "approved")
        .await
        .map_err(|e| match e {
            StoreError::NotFound { .. } => ApiError::NotFound(format!("effect intent {id}")),
            StoreError::InvalidTransition { message } => ApiError::InvalidState(message),
            other => ApiError::InternalError(other.to_string()),
        })?;

    let approval_id = Uuid::now_v7();
    let approved_at = Utc::now();

    // Build the approval record DTO.
    let approval_dto = ApprovalRecordDto {
        id: approval_id.to_string(),
        effect_id: id.to_string(),
        approval_type: "human".to_owned(),
        principal_id: "api".to_owned(),
        comment: body.comment.clone(),
        conditions: body.conditions.clone(),
        decision: "approved".to_owned(),
        created_at: approved_at,
    };

    // Emit ApprovalGranted event on the bus (best-effort; ignore no-subscriber errors).
    let event = RunEvent::new_ephemeral(
        EventId::new(),
        updated.run_id,
        0, // sequence managed by recorder in production; 0 for bus-only events
        EventKind::ApprovalGranted {
            approval_id: approval_id.to_string(),
        },
    );
    state.event_bus.publish(event);

    state.metrics.effects_approved();
    info!(effect_id = %id, approval_id = %approval_id, "effect intent approved");

    Ok(Json(ApproveEffectResponse {
        version: API_VERSION.to_owned(),
        effect_id: updated.id.to_string(),
        new_state: updated.state.clone(),
        approval: approval_dto,
        approved_at,
    }))
}

// ---------------------------------------------------------------------------
// POST /effects/:id/deny
// ---------------------------------------------------------------------------

/// Deny a pending effect intent (PRD-14 §4).
///
/// - Looks up the effect intent in the store.
/// - Verifies it is in `"awaiting_approval"` state — returns 409 for any
///   other state, including already-decided (`"approved"` / `"denied"`).
/// - Transitions the intent to `"denied"` via the store.
/// - Creates an `ApprovalRecord` and
///   emits an [`EventKind::ApprovalDenied`] event on the bus.
/// - Returns 200 with the updated effect state and the denial record.
///
/// # Errors
///
/// - 404 if no intent with `id` exists.
/// - 409 if the intent is not in `"awaiting_approval"` state.
#[instrument(skip(state, body), fields(effect_id = %id))]
pub async fn deny_effect(
    State(state): State<AppState>,
    Path(id): Path<EffectId>,
    body: Option<Json<DenyEffectRequest>>,
) -> Result<impl IntoResponse, ApiError> {
    let body = body.map(|b| b.0).unwrap_or_default();
    let reason = body.reason.clone();

    // Look up the intent, returning 404 if it doesn't exist.
    let intent = state
        .effect_store
        .get_intent(id)
        .await
        .map_err(|e| match e {
            StoreError::NotFound { .. } => ApiError::NotFound(format!("effect intent {id}")),
            other => ApiError::InternalError(other.to_string()),
        })?;

    // Return 409 Conflict if already decided.
    if DECIDED_STATES.contains(&intent.state.as_str()) {
        return Err(ApiError::InvalidState(format!(
            "effect intent {id} has already been decided (state: '{}')",
            intent.state
        )));
    }

    // Validate that the intent is awaiting approval.
    if intent.state.as_str() != AWAITING_APPROVAL {
        return Err(ApiError::InvalidState(format!(
            "effect intent {id} is in state '{}'; only '{}' intents may be denied",
            intent.state, AWAITING_APPROVAL
        )));
    }

    // Transition to "denied" via the store.
    let updated = state
        .effect_store
        .update_intent_state(id, "denied")
        .await
        .map_err(|e| match e {
            StoreError::NotFound { .. } => ApiError::NotFound(format!("effect intent {id}")),
            StoreError::InvalidTransition { message } => ApiError::InvalidState(message),
            other => ApiError::InternalError(other.to_string()),
        })?;

    let approval_id = Uuid::now_v7();
    let denied_at = Utc::now();
    let denial_reason = reason.clone().unwrap_or_default();

    // Build the denial record DTO.
    let approval_dto = ApprovalRecordDto {
        id: approval_id.to_string(),
        effect_id: id.to_string(),
        approval_type: "human".to_owned(),
        principal_id: "api".to_owned(),
        comment: body.comment.clone(),
        conditions: vec![],
        decision: "denied".to_owned(),
        created_at: denied_at,
    };

    // Emit ApprovalDenied event on the bus (best-effort; ignore no-subscriber errors).
    let event = RunEvent::new_ephemeral(
        EventId::new(),
        updated.run_id,
        0, // sequence managed by recorder in production; 0 for bus-only events
        EventKind::ApprovalDenied {
            reason: denial_reason,
        },
    );
    state.event_bus.publish(event);

    state.metrics.effects_denied();
    info!(effect_id = %id, approval_id = %approval_id, "effect intent denied");

    Ok(Json(DenyEffectResponse {
        version: API_VERSION.to_owned(),
        effect_id: updated.id.to_string(),
        new_state: updated.state.clone(),
        approval: approval_dto,
        reason,
        denied_at,
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use axum::http::StatusCode;
    use axum_test::TestServer;
    use chrono::Utc;
    use polkagent_config::Config;
    use polkagent_core::{EffectAttemptId, EffectId, EffectOutcomeId, RunId, StepId, WorkerId};
    use polkagent_event::EventBus;
    use polkagent_store_trait::{
        EffectStore, StoreError, StoreRetryClass, StoredIntent, StoredOutcome,
    };
    use serde_json::json;
    use tokio::sync::RwLock;

    use crate::run::InMemoryRunManager;
    use crate::state::{AppState, InMemoryAgentStore};

    // -----------------------------------------------------------------------
    // Fake in-memory EffectStore for tests
    // -----------------------------------------------------------------------

    #[derive(Debug, Default)]
    struct FakeEffectStore {
        intents: RwLock<HashMap<EffectId, StoredIntent>>,
    }

    impl FakeEffectStore {
        fn new() -> Self {
            Self::default()
        }

        /// Pre-populate an intent with the given state.
        async fn insert(&self, intent: StoredIntent) {
            self.intents.write().await.insert(intent.id, intent);
        }

        /// Build a minimal `StoredIntent` with the given state.
        fn make_intent(id: EffectId, state: impl Into<String>) -> StoredIntent {
            StoredIntent {
                id,
                run_id: RunId::new(),
                step_id: StepId::new(),
                state: state.into(),
                lease_owner: None,
                lease_expires: None,
                retry_class: StoreRetryClass::Idempotent,
                payload: serde_json::Value::Null,
                idempotency_key: "test-key".into(),
                created_at: Utc::now(),
            }
        }
    }

    #[async_trait]
    impl EffectStore for FakeEffectStore {
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
                    resource_type: "StoredIntent",
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
                    resource_type: "StoredIntent",
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

        async fn expired_leases(
            &self,
            _cutoff: chrono::DateTime<Utc>,
        ) -> Result<Vec<StoredIntent>, StoreError> {
            Ok(vec![])
        }

        async fn update_intent_state(
            &self,
            intent_id: EffectId,
            new_state: &str,
        ) -> Result<StoredIntent, StoreError> {
            let mut guard = self.intents.write().await;
            match guard.get_mut(&intent_id) {
                None => Err(StoreError::NotFound {
                    resource_type: "StoredIntent",
                    id: intent_id.to_string(),
                }),
                Some(intent) => {
                    intent.state = new_state.to_owned();
                    Ok(intent.clone())
                }
            }
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

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn make_app(store: Arc<FakeEffectStore>) -> TestServer {
        let agents = Arc::new(InMemoryAgentStore::new());
        let run_manager = Arc::new(InMemoryRunManager::new());
        let effect_store: Arc<dyn EffectStore> = store;
        let event_bus = EventBus::with_default_capacity();
        let state = AppState::new(
            Config::default(),
            agents,
            run_manager,
            effect_store,
            event_bus,
        );
        let router = crate::routes::register(state);
        TestServer::new(router)
    }

    // -----------------------------------------------------------------------
    // GET /effects/:id
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn get_effect_not_found_returns_404() {
        let store = Arc::new(FakeEffectStore::new());
        let srv = make_app(store);
        let id = EffectId::new();
        let resp = srv.get(&format!("/api/v1alpha1/effects/{id}")).await;
        assert_eq!(resp.status_code(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_effect_returns_200_when_found() {
        let store = Arc::new(FakeEffectStore::new());
        let id = EffectId::new();
        store
            .insert(FakeEffectStore::make_intent(id, "awaiting_approval"))
            .await;
        let srv = make_app(store);
        let resp = srv.get(&format!("/api/v1alpha1/effects/{id}")).await;
        assert_eq!(resp.status_code(), StatusCode::OK);
        let body: serde_json::Value = resp.json();
        assert_eq!(body["version"], "v1alpha1");
    }

    // -----------------------------------------------------------------------
    // POST /effects/:id/approve
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn approve_effect_returns_404_when_not_found() {
        let store = Arc::new(FakeEffectStore::new());
        let srv = make_app(store);
        let id = EffectId::new();
        let resp = srv
            .post(&format!("/api/v1alpha1/effects/{id}/approve"))
            .json(&json!({}))
            .await;
        assert_eq!(resp.status_code(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn approve_effect_in_awaiting_approval_returns_200() {
        let store = Arc::new(FakeEffectStore::new());
        let id = EffectId::new();
        store
            .insert(FakeEffectStore::make_intent(id, "awaiting_approval"))
            .await;
        let srv = make_app(store);
        let resp = srv
            .post(&format!("/api/v1alpha1/effects/{id}/approve"))
            .json(&json!({ "comment": "Looks good", "conditions": ["max_value:1000"] }))
            .await;
        assert_eq!(resp.status_code(), StatusCode::OK);
        let body: serde_json::Value = resp.json();
        assert_eq!(body["new_state"], "approved");
        assert_eq!(body["approval"]["decision"], "approved");
        assert_eq!(body["approval"]["comment"], "Looks good");
        assert_eq!(body["approval"]["conditions"][0], "max_value:1000");
    }

    #[tokio::test]
    async fn approve_effect_in_pending_returns_409() {
        let store = Arc::new(FakeEffectStore::new());
        let id = EffectId::new();
        store
            .insert(FakeEffectStore::make_intent(id, "pending"))
            .await;
        let srv = make_app(store);
        let resp = srv
            .post(&format!("/api/v1alpha1/effects/{id}/approve"))
            .json(&json!({}))
            .await;
        assert_eq!(resp.status_code(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn approve_effect_already_approved_returns_409() {
        let store = Arc::new(FakeEffectStore::new());
        let id = EffectId::new();
        store
            .insert(FakeEffectStore::make_intent(id, "approved"))
            .await;
        let srv = make_app(store);
        let resp = srv
            .post(&format!("/api/v1alpha1/effects/{id}/approve"))
            .json(&json!({}))
            .await;
        assert_eq!(resp.status_code(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn approve_effect_already_denied_returns_409() {
        let store = Arc::new(FakeEffectStore::new());
        let id = EffectId::new();
        store
            .insert(FakeEffectStore::make_intent(id, "denied"))
            .await;
        let srv = make_app(store);
        let resp = srv
            .post(&format!("/api/v1alpha1/effects/{id}/approve"))
            .json(&json!({}))
            .await;
        assert_eq!(resp.status_code(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn approve_effect_no_body_uses_defaults() {
        let store = Arc::new(FakeEffectStore::new());
        let id = EffectId::new();
        store
            .insert(FakeEffectStore::make_intent(id, "awaiting_approval"))
            .await;
        let srv = make_app(store);
        // POST with no body should default to empty ApproveEffectRequest
        let resp = srv
            .post(&format!("/api/v1alpha1/effects/{id}/approve"))
            .await;
        assert_eq!(resp.status_code(), StatusCode::OK);
        let body: serde_json::Value = resp.json();
        assert_eq!(body["new_state"], "approved");
    }

    #[tokio::test]
    async fn approve_effect_response_contains_approval_record_id() {
        let store = Arc::new(FakeEffectStore::new());
        let id = EffectId::new();
        store
            .insert(FakeEffectStore::make_intent(id, "awaiting_approval"))
            .await;
        let srv = make_app(store);
        let resp = srv
            .post(&format!("/api/v1alpha1/effects/{id}/approve"))
            .json(&json!({}))
            .await;
        assert_eq!(resp.status_code(), StatusCode::OK);
        let body: serde_json::Value = resp.json();
        assert!(body["approval"]["id"].is_string());
        assert!(!body["approval"]["id"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn approve_effect_response_has_effect_id_in_approval() {
        let store = Arc::new(FakeEffectStore::new());
        let id = EffectId::new();
        store
            .insert(FakeEffectStore::make_intent(id, "awaiting_approval"))
            .await;
        let srv = make_app(store);
        let resp = srv
            .post(&format!("/api/v1alpha1/effects/{id}/approve"))
            .json(&json!({}))
            .await;
        let body: serde_json::Value = resp.json();
        assert_eq!(body["approval"]["effect_id"], id.to_string());
    }

    #[tokio::test]
    async fn approve_effect_response_approval_type_is_human() {
        let store = Arc::new(FakeEffectStore::new());
        let id = EffectId::new();
        store
            .insert(FakeEffectStore::make_intent(id, "awaiting_approval"))
            .await;
        let srv = make_app(store);
        let resp = srv
            .post(&format!("/api/v1alpha1/effects/{id}/approve"))
            .json(&json!({}))
            .await;
        let body: serde_json::Value = resp.json();
        assert_eq!(body["approval"]["approval_type"], "human");
    }

    // -----------------------------------------------------------------------
    // POST /effects/:id/deny
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn deny_effect_returns_404_when_not_found() {
        let store = Arc::new(FakeEffectStore::new());
        let srv = make_app(store);
        let id = EffectId::new();
        let resp = srv
            .post(&format!("/api/v1alpha1/effects/{id}/deny"))
            .json(&json!({}))
            .await;
        assert_eq!(resp.status_code(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn deny_effect_in_awaiting_approval_returns_200() {
        let store = Arc::new(FakeEffectStore::new());
        let id = EffectId::new();
        store
            .insert(FakeEffectStore::make_intent(id, "awaiting_approval"))
            .await;
        let srv = make_app(store);
        let resp = srv
            .post(&format!("/api/v1alpha1/effects/{id}/deny"))
            .json(&json!({ "comment": "Risk too high", "reason": "policy_violation" }))
            .await;
        assert_eq!(resp.status_code(), StatusCode::OK);
        let body: serde_json::Value = resp.json();
        assert_eq!(body["new_state"], "denied");
        assert_eq!(body["reason"], "policy_violation");
        assert_eq!(body["approval"]["decision"], "denied");
        assert_eq!(body["approval"]["comment"], "Risk too high");
    }

    #[tokio::test]
    async fn deny_effect_in_pending_returns_409() {
        let store = Arc::new(FakeEffectStore::new());
        let id = EffectId::new();
        store
            .insert(FakeEffectStore::make_intent(id, "pending"))
            .await;
        let srv = make_app(store);
        let resp = srv
            .post(&format!("/api/v1alpha1/effects/{id}/deny"))
            .json(&json!({}))
            .await;
        assert_eq!(resp.status_code(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn deny_effect_already_approved_returns_409() {
        let store = Arc::new(FakeEffectStore::new());
        let id = EffectId::new();
        store
            .insert(FakeEffectStore::make_intent(id, "approved"))
            .await;
        let srv = make_app(store);
        let resp = srv
            .post(&format!("/api/v1alpha1/effects/{id}/deny"))
            .json(&json!({}))
            .await;
        assert_eq!(resp.status_code(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn deny_effect_already_denied_returns_409() {
        let store = Arc::new(FakeEffectStore::new());
        let id = EffectId::new();
        store
            .insert(FakeEffectStore::make_intent(id, "denied"))
            .await;
        let srv = make_app(store);
        let resp = srv
            .post(&format!("/api/v1alpha1/effects/{id}/deny"))
            .json(&json!({}))
            .await;
        assert_eq!(resp.status_code(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn deny_effect_no_body_uses_defaults() {
        let store = Arc::new(FakeEffectStore::new());
        let id = EffectId::new();
        store
            .insert(FakeEffectStore::make_intent(id, "awaiting_approval"))
            .await;
        let srv = make_app(store);
        let resp = srv.post(&format!("/api/v1alpha1/effects/{id}/deny")).await;
        assert_eq!(resp.status_code(), StatusCode::OK);
        let body: serde_json::Value = resp.json();
        assert_eq!(body["new_state"], "denied");
    }

    #[tokio::test]
    async fn deny_effect_response_contains_approval_record_id() {
        let store = Arc::new(FakeEffectStore::new());
        let id = EffectId::new();
        store
            .insert(FakeEffectStore::make_intent(id, "awaiting_approval"))
            .await;
        let srv = make_app(store);
        let resp = srv
            .post(&format!("/api/v1alpha1/effects/{id}/deny"))
            .json(&json!({}))
            .await;
        assert_eq!(resp.status_code(), StatusCode::OK);
        let body: serde_json::Value = resp.json();
        assert!(body["approval"]["id"].is_string());
        assert!(!body["approval"]["id"].as_str().unwrap().is_empty());
    }

    // -----------------------------------------------------------------------
    // List effects
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn list_effects_for_run_returns_empty_when_none() {
        let store = Arc::new(FakeEffectStore::new());
        let srv = make_app(store);
        let run_id = RunId::new();
        let resp = srv
            .get(&format!("/api/v1alpha1/runs/{run_id}/effects"))
            .await;
        assert_eq!(resp.status_code(), StatusCode::OK);
        let body: serde_json::Value = resp.json();
        assert_eq!(body["data"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn list_effects_for_run_returns_matching_intents() {
        let store = Arc::new(FakeEffectStore::new());
        let run_id = RunId::new();
        let id1 = EffectId::new();
        let id2 = EffectId::new();
        let mut intent1 = FakeEffectStore::make_intent(id1, "awaiting_approval");
        intent1.run_id = run_id;
        let mut intent2 = FakeEffectStore::make_intent(id2, "approved");
        intent2.run_id = run_id;
        // A third intent for a different run — should NOT appear.
        let id3 = EffectId::new();
        store.insert(intent1).await;
        store.insert(intent2).await;
        store
            .insert(FakeEffectStore::make_intent(id3, "pending"))
            .await;
        let srv = make_app(Arc::clone(&store));
        let resp = srv
            .get(&format!("/api/v1alpha1/runs/{run_id}/effects"))
            .await;
        assert_eq!(resp.status_code(), StatusCode::OK);
        let body: serde_json::Value = resp.json();
        assert_eq!(body["data"].as_array().unwrap().len(), 2);
    }
}
