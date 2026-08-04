//! File-system-backed secret store.
//!
//! Each secret is stored as a separate JSON file in
//! `~/.polkagent/secrets/<id>.json` with permissions set to `0600`
//! (owner read/write only).
//!
//! # File format
//!
//! ```json
//! {
//!   "value": "the-secret-value",
//!   "metadata": { "id": "...", "label": "...", ... }
//! }
//! ```
//!
//! **Note:** The stored value is **not encrypted**. Encryption is planned as
//! a future enhancement.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

use crate::store::{Result, SecretError, SecretStore};
use crate::types::{SecretId, SecretMetadata, SecretValue};

// ---------------------------------------------------------------------------
// On-disk format
// ---------------------------------------------------------------------------

/// JSON structure persisted to each secret file.
#[derive(Serialize, Deserialize)]
struct StoredSecret {
    value: String,
    metadata: SecretMetadata,
}

// ---------------------------------------------------------------------------
// FileSecretStore
// ---------------------------------------------------------------------------

/// A read-write [`SecretStore`] that persists secrets as JSON files.
///
/// Default directory: `~/.polkagent/secrets/`.
pub struct FileSecretStore {
    /// Root directory for secret files.
    secrets_dir: PathBuf,
}

impl FileSecretStore {
    /// Create a new `FileSecretStore` using the default directory
    /// (`~/.polkagent/secrets/`).
    ///
    /// Returns an error if the home directory cannot be determined.
    pub fn new() -> Result<Self> {
        let home = dirs::home_dir().ok_or_else(|| SecretError::Internal {
            message: "could not determine home directory".into(),
        })?;
        let secrets_dir = home.join(".polkagent").join("secrets");
        Self::with_dir(secrets_dir)
    }

    /// Create a `FileSecretStore` backed by a custom directory.
    ///
    /// The directory is created (with parents) if it does not exist.
    pub fn with_dir(secrets_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&secrets_dir)?;

        // Set directory permissions to 0700 on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o700);
            std::fs::set_permissions(&secrets_dir, perms)?;
        }

        debug!(dir = %secrets_dir.display(), "initialised file secret store");
        Ok(Self { secrets_dir })
    }

    /// Return the path for a given secret ID.
    fn secret_path(&self, id: &SecretId) -> PathBuf {
        self.secrets_dir.join(id.to_filename())
    }

    /// Read and deserialize a secret file.
    fn read_stored(&self, path: &Path) -> Result<StoredSecret> {
        let content = std::fs::read_to_string(path)?;
        let stored: StoredSecret = serde_json::from_str(&content)?;
        Ok(stored)
    }

    /// Write a secret file with restricted permissions.
    fn write_stored(&self, path: &Path, stored: &StoredSecret) -> Result<()> {
        let content = serde_json::to_string_pretty(stored)?;
        std::fs::write(path, &content)?;

        // Set file permissions to 0600 on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(path, perms)?;
        }

        Ok(())
    }
}

#[async_trait]
impl SecretStore for FileSecretStore {
    async fn get(&self, id: &SecretId) -> Result<SecretValue> {
        let path = self.secret_path(id);
        if !path.exists() {
            return Err(SecretError::NotFound {
                id: id.as_str().to_string(),
            });
        }

        let stored = self.read_stored(&path)?;
        debug!(secret_id = %id, "read secret from file");
        Ok(SecretValue::new(stored.value))
    }

    async fn set(&self, id: SecretId, value: SecretValue, metadata: SecretMetadata) -> Result<()> {
        let path = self.secret_path(&id);
        let stored = StoredSecret {
            value: value.inner().to_string(),
            metadata,
        };
        self.write_stored(&path, &stored)?;
        debug!(secret_id = %id, path = %path.display(), "wrote secret to file");
        Ok(())
    }

    async fn delete(&self, id: &SecretId) -> Result<()> {
        let path = self.secret_path(id);
        if !path.exists() {
            return Err(SecretError::NotFound {
                id: id.as_str().to_string(),
            });
        }
        std::fs::remove_file(&path)?;
        debug!(secret_id = %id, "deleted secret file");
        Ok(())
    }

