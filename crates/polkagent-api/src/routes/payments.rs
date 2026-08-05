//! Payment tracking and budget endpoints.
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `GET` | `/payments/balance` | [`get_balance`] |
//! | `GET` | `/payments/usage` | [`get_usage`] |
//! | `GET` | `/payments/receipts` | [`list_receipts`] |
//! | `GET` | `/payments/receipts/:receipt_id` | [`get_receipt`] |
//!
//! All endpoints return 501 Not Implemented when no [`polkagent_payment::PaymentStore`] has been
//! configured in [`AppState`].

use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Json,
};
use chrono::Utc;
use tracing::instrument;
use uuid::Uuid;

use crate::{
    dto::{
        ListPaymentReceiptsResponse, PaymentBalanceResponse, PaymentReceiptResponse,
        PaymentUsageResponse, API_VERSION,
    },
    error::ApiError,
    state::AppState,
};

/// Convert a [`polkagent_payment::types::PaymentReceipt`] into the API DTO.
fn receipt_to_response(
    receipt: &polkagent_payment::types::PaymentReceipt,
) -> PaymentReceiptResponse {
    PaymentReceiptResponse {
        version: API_VERSION.to_owned(),
        intent_id: receipt.intent_id.to_string(),
        tx_hash: receipt.tx_hash.clone(),
        block_number: receipt.block_number,
        fee_paid: receipt.fee_paid.display_human(),
        confirmed_at: receipt.confirmed_at.to_rfc3339(),
    }
}

// ---------------------------------------------------------------------------
// GET /payments/balance
// ---------------------------------------------------------------------------

/// Get the current agent budget and balance summary.
///
/// Returns 501 Not Implemented when no payment store is configured.
pub async fn get_balance(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let store = state
        .payment_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("payment store not configured".to_owned()))?;

    let summary = store
        .get_balance()
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    Ok(Json(PaymentBalanceResponse {
        version: API_VERSION.to_owned(),
        balance: serde_json::json!({
            "available": summary.available,
            "currency": summary.currency,
            "total_spent": summary.total_spent,
        }),
        budget: serde_json::json!({
            "configured": summary.budget_configured,
            "limit": summary.budget_limit,
        }),
    }))
}

// ---------------------------------------------------------------------------
// GET /payments/usage
// ---------------------------------------------------------------------------

/// Get aggregated usage statistics (token counts, estimated cost).
///
/// Returns 501 Not Implemented when no payment store is configured.
pub async fn get_usage(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let store = state
        .payment_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("payment store not configured".to_owned()))?;

    // Query usage over the last 30 days as a default window.
    let until = Utc::now();
    let since = until - chrono::Duration::days(30);

    let summary = store
        .get_usage("", since, until)
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    Ok(Json(PaymentUsageResponse {
        version: API_VERSION.to_owned(),
        total_runs: summary.total_runs,
        total_tokens: summary.total_tokens,
        estimated_usd: summary.estimated_usd,
        period_start: summary.period_start.to_rfc3339(),
        period_end: summary.period_end.to_rfc3339(),
    }))
}

// ---------------------------------------------------------------------------
// GET /payments/receipts
// ---------------------------------------------------------------------------

/// List all payment receipts.
///
/// Returns 501 Not Implemented when no payment store is configured.
pub async fn list_receipts(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let store = state
        .payment_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("payment store not configured".to_owned()))?;

    let receipts = store
        .list_receipts()
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    let data: Vec<PaymentReceiptResponse> = receipts.iter().map(receipt_to_response).collect();

    Ok(Json(ListPaymentReceiptsResponse {
        version: API_VERSION.to_owned(),
        data,
    }))
}

// ---------------------------------------------------------------------------
// GET /payments/receipts/:receipt_id
// ---------------------------------------------------------------------------

