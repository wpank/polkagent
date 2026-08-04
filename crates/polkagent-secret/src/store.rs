//! Secret storage trait and error types.
//!
//! The [`SecretStore`] trait defines the hexagonal-architecture port for
//! secret persistence. Implementations (environment variable, file-based,
//! keychain) live alongside it in this crate.

use async_trait::async_trait;
use thiserror::Error;

use crate::types::{SecretId, SecretMetadata, SecretValue};

// ---------------------------------------------------------------------------
// SecretError
// ---------------------------------------------------------------------------

/// Error type for secret store operations.
#[derive(Debug, Error)]
pub enum SecretError {
    /// The requested secret does not exist.
    #[error("secret not found: {id}")]
    NotFound {
        /// The secret ID that was looked up.
        id: String,
    },

    /// The store does not support this operation (e.g. write on a read-only
    /// store).
    #[error("operation not supported: {message}")]
    ReadOnly {
        /// Explanation of why the operation is not supported.
        message: String,
    },

    /// An I/O error occurred while reading or writing a secret.
    #[error("I/O error: {message}")]
    Io {
        /// Description of the I/O failure.
        message: String,
    },

    /// Serialization or deserialization of secret data failed.
    #[error("serialization error: {message}")]
    Serialization {
        /// Description of the serialization failure.
        message: String,
    },

    /// A permission or access error occurred.
    #[error("permission denied: {message}")]
    PermissionDenied {
        /// Description of the permission failure.
        message: String,
    },

    /// An internal error that does not fit other variants.
    #[error("internal error: {message}")]
    Internal {
        /// Description of the error.
        message: String,
    },
}

impl From<std::io::Error> for SecretError {
    fn from(err: std::io::Error) -> Self {
        Self::Io {
            message: err.to_string(),
        }
    }
}

impl From<serde_json::Error> for SecretError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization {
            message: err.to_string(),
        }
    }
}

/// Convenience alias for `Result<T, SecretError>`.
pub type Result<T> = std::result::Result<T, SecretError>;

// ---------------------------------------------------------------------------
// SecretStore
// ---------------------------------------------------------------------------

/// Trait for secret storage backends.
///
/// Implementations may be read-only (e.g. [`EnvSecretStore`](crate::env::EnvSecretStore))
/// or read-write (e.g. [`FileSecretStore`](crate::file::FileSecretStore)).
/// Read-only stores return [`SecretError::ReadOnly`] from `set` and `delete`.
///
/// # Object safety
///
/// This trait is object-safe and can be used as `dyn SecretStore`.
#[async_trait]
pub trait SecretStore: Send + Sync {
    /// Retrieve a secret value by ID.
    ///
    /// Returns [`SecretError::NotFound`] if the secret does not exist in this
    /// store.
    async fn get(&self, id: &SecretId) -> Result<SecretValue>;

    /// Store or update a secret.
    ///
    /// Read-only stores return [`SecretError::ReadOnly`].
    async fn set(&self, id: SecretId, value: SecretValue, metadata: SecretMetadata) -> Result<()>;

    /// Delete a secret by ID.
    ///
    /// Returns [`SecretError::NotFound`] if the secret does not exist.
    /// Read-only stores return [`SecretError::ReadOnly`].
    async fn delete(&self, id: &SecretId) -> Result<()>;

    /// List metadata for all secrets in this store.
    ///
    /// This method **never** returns secret values — only metadata.
    async fn list(&self) -> Result<Vec<SecretMetadata>>;

    /// Check whether a secret exists in this store.
    async fn exists(&self, id: &SecretId) -> Result<bool>;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_error_not_found_display() {
        let err = SecretError::NotFound {
            id: "my-key".into(),
        };
        assert_eq!(format!("{err}"), "secret not found: my-key");
    }

    #[test]
    fn secret_error_read_only_display() {
        let err = SecretError::ReadOnly {
            message: "env store is read-only".into(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("not supported"));
    }

    #[test]
    fn secret_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file gone");
        let err = SecretError::from(io_err);
        assert!(matches!(err, SecretError::Io { .. }));
    }

    #[test]
    fn secret_error_from_serde() {
        let json_err = serde_json::from_str::<String>("not json").unwrap_err();
        let err = SecretError::from(json_err);
        assert!(matches!(err, SecretError::Serialization { .. }));
    }

    /// Compile-time check: `SecretStore` can be used as a `dyn` trait object.
    #[allow(dead_code)]
    fn _secret_store_is_object_safe(_s: &dyn SecretStore) {}
}
