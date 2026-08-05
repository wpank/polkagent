//! Prometheus metrics exposition endpoint.
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `GET` | `/metrics` | [`prometheus_metrics`] |
//!
//! The endpoint renders all registered metric families from the
//! [`polkagent_telemetry::PrometheusRegistry`] in the standard Prometheus text exposition format.
//! It is intended to be scraped by Prometheus, Grafana Agent, or any other
//! OpenMetrics-compatible collector.

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::IntoResponse,
};

use crate::state::AppState;

/// Prometheus content type header value.
///
/// The Prometheus specification requires `text/plain` with `version=0.0.4`
/// and `charset=utf-8`.
const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

// ---------------------------------------------------------------------------
// GET /metrics
// ---------------------------------------------------------------------------

/// Render all Prometheus metrics in text exposition format.
///
/// Returns a `200 OK` response with `Content-Type: text/plain; version=0.0.4;
/// charset=utf-8`. The body is the full rendering of every metric family
/// registered in the [`polkagent_telemetry::PrometheusRegistry`] attached to `AppState`.
pub async fn prometheus_metrics(State(state): State<AppState>) -> impl IntoResponse {
    let body = state.prometheus.render();

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, PROMETHEUS_CONTENT_TYPE)],
        body,
    )
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "metrics tests intentionally fail fast when requests or response bodies are malformed"
)]
mod tests {
    use super::*;

    use std::sync::Arc;
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt; // for `oneshot`

    use polkagent_config::Config;
    use polkagent_event::EventBus;
    use polkagent_telemetry::prometheus::Label;
    use polkagent_telemetry::PrometheusRegistry;

    use crate::run::InMemoryRunManager;
    use crate::state::InMemoryAgentStore;

    // -- Minimal EffectStore for tests --------------------------------------

    use polkagent_core::{EffectAttemptId, EffectId, EffectOutcomeId, RunId, Timestamp, WorkerId};
    use polkagent_store_trait::{EffectStore, StoreError, StoredIntent, StoredOutcome};

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

        async fn update_intent_state(
            &self,
            intent_id: EffectId,
            _new_state: &str,
        ) -> Result<StoredIntent, StoreError> {
            Err(StoreError::NotFound {
                resource_type: "EffectIntent",
                id: intent_id.to_string(),
            })
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

    // -- Helpers ------------------------------------------------------------

    fn test_state() -> AppState {
        AppState::new(
            Config::default(),
            Arc::new(InMemoryAgentStore::new()),
            Arc::new(InMemoryRunManager::new()),
            Arc::new(NoopEffectStore),
            EventBus::with_default_capacity(),
        )
    }

    fn test_state_with_registry(registry: PrometheusRegistry) -> AppState {
        let mut state = test_state();
        state.prometheus = registry;
        state
    }

    fn metrics_router(state: AppState) -> Router {
        Router::new()
            .route("/metrics", get(prometheus_metrics))
            .with_state(state)
    }

    async fn get_response(router: &Router, uri: &str) -> (StatusCode, String, Option<String>) {
        let req = Request::builder()
            .uri(uri)
            .body(Body::empty())
            .expect("failed to build request");

        let response = router.clone().oneshot(req).await.expect("request failed");
        let status = response.status();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap_or("").to_owned());
        let body = axum::body::to_bytes(response.into_body(), 1_048_576)
            .await
            .expect("failed to read body");
        let text = String::from_utf8(body.to_vec()).expect("body is not UTF-8");
        (status, text, content_type)
    }

    // -----------------------------------------------------------------------
    // 1. Metrics endpoint returns 200
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn metrics_returns_200() {
        let state = test_state();
        let router = metrics_router(state);
        let (status, _body, _ct) = get_response(&router, "/metrics").await;

        assert_eq!(status, StatusCode::OK);
    }

    // -----------------------------------------------------------------------
    // 2. Content type is correct
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn metrics_content_type_is_prometheus_format() {
        let state = test_state();
        let router = metrics_router(state);
        let (_status, _body, content_type) = get_response(&router, "/metrics").await;

        assert_eq!(
            content_type.as_deref(),
            Some("text/plain; version=0.0.4; charset=utf-8"),
        );
    }

