//! `GET /openapi.json` — serve the OpenAPI 3.1 specification as JSON.
//!
//! The YAML spec file is embedded at compile time via `include_str!` so the
//! binary is fully self-contained (no runtime file I/O required). On the
//! first request the YAML is parsed and converted to JSON; the result is
//! cached in a `OnceLock` for all subsequent requests.
//!
//! # Endpoint
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `GET` | `/openapi.json` | [`serve_openapi`] |
//!
//! The endpoint is mounted at the root (without the `/api/v1alpha1` prefix)
//! so it is trivially discoverable and matches common tooling conventions.

use axum::{
    http::{header, StatusCode},
    response::IntoResponse,
};
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Embedded spec
// ---------------------------------------------------------------------------

/// The raw OpenAPI YAML, embedded at compile time.
///
/// The path is relative to the crate root (`polkagent-api/`), which means
/// `../../../../openapi.yaml` resolves to the workspace root `openapi.yaml`.
const OPENAPI_YAML: &str = include_str!("../../../../openapi.yaml");

// ---------------------------------------------------------------------------
// Cached JSON conversion
// ---------------------------------------------------------------------------

/// Lazily computed JSON representation of the OpenAPI spec.
static OPENAPI_JSON: OnceLock<Result<String, String>> = OnceLock::new();

/// Parse the embedded YAML and serialise it as a compact JSON string.
///
/// Errors are stored as `Err(String)` inside the `OnceLock` so that a
/// malformed spec results in a `500` on every request rather than a panic.
fn openapi_json() -> &'static Result<String, String> {
    OPENAPI_JSON.get_or_init(|| {
        let value: serde_json::Value =
            serde_yaml::from_str(OPENAPI_YAML).map_err(|e| e.to_string())?;
        serde_json::to_string_pretty(&value).map_err(|e| e.to_string())
    })
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `GET /openapi.json`
///
/// Returns the OpenAPI 3.1 specification as a JSON document.
/// The YAML is embedded at compile time and converted to JSON on first access.
///
/// This endpoint deliberately has no authentication requirement so that
/// developer tooling (Swagger UI, Redoc, clients) can fetch it without
/// credentials.
pub async fn serve_openapi() -> impl IntoResponse {
    match openapi_json() {
        Ok(json) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            json.clone(),
        )
            .into_response(),
        Err(msg) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [(header::CONTENT_TYPE, "application/json")],
            format!(r#"{{"error":{{"code":"INTERNAL_ERROR","message":"failed to parse OpenAPI spec: {msg}"}}}}"#),
        )
            .into_response(),
    }
}
