//! Composite secret store that chains multiple backends.
//!
//! The [`ChainSecretStore`] tries each backend in order for reads. The first
//! backend that returns a value wins. Writes are directed to the
//! [`FileSecretStore`].
//!
//! Default chain order: `EnvSecretStore` -> `FileSecretStore`.

use async_trait::async_trait;
use tracing::debug;

use crate::env::EnvSecretStore;
use crate::file::FileSecretStore;
use crate::store::{Result, SecretError, SecretStore};
use crate::types::{SecretId, SecretMetadata, SecretValue};

// ---------------------------------------------------------------------------
// ChainSecretStore
// ---------------------------------------------------------------------------

/// A composite [`SecretStore`] that reads from multiple backends in order
/// and writes to the file-based store.
///
/// The default chain is:
/// 1. [`EnvSecretStore`] — environment variables (read-only)
/// 2. [`FileSecretStore`] — file system (read-write)
///
/// On `get`, the first store that has the secret wins.
/// On `set` and `delete`, the file store is used.
pub struct ChainSecretStore {
    env_store: EnvSecretStore,
    file_store: FileSecretStore,
}

impl ChainSecretStore {
    /// Create a new `ChainSecretStore` with the default env + file chain.
    ///
    /// Returns an error if the file store directory cannot be created.
    pub fn new() -> Result<Self> {
        Ok(Self {
            env_store: EnvSecretStore::new(),
            file_store: FileSecretStore::new()?,
        })
    }

    /// Create a `ChainSecretStore` with custom store instances.
    pub fn with_stores(env_store: EnvSecretStore, file_store: FileSecretStore) -> Self {
        Self {
            env_store,
            file_store,
        }
    }
}

#[async_trait]
impl SecretStore for ChainSecretStore {
    async fn get(&self, id: &SecretId) -> Result<SecretValue> {
        // Try env first.
        match self.env_store.get(id).await {
            Ok(val) => {
                debug!(secret_id = %id, source = "env", "secret found in chain");
                return Ok(val);
            }
            Err(SecretError::NotFound { .. }) => {
                // Fall through to file store.
            }
            Err(e) => return Err(e),
        }

        // Try file store.
        match self.file_store.get(id).await {
            Ok(val) => {
                debug!(secret_id = %id, source = "file", "secret found in chain");
                Ok(val)
            }
            Err(e) => Err(e),
        }
    }

    async fn set(&self, id: SecretId, value: SecretValue, metadata: SecretMetadata) -> Result<()> {
        self.file_store.set(id, value, metadata).await
    }

    async fn delete(&self, id: &SecretId) -> Result<()> {
        self.file_store.delete(id).await
    }

    async fn list(&self) -> Result<Vec<SecretMetadata>> {
        let mut all = self.env_store.list().await?;
        let file_metas = self.file_store.list().await?;

        // Deduplicate: env wins if the same ID exists in both.
        let env_ids: std::collections::HashSet<_> = all.iter().map(|m| m.id.clone()).collect();

        for meta in file_metas {
            if !env_ids.contains(&meta.id) {
                all.push(meta);
            }
        }

        Ok(all)
    }

    async fn exists(&self, id: &SecretId) -> Result<bool> {
        if self.env_store.exists(id).await? {
            return Ok(true);
        }
        self.file_store.exists(id).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;

    fn temp_chain() -> (tempfile::TempDir, ChainSecretStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_store = FileSecretStore::with_dir(dir.path().to_path_buf()).expect("file store");
        let env_store = EnvSecretStore::new();
        let chain = ChainSecretStore::with_stores(env_store, file_store);
        (dir, chain)
    }

    #[tokio::test]
    async fn env_takes_precedence_over_file() {
        let (_dir, chain) = temp_chain();
        let id = SecretId::new("chain-prio");

        // Set in file store.
        let meta = SecretMetadata::new(
            id.clone(),
            "File Version",
            crate::types::SecretSource::Manual,
        );
        chain
            .set(id.clone(), SecretValue::new("file-value"), meta)
            .await
            .expect("set in file");

        // Set in env.
        unsafe { std::env::set_var("POLKAGENT_CHAIN_PRIO", "env-value") };

        let val = chain.get(&id).await.expect("get");
        assert_eq!(val.inner(), "env-value");

        unsafe { std::env::remove_var("POLKAGENT_CHAIN_PRIO") };
    }

    #[tokio::test]
    async fn falls_through_to_file() {
        let (_dir, chain) = temp_chain();
        let id = SecretId::new("file-only");

        let meta = SecretMetadata::new(id.clone(), "File Only", crate::types::SecretSource::Manual);
        chain
            .set(id.clone(), SecretValue::new("from-file"), meta)
            .await
            .expect("set");

        let val = chain.get(&id).await.expect("get");
        assert_eq!(val.inner(), "from-file");
    }

    #[tokio::test]
    async fn not_found_in_any_store() {
        let (_dir, chain) = temp_chain();
        let id = SecretId::new("nowhere");

        let err = chain.get(&id).await.unwrap_err();
        assert!(matches!(err, SecretError::NotFound { .. }));
    }

    #[tokio::test]
    async fn set_writes_to_file_store() {
        let (_dir, chain) = temp_chain();
        let id = SecretId::new("chain-write");
        let meta =
            SecretMetadata::new(id.clone(), "Write Test", crate::types::SecretSource::Manual);

        chain
            .set(id.clone(), SecretValue::new("written"), meta)
            .await
            .expect("set");

        // Should be retrievable from file store directly.
        let val = chain.get(&id).await.expect("get");
        assert_eq!(val.inner(), "written");
    }

    #[tokio::test]
    async fn delete_removes_from_file_store() {
        let (_dir, chain) = temp_chain();
        let id = SecretId::new("chain-del");
        let meta = SecretMetadata::new(
            id.clone(),
            "Delete Test",
            crate::types::SecretSource::Manual,
        );

        chain
            .set(id.clone(), SecretValue::new("temp"), meta)
            .await
            .expect("set");

        chain.delete(&id).await.expect("delete");
        assert!(!chain.exists(&id).await.expect("exists"));
    }

    #[tokio::test]
    async fn exists_checks_both_stores() {
        let (_dir, chain) = temp_chain();

        // Set in env only.
        unsafe { std::env::set_var("POLKAGENT_ENV_ONLY_CHAIN", "1") };
        assert!(chain
            .exists(&SecretId::new("env-only-chain"))
            .await
            .expect("exists env"));
        unsafe { std::env::remove_var("POLKAGENT_ENV_ONLY_CHAIN") };

        // Set in file only.
        let id = SecretId::new("file-only-chain");
        let meta = SecretMetadata::new(id.clone(), "File", crate::types::SecretSource::Manual);
        chain
            .set(id.clone(), SecretValue::new("v"), meta)
            .await
            .expect("set");
        assert!(chain.exists(&id).await.expect("exists file"));
    }

    #[tokio::test]
    async fn list_merges_and_deduplicates() {
        let (_dir, chain) = temp_chain();

        // Add a file-only secret.
        let id = SecretId::new("list-file");
        let meta = SecretMetadata::new(id.clone(), "From File", crate::types::SecretSource::Manual);
        chain
            .set(id, SecretValue::new("v"), meta)
            .await
            .expect("set");

        let metas = chain.list().await.expect("list");
        assert!(metas.iter().any(|m| m.id.as_str() == "list-file"));
    }
}
