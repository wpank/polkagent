//! Error types for the plugin system.
//!
//! [`PluginError`] covers all failure modes in plugin loading, capability
//! validation, dependency resolution, and lifecycle management.

use std::path::PathBuf;

use thiserror::Error;

/// Errors produced by the plugin system.
#[derive(Debug, Error)]
pub enum PluginError {
    /// A plugin manifest file could not be parsed as valid TOML or fails
    /// semantic validation (missing fields, invalid names, bad versions).
    #[error("invalid plugin manifest{}: {reason}", skill_display(.plugin_name))]
    ManifestInvalid {
        /// The plugin name (if available).
        plugin_name: Option<String>,
        /// What went wrong.
        reason: String,
    },

    /// A plugin attempted an operation that its declared capabilities do not
    /// permit.
    #[error("capability denied for plugin '{plugin_name}': {capability} is not granted")]
    CapabilityDenied {
        /// The plugin that attempted the operation.
        plugin_name: String,
        /// The capability that was required but not granted.
        capability: String,
    },

    /// Two plugins declare incompatible version requirements for the same
    /// dependency, or a dependency's available version does not satisfy the
    /// requirement.
    #[error("dependency conflict for '{dependency}': required '{required}', found '{found}'")]
    DependencyConflict {
        /// The dependency name.
        dependency: String,
        /// The version requirement that was not satisfied.
        required: String,
        /// The version that was available.
        found: String,
    },

    /// A plugin referenced by name was not found in the registry or on disk.
    #[error("plugin not found: '{name}'")]
    NotFound {
        /// The plugin name that was looked up.
        name: String,
    },

    /// A plugin could not be loaded from disk (I/O errors, missing files,
    /// corrupt data).
    #[error("failed to load plugin{}: {reason}", path_display(.path))]
    LoadFailed {
        /// The filesystem path involved (if any).
        path: Option<PathBuf>,
        /// What went wrong.
        reason: String,
    },

    /// A cyclic dependency was detected during topological resolution.
    #[error("cyclic dependency detected: {cycle}")]
    CyclicDependency {
        /// A human-readable description of the cycle (e.g. `"a -> b -> a"`).
        cycle: String,
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

    /// A lifecycle operation failed (init, start, stop, health check).
    #[error("lifecycle error for plugin '{plugin_name}': {reason}")]
    LifecycleError {
        /// The plugin involved.
        plugin_name: String,
        /// What went wrong.
        reason: String,
    },

    /// A package's cosign signature is missing or invalid.
    #[error("signature verification failed for package '{package_name}': {reason}")]
    SignatureVerificationFailed {
        /// The package that failed verification.
        package_name: String,
        /// What went wrong.
        reason: String,
    },

    /// A package is unsigned and the policy does not allow unsigned packages.
    #[error("unsigned package rejected: '{package_name}' has no cosign signature")]
    UnsignedPackageRejected {
        /// The package that was rejected.
        package_name: String,
    },

    /// SLSA provenance attestation is missing or invalid.
    #[error("SLSA attestation failed for package '{package_name}': {reason}")]
    SlsaAttestationFailed {
        /// The package that failed attestation.
        package_name: String,
        /// What went wrong.
        reason: String,
    },

    /// A sandboxed operation was denied at the WASM host boundary.
    #[error("sandbox denied operation for plugin '{plugin_name}': {operation}")]
    SandboxDenied {
        /// The plugin that attempted the operation.
        plugin_name: String,
        /// The operation that was denied.
        operation: String,
    },

    /// WASM sandbox resource limits were exceeded.
    #[error("sandbox resource limit exceeded for plugin '{plugin_name}': {resource}")]
    SandboxResourceExhausted {
        /// The plugin that exceeded limits.
        plugin_name: String,
        /// The resource that was exhausted.
        resource: String,
    },
}

/// Helper for optional plugin name display.
fn skill_display(name: &Option<String>) -> String {
    match name {
        Some(n) => format!(" for '{n}'"),
        None => String::new(),
    }
}

/// Helper for optional path display.
fn path_display(path: &Option<PathBuf>) -> String {
    match path {
        Some(p) => format!(" at '{}'", p.display()),
        None => String::new(),
    }
}

impl PluginError {
    /// Construct a [`PluginError::ManifestInvalid`] with a known plugin name.
    pub fn manifest_invalid(plugin_name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::ManifestInvalid {
            plugin_name: Some(plugin_name.into()),
            reason: reason.into(),
        }
    }

