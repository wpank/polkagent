//! Health-check endpoints with full component diagnostics.
//!
//! This module provides four probes for container orchestrators and
//! load-balancers, plus shared [`HealthState`] for tracking component
//! health across the system.
//!
//! | Path | Purpose | Failure status |
//! |---|---|---|
//! | `GET /health` | Full health check with component statuses | 200/503 |
//! | `GET /health/live` | Liveness — always 200 if the process is alive | 200 |
//! | `GET /health/ready` | Readiness — 200 only when all critical deps healthy | 200/503 |
//! | `GET /health/startup` | Startup — 200 after initial setup completes | 200/503 |

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::{
    extract::State as AxumState,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Serialize;
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Top-level health response returned by `GET /health`.
#[derive(Debug, Clone, Serialize)]
pub struct HealthResponse {
    /// Aggregate status: `"ok"`, `"degraded"`, or `"error"`.
    pub status: &'static str,
    /// Crate version compiled into the binary.
    pub version: &'static str,
    /// Seconds elapsed since the server process started.
    pub uptime_seconds: u64,
    /// Per-component health details.
    pub checks: Vec<ComponentHealth>,
}

/// Health status for a single component (database, event bus, etc.).
#[derive(Debug, Clone, Serialize)]
pub struct ComponentHealth {
    /// Human-readable component name (e.g. `"database"`).
    pub name: String,
    /// Component-level status: `"ok"`, `"degraded"`, or `"error"`.
    pub status: &'static str,
    /// Round-trip latency of the probe in milliseconds, if measured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    /// Optional diagnostic message (usually populated on failure).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// HealthState — shared mutable state for the probes
// ---------------------------------------------------------------------------

/// Shared state for all health endpoints.
///
/// Wrap in `Arc<HealthState>` and pass as Axum state so every handler can
/// read (and, where needed, update) the health picture.
pub struct HealthState {
    /// Monotonic clock marking process start.
    pub startup_time: Instant,
    /// Flipped to `true` once one-time initialisation completes.
    pub ready: AtomicBool,
    /// Dynamically updated component health map.
    pub component_status: RwLock<HashMap<String, ComponentHealth>>,
}

impl HealthState {
    /// Create a new `HealthState` with `ready` set to `false`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            startup_time: Instant::now(),
            ready: AtomicBool::new(false),
            component_status: RwLock::new(HashMap::new()),
        }
    }

    /// Create a `HealthState` that is already marked ready.
    ///
    /// Useful for tests and simple deployments that have no async init phase.
    #[must_use]
    pub fn new_ready() -> Self {
        let state = Self::new();
        state.ready.store(true, Ordering::SeqCst);
        state
    }

    /// Mark the server as ready (startup complete).
    pub fn mark_ready(&self) {
        self.ready.store(true, Ordering::SeqCst);
    }

    /// Returns `true` when startup has completed.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    /// Seconds elapsed since the process started.
    #[must_use]
    pub fn uptime_seconds(&self) -> u64 {
        self.startup_time.elapsed().as_secs()
    }

    /// Record the health of a named component.
    pub async fn set_component(&self, health: ComponentHealth) {
        let mut map = self.component_status.write().await;
        map.insert(health.name.clone(), health);
    }

    /// Remove a component from the health map.
    pub async fn remove_component(&self, name: &str) {
        let mut map = self.component_status.write().await;
        map.remove(name);
    }

    /// Snapshot the current component health as a sorted `Vec`.
    pub async fn component_snapshot(&self) -> Vec<ComponentHealth> {
        let map = self.component_status.read().await;
        let mut components: Vec<ComponentHealth> = map.values().cloned().collect();
        components.sort_by(|a, b| a.name.cmp(&b.name));
        components
    }
}

impl Default for HealthState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Component checkers
// ---------------------------------------------------------------------------

