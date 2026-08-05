//! Audit log query endpoints.
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `GET` | `/audit` | [`list_audit_entries`] |
//! | `GET` | `/audit/verify` | [`verify_audit_integrity`] |
//! | `GET` | `/audit/:id` | [`get_audit_entry`] |
//!
//! All endpoints return 501 Not Implemented when no audit store is configured
//! via [`AppState::audit_store`]. When a store is configured the handlers
//! delegate to the [`AuditStore`](polkagent_audit::AuditStore) trait.

use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    Json,
};
use chrono::DateTime;
use polkagent_audit::{entry::AuditId, AuditQuery};
use tracing::instrument;

use crate::{
    dto::{
        AuditEntryResponse, AuditListParams, AuditListResponse, AuditVerifyResponse, API_VERSION,
    },
    error::ApiError,
    state::AppState,
};

/// Default number of entries returned when no `limit` query parameter is set.
const DEFAULT_LIMIT: usize = 100;

// ---------------------------------------------------------------------------
// GET /audit
// ---------------------------------------------------------------------------

/// List audit entries with optional filtering.
///
/// Query parameters:
/// - `actor` — filter by actor ID (exact match).
/// - `action` — filter by action type (`snake_case`, e.g. `"run_started"`).
/// - `since` — only include entries at or after this RFC-3339 timestamp.
/// - `until` — only include entries at or before this RFC-3339 timestamp.
/// - `limit` — maximum number of entries to return (default 100).
///
/// Returns 501 Not Implemented when no audit store is configured.
#[instrument(skip(state, params))]
pub async fn list_audit_entries(
    State(state): State<AppState>,
    Query(params): Query<AuditListParams>,
) -> Result<impl IntoResponse, ApiError> {
    let store = state
        .audit_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("audit store not configured".to_owned()))?;

    let mut query = AuditQuery::new();

    if let Some(ref actor) = params.actor {
        query = query.actor(actor);
    }

    if let Some(ref action_str) = params.action {
        let action: polkagent_audit::AuditAction = serde_json::from_value(
            serde_json::Value::String(action_str.clone()),
        )
        .map_err(|_| ApiError::ValidationError(format!("invalid audit action: '{action_str}'")))?;
        query = query.action(action);
    }

    if let Some(ref since_str) = params.since {
        let since = DateTime::parse_from_rfc3339(since_str)
            .map_err(|e| ApiError::ValidationError(format!("invalid 'since' timestamp: {e}")))?
            .to_utc();
        query = query.since(since);
    }

    if let Some(ref until_str) = params.until {
        let until = DateTime::parse_from_rfc3339(until_str)
            .map_err(|e| ApiError::ValidationError(format!("invalid 'until' timestamp: {e}")))?
            .to_utc();
        query = query.until(until);
    }

    let limit = params.limit.unwrap_or(DEFAULT_LIMIT);
    query = query.limit(limit);

    let query = query.build();

    let entries = store
        .query(&query)
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    let count = entries.len();

    Ok(Json(AuditListResponse {
        version: API_VERSION.to_owned(),
        data: entries,
        count,
    }))
}

// ---------------------------------------------------------------------------
// GET /audit/verify
// ---------------------------------------------------------------------------

/// Verify the integrity of the audit hash chain.
///
/// Returns a JSON response indicating whether the chain is valid and, if not,
/// the first integrity violation found.
///
/// Returns 501 Not Implemented when no audit store is configured.
#[instrument(skip(state))]
pub async fn verify_audit_integrity(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    let store = state
        .audit_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("audit store not configured".to_owned()))?;

    let total = store
        .count()
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    match store.verify_integrity().await {
        Ok(()) => Ok(Json(AuditVerifyResponse {
            version: API_VERSION.to_owned(),
            valid: true,
            entries_checked: total,
            error: None,
        })),
        Err(e) => Ok(Json(AuditVerifyResponse {
            version: API_VERSION.to_owned(),
            valid: false,
            entries_checked: total,
            error: Some(e.to_string()),
        })),
    }
}

// ---------------------------------------------------------------------------
// GET /audit/:id
// ---------------------------------------------------------------------------

