//! API authentication middleware.
//!
//! When `config.auth.enabled` is `true`, every request must carry a valid API
//! key as either:
//!
//! - `Authorization: Bearer <token>` header, or
//! - `X-Api-Key: <token>` header.
//!
//! The incoming token is SHA-256 hashed (lowercase hex) and compared against
//! the pre-hashed entries in `config.auth.api_keys`.  An exact match against
//! any entry grants access; otherwise the request is rejected with
//! `401 Unauthorized`.
//!
//! When `config.auth.enabled` is `false` the middleware is a no-op
//! pass-through (suitable for local development).
//!
//! # Hash format
//!
//! API key hashes stored in `config.auth.api_keys` are SHA-256 digests of the
//! plaintext token, encoded as lowercase hexadecimal strings (64 characters).
//! This matches the format produced by:
//!
//! ```text
//! echo -n "my-secret-token" | sha256sum
//! ```

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};
use tracing::warn;

use polkagent_config::AuthConfig;

// ---------------------------------------------------------------------------
// AuthState
// ---------------------------------------------------------------------------

/// Middleware state: holds auth config needed to validate tokens.
#[derive(Debug, Clone)]
pub struct AuthState {
    /// When `false`, the middleware is a no-op.
    pub enabled: bool,
    /// Pre-hashed API keys (SHA-256 hex).
    pub api_key_hashes: Vec<String>,
}

impl AuthState {
    /// Build from the auth section of the platform config.
    #[must_use]
    pub fn from_config(config: &AuthConfig) -> Self {
        Self {
            enabled: config.enabled,
            api_key_hashes: config.api_keys.clone(),
        }
    }

    /// Return `true` if `token` hashes to an entry in `api_key_hashes`.
    pub fn is_valid_token(&self, token: &str) -> bool {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let digest = format!("{:x}", hasher.finalize());
        self.api_key_hashes.iter().any(|h| h == &digest)
    }
}

// ---------------------------------------------------------------------------
// Token extraction
// ---------------------------------------------------------------------------

