//! Artifact REST endpoints.
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `GET` | `/artifacts/:id` | [`get_artifact`] |
//! | `GET` | `/artifacts/:id/content` | [`get_artifact_content`] |
//! | `GET` | `/artifacts/:id/provenance` | [`get_artifact_provenance`] |

use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use polkagent_core::ArtifactId;
use tracing::instrument;

use crate::{
    dto::{ArtifactResponse, ProvenanceResponse, API_VERSION},
    error::ApiError,
    state::AppState,
};

// ---------------------------------------------------------------------------
// GET /artifacts/:id
// ---------------------------------------------------------------------------

/// Get metadata for a single artifact.
///
/// Returns 501 if no artifact store is configured.
/// Returns 404 if the artifact does not exist.
#[instrument(skip(state), fields(artifact_id = %id))]
pub async fn get_artifact(
    State(state): State<AppState>,
    Path(id): Path<ArtifactId>,
) -> Result<impl IntoResponse, ApiError> {
    let store = state
        .artifact_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("artifact store not configured".to_owned()))?;

    let summary = store.get(id).await.map_err(|e| match e {
        polkagent_store_trait::StoreError::NotFound { .. } => {
            ApiError::NotFound(format!("artifact {id}"))
        }
        other => ApiError::InternalError(other.to_string()),
    })?;

    Ok(Json(ArtifactResponse {
        version: API_VERSION.to_owned(),
        id: summary.id.to_string(),
        kind: summary.kind,
        algorithm: summary.algorithm,
        digest_hex: summary.digest_hex,
        classification: summary.classification,
        run_id: summary.run_id,
        created_at: summary.created_at.to_rfc3339(),
    }))
}

// ---------------------------------------------------------------------------
// GET /artifacts/:id/content
// ---------------------------------------------------------------------------

/// Download the raw artifact body as bytes.
///
/// Returns the body with `application/octet-stream` content type.
/// Returns 501 if no artifact store is configured.
/// Returns 404 if the artifact does not exist.
#[instrument(skip(state), fields(artifact_id = %id))]
pub async fn get_artifact_content(
    State(state): State<AppState>,
    Path(id): Path<ArtifactId>,
) -> Result<impl IntoResponse, ApiError> {
    let store = state
        .artifact_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("artifact store not configured".to_owned()))?;

    let body = store.get_body(id).await.map_err(|e| match e {
        polkagent_store_trait::StoreError::NotFound { .. } => {
            ApiError::NotFound(format!("artifact {id}"))
        }
        polkagent_store_trait::StoreError::IntegrityError { .. } => {
            ApiError::InternalError(format!("integrity check failed for artifact {id}"))
        }
        other => ApiError::InternalError(other.to_string()),
    })?;

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        body,
    ))
}

// ---------------------------------------------------------------------------
// GET /artifacts/:id/provenance
// ---------------------------------------------------------------------------

/// Get the lineage chain for an artifact.
///
/// **Stub:** Currently returns a chain containing only the requested artifact.
/// Full provenance traversal will be implemented when the lineage graph is
/// available.
///
/// Returns 501 if no artifact store is configured.
/// Returns 404 if the artifact does not exist.
#[instrument(skip(state), fields(artifact_id = %id))]
pub async fn get_artifact_provenance(
    State(state): State<AppState>,
    Path(id): Path<ArtifactId>,
) -> Result<impl IntoResponse, ApiError> {
    let store = state
        .artifact_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("artifact store not configured".to_owned()))?;

    let summary = store.get(id).await.map_err(|e| match e {
        polkagent_store_trait::StoreError::NotFound { .. } => {
            ApiError::NotFound(format!("artifact {id}"))
        }
        other => ApiError::InternalError(other.to_string()),
    })?;

    // For now, return a single-element chain with just this artifact.
    let artifact_resp = ArtifactResponse {
        version: API_VERSION.to_owned(),
        id: summary.id.to_string(),
        kind: summary.kind,
        algorithm: summary.algorithm,
        digest_hex: summary.digest_hex,
        classification: summary.classification,
        run_id: summary.run_id,
        created_at: summary.created_at.to_rfc3339(),
    };

    Ok(Json(ProvenanceResponse {
        version: API_VERSION.to_owned(),
        chain: vec![artifact_resp],
    }))
}
