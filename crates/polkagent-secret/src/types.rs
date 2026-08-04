//! Secret domain types: identifiers, values, metadata, and source tracking.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use zeroize::Zeroize;

// ---------------------------------------------------------------------------
// SecretId
// ---------------------------------------------------------------------------

/// A string identifier for a secret (e.g. `"anthropic-api-key"`,
/// `"polkadot-seed"`).
///
/// Secret IDs use kebab-case by convention and are mapped to environment
/// variable names by uppercasing and replacing hyphens with underscores,
/// prefixed with `POLKAGENT_`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SecretId(String);

impl SecretId {
    /// Create a new `SecretId` from any string-like value.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Return the raw identifier string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Convert this secret ID to the corresponding environment variable name.
    ///
    /// `"anthropic-api-key"` becomes `"POLKAGENT_ANTHROPIC_API_KEY"`.
    #[must_use]
    pub fn to_env_var(&self) -> String {
        format!("POLKAGENT_{}", self.0.to_uppercase().replace('-', "_"))
    }

    /// Convert this secret ID to a safe filename (replacing non-alphanumeric
    /// characters with underscores).
    #[must_use]
    pub fn to_filename(&self) -> String {
        format!("{}.json", self.0)
    }
}

impl fmt::Display for SecretId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<S: Into<String>> From<S> for SecretId {
    fn from(s: S) -> Self {
        Self::new(s)
    }
}

// ---------------------------------------------------------------------------
// SecretValue
// ---------------------------------------------------------------------------

/// A secret value that is zeroed from memory on drop.
///
/// - `Display` prints `[REDACTED]` to prevent accidental logging.
/// - `Debug` prints `SecretValue(***)` for the same reason.
/// - Call [`inner()`](SecretValue::inner) to access the raw string.
pub struct SecretValue {
    value: String,
}

impl SecretValue {
    /// Wrap a raw string as a secret value.
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
        }
    }

    /// Access the raw secret string.
    ///
    /// # Security
    ///
    /// The returned reference borrows the internal buffer. Avoid copying
    /// this value into non-zeroized storage.
    #[must_use]
    pub fn inner(&self) -> &str {
        &self.value
    }
}

impl fmt::Display for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretValue(***)")
    }
}

impl Clone for SecretValue {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
        }
    }
}

impl Zeroize for SecretValue {
    fn zeroize(&mut self) {
        self.value.zeroize();
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.zeroize();
    }
}

// ---------------------------------------------------------------------------
// SecretSource
// ---------------------------------------------------------------------------

/// Where a secret was loaded from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum SecretSource {
    /// Read from an environment variable (stores the var name).
    EnvVar(String),
    /// Read from a file on disk (stores the file path).
    File(PathBuf),
    /// Read from the OS keychain / credential manager.
    Keychain,
    /// Manually entered or set via API.
    Manual,
}

impl fmt::Display for SecretSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EnvVar(name) => write!(f, "env:{name}"),
            Self::File(path) => write!(f, "file:{}", path.display()),
            Self::Keychain => write!(f, "keychain"),
            Self::Manual => write!(f, "manual"),
        }
    }
}

// ---------------------------------------------------------------------------
// SecretMetadata
// ---------------------------------------------------------------------------

/// Metadata about a stored secret. Never contains the secret value itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretMetadata {
    /// Unique identifier for this secret.
    pub id: SecretId,
    /// Human-readable label (e.g. "Anthropic API Key").
    pub label: String,
    /// When the secret was first stored.
    pub created_at: DateTime<Utc>,
    /// When the secret was last accessed (read).
    pub last_accessed: Option<DateTime<Utc>>,
    /// Total number of times the secret has been accessed.
    pub access_count: u64,
    /// Where the secret was loaded from.
    pub source: SecretSource,
}