/// Get a single audit entry by its UUID.
///
/// Returns 404 if the entry does not exist.
/// Returns 501 Not Implemented when no audit store is configured.
#[instrument(skip(state), fields(audit_id = %id))]
pub async fn get_audit_entry(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let store = state
        .audit_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("audit store not configured".to_owned()))?;

    let audit_id: AuditId = id
        .parse()
        .map_err(|_| ApiError::ValidationError(format!("invalid audit entry ID: '{id}'")))?;

    let entry = store
        .get_by_id(audit_id)
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound(format!("audit entry '{id}'")))?;

    Ok(Json(AuditEntryResponse {
        version: API_VERSION.to_owned(),
        data: entry,
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "route tests intentionally fail fast when fixture operations violate expectations"
)]
mod tests {
    use super::*;
    use axum::{routing::get, Router};
    use axum_test::TestServer;
    use polkagent_audit::{
        ActionOutcome, ActorInfo, AuditAction, AuditStore, InMemoryAuditStore, ResourceInfo,
    };
    use std::sync::Arc;

    use crate::state::AppState;
    use polkagent_config::Config;
    use polkagent_event::EventBus;

    // -- Minimal noop EffectStore for test state ----------------------------

    struct NoopEffectStore;

    #[async_trait::async_trait]
    impl polkagent_store_trait::EffectStore for NoopEffectStore {
        async fn propose_intent(
            &self,
            _intent: polkagent_store_trait::StoredIntent,
        ) -> Result<(), polkagent_store_trait::StoreError> {
            Ok(())
        }

        async fn claim_intent(
            &self,
            _worker_id: polkagent_core::WorkerId,
            _lease_duration: std::time::Duration,
        ) -> Result<Option<polkagent_store_trait::StoredIntent>, polkagent_store_trait::StoreError>
        {
            Ok(None)
        }

        async fn claim_intent_by_id(
            &self,
            intent_id: polkagent_core::EffectId,
            _worker_id: polkagent_core::WorkerId,
            _lease_duration: std::time::Duration,
        ) -> Result<polkagent_store_trait::StoredIntent, polkagent_store_trait::StoreError>
        {
            Err(polkagent_store_trait::StoreError::NotFound {
                resource_type: "EffectIntent",
                id: intent_id.to_string(),
            })
        }

        async fn release_claim(
            &self,
            _intent_id: polkagent_core::EffectId,
            _worker_id: polkagent_core::WorkerId,
        ) -> Result<(), polkagent_store_trait::StoreError> {
            Ok(())
        }

        async fn get_intent(
            &self,
            intent_id: polkagent_core::EffectId,
        ) -> Result<polkagent_store_trait::StoredIntent, polkagent_store_trait::StoreError>
        {
            Err(polkagent_store_trait::StoreError::NotFound {
                resource_type: "EffectIntent",
                id: intent_id.to_string(),
            })
        }

        async fn get_by_run(
            &self,
            _run_id: polkagent_core::RunId,
        ) -> Result<Vec<polkagent_store_trait::StoredIntent>, polkagent_store_trait::StoreError>
        {
            Ok(vec![])
        }

        async fn expired_leases(
            &self,
            _cutoff: polkagent_core::Timestamp,
        ) -> Result<Vec<polkagent_store_trait::StoredIntent>, polkagent_store_trait::StoreError>
        {
            Ok(vec![])
        }

        async fn record_attempt_start(
            &self,
            _attempt_id: polkagent_core::EffectAttemptId,
            _intent_id: polkagent_core::EffectId,
            _worker_id: polkagent_core::WorkerId,
            _payload: serde_json::Value,
        ) -> Result<(), polkagent_store_trait::StoreError> {
            Ok(())
        }

        async fn record_outcome(
            &self,
            _outcome: polkagent_store_trait::StoredOutcome,
        ) -> Result<(), polkagent_store_trait::StoreError> {
            Ok(())
        }

        async fn unconsumed_outcomes(
            &self,
            _run_id: polkagent_core::RunId,
        ) -> Result<Vec<polkagent_store_trait::StoredOutcome>, polkagent_store_trait::StoreError>
        {
            Ok(vec![])
        }

        async fn mark_outcomes_consumed(
            &self,
            _outcome_ids: &[polkagent_core::EffectOutcomeId],
        ) -> Result<(), polkagent_store_trait::StoreError> {
            Ok(())
        }

        async fn update_intent_state(
            &self,
            intent_id: polkagent_core::EffectId,
            _new_state: &str,
        ) -> Result<polkagent_store_trait::StoredIntent, polkagent_store_trait::StoreError>
        {
            Err(polkagent_store_trait::StoreError::NotFound {
                resource_type: "EffectIntent",
                id: intent_id.to_string(),
            })
        }
    }

    // -- Test helpers -------------------------------------------------------

    /// Build a test router with an audit store.
    fn audit_router(audit_store: Arc<dyn AuditStore>) -> Router {
        let agents = Arc::new(crate::state::InMemoryAgentStore::new());
        let run_manager = Arc::new(crate::run::InMemoryRunManager::new());
        let effect_store: Arc<dyn polkagent_store_trait::EffectStore> = Arc::new(NoopEffectStore);
        let event_bus = EventBus::with_default_capacity();

        let state = AppState::new(
            Config::default(),
            agents,
            run_manager,
            effect_store,
            event_bus,
        )
        .with_audit_store(audit_store);

        Router::new()
            .route("/audit", get(list_audit_entries))
            .route("/audit/verify", get(verify_audit_integrity))
            .route("/audit/{id}", get(get_audit_entry))
            .with_state(state)
    }

    /// Build a test router without an audit store (to test 501 responses).
    fn audit_router_no_store() -> Router {
        let agents = Arc::new(crate::state::InMemoryAgentStore::new());
        let run_manager = Arc::new(crate::run::InMemoryRunManager::new());
        let effect_store: Arc<dyn polkagent_store_trait::EffectStore> = Arc::new(NoopEffectStore);
        let event_bus = EventBus::with_default_capacity();

        let state = AppState::new(
            Config::default(),
            agents,
            run_manager,
            effect_store,
            event_bus,
        );

        Router::new()
            .route("/audit", get(list_audit_entries))
            .route("/audit/verify", get(verify_audit_integrity))
            .route("/audit/{id}", get(get_audit_entry))
            .with_state(state)
    }

    /// Helper: create an in-memory store and populate it with test entries.
    async fn populated_store() -> Arc<InMemoryAuditStore> {
        let store = Arc::new(InMemoryAuditStore::new());

        store
            .append(polkagent_audit::AuditEntry::new(
                ActorInfo::agent("agent-1"),
                AuditAction::RunStarted,
                ResourceInfo::new("run", "run-1"),
                ActionOutcome::Success,
                serde_json::json!({"trigger": "api"}),
            ))
            .await
            .expect("append");

        store
            .append(polkagent_audit::AuditEntry::new(
                ActorInfo::agent("agent-2"),
                AuditAction::ToolInvoked,
                ResourceInfo::new("tool", "tool-1"),
                ActionOutcome::Success,
                serde_json::Value::Null,
            ))
            .await
            .expect("append");

        store
            .append(polkagent_audit::AuditEntry::new(
                ActorInfo::agent("agent-1"),
                AuditAction::RunCompleted,
                ResourceInfo::new("run", "run-1"),
                ActionOutcome::Success,
                serde_json::Value::Null,
            ))
            .await
            .expect("append");

        store
    }

    // -- Tests --------------------------------------------------------------

    #[tokio::test]
    async fn list_entries_returns_all() {
        let store = populated_store().await;
        let server = TestServer::new(audit_router(store));

        let resp = server.get("/audit").await;
        resp.assert_status_ok();

        let body: AuditListResponse = resp.json();
        assert_eq!(body.version, API_VERSION);
        assert_eq!(body.count, 3);
        assert_eq!(body.data.len(), 3);
    }

    #[tokio::test]
    async fn list_entries_filter_by_actor() {
        let store = populated_store().await;
        let server = TestServer::new(audit_router(store));

        let resp = server.get("/audit?actor=agent-1").await;
        resp.assert_status_ok();

        let body: AuditListResponse = resp.json();
        assert_eq!(body.count, 2);
        assert!(body.data.iter().all(|e| e.actor.id == "agent-1"));
    }

    #[tokio::test]
    async fn list_entries_filter_by_action() {
        let store = populated_store().await;
        let server = TestServer::new(audit_router(store));

        let resp = server.get("/audit?action=tool_invoked").await;
        resp.assert_status_ok();

        let body: AuditListResponse = resp.json();
        assert_eq!(body.count, 1);
        assert_eq!(body.data[0].action, AuditAction::ToolInvoked);
    }

    #[tokio::test]
    async fn list_entries_with_limit() {
        let store = populated_store().await;
        let server = TestServer::new(audit_router(store));

        let resp = server.get("/audit?limit=2").await;
        resp.assert_status_ok();

        let body: AuditListResponse = resp.json();
        assert_eq!(body.count, 2);
        assert_eq!(body.data.len(), 2);
    }

    #[tokio::test]
    async fn list_entries_combined_filters() {
        let store = populated_store().await;
        let server = TestServer::new(audit_router(store));

        let resp = server.get("/audit?actor=agent-1&action=run_started").await;
        resp.assert_status_ok();

        let body: AuditListResponse = resp.json();
        assert_eq!(body.count, 1);
        assert_eq!(body.data[0].actor.id, "agent-1");
        assert_eq!(body.data[0].action, AuditAction::RunStarted);
    }

    #[tokio::test]
    async fn get_by_id_returns_entry() {
        let store = populated_store().await;
        let entries = store.snapshot();
        let target = &entries[1];
        let target_id = target.id.to_string();

        let server = TestServer::new(audit_router(store));

        let resp = server.get(&format!("/audit/{target_id}")).await;
        resp.assert_status_ok();

        let body: AuditEntryResponse = resp.json();
        assert_eq!(body.version, API_VERSION);
        assert_eq!(body.data.id.to_string(), target_id);
        assert_eq!(body.data.action, AuditAction::ToolInvoked);
    }

    #[tokio::test]
    async fn get_by_id_not_found() {
        let store = populated_store().await;
        let server = TestServer::new(audit_router(store));

        let fake_id = uuid::Uuid::now_v7().to_string();
        let resp = server.get(&format!("/audit/{fake_id}")).await;
        resp.assert_status_not_found();
    }

    #[tokio::test]
    async fn get_by_id_invalid_uuid() {
        let store = populated_store().await;
        let server = TestServer::new(audit_router(store));

        let resp = server.get("/audit/not-a-uuid").await;
        resp.assert_status(axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn list_returns_501_when_not_configured() {
        let server = TestServer::new(audit_router_no_store());

        let resp = server.get("/audit").await;
        resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn get_returns_501_when_not_configured() {
        let server = TestServer::new(audit_router_no_store());

        let fake_id = uuid::Uuid::now_v7().to_string();
        let resp = server.get(&format!("/audit/{fake_id}")).await;
        resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn verify_returns_501_when_not_configured() {
        let server = TestServer::new(audit_router_no_store());

        let resp = server.get("/audit/verify").await;
        resp.assert_status(axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn verify_integrity_ok() {
        let store = populated_store().await;
        let server = TestServer::new(audit_router(store));

        let resp = server.get("/audit/verify").await;
        resp.assert_status_ok();

        let body: AuditVerifyResponse = resp.json();
        assert_eq!(body.version, API_VERSION);
        assert!(body.valid);
        assert_eq!(body.entries_checked, 3);
        assert!(body.error.is_none());
    }

    #[tokio::test]
    async fn list_entries_filter_invalid_action() {
        let store = populated_store().await;
        let server = TestServer::new(audit_router(store));

        let resp = server.get("/audit?action=bogus_action").await;
        resp.assert_status(axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn list_entries_empty_store() {
        let store = Arc::new(InMemoryAuditStore::new());
        let server = TestServer::new(audit_router(store));

        let resp = server.get("/audit").await;
        resp.assert_status_ok();

        let body: AuditListResponse = resp.json();
        assert_eq!(body.count, 0);
        assert!(body.data.is_empty());
    }

    #[tokio::test]
    async fn verify_integrity_empty_store() {
        let store = Arc::new(InMemoryAuditStore::new());
        let server = TestServer::new(audit_router(store));

        let resp = server.get("/audit/verify").await;
        resp.assert_status_ok();

        let body: AuditVerifyResponse = resp.json();
        assert!(body.valid);
        assert_eq!(body.entries_checked, 0);
    }
}