/// Get a single payment receipt by its intent ID.
///
/// Returns 501 Not Implemented when no payment store is configured.
#[instrument(skip(state), fields(receipt_id = %receipt_id))]
pub async fn get_receipt(
    State(state): State<AppState>,
    Path(receipt_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let store = state
        .payment_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("payment store not configured".to_owned()))?;

    let intent_id = receipt_id
        .parse::<Uuid>()
        .map_err(|_| ApiError::ValidationError(format!("invalid UUID: '{receipt_id}'")))?;

    let receipt = store.get_receipt(intent_id).await.map_err(|e| match e {
        polkagent_payment::PaymentError::IntentNotFound { .. } => {
            ApiError::NotFound(format!("receipt '{receipt_id}'"))
        }
        other => ApiError::InternalError(other.to_string()),
    })?;

    Ok(Json(receipt_to_response(&receipt)))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use chrono::{DateTime, Utc};
    use polkagent_config::Config;
    use polkagent_event::EventBus;
    use tokio::sync::RwLock;
    use tower::ServiceExt; // for `oneshot`
    use uuid::Uuid;

    use polkagent_payment::store::BalanceSummary;
    use polkagent_payment::types::{
        Amount, AssetId, CostRecord, PaymentIntent, PaymentReceipt, PaymentStatus, UsageSummary,
    };
    use polkagent_payment::{PaymentError, PaymentStore};

    use crate::run::InMemoryRunManager;
    use crate::state::{AppState, InMemoryAgentStore};

    // -----------------------------------------------------------------------
    // In-memory PaymentStore for testing
    // -----------------------------------------------------------------------

    /// A minimal in-memory PaymentStore for route tests.
    #[derive(Debug, Default)]
    struct MockPaymentStore {
        receipts: RwLock<HashMap<Uuid, PaymentReceipt>>,
        costs: RwLock<Vec<CostRecord>>,
        intents: RwLock<HashMap<Uuid, PaymentIntent>>,
        balance: RwLock<BalanceSummary>,
    }

    impl MockPaymentStore {
        fn new() -> Self {
            Self {
                receipts: RwLock::new(HashMap::new()),
                costs: RwLock::new(Vec::new()),
                intents: RwLock::new(HashMap::new()),
                balance: RwLock::new(BalanceSummary {
                    available: Some(5_000_000_000),
                    currency: "NATIVE".to_owned(),
                    total_spent: 1_500_000,
                    budget_configured: true,
                    budget_limit: Some(10_000_000_000),
                }),
            }
        }
    }

    #[async_trait]
    impl PaymentStore for MockPaymentStore {
        async fn record_cost(&self, cost_record: CostRecord) -> Result<(), PaymentError> {
            self.costs.write().await.push(cost_record);
            Ok(())
        }

        async fn get_costs(&self, run_id: &str) -> Result<Vec<CostRecord>, PaymentError> {
            let costs = self.costs.read().await;
            Ok(costs
                .iter()
                .filter(|c| c.run_id == run_id)
                .cloned()
                .collect())
        }

        async fn get_usage(
            &self,
            _agent_id: &str,
            since: DateTime<Utc>,
            until: DateTime<Utc>,
        ) -> Result<UsageSummary, PaymentError> {
            let costs = self.costs.read().await;
            let total_tokens: u64 = costs.iter().map(|c| c.input_tokens + c.output_tokens).sum();
            let estimated_usd: f64 = costs.iter().map(|c| c.estimated_usd).sum();
            let total_runs = costs
                .iter()
                .map(|c| c.run_id.clone())
                .collect::<std::collections::HashSet<_>>()
                .len() as u64;

            Ok(UsageSummary {
                total_runs,
                total_tokens,
                estimated_usd,
                period_start: since,
                period_end: until,
            })
        }

        async fn create_intent(&self, intent: PaymentIntent) -> Result<(), PaymentError> {
            self.intents.write().await.insert(intent.id, intent);
            Ok(())
        }

        async fn get_intent(&self, id: Uuid) -> Result<PaymentIntent, PaymentError> {
            self.intents
                .read()
                .await
                .get(&id)
                .cloned()
                .ok_or(PaymentError::IntentNotFound { id })
        }

        async fn update_intent_status(
            &self,
            id: Uuid,
            status: PaymentStatus,
        ) -> Result<(), PaymentError> {
            let mut intents = self.intents.write().await;
            let intent = intents
                .get_mut(&id)
                .ok_or(PaymentError::IntentNotFound { id })?;
            intent.status = status;
            Ok(())
        }

        async fn create_receipt(&self, receipt: PaymentReceipt) -> Result<(), PaymentError> {
            self.receipts
                .write()
                .await
                .insert(receipt.intent_id, receipt);
            Ok(())
        }

        async fn list_receipts(&self) -> Result<Vec<PaymentReceipt>, PaymentError> {
            let receipts = self.receipts.read().await;
            let mut list: Vec<PaymentReceipt> = receipts.values().cloned().collect();
            list.sort_by(|a, b| b.confirmed_at.cmp(&a.confirmed_at));
            Ok(list)
        }

        async fn get_receipt(&self, intent_id: Uuid) -> Result<PaymentReceipt, PaymentError> {
            self.receipts
                .read()
                .await
                .get(&intent_id)
                .cloned()
                .ok_or(PaymentError::IntentNotFound { id: intent_id })
        }

        async fn get_balance(&self) -> Result<BalanceSummary, PaymentError> {
            Ok(self.balance.read().await.clone())
        }
    }

    // -----------------------------------------------------------------------
    // Stub EffectStore (to satisfy AppState)
    // -----------------------------------------------------------------------

    /// Stub effect store that satisfies the `EffectStore` trait.
    #[derive(Debug, Default)]
    struct StubEffectStore;

    #[async_trait]
    impl polkagent_store_trait::EffectStore for StubEffectStore {
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
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_test_state(payment_store: Option<Arc<dyn PaymentStore>>) -> AppState {
        let config = Config::default();
        let agents = Arc::new(InMemoryAgentStore::new());
        let run_manager = Arc::new(InMemoryRunManager::new());
        let effect_store: Arc<dyn polkagent_store_trait::EffectStore> = Arc::new(StubEffectStore);
        let event_bus = EventBus::new(128);

        let mut state = AppState::new(config, agents, run_manager, effect_store, event_bus);
        if let Some(ps) = payment_store {
            state = state.with_payment_store(ps);
        }
        state
    }

    fn payment_router(state: AppState) -> Router {
        Router::new()
            .route("/payments/balance", get(get_balance))
            .route("/payments/usage", get(get_usage))
            .route("/payments/receipts", get(list_receipts))
            .route("/payments/receipts/{receipt_id}", get(get_receipt))
            .with_state(state)
    }

    async fn get_json(router: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let req = Request::builder()
            .uri(uri)
            .body(Body::empty())
            .expect("build request");

        let response = router.clone().oneshot(req).await.expect("send request");
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 1_048_576)
            .await
            .expect("read body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("parse JSON");
        (status, json)
    }

    fn make_test_receipt(intent_id: Uuid) -> PaymentReceipt {
        PaymentReceipt {
            intent_id,
            tx_hash: format!("0x{}", hex_stub(intent_id)),
            block_number: 42_000,
            fee_paid: Amount::new(10_000_000, AssetId::Native, 10),
            confirmed_at: Utc::now(),
        }
    }

    fn hex_stub(id: Uuid) -> String {
        id.to_string().replace('-', "")[..16].to_owned()
    }

    // -----------------------------------------------------------------------
    // 1. Balance endpoint: 501 when no store
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn balance_returns_501_without_store() {
        let state = make_test_state(None);
        let router = payment_router(state);
        let (status, body) = get_json(&router, "/payments/balance").await;

        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
    }

    // -----------------------------------------------------------------------
    // 2. Balance endpoint: real data from store
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn balance_returns_real_data_from_store() {
        let store = Arc::new(MockPaymentStore::new());
        let state = make_test_state(Some(store));
        let router = payment_router(state);
        let (status, body) = get_json(&router, "/payments/balance").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["version"], "v1alpha1");
        assert_eq!(body["balance"]["currency"], "NATIVE");
        assert_eq!(body["balance"]["available"], 5_000_000_000u64);
        assert_eq!(body["balance"]["total_spent"], 1_500_000u64);
        assert_eq!(body["budget"]["configured"], true);
        assert_eq!(body["budget"]["limit"], 10_000_000_000u64);
    }

    // -----------------------------------------------------------------------
    // 3. Usage endpoint: 501 without store
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn usage_returns_501_without_store() {
        let state = make_test_state(None);
        let router = payment_router(state);
        let (status, body) = get_json(&router, "/payments/usage").await;

        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
    }

    // -----------------------------------------------------------------------
    // 4. Usage endpoint: returns real data
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn usage_returns_real_aggregated_data() {
        let store = Arc::new(MockPaymentStore::new());

        // Seed some cost records.
        store
            .record_cost(CostRecord {
                run_id: "run-1".to_owned(),
                provider: "anthropic".to_owned(),
                model: "claude-sonnet-4".to_owned(),
                input_tokens: 1000,
                output_tokens: 500,
                estimated_usd: 0.01,
                recorded_at: Utc::now(),
            })
            .await
            .expect("record cost 1");
        store
            .record_cost(CostRecord {
                run_id: "run-2".to_owned(),
                provider: "anthropic".to_owned(),
                model: "claude-sonnet-4".to_owned(),
                input_tokens: 2000,
                output_tokens: 1000,
                estimated_usd: 0.02,
                recorded_at: Utc::now(),
            })
            .await
            .expect("record cost 2");

        let state = make_test_state(Some(store));
        let router = payment_router(state);
        let (status, body) = get_json(&router, "/payments/usage").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["version"], "v1alpha1");
        assert_eq!(body["total_runs"], 2);
        assert_eq!(body["total_tokens"], 4500); // (1000+500) + (2000+1000)
        let usd = body["estimated_usd"].as_f64().expect("float");
        assert!((usd - 0.03).abs() < 1e-9);
        assert!(body["period_start"].as_str().is_some());
        assert!(body["period_end"].as_str().is_some());
    }

    // -----------------------------------------------------------------------
    // 5. List receipts: 501 without store
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn list_receipts_returns_501_without_store() {
        let state = make_test_state(None);
        let router = payment_router(state);
        let (status, body) = get_json(&router, "/payments/receipts").await;

        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
    }

    // -----------------------------------------------------------------------
    // 6. List receipts: returns real data
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn list_receipts_returns_stored_receipts() {
        let store = Arc::new(MockPaymentStore::new());

        let id1 = Uuid::now_v7();
        let id2 = Uuid::now_v7();
        store
            .create_receipt(make_test_receipt(id1))
            .await
            .expect("create receipt 1");
        store
            .create_receipt(make_test_receipt(id2))
            .await
            .expect("create receipt 2");

        let state = make_test_state(Some(store));
        let router = payment_router(state);
        let (status, body) = get_json(&router, "/payments/receipts").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["version"], "v1alpha1");
        let data = body["data"].as_array().expect("data array");
        assert_eq!(data.len(), 2);

        // Each receipt has the expected fields.
        for item in data {
            assert!(item["intent_id"].as_str().is_some());
            assert!(item["tx_hash"].as_str().is_some());
            assert!(item["block_number"].as_u64().is_some());
            assert!(item["fee_paid"].as_str().is_some());
            assert!(item["confirmed_at"].as_str().is_some());
        }
    }

    // -----------------------------------------------------------------------
    // 7. List receipts: empty when no receipts exist
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn list_receipts_returns_empty_when_none() {
        let store = Arc::new(MockPaymentStore::new());
        let state = make_test_state(Some(store));
        let router = payment_router(state);
        let (status, body) = get_json(&router, "/payments/receipts").await;

        assert_eq!(status, StatusCode::OK);
        let data = body["data"].as_array().expect("data array");
        assert!(data.is_empty());
    }

    // -----------------------------------------------------------------------
    // 8. Get receipt: 501 without store
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn get_receipt_returns_501_without_store() {
        let state = make_test_state(None);
        let router = payment_router(state);
        let id = Uuid::now_v7();
        let (status, body) = get_json(&router, &format!("/payments/receipts/{id}")).await;

        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(body["error"]["code"], "NOT_IMPLEMENTED");
    }

    // -----------------------------------------------------------------------
    // 9. Get receipt: returns real data by intent ID
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn get_receipt_returns_receipt_by_intent_id() {
        let store = Arc::new(MockPaymentStore::new());
        let intent_id = Uuid::now_v7();
        store
            .create_receipt(make_test_receipt(intent_id))
            .await
            .expect("create receipt");

        let state = make_test_state(Some(store));
        let router = payment_router(state);
        let (status, body) = get_json(&router, &format!("/payments/receipts/{intent_id}")).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["version"], "v1alpha1");
        assert_eq!(body["intent_id"], intent_id.to_string());
        assert_eq!(body["block_number"], 42_000);
        assert!(body["tx_hash"].as_str().expect("tx_hash").starts_with("0x"));
        assert!(body["fee_paid"].as_str().is_some());
        assert!(body["confirmed_at"].as_str().is_some());
    }

    // -----------------------------------------------------------------------
    // 10. Get receipt: 404 when not found
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn get_receipt_returns_404_when_not_found() {
        let store = Arc::new(MockPaymentStore::new());
        let state = make_test_state(Some(store));
        let router = payment_router(state);
        let missing_id = Uuid::now_v7();
        let (status, body) = get_json(&router, &format!("/payments/receipts/{missing_id}")).await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "NOT_FOUND");
    }

    // -----------------------------------------------------------------------
    // 11. Get receipt: 422 for invalid UUID
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn get_receipt_returns_422_for_invalid_uuid() {
        let store = Arc::new(MockPaymentStore::new());
        let state = make_test_state(Some(store));
        let router = payment_router(state);
        let (status, body) = get_json(&router, "/payments/receipts/not-a-uuid").await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["error"]["code"], "VALIDATION_ERROR");
    }

    // -----------------------------------------------------------------------
    // 12. Usage endpoint: empty store returns zeroes
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn usage_returns_zeroes_for_empty_store() {
        let store = Arc::new(MockPaymentStore::new());
        let state = make_test_state(Some(store));
        let router = payment_router(state);
        let (status, body) = get_json(&router, "/payments/usage").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["total_runs"], 0);
        assert_eq!(body["total_tokens"], 0);
        let usd = body["estimated_usd"].as_f64().expect("float");
        assert!((usd - 0.0).abs() < f64::EPSILON);
    }
}