/// Probe a database connection by running `SELECT 1`.
///
/// `query_fn` is an async closure that executes the probe; abstracting it
/// this way allows callers to inject any pool type without coupling this
/// module to a concrete database driver.
pub async fn check_database<F, Fut>(query_fn: F) -> ComponentHealth
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let start = Instant::now();
    match query_fn().await {
        Ok(()) => ComponentHealth {
            name: "database".to_owned(),
            status: "ok",
            latency_ms: Some(start.elapsed().as_millis() as u64),
            message: None,
        },
        Err(err) => ComponentHealth {
            name: "database".to_owned(),
            status: "error",
            latency_ms: Some(start.elapsed().as_millis() as u64),
            message: Some(err),
        },
    }
}

/// Check whether the in-process event bus is operational.
///
/// The check verifies that the bus exists and has non-zero capacity. In
/// production the bus is always available once constructed; this check
/// exists to give a uniform `ComponentHealth` report.
pub fn check_event_bus(capacity: usize) -> ComponentHealth {
    if capacity > 0 {
        ComponentHealth {
            name: "event_bus".to_owned(),
            status: "ok",
            latency_ms: Some(0),
            message: None,
        }
    } else {
        ComponentHealth {
            name: "event_bus".to_owned(),
            status: "error",
            latency_ms: Some(0),
            message: Some("event bus capacity is zero".to_owned()),
        }
    }
}

/// Check whether the memory store subsystem is reachable.
///
/// Accepts `configured` — `true` if a memory store was injected into the
/// app state. The memory store is optional, so being unconfigured is
/// reported as `"degraded"` rather than `"error"`.
pub fn check_memory_store(configured: bool) -> ComponentHealth {
    if configured {
        ComponentHealth {
            name: "memory_store".to_owned(),
            status: "ok",
            latency_ms: Some(0),
            message: None,
        }
    } else {
        ComponentHealth {
            name: "memory_store".to_owned(),
            status: "degraded",
            latency_ms: None,
            message: Some("memory store not configured".to_owned()),
        }
    }
}

// ---------------------------------------------------------------------------
// Endpoint handlers
// ---------------------------------------------------------------------------

/// `GET /health`
///
/// Full health report: aggregate status, version, uptime, and per-component
/// checks. Returns 200 when aggregate is `"ok"` or `"degraded"`, 503 when
/// any component reports `"error"`.
pub async fn health(
    AxumState(state): AxumState<Arc<HealthState>>,
) -> impl IntoResponse {
    let checks = state.component_snapshot().await;
    let status = aggregate_status(&checks);
    let code = if status == "error" {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    };

    let body = HealthResponse {
        status,
        version: env!("CARGO_PKG_VERSION"),
        uptime_seconds: state.uptime_seconds(),
        checks,
    };

    (code, Json(body))
}

/// `GET /health/live`
///
/// Always returns 200 OK. If this endpoint fails, the process is dead.
pub async fn liveness() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(serde_json::json!({ "status": "ok" })),
    )
}

/// `GET /health/ready`
///
/// Returns 200 when all critical components are healthy (no `"error"`
/// entries) and `ready` is `true`. Returns 503 otherwise.
pub async fn readiness(
    AxumState(state): AxumState<Arc<HealthState>>,
) -> impl IntoResponse {
    let checks = state.component_snapshot().await;
    let all_ok = checks.iter().all(|c| c.status != "error");
    let ready = state.is_ready() && all_ok;

    let (code, status) = if ready {
        (StatusCode::OK, "ok")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "unavailable")
    };

    (
        code,
        Json(serde_json::json!({
            "status": status,
            "ready": ready,
            "checks": checks,
        })),
    )
}

/// `GET /health/startup`
///
/// Returns 200 once `HealthState::ready` has been set to `true` (i.e.,
/// initial setup is complete). Returns 503 while the server is still
/// initialising.
pub async fn startup(
    AxumState(state): AxumState<Arc<HealthState>>,
) -> impl IntoResponse {
    if state.is_ready() {
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "ok",
                "detail": "initialisation complete",
            })),
        )
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "starting",
                "detail": "initialisation in progress",
            })),
        )
    }
}

