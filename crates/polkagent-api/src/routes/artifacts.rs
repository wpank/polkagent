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

/// Map artifact persistence failures without exposing backend or corrupt-row
/// details to HTTP clients. The full error remains available to operators via
/// structured diagnostics.
pub(crate) fn map_artifact_store_error(
    error: polkagent_store_trait::StoreError,
    id: impl std::fmt::Display,
) -> ApiError {
    use polkagent_store_trait::StoreError;
    let id = id.to_string();

    match error {
        StoreError::NotFound { .. } => ApiError::NotFound(format!("artifact {id}")),
        StoreError::IntegrityError { .. } => {
            tracing::warn!(artifact_id = %id, "artifact integrity verification failed");
            ApiError::InternalError("artifact content failed integrity verification".to_owned())
        }
        StoreError::Serialisation { message } => {
            tracing::warn!(artifact_id = %id, error = %message, "invalid artifact projection");
            ApiError::InternalError("stored artifact projection is invalid".to_owned())
        }
        StoreError::ConnectionError { message } => {
            tracing::warn!(artifact_id = %id, error = %message, "artifact store unavailable");
            ApiError::InternalError("artifact store is unavailable".to_owned())
        }
        other => {
            tracing::warn!(artifact_id = %id, error = %other, "artifact store operation failed");
            ApiError::InternalError("artifact store operation failed".to_owned())
        }
    }
}

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

    let summary = store
        .get(id)
        .await
        .map_err(|error| map_artifact_store_error(error, id))?;

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

    let body = store
        .get_body(id)
        .await
        .map_err(|error| map_artifact_store_error(error, id))?;

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
/// Walks the artifact's ancestor graph via `get_lineage` and returns the
/// full provenance chain ordered from oldest ancestor to the requested
/// artifact itself.
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

    // Fetch the requested artifact (also validates that it exists).
    let summary = store
        .get(id)
        .await
        .map_err(|error| map_artifact_store_error(error, id))?;

    // Walk the ancestor lineage (BFS order: nearest parents first).
    let ancestor_ids = store
        .get_lineage(id)
        .await
        .map_err(|error| map_artifact_store_error(error, id))?;

    // Fetch metadata for each ancestor. Errors on individual ancestors are
    // treated as internal errors (the lineage references them, so they
    // should exist).
    let mut ancestor_responses = Vec::with_capacity(ancestor_ids.len());
    for ancestor_id in &ancestor_ids {
        let ancestor = store
            .get(*ancestor_id)
            .await
            .map_err(|error| map_artifact_store_error(error, *ancestor_id))?;
        ancestor_responses.push(ArtifactResponse {
            version: API_VERSION.to_owned(),
            id: ancestor.id.to_string(),
            kind: ancestor.kind,
            algorithm: ancestor.algorithm,
            digest_hex: ancestor.digest_hex,
            classification: ancestor.classification,
            run_id: ancestor.run_id,
            created_at: ancestor.created_at.to_rfc3339(),
        });
    }

    // Build the chain: oldest ancestor first, requested artifact last.
    // `ancestor_responses` is in BFS order (nearest first), so reverse it.
    ancestor_responses.reverse();

    // Append the requested artifact itself at the end.
    ancestor_responses.push(ArtifactResponse {
        version: API_VERSION.to_owned(),
        id: summary.id.to_string(),
        kind: summary.kind,
        algorithm: summary.algorithm,
        digest_hex: summary.digest_hex,
        classification: summary.classification,
        run_id: summary.run_id,
        created_at: summary.created_at.to_rfc3339(),
    });

    Ok(Json(ProvenanceResponse {
        version: API_VERSION.to_owned(),
        chain: ancestor_responses,
    }))
}