    async fn list(&self) -> Result<Vec<SecretMetadata>> {
        let mut metas = Vec::new();
        let entries = std::fs::read_dir(&self.secrets_dir)?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            match self.read_stored(&path) {
                Ok(stored) => metas.push(stored.metadata),
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "skipping malformed secret file");
                }
            }
        }

        Ok(metas)
    }

    async fn exists(&self, id: &SecretId) -> Result<bool> {
        Ok(self.secret_path(id).exists())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SecretSource;

    fn temp_store() -> (tempfile::TempDir, FileSecretStore) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let store = FileSecretStore::with_dir(dir.path().to_path_buf()).expect("create store");
        (dir, store)
    }

    #[tokio::test]
    async fn set_and_get_round_trip() {
        let (_dir, store) = temp_store();
        let id = SecretId::new("my-key");
        let val = SecretValue::new("super-secret");
        let meta = SecretMetadata::new(id.clone(), "My Key", SecretSource::Manual);

        store
            .set(id.clone(), val, meta)
            .await
            .expect("set should succeed");

        let retrieved = store.get(&id).await.expect("get should succeed");
        assert_eq!(retrieved.inner(), "super-secret");
    }

    #[tokio::test]
    async fn get_missing_returns_not_found() {
        let (_dir, store) = temp_store();
        let id = SecretId::new("nonexistent");

        let err = store.get(&id).await.unwrap_err();
        assert!(matches!(err, SecretError::NotFound { .. }));
    }

    #[tokio::test]
    async fn delete_removes_file() {
        let (_dir, store) = temp_store();
        let id = SecretId::new("to-delete");
        let val = SecretValue::new("bye");
        let meta = SecretMetadata::new(id.clone(), "Delete Me", SecretSource::Manual);

        store.set(id.clone(), val, meta).await.expect("set");
        assert!(store.exists(&id).await.expect("exists"));

        store.delete(&id).await.expect("delete");
        assert!(!store.exists(&id).await.expect("exists after delete"));
    }

    #[tokio::test]
    async fn delete_missing_returns_not_found() {
        let (_dir, store) = temp_store();
        let id = SecretId::new("ghost");

        let err = store.delete(&id).await.unwrap_err();
        assert!(matches!(err, SecretError::NotFound { .. }));
    }

    #[tokio::test]
    async fn list_returns_all_metadata() {
        let (_dir, store) = temp_store();

        for i in 0..3 {
            let id = SecretId::new(format!("key-{i}"));
            let val = SecretValue::new(format!("value-{i}"));
            let meta = SecretMetadata::new(id.clone(), format!("Key {i}"), SecretSource::Manual);
            store.set(id, val, meta).await.expect("set");
        }

        let metas = store.list().await.expect("list");
        assert_eq!(metas.len(), 3);
    }

    #[tokio::test]
    async fn exists_returns_correct_state() {
        let (_dir, store) = temp_store();
        let id = SecretId::new("check-me");

        assert!(!store.exists(&id).await.expect("before set"));

        let val = SecretValue::new("val");
        let meta = SecretMetadata::new(id.clone(), "Check", SecretSource::Manual);
        store.set(id.clone(), val, meta).await.expect("set");

        assert!(store.exists(&id).await.expect("after set"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn file_permissions_are_0600() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, store) = temp_store();
        let id = SecretId::new("perms-test");
        let val = SecretValue::new("secret");
        let meta = SecretMetadata::new(id.clone(), "Perms", SecretSource::Manual);
        store.set(id.clone(), val, meta).await.expect("set");

        let path = store.secret_path(&id);
        let perms = std::fs::metadata(&path).expect("metadata").permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);
    }

    #[tokio::test]
    async fn overwrite_existing_secret() {
        let (_dir, store) = temp_store();
        let id = SecretId::new("overwrite");

        let val1 = SecretValue::new("first");
        let meta1 = SecretMetadata::new(id.clone(), "V1", SecretSource::Manual);
        store.set(id.clone(), val1, meta1).await.expect("set v1");

        let val2 = SecretValue::new("second");
        let meta2 = SecretMetadata::new(id.clone(), "V2", SecretSource::Manual);
        store.set(id.clone(), val2, meta2).await.expect("set v2");

        let retrieved = store.get(&id).await.expect("get");
        assert_eq!(retrieved.inner(), "second");
    }
}