// ---------------------------------------------------------------------------
// Router factory
// ---------------------------------------------------------------------------

/// Build the health sub-router.
///
/// Mount this at the root of the application so that probes are reachable
/// without the versioned API prefix:
///
/// ```text
/// GET /health
/// GET /health/live
/// GET /health/ready
/// GET /health/startup
/// ```
#[must_use]
pub fn health_router(state: Arc<HealthState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/health/live", get(liveness))
        .route("/health/ready", get(readiness))
        .route("/health/startup", get(startup))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Derive an aggregate status from a list of component checks.
///
/// - All `"ok"` -> `"ok"`
/// - Any `"degraded"` (but no `"error"`) -> `"degraded"`
/// - Any `"error"` -> `"error"`
fn aggregate_status(checks: &[ComponentHealth]) -> &'static str {
    if checks.is_empty() {
        return "ok";
    }

    let has_error = checks.iter().any(|c| c.status == "error");
    let has_degraded = checks.iter().any(|c| c.status == "degraded");

    if has_error {
        "error"
    } else if has_degraded {
        "degraded"
    } else {
        "ok"
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt; // for `oneshot`

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn test_state_ready() -> Arc<HealthState> {
        Arc::new(HealthState::new_ready())
    }

    fn test_state_not_ready() -> Arc<HealthState> {
        Arc::new(HealthState::new())
    }

    fn test_router(state: Arc<HealthState>) -> Router {
        health_router(state)
    }

    async fn get_json(
        router: &Router,
        uri: &str,
    ) -> (StatusCode, serde_json::Value) {
        let req = Request::builder()
            .uri(uri)
            .body(Body::empty())
            .unwrap();

        let response = router.clone().oneshot(req).await.unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 1_048_576)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        (status, json)
    }

    // -----------------------------------------------------------------------
    // 1. Liveness always returns 200
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn liveness_returns_200() {
        let state = test_state_ready();
        let router = test_router(state);
        let (status, body) = get_json(&router, "/health/live").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn liveness_returns_200_even_when_not_ready() {
        let state = test_state_not_ready();
        let router = test_router(state);
        let (status, body) = get_json(&router, "/health/live").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
    }

    // -----------------------------------------------------------------------
    // 2. Readiness probe
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn readiness_ok_when_ready_and_no_errors() {
        let state = test_state_ready();
        state.set_component(ComponentHealth {
            name: "database".to_owned(),
            status: "ok",
            latency_ms: Some(1),
            message: None,
        }).await;

        let router = test_router(state);
        let (status, body) = get_json(&router, "/health/ready").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        assert_eq!(body["ready"], true);
    }

    #[tokio::test]
    async fn readiness_503_when_not_ready() {
        let state = test_state_not_ready();
        let router = test_router(state);
        let (status, body) = get_json(&router, "/health/ready").await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["status"], "unavailable");
        assert_eq!(body["ready"], false);
    }

    #[tokio::test]
    async fn readiness_503_when_component_error() {
        let state = test_state_ready();
        state.set_component(ComponentHealth {
            name: "database".to_owned(),
            status: "error",
            latency_ms: None,
            message: Some("connection refused".to_owned()),
        }).await;

        let router = test_router(state);
        let (status, body) = get_json(&router, "/health/ready").await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["ready"], false);
    }

    #[tokio::test]
    async fn readiness_ok_with_degraded_component() {
        let state = test_state_ready();
        state.set_component(ComponentHealth {
            name: "memory_store".to_owned(),
            status: "degraded",
            latency_ms: None,
            message: Some("not configured".to_owned()),
        }).await;

        let router = test_router(state);
        let (status, body) = get_json(&router, "/health/ready").await;

        // Degraded is not an error, so readiness should still be 200.
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ready"], true);
    }

    // -----------------------------------------------------------------------
    // 3. Startup probe
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn startup_503_before_ready() {
        let state = test_state_not_ready();
        let router = test_router(state);
        let (status, body) = get_json(&router, "/health/startup").await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["status"], "starting");
    }

    #[tokio::test]
    async fn startup_200_after_ready() {
        let state = test_state_not_ready();
        state.mark_ready();
        let router = test_router(state);
        let (status, body) = get_json(&router, "/health/startup").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        assert_eq!(body["detail"], "initialisation complete");
    }

    // -----------------------------------------------------------------------
    // 4. Full health endpoint
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn health_ok_with_no_components() {
        let state = test_state_ready();
        let router = test_router(state);
        let (status, body) = get_json(&router, "/health").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        assert!(body["checks"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn health_includes_version() {
        let state = test_state_ready();
        let router = test_router(state);
        let (_, body) = get_json(&router, "/health").await;

        assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn health_200_with_all_ok_components() {
        let state = test_state_ready();
        state.set_component(ComponentHealth {
            name: "database".to_owned(),
            status: "ok",
            latency_ms: Some(2),
            message: None,
        }).await;
        state.set_component(ComponentHealth {
            name: "event_bus".to_owned(),
            status: "ok",
            latency_ms: Some(0),
            message: None,
        }).await;

        let router = test_router(state);
        let (status, body) = get_json(&router, "/health").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        assert_eq!(body["checks"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn health_degraded_with_degraded_component() {
        let state = test_state_ready();
        state.set_component(ComponentHealth {
            name: "database".to_owned(),
            status: "ok",
            latency_ms: Some(1),
            message: None,
        }).await;
        state.set_component(ComponentHealth {
            name: "memory_store".to_owned(),
            status: "degraded",
            latency_ms: None,
            message: Some("not configured".to_owned()),
        }).await;

        let router = test_router(state);
        let (status, body) = get_json(&router, "/health").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "degraded");
    }

    #[tokio::test]
    async fn health_503_with_error_component() {
        let state = test_state_ready();
        state.set_component(ComponentHealth {
            name: "database".to_owned(),
            status: "error",
            latency_ms: Some(500),
            message: Some("connection timeout".to_owned()),
        }).await;

        let router = test_router(state);
        let (status, body) = get_json(&router, "/health").await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["status"], "error");
    }

    #[tokio::test]
    async fn health_error_overrides_degraded() {
        let state = test_state_ready();
        state.set_component(ComponentHealth {
            name: "memory_store".to_owned(),
            status: "degraded",
            latency_ms: None,
            message: None,
        }).await;
        state.set_component(ComponentHealth {
            name: "database".to_owned(),
            status: "error",
            latency_ms: None,
            message: Some("down".to_owned()),
        }).await;

        let router = test_router(state);
        let (_, body) = get_json(&router, "/health").await;

        assert_eq!(body["status"], "error");
    }

    // -----------------------------------------------------------------------
    // 5. Uptime calculation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn uptime_is_zero_or_low_on_fresh_state() {
        let state = test_state_ready();
        let router = test_router(state);
        let (_, body) = get_json(&router, "/health").await;

        let uptime = body["uptime_seconds"].as_u64().unwrap();
        // Should be 0 or 1 given how fast the test runs.
        assert!(uptime <= 1, "uptime should be near zero, got {uptime}");
    }

    #[tokio::test]
    async fn uptime_advances_with_time() {
        let state = Arc::new(HealthState {
            startup_time: Instant::now() - std::time::Duration::from_secs(42),
            ready: AtomicBool::new(true),
            component_status: RwLock::new(HashMap::new()),
        });

        let router = test_router(state);
        let (_, body) = get_json(&router, "/health").await;

        let uptime = body["uptime_seconds"].as_u64().unwrap();
        assert!(uptime >= 42, "uptime should be >= 42, got {uptime}");
    }

    // -----------------------------------------------------------------------
    // 6. HealthState unit tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn state_new_starts_not_ready() {
        let state = HealthState::new();
        assert!(!state.is_ready());
    }

    #[tokio::test]
    async fn state_new_ready_starts_ready() {
        let state = HealthState::new_ready();
        assert!(state.is_ready());
    }

    #[tokio::test]
    async fn state_mark_ready_transitions() {
        let state = HealthState::new();
        assert!(!state.is_ready());
        state.mark_ready();
        assert!(state.is_ready());
    }

    #[tokio::test]
    async fn state_set_and_snapshot_component() {
        let state = HealthState::new();
        state.set_component(ComponentHealth {
            name: "alpha".to_owned(),
            status: "ok",
            latency_ms: Some(5),
            message: None,
        }).await;

        let snap = state.component_snapshot().await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].name, "alpha");
        assert_eq!(snap[0].status, "ok");
    }

    #[tokio::test]
    async fn state_remove_component() {
        let state = HealthState::new();
        state.set_component(ComponentHealth {
            name: "temp".to_owned(),
            status: "ok",
            latency_ms: None,
            message: None,
        }).await;
        assert_eq!(state.component_snapshot().await.len(), 1);

        state.remove_component("temp").await;
        assert!(state.component_snapshot().await.is_empty());
    }

    #[tokio::test]
    async fn state_snapshot_sorted_by_name() {
        let state = HealthState::new();
        state.set_component(ComponentHealth {
            name: "zebra".to_owned(),
            status: "ok",
            latency_ms: None,
            message: None,
        }).await;
        state.set_component(ComponentHealth {
            name: "alpha".to_owned(),
            status: "ok",
            latency_ms: None,
            message: None,
        }).await;

        let snap = state.component_snapshot().await;
        assert_eq!(snap[0].name, "alpha");
        assert_eq!(snap[1].name, "zebra");
    }

    // -----------------------------------------------------------------------
    // 7. Component checkers
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn check_database_ok() {
        let result = check_database(|| async { Ok(()) }).await;
        assert_eq!(result.name, "database");
        assert_eq!(result.status, "ok");
        assert!(result.latency_ms.is_some());
        assert!(result.message.is_none());
    }

    #[tokio::test]
    async fn check_database_error() {
        let result =
            check_database(|| async { Err("connection refused".to_owned()) }).await;
        assert_eq!(result.name, "database");
        assert_eq!(result.status, "error");
        assert!(result.latency_ms.is_some());
        assert_eq!(result.message.as_deref(), Some("connection refused"));
    }

    #[test]
    fn check_event_bus_ok() {
        let result = check_event_bus(1024);
        assert_eq!(result.status, "ok");
        assert_eq!(result.name, "event_bus");
    }

    #[test]
    fn check_event_bus_zero_capacity() {
        let result = check_event_bus(0);
        assert_eq!(result.status, "error");
        assert!(result.message.is_some());
    }

    #[test]
    fn check_memory_store_configured() {
        let result = check_memory_store(true);
        assert_eq!(result.status, "ok");
        assert_eq!(result.name, "memory_store");
    }

    #[test]
    fn check_memory_store_not_configured() {
        let result = check_memory_store(false);
        assert_eq!(result.status, "degraded");
        assert!(result.message.is_some());
    }

    // -----------------------------------------------------------------------
    // 8. aggregate_status logic
    // -----------------------------------------------------------------------

    #[test]
    fn aggregate_empty_is_ok() {
        assert_eq!(aggregate_status(&[]), "ok");
    }

    #[test]
    fn aggregate_all_ok() {
        let checks = vec![
            ComponentHealth {
                name: "a".to_owned(),
                status: "ok",
                latency_ms: None,
                message: None,
            },
            ComponentHealth {
                name: "b".to_owned(),
                status: "ok",
                latency_ms: None,
                message: None,
            },
        ];
        assert_eq!(aggregate_status(&checks), "ok");
    }

    #[test]
    fn aggregate_degraded() {
        let checks = vec![
            ComponentHealth {
                name: "a".to_owned(),
                status: "ok",
                latency_ms: None,
                message: None,
            },
            ComponentHealth {
                name: "b".to_owned(),
                status: "degraded",
                latency_ms: None,
                message: None,
            },
        ];
        assert_eq!(aggregate_status(&checks), "degraded");
    }

    #[test]
    fn aggregate_error_takes_precedence() {
        let checks = vec![
            ComponentHealth {
                name: "a".to_owned(),
                status: "degraded",
                latency_ms: None,
                message: None,
            },
            ComponentHealth {
                name: "b".to_owned(),
                status: "error",
                latency_ms: None,
                message: None,
            },
        ];
        assert_eq!(aggregate_status(&checks), "error");
    }

    // -----------------------------------------------------------------------
    // 9. Concurrent access to HealthState
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn concurrent_component_updates() {
        let state = Arc::new(HealthState::new());
        let mut handles = Vec::new();

        for i in 0..20 {
            let s = Arc::clone(&state);
            handles.push(tokio::spawn(async move {
                s.set_component(ComponentHealth {
                    name: format!("component_{i}"),
                    status: "ok",
                    latency_ms: Some(i),
                    message: None,
                }).await;
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        let snap = state.component_snapshot().await;
        assert_eq!(snap.len(), 20);
    }

    #[tokio::test]
    async fn concurrent_reads_and_writes() {
        let state = Arc::new(HealthState::new_ready());

        // Seed one component.
        state.set_component(ComponentHealth {
            name: "db".to_owned(),
            status: "ok",
            latency_ms: Some(1),
            message: None,
        }).await;

        let mut handles = Vec::new();

        // 10 writers.
        for i in 0..10 {
            let s = Arc::clone(&state);
            handles.push(tokio::spawn(async move {
                s.set_component(ComponentHealth {
                    name: format!("writer_{i}"),
                    status: "ok",
                    latency_ms: Some(i),
                    message: None,
                }).await;
            }));
        }

        // 10 readers.
        for _ in 0..10 {
            let s = Arc::clone(&state);
            handles.push(tokio::spawn(async move {
                let _snap = s.component_snapshot().await;
                // Should never panic; verifies no deadlock.
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        // All 10 writers + 1 seed.
        let snap = state.component_snapshot().await;
        assert_eq!(snap.len(), 11);
    }

    // -----------------------------------------------------------------------
    // 10. Serialization
    // -----------------------------------------------------------------------

    #[test]
    fn health_response_serializes() {
        let resp = HealthResponse {
            status: "ok",
            version: "0.1.0",
            uptime_seconds: 100,
            checks: vec![ComponentHealth {
                name: "db".to_owned(),
                status: "ok",
                latency_ms: Some(5),
                message: None,
            }],
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["version"], "0.1.0");
        assert_eq!(json["uptime_seconds"], 100);
        assert_eq!(json["checks"][0]["name"], "db");
        // `message` should be omitted (skip_serializing_if).
        assert!(json["checks"][0].get("message").is_none());
    }

    #[test]
    fn component_health_includes_optional_fields_when_present() {
        let component = ComponentHealth {
            name: "db".to_owned(),
            status: "error",
            latency_ms: Some(250),
            message: Some("timeout".to_owned()),
        };

        let json = serde_json::to_value(&component).unwrap();
        assert_eq!(json["latency_ms"], 250);
        assert_eq!(json["message"], "timeout");
    }

    #[test]
    fn component_health_omits_none_fields() {
        let component = ComponentHealth {
            name: "bus".to_owned(),
            status: "ok",
            latency_ms: None,
            message: None,
        };

        let json = serde_json::to_value(&component).unwrap();
        assert!(json.get("latency_ms").is_none());
        assert!(json.get("message").is_none());
    }
}
