//! Error types for the `polkagent-config` crate.

use std::path::PathBuf;

use thiserror::Error;

/// Alias for `Result<T, ConfigError>`.
pub type Result<T> = std::result::Result<T, ConfigError>;

/// Errors that can occur while loading or processing configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// An I/O error occurred while reading a config file.
    #[error("could not read config file '{0}': {1}")]
    Io(PathBuf, #[source] std::io::Error),

    /// A config file contained invalid TOML syntax or an unrecognised key.
    #[error("failed to parse config file '{0}': {1}")]
    Parse(String, String),

    /// A `Config` value could not be serialized to a TOML value for merging.
    #[error("failed to serialize config: {0}")]
    Serialize(String),

    /// A merged TOML value could not be deserialized back into `Config`.
    #[error("failed to deserialize config: {0}")]
    Deserialize(String),

    /// Configuration failed one or more validation checks.
    #[error("configuration is invalid:\n{}", format_validation_errors(.0))]
    Invalid(Vec<crate::validate::ValidationError>),
}

fn format_validation_errors(errors: &[crate::validate::ValidationError]) -> String {
    errors
        .iter()
        .map(|e| format!("  - {e}"))
        .collect::<Vec<_>>()
        .join("\n")
}
