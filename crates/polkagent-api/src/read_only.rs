//! Axum middleware that enforces read-only mode for the API server.
//!
//! When `config.api.read_only` is `true`, the middleware rejects any request
//! whose HTTP method is not `GET`, `HEAD`, or `OPTIONS` with
//! `405 Method Not Allowed` and a JSON error body.
//!
//! # HTTP behaviour
//!
//! - `GET`, `HEAD`, and `OPTIONS` requests are always passed through.
//! - All other methods (`POST`, `PUT`, `PATCH`, `DELETE`, etc.) receive:
//!   - Status: `405 Method Not Allowed`
//!   - Body: `{"error": "server is in read-only mode"}`
//!
//! # Configuration
//!
//! Enabled via [`polkagent_config::ApiConfig`]:
//!
//! ```toml
//! [api]
//! read_only = true
//! ```

use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

// ---------------------------------------------------------------------------
// Middleware function
// ---------------------------------------------------------------------------

/// Axum middleware that blocks non-read-only HTTP methods when `read_only` is
/// enabled.
///
/// Install via [`axum::middleware::from_fn_with_state`]:
///
/// ```rust,ignore
/// use axum::middleware;
///
/// let read_only = config.api.read_only;
/// let app = Router::new()
///     .route("/api/v1alpha1/...", get(handler))
///     .layer(middleware::from_fn_with_state(
///         read_only,
///         read_only_middleware,
///     ));
/// ```
pub async fn read_only_middleware(
    axum::extract::State(read_only): axum::extract::State<bool>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if !read_only {
        // Read-only mode is disabled — pass through unconditionally.
        return next.run(req).await;
    }

    // Allow safe, idempotent methods.
    match *req.method() {
        Method::GET | Method::HEAD | Method::OPTIONS => next.run(req).await,
        _ => {
            let body = json!({"error": "server is in read-only mode"});
            (StatusCode::METHOD_NOT_ALLOWED, Json(body)).into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "test fixtures intentionally fail fast when constructing invalid requests"
)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        middleware,
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    /// Build a minimal test router with the read-only middleware applied.
    fn test_router(read_only: bool) -> Router {
        Router::new()
            .route("/test", get(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(
                read_only,
                read_only_middleware,
            ))
    }

    fn make_request(method: &str) -> Request<Body> {
        Request::builder()
            .uri("/test")
            .method(method)
            .body(Body::empty())
            .unwrap()
    }

    // -----------------------------------------------------------------------
    // Test 1: GET is always allowed in read-only mode
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn get_allowed_in_read_only_mode() {
        let app = test_router(true);
        let resp = app.oneshot(make_request("GET")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // -----------------------------------------------------------------------
    // Test 2: HEAD is allowed in read-only mode
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn head_allowed_in_read_only_mode() {
        let app = test_router(true);
        let resp = app.oneshot(make_request("HEAD")).await.unwrap();
        // HEAD to a GET-only route returns 200 (body stripped) or 405 from
        // Axum's routing layer — either way the read-only middleware passes it.
        // We only care that it did NOT return our custom 405.
        assert_ne!(
            resp.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "HEAD should not be blocked by read-only middleware"
        );
    }

    // -----------------------------------------------------------------------
    // Test 3: OPTIONS is allowed in read-only mode
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn options_allowed_in_read_only_mode() {
        let app = test_router(true);
        let resp = app.oneshot(make_request("OPTIONS")).await.unwrap();
        // Axum's router may return 405 for OPTIONS when no explicit handler
        // exists, but our read-only middleware should NOT be the one rejecting
        // it.  Verify by checking the body does not contain our custom error.
        if resp.status() == StatusCode::METHOD_NOT_ALLOWED {
            let body_bytes = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
            let text = String::from_utf8_lossy(&body_bytes);
            assert!(
                !text.contains("server is in read-only mode"),
                "OPTIONS should not be blocked by read-only middleware; body: {text}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Test 4: POST is rejected in read-only mode
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn post_rejected_in_read_only_mode() {
        let app = test_router(true);
        let resp = app.oneshot(make_request("POST")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    // -----------------------------------------------------------------------
    // Test 5: PUT is rejected in read-only mode
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn put_rejected_in_read_only_mode() {
        let app = test_router(true);
        let resp = app.oneshot(make_request("PUT")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    // -----------------------------------------------------------------------
    // Test 6: DELETE is rejected in read-only mode
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn delete_rejected_in_read_only_mode() {
        let app = test_router(true);
        let resp = app.oneshot(make_request("DELETE")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    // -----------------------------------------------------------------------
    // Test 7: PATCH is rejected in read-only mode
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn patch_rejected_in_read_only_mode() {
        let app = test_router(true);
        let resp = app.oneshot(make_request("PATCH")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    // -----------------------------------------------------------------------
    // Test 8: rejected response body contains the expected JSON
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn rejected_response_body_is_json_error() {
        let app = test_router(true);
        let resp = app.oneshot(make_request("POST")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);

        let body_bytes = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["error"], "server is in read-only mode");
    }

    // -----------------------------------------------------------------------
    // Test 9: POST is allowed when read-only mode is disabled
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn post_allowed_when_read_only_disabled() {
        let app = test_router(false);
        let resp = app.oneshot(make_request("POST")).await.unwrap();
        // Axum will return 405 from its own routing layer (no POST handler
        // registered), but our middleware should not have blocked it.
        // We verify by confirming the body is NOT our custom error.
        let status = resp.status();
        if status == StatusCode::METHOD_NOT_ALLOWED {
            let body_bytes = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
            let text = String::from_utf8_lossy(&body_bytes);
            assert!(
                !text.contains("server is in read-only mode"),
                "middleware must not block POST when read_only=false; body: {text}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Test 10: GET is allowed when read-only mode is disabled
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn get_allowed_when_read_only_disabled() {
        let app = test_router(false);
        let resp = app.oneshot(make_request("GET")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