/// Extract a bearer token or `X-Api-Key` value from the request headers.
///
/// Checks `Authorization: Bearer <token>` first, then `X-Api-Key`.
/// Returns `None` if neither header is present or parseable.
fn extract_token<B>(req: &Request<B>) -> Option<String> {
    // 1. Authorization: Bearer <token>
    if let Some(auth) = req.headers().get("authorization") {
        if let Ok(val) = auth.to_str() {
            if let Some(token) = val.strip_prefix("Bearer ") {
                let token = token.trim();
                if !token.is_empty() {
                    return Some(token.to_owned());
                }
            }
        }
    }

    // 2. X-Api-Key: <token>
    if let Some(key) = req.headers().get("x-api-key") {
        if let Ok(val) = key.to_str() {
            let val = val.trim();
            if !val.is_empty() {
                return Some(val.to_owned());
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Middleware function
// ---------------------------------------------------------------------------

/// Axum middleware that enforces API key authentication.
///
/// When auth is disabled (`config.auth.enabled = false`), all requests pass
/// through unchanged. When enabled, the request must carry a valid API key via
/// `Authorization: Bearer` or `X-Api-Key`; invalid or missing keys produce a
/// `401 Unauthorized` response.
pub async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<AuthState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    // Auth disabled — unconditional pass-through.
    if !state.enabled {
        return next.run(req).await;
    }

    // Auth enabled — require a valid token.
    match extract_token(&req) {
        Some(token) if state.is_valid_token(&token) => next.run(req).await,
        Some(_) => {
            warn!("API request rejected: invalid token");
            unauthorized_response("invalid API key")
        }
        None => {
            warn!("API request rejected: no credentials provided");
            unauthorized_response("missing credentials")
        }
    }
}

/// Build a JSON 401 Unauthorized response.
fn unauthorized_response(reason: &str) -> Response {
    let body = serde_json::json!({
        "error": {
            "code": "UNAUTHORIZED",
            "message": reason,
        }
    });
    (StatusCode::UNAUTHORIZED, axum::Json(body)).into_response()
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
    use polkagent_config::AuthConfig;
    use sha2::{Digest, Sha256};
    use tower::ServiceExt;

    /// Hex-encode the SHA-256 hash of `token`.
    fn sha256_hex(token: &str) -> String {
        let mut h = Sha256::new();
        h.update(token.as_bytes());
        format!("{:x}", h.finalize())
    }

    /// Build a minimal test router with auth middleware applied.
    fn test_router(config: &AuthConfig) -> Router {
        let state = Arc::new(AuthState::from_config(config));
        Router::new()
            .route("/test", get(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(state, auth_middleware))
    }

    fn get_req(auth_header: Option<(&str, &str)>) -> Request<Body> {
        let mut builder = Request::builder().uri("/test").method("GET");
        if let Some((name, value)) = auth_header {
            builder = builder.header(name, value);
        }
        builder.body(Body::empty()).unwrap()
    }

    // ── auth disabled ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn auth_disabled_passes_all_requests() {
        let config = AuthConfig {
            enabled: false,
            api_keys: vec![sha256_hex("secret")],
            ..Default::default()
        };
        let app = test_router(&config);

        // No credentials — should still pass through.
        let resp = app.oneshot(get_req(None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── no credentials ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn auth_enabled_no_credentials_returns_401() {
        let config = AuthConfig {
            enabled: true,
            api_keys: vec![sha256_hex("secret")],
            ..Default::default()
        };
        let app = test_router(&config);

        let resp = app.oneshot(get_req(None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ── bearer token ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn valid_bearer_token_passes() {
        let config = AuthConfig {
            enabled: true,
            api_keys: vec![sha256_hex("my-token")],
            ..Default::default()
        };
        let app = test_router(&config);

        let resp = app
            .oneshot(get_req(Some(("authorization", "Bearer my-token"))))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn invalid_bearer_token_returns_401() {
        let config = AuthConfig {
            enabled: true,
            api_keys: vec![sha256_hex("my-token")],
            ..Default::default()
        };
        let app = test_router(&config);

        let resp = app
            .oneshot(get_req(Some(("authorization", "Bearer wrong-token"))))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ── x-api-key header ────────────────────────────────────────────────────

    #[tokio::test]
    async fn valid_x_api_key_passes() {
        let config = AuthConfig {
            enabled: true,
            api_keys: vec![sha256_hex("my-token")],
            ..Default::default()
        };
        let app = test_router(&config);

        let resp = app
            .oneshot(get_req(Some(("x-api-key", "my-token"))))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn invalid_x_api_key_returns_401() {
        let config = AuthConfig {
            enabled: true,
            api_keys: vec![sha256_hex("my-token")],
            ..Default::default()
        };
        let app = test_router(&config);

        let resp = app
            .oneshot(get_req(Some(("x-api-key", "bad-key"))))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ── multiple valid keys ──────────────────────────────────────────────────

    #[tokio::test]
    async fn multiple_keys_any_valid_passes() {
        let config = AuthConfig {
            enabled: true,
            api_keys: vec![sha256_hex("key-a"), sha256_hex("key-b")],
            ..Default::default()
        };
        let app = test_router(&config);

        let resp_a = app
            .clone()
            .oneshot(get_req(Some(("x-api-key", "key-a"))))
            .await
            .unwrap();
        assert_eq!(resp_a.status(), StatusCode::OK);

        let resp_b = app
            .oneshot(get_req(Some(("x-api-key", "key-b"))))
            .await
            .unwrap();
        assert_eq!(resp_b.status(), StatusCode::OK);
    }

    // ── auth enabled but no keys configured ─────────────────────────────────

    #[tokio::test]
    async fn auth_enabled_no_keys_configured_always_401() {
        let config = AuthConfig {
            enabled: true,
            api_keys: vec![],
            ..Default::default()
        };
        let app = test_router(&config);

        let resp = app
            .oneshot(get_req(Some(("x-api-key", "any-token"))))
            .await
            .unwrap();
        // No configured keys means nothing can match.
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ── is_valid_token ───────────────────────────────────────────────────────

    #[test]
    fn is_valid_token_matches_sha256_hash() {
        let state = AuthState {
            enabled: true,
            api_key_hashes: vec![sha256_hex("correct-horse-battery-staple")],
        };
        assert!(state.is_valid_token("correct-horse-battery-staple"));
        assert!(!state.is_valid_token("wrong-password"));
        assert!(!state.is_valid_token(""));
    }

    #[test]
    fn is_valid_token_empty_haystack_always_false() {
        let state = AuthState {
            enabled: true,
            api_key_hashes: vec![],
        };
        assert!(!state.is_valid_token("any-token"));
    }
}