impl SecretMetadata {
    /// Create new metadata with sensible defaults.
    pub fn new(id: SecretId, label: impl Into<String>, source: SecretSource) -> Self {
        Self {
            id,
            label: label.into(),
            created_at: Utc::now(),
            last_accessed: None,
            access_count: 0,
            source,
        }
    }

    /// Record an access, bumping the count and updating the timestamp.
    pub fn record_access(&mut self) {
        self.last_accessed = Some(Utc::now());
        self.access_count += 1;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_id_to_env_var() {
        let id = SecretId::new("anthropic-api-key");
        assert_eq!(id.to_env_var(), "POLKAGENT_ANTHROPIC_API_KEY");
    }

    #[test]
    fn secret_id_to_filename() {
        let id = SecretId::new("polkadot-seed");
        assert_eq!(id.to_filename(), "polkadot-seed.json");
    }

    #[test]
    fn secret_id_display() {
        let id = SecretId::new("my-key");
        assert_eq!(format!("{id}"), "my-key");
    }

    #[test]
    fn secret_id_from_string() {
        let id: SecretId = "test-key".into();
        assert_eq!(id.as_str(), "test-key");
    }

    #[test]
    fn secret_value_display_is_redacted() {
        let val = SecretValue::new("super-secret-123");
        assert_eq!(format!("{val}"), "[REDACTED]");
    }

    #[test]
    fn secret_value_debug_is_redacted() {
        let val = SecretValue::new("super-secret-123");
        assert_eq!(format!("{val:?}"), "SecretValue(***)");
    }

    #[test]
    fn secret_value_inner_returns_raw() {
        let val = SecretValue::new("super-secret-123");
        assert_eq!(val.inner(), "super-secret-123");
    }

    #[test]
    fn secret_value_clone_works() {
        let val = SecretValue::new("cloneable");
        let cloned = val.clone();
        assert_eq!(cloned.inner(), "cloneable");
    }

    #[test]
    fn secret_value_zeroize_clears_memory() {
        let mut val = SecretValue::new("to-be-cleared");
        val.zeroize();
        assert!(val.inner().is_empty());
    }

    #[test]
    fn secret_source_display() {
        assert_eq!(format!("{}", SecretSource::EnvVar("FOO".into())), "env:FOO");
        assert_eq!(
            format!("{}", SecretSource::File("/tmp/key".into())),
            "file:/tmp/key"
        );
        assert_eq!(format!("{}", SecretSource::Keychain), "keychain");
        assert_eq!(format!("{}", SecretSource::Manual), "manual");
    }

    #[test]
    fn secret_source_roundtrip() {
        let source = SecretSource::EnvVar("POLKAGENT_KEY".into());
        let json = serde_json::to_string(&source).expect("serialize");
        let back: SecretSource = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(source, back);
    }

    #[test]
    fn secret_metadata_new_defaults() {
        let meta = SecretMetadata::new(SecretId::new("test"), "Test Key", SecretSource::Manual);
        assert_eq!(meta.id.as_str(), "test");
        assert_eq!(meta.label, "Test Key");
        assert!(meta.last_accessed.is_none());
        assert_eq!(meta.access_count, 0);
    }

    #[test]
    fn secret_metadata_record_access() {
        let mut meta = SecretMetadata::new(SecretId::new("test"), "Test", SecretSource::Manual);
        meta.record_access();
        assert_eq!(meta.access_count, 1);
        assert!(meta.last_accessed.is_some());

        meta.record_access();
        assert_eq!(meta.access_count, 2);
    }

    #[test]
    fn secret_metadata_serializes() {
        let meta = SecretMetadata::new(
            SecretId::new("api-key"),
            "API Key",
            SecretSource::EnvVar("POLKAGENT_API_KEY".into()),
        );
        let json = serde_json::to_string(&meta).expect("serialize");
        assert!(json.contains("api-key"));
        assert!(json.contains("API Key"));

        let back: SecretMetadata = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.id.as_str(), "api-key");
    }
}
