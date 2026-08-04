//! Kit error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum KitError {
    #[error("kit manifest parse error: {reason}")]
    ManifestParse { reason: String },

    #[error("kit manifest validation failed for '{kit_name}': {reason}")]
    ManifestInvalid { kit_name: String, reason: String },

    #[error("invalid version '{version}': {reason}")]
    InvalidVersion { version: String, reason: String },

    #[error("invalid version requirement '{requirement}': {reason}")]
    InvalidVersionReq { requirement: String, reason: String },

    #[error("capability not granted: {capability}")]
    CapabilityDenied { capability: String },

    #[error("kit '{name}' is not installed")]
    NotInstalled { name: String },

    #[error("kit '{name}' is already installed (version {version})")]
    AlreadyInstalled { name: String, version: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

impl KitError {
    pub fn validation(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::ManifestInvalid {
            kit_name: name.into(),
            reason: reason.into(),
        }
    }
}