    /// Construct a [`PluginError::ManifestInvalid`] without a plugin name.
    pub fn manifest_parse(reason: impl Into<String>) -> Self {
        Self::ManifestInvalid {
            plugin_name: None,
            reason: reason.into(),
        }
    }

    /// Construct a [`PluginError::LoadFailed`] from a path and reason.
    pub fn load_failed(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        Self::LoadFailed {
            path: Some(path.into()),
            reason: reason.into(),
        }
    }

    /// Construct a [`PluginError::LoadFailed`] from an I/O error.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        let path = path.into();
        Self::LoadFailed {
            path: Some(path),
            reason: source.to_string(),
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
    fn manifest_invalid_with_name_display() {
        let err = PluginError::manifest_invalid("my-plugin", "name is empty");
        let msg = err.to_string();
        assert!(msg.contains("my-plugin"));
        assert!(msg.contains("name is empty"));
    }

    #[test]
    fn manifest_parse_display() {
        let err = PluginError::manifest_parse("unexpected EOF");
        let msg = err.to_string();
        assert!(msg.contains("unexpected EOF"));
        assert!(!msg.contains("for '"));
    }

    #[test]
    fn capability_denied_display() {
        let err = PluginError::CapabilityDenied {
            plugin_name: "rogue-plugin".into(),
            capability: "WriteFileSystem".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("rogue-plugin"));
        assert!(msg.contains("WriteFileSystem"));
    }

    #[test]
    fn dependency_conflict_display() {
        let err = PluginError::DependencyConflict {
            dependency: "core-lib".into(),
            required: ">=2.0.0".into(),
            found: "1.5.0".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("core-lib"));
        assert!(msg.contains(">=2.0.0"));
        assert!(msg.contains("1.5.0"));
    }

    #[test]
    fn not_found_display() {
        let err = PluginError::NotFound {
            name: "missing-plugin".into(),
        };
        assert!(err.to_string().contains("missing-plugin"));
    }

    #[test]
    fn load_failed_with_path_display() {
        let err = PluginError::load_failed("/some/path", "file corrupted");
        let msg = err.to_string();
        assert!(msg.contains("/some/path"));
        assert!(msg.contains("file corrupted"));
    }

    #[test]
    fn io_error_display() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file gone");
        let err = PluginError::io("/bad/path", io_err);
        let msg = err.to_string();
        assert!(msg.contains("/bad/path"));
        assert!(msg.contains("file gone"));
    }

    #[test]
    fn cyclic_dependency_display() {
        let err = PluginError::CyclicDependency {
            cycle: "a -> b -> a".into(),
        };
        assert!(err.to_string().contains("a -> b -> a"));
    }

    #[test]
    fn lifecycle_error_display() {
        let err = PluginError::LifecycleError {
            plugin_name: "flaky".into(),
            reason: "init timed out".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("flaky"));
        assert!(msg.contains("init timed out"));
    }

    #[test]
    fn signature_verification_failed_display() {
        let err = PluginError::SignatureVerificationFailed {
            package_name: "bad-sig".into(),
            reason: "certificate expired".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("bad-sig"));
        assert!(msg.contains("certificate expired"));
    }

    #[test]
    fn unsigned_package_rejected_display() {
        let err = PluginError::UnsignedPackageRejected {
            package_name: "no-sig".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("no-sig"));
        assert!(msg.contains("unsigned"));
    }

    #[test]
    fn slsa_attestation_failed_display() {
        let err = PluginError::SlsaAttestationFailed {
            package_name: "bad-provenance".into(),
            reason: "missing build steps".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("bad-provenance"));
        assert!(msg.contains("missing build steps"));
    }

    #[test]
    fn sandbox_denied_display() {
        let err = PluginError::SandboxDenied {
            plugin_name: "rogue".into(),
            operation: "http_request to evil.com".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("rogue"));
        assert!(msg.contains("http_request to evil.com"));
    }

    #[test]
    fn sandbox_resource_exhausted_display() {
        let err = PluginError::SandboxResourceExhausted {
            plugin_name: "greedy".into(),
            resource: "fuel budget (10000 instructions)".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("greedy"));
        assert!(msg.contains("fuel budget"));
    }
}
