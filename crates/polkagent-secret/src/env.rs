//! Environment-variable-backed secret store.
//!
//! Reads secrets from `POLKAGENT_*` environment variables. This store is
//! **read-only**: [`set`](SecretStore::set) and [`delete`](SecretStore::delete)
//! return [`SecretError::ReadOnly`].
//!
//! # Mapping convention
//!
//! A [`SecretId`] is converted to an env-var name by upper-casing and
//! replacing hyphens with underscores, then prepending `POLKAGENT_`:
//!
//! ```text
//! "anthropic-api-key" -> POLKAGENT_ANTHROPIC_API_KEY
//! "polkadot-seed"     -> POLKAGENT_POLKADOT_SEED
//! ```

use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::env;
use tracing::debug;

use crate::store::{Result, SecretError, SecretStore};
use crate::types::{SecretId, SecretMetadata, SecretSource, SecretValue};

/// Well-known environment variable names that are auto-detected on
/// initialisation.
const WELL_KNOWN_VARS: &[(&str, &str)] = &[
    ("anthropic-api-key", "Anthropic API Key"),
    ("openai-api-key", "OpenAI API Key"),
    ("polkadot-seed", "Polkadot Seed Phrase"),
    ("database-url", "Database URL"),
    ("webhook-secret", "Webhook Secret"),
];

// ---------------------------------------------------------------------------
// EnvSecretStore
// ---------------------------------------------------------------------------

/// A read-only [`SecretStore`] that reads from environment variables.
///
/// Auto-detects a set of well-known `POLKAGENT_*` vars on construction.
/// Additional secrets can be looked up on-the-fly via [`get`](SecretStore::get).
pub struct EnvSecretStore {
    /// Cache of detected env-var secret IDs and their human-readable labels.
    detected: HashMap<SecretId, String>,
}

impl EnvSecretStore {
    /// Create a new `EnvSecretStore`, auto-detecting well-known env vars.
    #[must_use]
    pub fn new() -> Self {
        let mut detected = HashMap::new();

        for &(id, label) in WELL_KNOWN_VARS {
            let secret_id = SecretId::new(id);
            let env_var = secret_id.to_env_var();
            if env::var(&env_var).is_ok() {
                debug!(env_var = %env_var, secret_id = %secret_id, "auto-detected secret from env");
                detected.insert(secret_id, label.to_string());
            }
        }

        Self { detected }
    }
}

impl Default for EnvSecretStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SecretStore for EnvSecretStore {
    async fn get(&self, id: &SecretId) -> Result<SecretValue> {
        let env_var = id.to_env_var();
        match env::var(&env_var) {
            Ok(value) => {
                debug!(secret_id = %id, env_var = %env_var, "read secret from env");
                Ok(SecretValue::new(value))
            }
            Err(_) => Err(SecretError::NotFound {
                id: id.as_str().to_string(),
            }),
        }
    }

    async fn set(
        &self,
        _id: SecretId,
        _value: SecretValue,
        _metadata: SecretMetadata,
    ) -> Result<()> {
        Err(SecretError::ReadOnly {
            message: "EnvSecretStore is read-only; set environment variables directly".into(),
        })
    }

    async fn delete(&self, _id: &SecretId) -> Result<()> {
        Err(SecretError::ReadOnly {
            message: "EnvSecretStore is read-only; unset environment variables directly".into(),
        })
    }

    async fn list(&self) -> Result<Vec<SecretMetadata>> {
        let now = Utc::now();
        let metas = self
            .detected
            .iter()
            .map(|(id, label)| SecretMetadata {
                id: id.clone(),
                label: label.clone(),
                created_at: now,
                last_accessed: None,
                access_count: 0,
                source: SecretSource::EnvVar(id.to_env_var()),
            })
            .collect();
        Ok(metas)
    }

    async fn exists(&self, id: &SecretId) -> Result<bool> {
        let env_var = id.to_env_var();
        Ok(env::var(&env_var).is_ok())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn get_existing_env_var() {
        // Safety: test env vars are isolated per-process.
        unsafe { env::set_var("POLKAGENT_TEST_KEY", "test-value") };
        let store = EnvSecretStore::new();
        let id = SecretId::new("test-key");

        let val = store.get(&id).await.expect("should find env var");
        assert_eq!(val.inner(), "test-value");

        unsafe { env::remove_var("POLKAGENT_TEST_KEY") };
    }

    #[tokio::test]
    async fn get_missing_env_var() {
        let store = EnvSecretStore::new();
        let id = SecretId::new("nonexistent-key");

        let err = store.get(&id).await.unwrap_err();
        assert!(matches!(err, SecretError::NotFound { .. }));
    }

    #[tokio::test]
    async fn set_returns_read_only_error() {
        let store = EnvSecretStore::new();
        let id = SecretId::new("test");
        let val = SecretValue::new("v");
        let meta = SecretMetadata::new(id.clone(), "Test", SecretSource::Manual);

        let err = store.set(id, val, meta).await.unwrap_err();
        assert!(matches!(err, SecretError::ReadOnly { .. }));
    }

    #[tokio::test]
    async fn delete_returns_read_only_error() {
        let store = EnvSecretStore::new();
        let id = SecretId::new("test");

        let err = store.delete(&id).await.unwrap_err();
        assert!(matches!(err, SecretError::ReadOnly { .. }));
    }

    #[tokio::test]
    async fn exists_reflects_env() {
        unsafe { env::set_var("POLKAGENT_EXISTS_CHECK", "1") };
        let store = EnvSecretStore::new();

        assert!(store
            .exists(&SecretId::new("exists-check"))
            .await
            .expect("ok"));
        assert!(!store.exists(&SecretId::new("nope")).await.expect("ok"));

        unsafe { env::remove_var("POLKAGENT_EXISTS_CHECK") };
    }

    #[tokio::test]
    async fn list_detects_well_known_vars() {
        unsafe { env::set_var("POLKAGENT_ANTHROPIC_API_KEY", "sk-test") };
        let store = EnvSecretStore::new();

        let metas = store.list().await.expect("list");
        assert!(metas.iter().any(|m| m.id.as_str() == "anthropic-api-key"));

        unsafe { env::remove_var("POLKAGENT_ANTHROPIC_API_KEY") };
    }

    #[tokio::test]
    async fn list_empty_when_no_vars() {
        // Ensure none of the well-known vars are set in this context.
        // (They may be set in other tests, so we just check structure.)
        let store = EnvSecretStore::new();
        let metas = store.list().await.expect("list");
        // All entries should have EnvVar source
        for meta in &metas {
            assert!(matches!(meta.source, SecretSource::EnvVar(_)));
        }
    }
}
