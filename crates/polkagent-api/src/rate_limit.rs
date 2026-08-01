//! Axum middleware for per-client rate limiting.
//!
//! This module bridges [`polkagent_rate_limit`] into the API server as Axum
//! middleware.  Each inbound request is keyed by client IP (or an
//! `X-Api-Key` header when present) and checked against a shared
//! [`KeyedRateLimiter`].
//!
//! # HTTP behaviour
//!
//! - **All responses** include `X-RateLimit-Limit` and `X-RateLimit-Remaining`
//!   headers so that clients can self-throttle.
//! - When a request is rejected the response is `429 Too Many Requests` with a
//!   `Retry-After` header (in seconds).
//!
//! # Configuration
//!
//! Rate limiting is driven by [`polkagent_config::RateLimitConfig`]:
//!
//! | Field | Default | Meaning |
//! |---|---|---|
//! | `enabled` | `true` | Set to `false` to disable rate limiting entirely |
//! | `requests_per_second` | `100` | Steady-state refill rate per client |
//! | `burst` | `200` | Maximum token capacity (allows short spikes) |

use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{HeaderValue, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use polkagent_config::RateLimitConfig;
use polkagent_rate_limit::keyed::{KeyedRateLimiter, TokenBucketFactory};
use serde_json::json;
use tracing::warn;

// ---------------------------------------------------------------------------
// RateLimitState
// ---------------------------------------------------------------------------

/// Shared state for the rate-limit middleware.
///
/// Held inside an `Arc` and injected into each request via Axum extension.
#[derive(Debug, Clone)]
pub struct RateLimitState {
    /// The per-key rate limiter. `None` when rate limiting is disabled.
    limiter: Option<Arc<KeyedRateLimiter<String>>>,
    /// The configured burst capacity (used for the `X-RateLimit-Limit` header).
    limit: u32,
}

impl RateLimitState {
    /// Build from configuration.
    ///
    /// When `config.enabled` is `false` the limiter is `None` and the
    /// middleware becomes a no-op pass-through.
    #[must_use]
    pub fn from_config(config: &RateLimitConfig) -> Self {
        if !config.enabled {
            return Self {
                limiter: None,
                limit: config.burst,
            };
        }

        let factory = TokenBucketFactory {
            capacity: config.burst,
            refill_rate: f64::from(config.requests_per_second),
            refill_interval: Duration::from_secs(1),
        };

        Self {
            limiter: Some(Arc::new(KeyedRateLimiter::new(factory))),
            limit: config.burst,
        }
    }
}

// ---------------------------------------------------------------------------
// Key extraction
// ---------------------------------------------------------------------------

/// Extract a rate-limit key from the request.
///
/// Prefers the `X-Api-Key` header when present; otherwise falls back to the
/// client IP address from `ConnectInfo`.  If neither is available the
/// request is keyed as `"unknown"`.
fn extract_key<B>(req: &Request<B>) -> String {
    // 1. Try X-Api-Key header.
    if let Some(api_key) = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
    {
        if !api_key.is_empty() {
            return format!("apikey:{api_key}");
        }
    }

    // 2. Try ConnectInfo (requires axum::extract::connect_info::ConnectInfo).
    if let Some(connect_info) = req.extensions().get::<ConnectInfo<std::net::SocketAddr>>() {
        return format!("ip:{}", connect_info.0.ip());
    }

    // 3. Try X-Forwarded-For header (common behind reverse proxies).
    if let Some(forwarded) = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
    {
        // Take the first (leftmost) IP, which is the original client.
        if let Some(first_ip) = forwarded.split(',').next() {
            let ip = first_ip.trim();
            if !ip.is_empty() {
                return format!("ip:{ip}");
            }
        }
    }

    "unknown".to_owned()
}

// ---------------------------------------------------------------------------
// Middleware function
// ---------------------------------------------------------------------------

/// Axum middleware that enforces per-client rate limits.
///
/// Install via [`axum::middleware::from_fn_with_state`]:
///
/// ```rust,ignore
/// use axum::middleware;
///
/// let state = RateLimitState::from_config(&config.server.rate_limit);
/// let app = Router::new()
///     .route("/api/v1alpha1/...", get(handler))
///     .layer(middleware::from_fn_with_state(
///         Arc::new(state),
///         rate_limit_middleware,
///     ));
/// ```
pub async fn rate_limit_middleware(
    axum::extract::State(state): axum::extract::State<Arc<RateLimitState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let limiter = match &state.limiter {
        Some(l) => l,
        None => {
            // Rate limiting disabled — pass through.
            return next.run(req).await;
        }
    };

    let key = extract_key(&req);
    let result = limiter.check(&key, 1);

    if result.allowed {
        // Forward to inner handler, then append rate-limit headers.
        let mut response = next.run(req).await;
        let headers = response.headers_mut();
        headers.insert(
            "x-ratelimit-limit",
            HeaderValue::from(state.limit),
        );
        headers.insert(
            "x-ratelimit-remaining",
            HeaderValue::from(result.remaining),
        );
        response
    } else {
        // Compute Retry-After in whole seconds (minimum 1).
        let retry_secs = result
            .retry_after
            .unwrap_or(Duration::from_secs(1))
            .as_secs()
            .max(1);

        warn!(
            key,
            remaining = result.remaining,
            retry_after_secs = retry_secs,
            "rate limit exceeded"
        );

        let body = json!({
            "error": {
                "code": "RATE_LIMIT_EXCEEDED",
                "message": "too many requests",
                "retry_after": retry_secs,
            }
        });

        let mut response = (StatusCode::TOO_MANY_REQUESTS, axum::Json(body)).into_response();
        let headers = response.headers_mut();
        headers.insert(
            "retry-after",
            HeaderValue::from(retry_secs as u32),
        );
        headers.insert(
            "x-ratelimit-limit",
            HeaderValue::from(state.limit),
        );
        headers.insert(
            "x-ratelimit-remaining",
            HeaderValue::from(0u32),
        );
        response
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        middleware,
        routing::get,
        Router,
    };
    use polkagent_config::RateLimitConfig;
    use tower::ServiceExt;

    /// Build a minimal test router with rate limiting applied.
    fn test_router(config: &RateLimitConfig) -> Router {
        let state = Arc::new(RateLimitState::from_config(config));

        Router::new()
            .route("/test", get(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(
                state,
                rate_limit_middleware,
            ))
    }

    /// Helper: send a GET /test with an optional X-Api-Key header.
    fn make_request(api_key: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri("/test").method("GET");
        if let Some(key) = api_key {
            builder = builder.header("x-api-key", key);
        }
        builder.body(Body::empty()).unwrap()
    }

    // -----------------------------------------------------------------------
    // Test 1: requests under the limit succeed with 200
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn requests_under_limit_succeed() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 10,
            burst: 5,
        };
        let app = test_router(&config);

        for _ in 0..5 {
            let resp = app
                .clone()
                .oneshot(make_request(Some("client-a")))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }
    }

    // -----------------------------------------------------------------------
    // Test 2: requests over the limit get 429
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn requests_over_limit_get_429() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 0, // no refill
            burst: 2,
        };
        let app = test_router(&config);

        // First two should succeed.
        let r1 = app
            .clone()
            .oneshot(make_request(Some("over-client")))
            .await
            .unwrap();
        assert_eq!(r1.status(), StatusCode::OK);

        let r2 = app
            .clone()
            .oneshot(make_request(Some("over-client")))
            .await
            .unwrap();
        assert_eq!(r2.status(), StatusCode::OK);

        // Third should be rejected.
        let r3 = app
            .clone()
            .oneshot(make_request(Some("over-client")))
            .await
            .unwrap();
        assert_eq!(r3.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    // -----------------------------------------------------------------------
    // Test 3: 429 response includes Retry-After header
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn retry_after_header_present_on_429() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 0,
            burst: 1,
        };
        let app = test_router(&config);

        // Exhaust the bucket.
        let _ = app
            .clone()
            .oneshot(make_request(Some("retry-client")))
            .await
            .unwrap();

        // Next request should be 429 with Retry-After.
        let resp = app
            .clone()
            .oneshot(make_request(Some("retry-client")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(
            resp.headers().contains_key("retry-after"),
            "429 response must include Retry-After header"
        );
        // The value should be a positive integer.
        let retry_val: u64 = resp
            .headers()
            .get("retry-after")
            .unwrap()
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!(retry_val >= 1, "Retry-After should be at least 1 second");
    }

    // -----------------------------------------------------------------------
    // Test 4: rate limit headers present on all successful responses
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn rate_limit_headers_on_all_responses() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 100,
            burst: 50,
        };
        let app = test_router(&config);

        let resp = app
            .clone()
            .oneshot(make_request(Some("header-client")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // X-RateLimit-Limit should match the configured burst.
        let limit_val: u32 = resp
            .headers()
            .get("x-ratelimit-limit")
            .expect("X-RateLimit-Limit header missing")
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(limit_val, 50);

        // X-RateLimit-Remaining should be present and less than the burst
        // (since we consumed one token).
        let remaining_val: u32 = resp
            .headers()
            .get("x-ratelimit-remaining")
            .expect("X-RateLimit-Remaining header missing")
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!(remaining_val < 50);
    }

    // -----------------------------------------------------------------------
    // Test 5: different clients have independent limits
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn different_clients_independent_limits() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 0,
            burst: 1,
        };
        let app = test_router(&config);

        // Client A exhausts its bucket.
        let r1 = app
            .clone()
            .oneshot(make_request(Some("client-alpha")))
            .await
            .unwrap();
        assert_eq!(r1.status(), StatusCode::OK);
        let r2 = app
            .clone()
            .oneshot(make_request(Some("client-alpha")))
            .await
            .unwrap();
        assert_eq!(r2.status(), StatusCode::TOO_MANY_REQUESTS);

        // Client B should still have its full budget.
        let r3 = app
            .clone()
            .oneshot(make_request(Some("client-beta")))
            .await
            .unwrap();
        assert_eq!(r3.status(), StatusCode::OK);
    }

    // -----------------------------------------------------------------------
    // Test 6: rate limiting can be disabled via config
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn disabled_rate_limiting_passes_all_requests() {
        let config = RateLimitConfig {
            enabled: false,
            requests_per_second: 0,
            burst: 1,
        };
        let app = test_router(&config);

        // Even with burst=1 and no refill, all requests should pass when
        // rate limiting is disabled.
        for _ in 0..10 {
            let resp = app
                .clone()
                .oneshot(make_request(Some("disabled-client")))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }
    }

    // -----------------------------------------------------------------------
    // Test 7: 429 response body contains error JSON
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn rejected_response_body_is_json_error() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 0,
            burst: 1,
        };
        let app = test_router(&config);

        // Exhaust.
        let _ = app
            .clone()
            .oneshot(make_request(Some("body-client")))
            .await
            .unwrap();

        let resp = app
            .clone()
            .oneshot(make_request(Some("body-client")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

        let body_bytes = axum::body::to_bytes(resp.into_body(), 4096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["error"]["code"], "RATE_LIMIT_EXCEEDED");
        assert_eq!(body["error"]["message"], "too many requests");
        assert!(body["error"]["retry_after"].is_number());
    }

    // -----------------------------------------------------------------------
    // Test 8: X-RateLimit-Remaining decrements correctly
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn remaining_decrements_with_each_request() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 0, // no refill
            burst: 5,
        };
        let app = test_router(&config);

        let mut prev_remaining = 5u32;
        for i in 0..5 {
            let resp = app
                .clone()
                .oneshot(make_request(Some("decrement-client")))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "request {i} should succeed");

            let remaining: u32 = resp
                .headers()
                .get("x-ratelimit-remaining")
                .unwrap()
                .to_str()
                .unwrap()
                .parse()
                .unwrap();
            assert!(
                remaining < prev_remaining,
                "remaining should decrease: was {prev_remaining}, got {remaining}"
            );
            prev_remaining = remaining;
        }
    }

    // -----------------------------------------------------------------------
    // Test 9: rate limit headers also present on 429 responses
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn rate_limit_headers_on_429_responses() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 0,
            burst: 1,
        };
        let app = test_router(&config);

        // Exhaust.
        let _ = app
            .clone()
            .oneshot(make_request(Some("headers-429-client")))
            .await
            .unwrap();

        let resp = app
            .clone()
            .oneshot(make_request(Some("headers-429-client")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

        // Both rate limit headers should be present.
        assert!(resp.headers().contains_key("x-ratelimit-limit"));
        assert!(resp.headers().contains_key("x-ratelimit-remaining"));

        let remaining: u32 = resp
            .headers()
            .get("x-ratelimit-remaining")
            .unwrap()
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(remaining, 0, "remaining should be 0 on rejected requests");
    }
}
