//! Error types for the skill package system.
//!
//! [`SkillError`] covers all failure modes in skill loading, validation,
//! dependency resolution, and execution preparation.

use std::path::PathBuf;

use thiserror::Error;

/// Errors produced by the skill package system.
#[derive(Debug, Error)]
pub enum SkillError {
    /// A skill manifest file could not be parsed as valid TOML.
    #[error("failed to parse manifest: {reason}")]
    ManifestParse {
        /// What went wrong during parsing.
        reason: String,
    },

    /// A skill manifest is syntactically valid TOML but fails semantic
    /// validation (missing required fields, empty name, invalid version, etc.).
    #[error("manifest validation failed for '{skill_name}': {reason}")]
    ManifestValidation {
        /// The skill name (or `"<unknown>"` if name itself is missing).
        skill_name: String,
        /// What validation check failed.
        reason: String,
    },

    /// An I/O error occurred while reading a manifest file or scanning a
    /// skill directory.
    #[error("I/O error at '{}': {source}", path.display())]
    Io {
        /// The filesystem path involved.
        path: PathBuf,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// A skill referenced by name was not found in any search path.
    #[error("skill not found: '{name}'")]
    SkillNotFound {
        /// The name that was looked up.
        name: String,
    },

    /// A cyclic dependency was detected during topological resolution.
    #[error("cyclic dependency detected: {cycle}")]
    CyclicDependency {
        /// A human-readable description of the cycle (e.g. `"a -> b -> a"`).
        cycle: String,
    },

    /// Two skills require incompatible versions of the same dependency.
    #[error("version conflict for '{name}': required '{required}', found '{found}'")]
    VersionConflict {
        /// The dependency name.
        name: String,
        /// The version requirement that was not satisfied.
        required: String,
        /// The version that was available.
        found: String,
    },

    /// A tool listed in the manifest's `[capabilities]` section is not
    /// registered in the tool registry.
    #[error("tool not found in registry: '{tool_name}'")]
    ToolNotFound {
        /// The tool name from the manifest.
        tool_name: String,
    },

    /// A semver version string could not be parsed.
    #[error("invalid version '{version}': {reason}")]
    InvalidVersion {
        /// The version string that failed to parse.
        version: String,
        /// What went wrong.
        reason: String,
    },

    /// A semver version requirement string could not be parsed.
    #[error("invalid version requirement '{requirement}': {reason}")]
    InvalidVersionReq {
        /// The requirement string that failed to parse.
        requirement: String,
        /// What went wrong.
        reason: String,
    },
}

impl SkillError {
    /// Construct a [`SkillError::Io`] from a path and an I/O error.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// Construct a [`SkillError::ManifestValidation`] error.
    pub fn validation(skill_name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::ManifestValidation {
            skill_name: skill_name.into(),
            reason: reason.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parse_error_display() {
        let err = SkillError::ManifestParse {
            reason: "unexpected EOF".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("unexpected EOF"));
    }

    #[test]
    fn manifest_validation_error_display() {
        let err = SkillError::validation("my-skill", "version field is missing");
        let msg = err.to_string();
        assert!(msg.contains("my-skill"));
        assert!(msg.contains("version field is missing"));
    }

    #[test]
    fn io_error_display() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file gone");
        let err = SkillError::io("/some/path", io_err);
        let msg = err.to_string();
        assert!(msg.contains("/some/path"));
        assert!(msg.contains("file gone"));
    }

    #[test]
    fn skill_not_found_display() {
        let err = SkillError::SkillNotFound {
            name: "fancy-skill".into(),
        };
        assert!(err.to_string().contains("fancy-skill"));
    }

    #[test]
    fn cyclic_dependency_display() {
        let err = SkillError::CyclicDependency {
            cycle: "a -> b -> a".into(),
        };
        assert!(err.to_string().contains("a -> b -> a"));
    }

    #[test]
    fn version_conflict_display() {
        let err = SkillError::VersionConflict {
            name: "dep".into(),
            required: ">=2.0.0".into(),
            found: "1.3.0".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("dep"));
        assert!(msg.contains(">=2.0.0"));
        assert!(msg.contains("1.3.0"));
    }

    #[test]
    fn tool_not_found_display() {
        let err = SkillError::ToolNotFound {
            tool_name: "chain_query".into(),
        };
        assert!(err.to_string().contains("chain_query"));
    }

    #[test]
    fn invalid_version_display() {
        let err = SkillError::InvalidVersion {
            version: "not.valid".into(),
            reason: "bad format".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("not.valid"));
        assert!(msg.contains("bad format"));
    }
}
