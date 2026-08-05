//! Skill management endpoints.
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `GET` | `/skills` | [`list_skills`] |
//! | `GET` | `/skills/:skill_id` | [`get_skill`] |
//! | `POST` | `/skills/install` | [`install_skill`] |
//! | `POST` | `/skills/:skill_id/uninstall` | [`uninstall_skill`] |
//! | `PUT` | `/skills/:skill_id/config` | [`update_skill_config`] |
//!
//! All endpoints return 501 Not Implemented when no [`SkillRegistry`] has been
//! configured in [`AppState`]. When a registry is configured, all endpoints
//! delegate to the trait methods on [`SkillRegistry`].

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use tracing::instrument;

use crate::{
    dto::{
        InstallSkillRequest, ListSkillsResponse, SkillResponse, UpdateSkillConfigRequest,
        API_VERSION,
    },
    error::ApiError,
    state::AppState,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a `SkillManifest` into a [`SkillResponse`] DTO.
fn manifest_to_response(manifest: &polkagent_skill::SkillManifest) -> SkillResponse {
    let id = manifest
        .id()
        .map(|id| id.to_string())
        .unwrap_or_else(|_| manifest.skill.name.clone());

    SkillResponse {
        version: API_VERSION.to_owned(),
        id,
        name: manifest.skill.name.clone(),
        skill_version: manifest.skill.version.clone(),
        description: manifest.skill.description.clone(),
        required_grants: manifest.capabilities.required_grants.clone(),
        tools: manifest.capabilities.tools.clone(),
        active: true,
    }
}

// ---------------------------------------------------------------------------
// GET /skills
// ---------------------------------------------------------------------------

/// List all loaded skills from the skill registry.
///
/// Returns 501 Not Implemented when no skill registry is configured.
pub async fn list_skills(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let registry = state
        .skill_registry
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("skill registry not configured".to_owned()))?;

    let manifests = registry.list_skills().await;
    let data: Vec<SkillResponse> = manifests.iter().map(manifest_to_response).collect();

    Ok(Json(ListSkillsResponse {
        version: API_VERSION.to_owned(),
        data,
    }))
}

// ---------------------------------------------------------------------------
// GET /skills/:skill_id
// ---------------------------------------------------------------------------

/// Get the manifest and status for a single skill.
///
/// `skill_id` is the skill package name (without version). If multiple
/// versions are loaded, the first match is returned.
///
/// Returns 501 when no registry is configured, 404 when the skill is not found.
#[instrument(skip(state), fields(skill_id = %skill_id))]
pub async fn get_skill(
    State(state): State<AppState>,
    Path(skill_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let registry = state
        .skill_registry
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("skill registry not configured".to_owned()))?;

    let manifest = registry
        .get_skill(&skill_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("skill '{skill_id}'")))?;

    Ok(Json(manifest_to_response(&manifest)))
}

// ---------------------------------------------------------------------------
// POST /skills/install
// ---------------------------------------------------------------------------

/// Install a skill from a local filesystem path.
///
/// Loads the `skill.toml` manifest from the given directory and registers the
/// skill in the in-memory registry. Returns the newly installed skill.
///
/// Returns 501 when no skill registry is configured, 422 on validation or I/O
/// errors, and 409 if the skill is already installed.
#[instrument(skip(state, body), fields(path = %body.path))]
pub async fn install_skill(
    State(state): State<AppState>,
    Json(body): Json<InstallSkillRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let registry = state
        .skill_registry
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("skill registry not configured".to_owned()))?;

    let manifest = registry
        .install_skill(&body.path)
        .await
        .map_err(ApiError::ValidationError)?;

    Ok(Json(manifest_to_response(&manifest)))
}

// ---------------------------------------------------------------------------
// POST /skills/:skill_id/uninstall
// ---------------------------------------------------------------------------

/// Uninstall a skill by name, removing it from the registry.
///
/// Returns 501 when no skill registry is configured, 404 when the skill
/// is not found.
#[instrument(skip(state), fields(skill_id = %skill_id))]
pub async fn uninstall_skill(
    State(state): State<AppState>,
    Path(skill_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let registry = state
        .skill_registry
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("skill registry not configured".to_owned()))?;

    let removed = registry
        .uninstall_skill(&skill_id)
        .await
        .map_err(ApiError::InternalError)?;

    if !removed {
        return Err(ApiError::NotFound(format!("skill '{skill_id}'")));
    }

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// PUT /skills/:skill_id/config
// ---------------------------------------------------------------------------

/// Update the runtime configuration for a skill.
///
/// Merges the supplied key-value pairs into the skill's config map and
/// returns the updated skill. Returns 501 when no registry is configured,
/// 404 when the skill is not found.
#[instrument(skip(state, body), fields(skill_id = %skill_id))]
pub async fn update_skill_config(
    State(state): State<AppState>,
    Path(skill_id): Path<String>,
    Json(body): Json<UpdateSkillConfigRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let registry = state
        .skill_registry
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("skill registry not configured".to_owned()))?;

    let manifest = registry
        .update_skill_config(&skill_id, body.config)
        .await
        .map_err(|e| {
            if e.contains("not found") {
                ApiError::NotFound(format!("skill '{skill_id}'"))
            } else {
                ApiError::ValidationError(e)
            }
        })?;

    Ok(Json(manifest_to_response(&manifest)))
}
