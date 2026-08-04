//! Kit lifecycle manager: install, uninstall, and list operations.
//!
//! The manager reads `kit.toml` manifests, validates them, and tracks
//! installed kits and their constituent skills. Kit state is stored
//! externally (e.g. in SQLite via the CLI layer); this module provides
//! the domain logic.

use std::path::{Path, PathBuf};

use tracing::info;

use crate::error::KitError;
use crate::manifest::{KitManifest, SkillRole};

// ---------------------------------------------------------------------------
// InstalledKit — a fully validated and registered kit
// ---------------------------------------------------------------------------

/// Record of a kit that has been validated and installed.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InstalledKit {
    /// The kit name.
    pub name: String,
    /// The kit version.
    pub version: String,
    /// Description from the manifest.
    pub description: String,
    /// Filesystem path the kit was installed from.
    pub path: String,
    /// Names of skills that were registered as part of this kit.
    pub skill_names: Vec<String>,
    /// The serialized manifest content.
    pub manifest_json: String,
}

// ---------------------------------------------------------------------------
// Validation result
// ---------------------------------------------------------------------------

/// Result of validating a kit manifest for installation.
#[derive(Debug)]
pub struct KitValidation {
    /// The parsed manifest.
    pub manifest: KitManifest,
    /// The canonical path to the kit directory or manifest file.
    pub path: PathBuf,
    /// Capabilities that are required but not granted.
    pub denied_capabilities: Vec<String>,
    /// Skills that are required but not available.
    pub missing_skills: Vec<String>,
}

impl KitValidation {
    /// Whether the kit can be installed (no blocking issues).
    pub fn can_install(&self) -> bool {
        self.denied_capabilities.is_empty() && self.missing_skills.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Manager functions
// ---------------------------------------------------------------------------

/// Parse and validate a kit manifest from a local path.
///
/// If `path` is a directory, looks for `kit.toml` inside it. If it is a file,
/// reads it directly.
///
/// `granted_capabilities` is the set of capability strings currently granted
/// to the system (from Cedar policy / config). Skills listed as required are
/// checked against available skill names.
pub fn validate_kit(
    path: &Path,
    granted_capabilities: &[String],
    available_skills: &[String],
) -> Result<KitValidation, KitError> {
    let manifest_path = if path.is_dir() {
        let candidate = path.join("kit.toml");
        if !candidate.exists() {
            return Err(KitError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "No kit.toml found in '{}'. A kit directory must contain a kit.toml manifest.",
                    path.display()
                ),
            )));
        }
        candidate
    } else {
        path.to_path_buf()
    };

    let content = std::fs::read_to_string(&manifest_path)?;
    let manifest = KitManifest::from_toml(&content)?;

    // Check capabilities against grants.
    let denied_capabilities: Vec<String> = manifest
        .capabilities
        .required_grants
        .iter()
        .filter(|cap| !granted_capabilities.contains(cap))
        .cloned()
        .collect();

    // Check required skills against available skills.
    let missing_skills: Vec<String> = manifest
        .required_skills()
        .iter()
        .filter(|(skill_name, _)| !available_skills.contains(skill_name))
        .map(|(skill_name, _)| (*skill_name).clone())
        .collect();

    let canon_path = manifest_path
        .parent()
        .unwrap_or(&manifest_path)
        .to_path_buf();

    Ok(KitValidation {
        manifest,
        path: canon_path,
        denied_capabilities,
        missing_skills,
    })
}

/// Build an [`InstalledKit`] record from a validated manifest.
///
/// This does not persist anything — the caller (CLI layer) is responsible
/// for writing to the database.
pub fn prepare_install(
    manifest: &KitManifest,
    path: &Path,
) -> Result<InstalledKit, KitError> {
    let skill_names: Vec<String> = manifest.skills.keys().cloned().collect();
    let manifest_json = serde_json::to_string(manifest)
        .map_err(|e| KitError::Other(format!("failed to serialize manifest: {e}")))?;

    info!(
        kit = %manifest.kit.name,
        version = %manifest.kit.version,
        skills = ?skill_names,
        "preparing kit install"
    );

    Ok(InstalledKit {
        name: manifest.kit.name.clone(),
        version: manifest.kit.version.clone(),
        description: manifest.kit.description.clone(),
        path: path.to_string_lossy().to_string(),
        skill_names,
        manifest_json,
    })
}

/// Return the list of skill names that should be unregistered when a kit
/// is removed.
///
/// Only skills whose role is not `Optional` are considered owned by the kit.
/// Optional skills may be shared with other kits and are left alone.
pub fn skills_to_unregister(manifest: &KitManifest) -> Vec<String> {
    manifest
        .skills
        .iter()
        .filter(|(_, entry)| entry.role != SkillRole::Optional)
        .map(|(name, _)| name.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_kit_toml(dir: &Path, content: &str) {
        fs::write(dir.join("kit.toml"), content).expect("write");
    }

    const KIT_TOML: &str = r#"
[kit]
name = "test-kit"
version = "0.1.0"
description = "A test kit"

[capabilities]
required_grants = ["chain.query"]

[skills]
skill-a = { version = "^1.0.0", role = "primary" }
skill-b = { version = "^1.0.0", role = "optional" }
"#;

    #[test]
    fn validate_kit_success() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_kit_toml(dir.path(), KIT_TOML);

        let result = validate_kit(
            dir.path(),
            &["chain.query".to_string()],
            &["skill-a".to_string()],
        );

        let validation = result.expect("should validate");
        assert!(validation.can_install());
        assert!(validation.denied_capabilities.is_empty());
        assert!(validation.missing_skills.is_empty());
    }

    #[test]
    fn validate_kit_missing_capability() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_kit_toml(dir.path(), KIT_TOML);

        let result = validate_kit(dir.path(), &[], &["skill-a".to_string()]);
        let validation = result.expect("should validate");
        assert!(!validation.can_install());
        assert_eq!(validation.denied_capabilities, vec!["chain.query"]);
    }

    #[test]
    fn validate_kit_missing_skill() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_kit_toml(dir.path(), KIT_TOML);

        let result = validate_kit(dir.path(), &["chain.query".to_string()], &[]);
        let validation = result.expect("should validate");
        assert!(!validation.can_install());
        assert_eq!(validation.missing_skills, vec!["skill-a"]);
    }

    #[test]
    fn validate_kit_no_manifest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = validate_kit(dir.path(), &[], &[]);
        assert!(result.is_err());
    }

    #[test]
    fn prepare_install_builds_record() {
        let manifest = KitManifest::from_toml(KIT_TOML).expect("parse");
        let installed = prepare_install(&manifest, Path::new("/tmp/test-kit"))
            .expect("should prepare");
        assert_eq!(installed.name, "test-kit");
        assert_eq!(installed.version, "0.1.0");
        assert_eq!(installed.skill_names.len(), 2);
    }

    #[test]
    fn skills_to_unregister_excludes_optional() {
        let manifest = KitManifest::from_toml(KIT_TOML).expect("parse");
        let to_remove = skills_to_unregister(&manifest);
        assert_eq!(to_remove.len(), 1);
        assert!(to_remove.contains(&"skill-a".to_string()));
        assert!(!to_remove.contains(&"skill-b".to_string()));
    }
}
