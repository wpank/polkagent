//! API-level error type that maps domain errors to HTTP responses.
//!
//! Every handler returns `Result<impl IntoResponse, ApiError>`. Axum's
//! `IntoResponse` implementation on `ApiError` serialises the error into the
//! canonical envelope defined in PRD-14 §2.5 and sets the appropriate HTTP
//! status code.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use tracing::warn;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Error codes (SCREAMING_SNAKE_CASE per PRD-14 §2.5)
// ---------------------------------------------------------------------------

/// A stable, machine-readable error code sent in every error response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    AgentNotFound,
    RunNotFound,
    InvalidState,
    ValidationError,
    InternalError,
    NotImplemented,
}

impl ErrorCode {
    fn as_str(self) -> &'static str {
        match self {
            Self::AgentNotFound => "AGENT_NOT_FOUND",
            Self::RunNotFound => "RUN_NOT_FOUND",
            Self::InvalidState => "INVALID_STATE",
            Self::ValidationError => "VALIDATION_ERROR",
            Self::InternalError => "INTERNAL_ERROR",
            Self::NotImplemented => "NOT_IMPLEMENTED",
        }
    }

    fn http_status(self) -> StatusCode {
        match self {
            Self::AgentNotFound | Self::RunNotFound => StatusCode::NOT_FOUND,
            Self::InvalidState => StatusCode::CONFLICT,
            Self::ValidationError => StatusCode::UNPROCESSABLE_ENTITY,
            Self::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
            Self::NotImplemented => StatusCode::NOT_IMPLEMENTED,
        }
    }
}

// ---------------------------------------------------------------------------
// ApiError
// ---------------------------------------------------------------------------

/// The top-level error type for every API handler.
///
/// Variants map to semantic HTTP status codes. All variants carry a human-readable
/// `message` that is safe to surface to API clients (i.e., no internal stack
/// traces or database details).
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The requested agent was not found.
    #[error("agent not found: {0}")]
    AgentNotFound(String),

    /// The requested run was not found.
    #[error("run not found: {0}")]
    RunNotFound(String),

    /// The request would cause an invalid state transition.
    #[error("invalid state: {0}")]
    InvalidState(String),

    /// The request body or query parameters failed validation.
    #[error("validation error: {0}")]
    ValidationError(String),

    /// An unexpected internal error occurred.
    #[error("internal error: {0}")]
    InternalError(String),

    /// The endpoint exists but is not yet implemented.
    #[error("not implemented: {0}")]
    NotImplemented(String),
}

impl ApiError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::AgentNotFound(_) => ErrorCode::AgentNotFound,
            Self::RunNotFound(_) => ErrorCode::RunNotFound,
            Self::InvalidState(_) => ErrorCode::InvalidState,
            Self::ValidationError(_) => ErrorCode::ValidationError,
            Self::InternalError(_) => ErrorCode::InternalError,
            Self::NotImplemented(_) => ErrorCode::NotImplemented,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let code = self.code();
        let status = code.http_status();
        let request_id = Uuid::now_v7().to_string();

        if status.is_server_error() {
            warn!(
                error = %self,
                request_id = %request_id,
                "internal API error"
            );
        }

        let body = Json(json!({
            "error": {
                "code": code.as_str(),
                "message": self.to_string(),
                "request_id": request_id,
                "timestamp": chrono::Utc::now().to_rfc3339(),
            }
        }));

        (status, body).into_response()
    }
}

// ---------------------------------------------------------------------------
// Conversions from domain errors
// ---------------------------------------------------------------------------

impl From<crate::run::RunError> for ApiError {
    fn from(err: crate::run::RunError) -> Self {
        use crate::run::RunError;
        match err {
            RunError::NotFound(id) => Self::RunNotFound(id.to_string()),
            RunError::AgentNotFound(id) => Self::AgentNotFound(id.to_string()),
            RunError::InvalidTransition { id, reason } => {
                Self::InvalidState(format!("run {id}: {reason}"))
            }
            RunError::Internal(msg) => Self::InternalError(msg),
        }
    }
}
