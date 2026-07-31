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
//! configured in [`AppState`]. When a registry is configured, they delegate to
//! the trait methods.

use axum::{
    extract::{Path, State},
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
pub async fn list_skills(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
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
/// **Stub:** Returns 501 Not Implemented. Will be wired to the skill loader
/// once dynamic installation is supported.
#[instrument(skip(_state, body), fields(path = %body.path))]
pub async fn install_skill(
    State(_state): State<AppState>,
    Json(body): Json<InstallSkillRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let _ = body;
    Err::<Json<()>, _>(ApiError::NotImplemented(
        "skill installation is not yet implemented".to_owned(),
    ))
}

// ---------------------------------------------------------------------------
// POST /skills/:skill_id/uninstall
// ---------------------------------------------------------------------------

/// Uninstall a skill by name.
///
/// **Stub:** Returns 501 Not Implemented. Will be wired to the skill loader
/// once dynamic uninstallation is supported.
#[instrument(skip(_state), fields(skill_id = %skill_id))]
pub async fn uninstall_skill(
    State(_state): State<AppState>,
    Path(skill_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let _ = skill_id;
    Err::<Json<()>, _>(ApiError::NotImplemented(
        "skill uninstall is not yet implemented".to_owned(),
    ))
}

// ---------------------------------------------------------------------------
// PUT /skills/:skill_id/config
// ---------------------------------------------------------------------------

/// Update the runtime configuration for a skill.
///
/// **Stub:** Returns 501 Not Implemented. Will be wired to the skill runtime
/// once per-skill config mutation is supported.
#[instrument(skip(_state, body), fields(skill_id = %skill_id))]
pub async fn update_skill_config(
    State(_state): State<AppState>,
    Path(skill_id): Path<String>,
    Json(body): Json<UpdateSkillConfigRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let _ = (skill_id, body);
    Err::<Json<()>, _>(ApiError::NotImplemented(
        "skill config update is not yet implemented".to_owned(),
    ))
}