    // -----------------------------------------------------------------------
    // 3. Output contains expected metric names
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn metrics_output_contains_default_metric_names() {
        let state = test_state();
        // Increment a counter so it shows up in the output.
        state.prometheus.increment("polkagent_runs_total", &[], 1.0);

        let router = metrics_router(state);
        let (_status, body, _ct) = get_response(&router, "/metrics").await;

        // Counters
        assert!(
            body.contains("polkagent_runs_total"),
            "body should contain polkagent_runs_total, got:\n{body}"
        );
        assert!(
            body.contains("polkagent_effects_total"),
            "body should contain polkagent_effects_total"
        );
        assert!(
            body.contains("polkagent_model_tokens_total"),
            "body should contain polkagent_model_tokens_total"
        );

        // Gauges
        assert!(
            body.contains("polkagent_active_runs"),
            "body should contain polkagent_active_runs"
        );

        // Histograms
        assert!(
            body.contains("polkagent_run_duration_seconds"),
            "body should contain polkagent_run_duration_seconds"
        );
    }

    // -----------------------------------------------------------------------
    // 4. Empty metrics returns valid format (HELP/TYPE lines only)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn metrics_empty_registry_returns_valid_format() {
        // Use a completely empty registry (no families at all).
        let registry = PrometheusRegistry::new();
        let state = test_state_with_registry(registry);
        let router = metrics_router(state);
        let (status, body, ct) = get_response(&router, "/metrics").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            ct.as_deref(),
            Some("text/plain; version=0.0.4; charset=utf-8"),
        );
        // An empty registry produces an empty body, which is valid Prometheus text.
        assert!(
            body.is_empty(),
            "empty registry should produce empty body, got: {body}"
        );
    }

    // -----------------------------------------------------------------------
    // 5. Counter increments are reflected in the output
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn metrics_counter_increments_reflected() {
        let state = test_state();
        let labels = [
            Label::new("agent", "alpha"),
            Label::new("status", "completed"),
        ];
        state
            .prometheus
            .increment("polkagent_runs_total", &labels, 5.0);
        state
            .prometheus
            .increment("polkagent_runs_total", &labels, 3.0);

        let router = metrics_router(state);
        let (_status, body, _ct) = get_response(&router, "/metrics").await;

        assert!(
            body.contains("polkagent_runs_total{agent=\"alpha\",status=\"completed\"} 8"),
            "counter should show accumulated value of 8, got:\n{body}"
        );
    }

    // -----------------------------------------------------------------------
    // 6. Gauge values are reflected
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn metrics_gauge_values_reflected() {
        let state = test_state();
        state
            .prometheus
            .set_gauge("polkagent_active_runs", &[], 42.0);

        let router = metrics_router(state);
        let (_status, body, _ct) = get_response(&router, "/metrics").await;

        assert!(
            body.contains("polkagent_active_runs 42"),
            "gauge should show value 42, got:\n{body}"
        );
    }

    // -----------------------------------------------------------------------
    // 7. Histogram observations are reflected
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn metrics_histogram_observations_reflected() {
        let state = test_state();
        state
            .prometheus
            .observe("polkagent_run_duration_seconds", &[], 0.3);
        state
            .prometheus
            .observe("polkagent_run_duration_seconds", &[], 1.5);

        let router = metrics_router(state);
        let (_status, body, _ct) = get_response(&router, "/metrics").await;

        assert!(
            body.contains("polkagent_run_duration_seconds_count 2"),
            "histogram count should be 2, got:\n{body}"
        );
        assert!(
            body.contains("polkagent_run_duration_seconds_sum"),
            "histogram should contain _sum line"
        );
        assert!(
            body.contains("polkagent_run_duration_seconds_bucket"),
            "histogram should contain _bucket lines"
        );
    }

    // -----------------------------------------------------------------------
    // 8. HELP and TYPE lines are present
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn metrics_help_and_type_lines_present() {
        let state = test_state();
        let router = metrics_router(state);
        let (_status, body, _ct) = get_response(&router, "/metrics").await;

        assert!(
            body.contains("# HELP polkagent_runs_total"),
            "body should contain HELP line for polkagent_runs_total"
        );
        assert!(
            body.contains("# TYPE polkagent_runs_total counter"),
            "body should contain TYPE line for polkagent_runs_total"
        );
        assert!(
            body.contains("# TYPE polkagent_active_runs gauge"),
            "body should contain TYPE line for polkagent_active_runs"
        );
        assert!(
            body.contains("# TYPE polkagent_run_duration_seconds histogram"),
            "body should contain TYPE line for polkagent_run_duration_seconds"
        );
    }
}
