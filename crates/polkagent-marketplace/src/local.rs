//! Durable, local-only package lifecycle.
//!
//! This store intentionally does not execute package code or assert that a
//! signature is cryptographically valid. It gives local package management a
//! restart-safe foundation while keeping those two security boundaries
//! explicit for later runtime integration.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use fs2::FileExt;
use polkagent_kit::KitManifest;
use polkagent_plugin::PluginManifest;
use semver::Version;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const STATE_SCHEMA_VERSION: u32 = 1;
const STATE_FILE: &str = "state.json";
const LOCK_FILE: &str = ".state.lock";
const PACKAGES_DIR: &str = "packages";
const STAGING_DIR: &str = ".staging";
const TRASH_DIR: &str = ".trash";
const PLUGIN_MANIFEST: &str = "plugin.toml";
const KIT_MANIFEST: &str = "kit.toml";

/// The locally supported package manifest families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageKind {
    Plugin,
    Kit,
}

/// What the local store can honestly establish about package provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceStatus {
    /// No signature bundle was declared.
    Unsigned,
    /// Signature material was declared but no cryptographic verifier ran.
    SignatureClaimUnverified,
}

/// Integrity and publisher claims recorded at install time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageProvenance {
    pub status: ProvenanceStatus,
    pub signer_identity: Option<String>,
    /// Canonical `blake3:<hex>` digest calculated by this store.
    pub content_digest: String,
    /// Whether a digest declared by the manifest matched the calculated digest.
    pub declared_digest_verified: bool,
    /// `false` until a real signature verifier is integrated.
    pub signature_cryptographically_verified: bool,
}

/// A version retained in the local content store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageVersion {
    pub version: Version,
    pub content_path: PathBuf,
    pub source_path: PathBuf,
    pub provenance: PackageProvenance,
    pub installed_at: DateTime<Utc>,
}

/// The selected package version and rollback history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledPackage {
    pub name: String,
    pub kind: PackageKind,
    pub active: PackageVersion,
    /// Older versions, oldest first. Versions remain immutable on disk.
    pub history: Vec<PackageVersion>,
}

/// A source tree after manifest validation and digest calculation.
#[derive(Debug, Clone, Serialize)]
pub struct PackageCandidate {
    pub name: String,
    pub kind: PackageKind,
    pub version: Version,
    pub source_path: PathBuf,
    pub provenance: PackageProvenance,
}

/// Policy applied before a local source tree may be installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalInstallPolicy {
    pub allow_unsigned: bool,
    pub allow_unverified_signature_claims: bool,
    pub require_declared_digest: bool,
}

impl LocalInstallPolicy {
    /// Conservative policy for environments that expect a future verifier.
    pub const fn strict() -> Self {
        Self {
            allow_unsigned: false,
            allow_unverified_signature_claims: false,
            require_declared_digest: true,
        }
    }

    /// Explicit local-development policy. Integrity is still calculated and
    /// checked whenever the manifest declares a digest.
    pub const fn development() -> Self {
        Self {
            allow_unsigned: true,
            allow_unverified_signature_claims: true,
            require_declared_digest: false,
        }
    }

    /// Validate a candidate against this trust policy without installing it.
    pub fn validate(self, candidate: &PackageCandidate) -> Result<(), LocalPackageError> {
        match candidate.provenance.status {
            ProvenanceStatus::Unsigned if !self.allow_unsigned => {
                return Err(LocalPackageError::UnsignedRejected(candidate.name.clone()));
            }
            ProvenanceStatus::SignatureClaimUnverified
                if !self.allow_unverified_signature_claims =>
            {
                return Err(LocalPackageError::UnverifiedSignatureRejected(
                    candidate.name.clone(),
                ));
            }
            _ => {}
        }
        if self.require_declared_digest && !candidate.provenance.declared_digest_verified {
            return Err(LocalPackageError::DigestRequired(candidate.name.clone()));
        }
        Ok(())
    }
}

impl Default for LocalInstallPolicy {
    fn default() -> Self {
        Self::strict()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    Installed(InstalledPackage),
    AlreadyInstalled(InstalledPackage),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    Updated(InstalledPackage),
    AlreadyCurrent(InstalledPackage),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RollbackOutcome {
    RolledBack(InstalledPackage),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UninstallOutcome {
    Uninstalled,
    NotInstalled,
}

#[derive(Debug, thiserror::Error)]
pub enum LocalPackageError {
    #[error("package source '{0}' is not a directory")]
    SourceNotDirectory(PathBuf),
    #[error("package source '{package_source}' contains the local store root '{store}'")]
    SourceContainsStore {
        package_source: PathBuf,
        store: PathBuf,
    },
    #[error("package source must contain exactly one of plugin.toml or kit.toml")]
    AmbiguousManifest,
    #[error("invalid package manifest: {0}")]
    InvalidManifest(String),
    #[error("unsupported file type at '{0}'; symlinks and special files are rejected")]
    UnsupportedFileType(PathBuf),
    #[error("package path '{0}' is not valid UTF-8")]
    UnsupportedPathEncoding(PathBuf),
    #[error("package '{name}' is already installed at version {version}; use update")]
    AlreadyInstalled { name: String, version: Version },
    #[error("package '{0}' is not installed")]
    NotInstalled(String),
    #[error("update for '{name}' must be newer than {current}, got {candidate}")]
    NotNewer {
        name: String,
        current: Version,
        candidate: Version,
    },
    #[error("package kind changed for '{name}' from {current:?} to {candidate:?}")]
    KindChanged {
        name: String,
        current: PackageKind,
        candidate: PackageKind,
    },
    #[error("package '{name}' version {version} already exists with different content")]
    ImmutableVersionConflict { name: String, version: Version },
    #[error("rollback version {version} is not retained for package '{name}'")]
    RollbackVersionNotFound { name: String, version: Version },
    #[error("package '{0}' has no prior version to roll back to")]
    NoRollbackVersion(String),
    #[error("unsigned local package '{0}' is rejected by policy")]
    UnsignedRejected(String),
    #[error("package '{0}' contains an unverified signature claim rejected by policy")]
    UnverifiedSignatureRejected(String),
    #[error("package '{0}' must declare a content digest")]
    DigestRequired(String),
    #[error("invalid declared content digest '{0}'")]
    InvalidDigest(String),
    #[error("content digest mismatch: declared {declared}, calculated {calculated}")]
    DigestMismatch {
        declared: String,
        calculated: String,
    },
    #[error("local package state schema {found} is unsupported (expected {expected})")]
    UnsupportedStateSchema { found: u32, expected: u32 },
    #[error("local package state is corrupt: {0}")]
    CorruptState(String),
    #[error(
        "installed content for '{name}' version {version} failed integrity validation: {reason}"
    )]
    InstalledContentIntegrity {
        name: String,
        version: Version,
        reason: String,
    },
    #[error("I/O error at '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl LocalPackageError {
    fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LocalState {
    schema_version: u32,
    packages: BTreeMap<String, InstalledPackage>,
}

impl Default for LocalState {
    fn default() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            packages: BTreeMap::new(),
        }
    }
}

/// Filesystem-backed local package registry.
///
/// Every mutation reloads state while holding an OS file lock, allowing
/// multiple processes to use the same root without lost updates.
#[derive(Debug, Clone)]
pub struct LocalPackageStore {
    root: PathBuf,
    policy: LocalInstallPolicy,
}

impl LocalPackageStore {
    pub fn open(
        root: impl AsRef<Path>,
        policy: LocalInstallPolicy,
    ) -> Result<Self, LocalPackageError> {
        let root = root.as_ref().to_path_buf();
        create_dir(&root)?;
        let root = root
            .canonicalize()
            .map_err(|error| LocalPackageError::io(&root, error))?;
        create_dir(&root.join(PACKAGES_DIR))?;
        create_dir(&root.join(STAGING_DIR))?;
        create_dir(&root.join(TRASH_DIR))?;
        let store = Self { root, policy };
        let lock = store.lock()?;
        if store.state_path().exists() {
            let state = store.read_state()?;
            store.validate_state(&state)?;
            store.cleanup_abandoned_content(&state)?;
        } else {
            store.write_state(&LocalState::default())?;
        }
        drop(lock);
        Ok(store)
    }

    /// Open a previously initialized store without creating or cleaning any
    /// files. This is useful for read-only inspection and dry-run callers.
    pub fn open_existing(
        root: impl AsRef<Path>,
        policy: LocalInstallPolicy,
    ) -> Result<Self, LocalPackageError> {
        let supplied_root = root.as_ref();
        if !supplied_root.is_dir() {
            return Err(LocalPackageError::SourceNotDirectory(
                supplied_root.to_path_buf(),
            ));
        }
        let root = supplied_root
            .canonicalize()
            .map_err(|error| LocalPackageError::io(supplied_root, error))?;
        let store = Self { root, policy };
        let lock = store.lock()?;
        let state = store.read_state()?;
        store.validate_state(&state)?;
        drop(lock);
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Parse and hash a package source without changing installed state.
    pub fn inspect(&self, source: impl AsRef<Path>) -> Result<PackageCandidate, LocalPackageError> {
        inspect_source(source.as_ref())
    }

    pub fn list(&self) -> Result<Vec<InstalledPackage>, LocalPackageError> {
        let lock = self.lock()?;
        let state = self.read_state()?;
        self.validate_state(&state)?;
        let packages = state.packages.into_values().collect();
        drop(lock);
        Ok(packages)
    }

    pub fn get(&self, name: &str) -> Result<Option<InstalledPackage>, LocalPackageError> {
        let lock = self.lock()?;
        let state = self.read_state()?;
        self.validate_state(&state)?;
        let package = state.packages.get(name).cloned();
        drop(lock);
        Ok(package)
    }

    pub fn install(&self, source: impl AsRef<Path>) -> Result<InstallOutcome, LocalPackageError> {
        let candidate = inspect_source(source.as_ref())?;
        self.reject_source_containing_store(&candidate)?;
        self.enforce_policy(&candidate)?;
        let lock = self.lock()?;
        let mut state = self.read_state()?;
        self.validate_state(&state)?;
        self.cleanup_abandoned_content(&state)?;
        if let Some(existing) = state.packages.get(&candidate.name) {
            if existing.active.version == candidate.version
                && existing.active.provenance.content_digest == candidate.provenance.content_digest
            {
                return Ok(InstallOutcome::AlreadyInstalled(existing.clone()));
            }
            if existing.active.version == candidate.version {
                return Err(LocalPackageError::ImmutableVersionConflict {
                    name: candidate.name,
                    version: candidate.version,
                });
            }
            return Err(LocalPackageError::AlreadyInstalled {
                name: existing.name.clone(),
                version: existing.active.version.clone(),
            });
        }

        let active = self.materialize(&candidate)?;
        let package = InstalledPackage {
            name: candidate.name.clone(),
            kind: candidate.kind,
            active,
            history: Vec::new(),
        };
        state.packages.insert(candidate.name, package.clone());
        self.write_state(&state)?;
        drop(lock);
        Ok(InstallOutcome::Installed(package))
    }

    pub fn update(&self, source: impl AsRef<Path>) -> Result<UpdateOutcome, LocalPackageError> {
        let candidate = inspect_source(source.as_ref())?;
        self.reject_source_containing_store(&candidate)?;
        self.enforce_policy(&candidate)?;
        let lock = self.lock()?;
        let mut state = self.read_state()?;
        self.validate_state(&state)?;
        self.cleanup_abandoned_content(&state)?;
        let existing = state
            .packages
            .get(&candidate.name)
            .cloned()
            .ok_or_else(|| LocalPackageError::NotInstalled(candidate.name.clone()))?;
        if existing.kind != candidate.kind {
            return Err(LocalPackageError::KindChanged {
                name: candidate.name,
                current: existing.kind,
                candidate: candidate.kind,
            });
        }
        if existing.active.version == candidate.version {
            if existing.active.provenance.content_digest == candidate.provenance.content_digest {
                return Ok(UpdateOutcome::AlreadyCurrent(existing));
            }
            return Err(LocalPackageError::ImmutableVersionConflict {
                name: candidate.name,
                version: candidate.version,
            });
        }
        if candidate.version < existing.active.version {
            return Err(LocalPackageError::NotNewer {
                name: candidate.name,
                current: existing.active.version,
                candidate: candidate.version,
            });
        }

        let new_active = self.materialize(&candidate)?;
        let package = state
            .packages
            .get_mut(&candidate.name)
            .ok_or_else(|| LocalPackageError::NotInstalled(candidate.name.clone()))?;
        package
            .history
            .retain(|item| item.version != candidate.version);
        let prior = std::mem::replace(&mut package.active, new_active);
        if !package
            .history
            .iter()
            .any(|item| item.version == prior.version)
        {
            package.history.push(prior);
        }
        let result = package.clone();
        self.write_state(&state)?;
        drop(lock);
        Ok(UpdateOutcome::Updated(result))
    }

    pub fn rollback(
        &self,
        name: &str,
        target: Option<&Version>,
    ) -> Result<RollbackOutcome, LocalPackageError> {
        let lock = self.lock()?;
        let mut state = self.read_state()?;
        self.validate_state(&state)?;
        let package = state
            .packages
            .get_mut(name)
            .ok_or_else(|| LocalPackageError::NotInstalled(name.to_owned()))?;
        let index = match target {
            Some(version) => package
                .history
                .iter()
                .position(|item| &item.version == version)
                .ok_or_else(|| LocalPackageError::RollbackVersionNotFound {
                    name: name.to_owned(),
                    version: version.clone(),
                })?,
            None => package
                .history
                .len()
                .checked_sub(1)
                .ok_or_else(|| LocalPackageError::NoRollbackVersion(name.to_owned()))?,
        };
        let target_version = package.history.remove(index);
        let prior = std::mem::replace(&mut package.active, target_version);
        package.history.push(prior);
        let result = package.clone();
        self.write_state(&state)?;
        drop(lock);
        Ok(RollbackOutcome::RolledBack(result))
    }

    pub fn uninstall(&self, name: &str) -> Result<UninstallOutcome, LocalPackageError> {
        let lock = self.lock()?;
        let mut state = self.read_state()?;
        self.validate_state(&state)?;
        if !state.packages.contains_key(name) {
            return Ok(UninstallOutcome::NotInstalled);
        }

        let package_dir = self.root.join(PACKAGES_DIR).join(name);
        state.packages.remove(name);
        self.write_state(&state)?;
        if package_dir.exists() {
            let trash = self
                .root
                .join(TRASH_DIR)
                .join(format!("{name}-{}", Uuid::now_v7()));
            // State is already committed. Cleanup is recoverable on next open,
            // so a process crash cannot resurrect the package.
            if fs::rename(&package_dir, &trash).is_ok() {
                let _ = fs::remove_dir_all(&trash);
            }
        }
        drop(lock);
        Ok(UninstallOutcome::Uninstalled)
    }

    fn enforce_policy(&self, candidate: &PackageCandidate) -> Result<(), LocalPackageError> {
        self.policy.validate(candidate)
    }

    fn reject_source_containing_store(
        &self,
        candidate: &PackageCandidate,
    ) -> Result<(), LocalPackageError> {
        if self.root.starts_with(&candidate.source_path) {
            return Err(LocalPackageError::SourceContainsStore {
                package_source: candidate.source_path.clone(),
                store: self.root.clone(),
            });
        }
        Ok(())
    }

    fn materialize(
        &self,
        candidate: &PackageCandidate,
    ) -> Result<PackageVersion, LocalPackageError> {
        let destination = self
            .root
            .join(PACKAGES_DIR)
            .join(&candidate.name)
            .join(candidate.version.to_string())
            .join("content");
        if destination.exists() {
            let retained = inspect_source(&destination)?;
            if retained.provenance.content_digest != candidate.provenance.content_digest {
                return Err(LocalPackageError::ImmutableVersionConflict {
                    name: candidate.name.clone(),
                    version: candidate.version.clone(),
                });
            }
        } else {
            let staging = self.root.join(STAGING_DIR).join(Uuid::now_v7().to_string());
            if let Err(error) = copy_tree(&candidate.source_path, &staging) {
                let _ = fs::remove_dir_all(&staging);
                return Err(error);
            }
            let staged = match inspect_source(&staging) {
                Ok(staged) => staged,
                Err(error) => {
                    let _ = fs::remove_dir_all(&staging);
                    return Err(error);
                }
            };
            if staged.name != candidate.name
                || staged.kind != candidate.kind
                || staged.version != candidate.version
                || staged.provenance.content_digest != candidate.provenance.content_digest
            {
                let _ = fs::remove_dir_all(&staging);
                return Err(LocalPackageError::InstalledContentIntegrity {
                    name: candidate.name.clone(),
                    version: candidate.version.clone(),
                    reason: "source changed while it was being copied".to_owned(),
                });
            }
            if let Some(parent) = destination.parent() {
                create_dir(parent)?;
            }
            fs::rename(&staging, &destination)
                .map_err(|error| LocalPackageError::io(&destination, error))?;
        }
        Ok(PackageVersion {
            version: candidate.version.clone(),
            content_path: destination,
            source_path: candidate.source_path.clone(),
            provenance: candidate.provenance.clone(),
            installed_at: Utc::now(),
        })
    }

    fn lock(&self) -> Result<File, LocalPackageError> {
        let path = self.root.join(LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| LocalPackageError::io(&path, error))?;
        file.lock_exclusive()
            .map_err(|error| LocalPackageError::io(&path, error))?;
        Ok(file)
    }

    fn state_path(&self) -> PathBuf {
        self.root.join(STATE_FILE)
    }

    fn read_state(&self) -> Result<LocalState, LocalPackageError> {
        let path = self.state_path();
        let bytes = fs::read(&path).map_err(|error| LocalPackageError::io(&path, error))?;
        let state: LocalState = serde_json::from_slice(&bytes)
            .map_err(|error| LocalPackageError::CorruptState(error.to_string()))?;
        if state.schema_version != STATE_SCHEMA_VERSION {
            return Err(LocalPackageError::UnsupportedStateSchema {
                found: state.schema_version,
                expected: STATE_SCHEMA_VERSION,
            });
        }
        Ok(state)
    }

    fn write_state(&self, state: &LocalState) -> Result<(), LocalPackageError> {
        let destination = self.state_path();
        let temporary = self
            .root
            .join(format!(".{STATE_FILE}.{}.tmp", Uuid::now_v7()));
        let bytes = serde_json::to_vec_pretty(state)
            .map_err(|error| LocalPackageError::CorruptState(error.to_string()))?;
        let mut file =
            File::create(&temporary).map_err(|error| LocalPackageError::io(&temporary, error))?;
        file.write_all(&bytes)
            .map_err(|error| LocalPackageError::io(&temporary, error))?;
        file.sync_all()
            .map_err(|error| LocalPackageError::io(&temporary, error))?;
        fs::rename(&temporary, &destination)
            .map_err(|error| LocalPackageError::io(&destination, error))?;
        File::open(&self.root)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| LocalPackageError::io(&self.root, error))?;
        Ok(())
    }

    fn validate_state(&self, state: &LocalState) -> Result<(), LocalPackageError> {
        for (key, package) in &state.packages {
            if key != &package.name {
                return Err(LocalPackageError::CorruptState(format!(
                    "package map key '{key}' does not match record name '{}'",
                    package.name
                )));
            }
            for version in std::iter::once(&package.active).chain(package.history.iter()) {
                let expected_path = self
                    .root
                    .join(PACKAGES_DIR)
                    .join(&package.name)
                    .join(version.version.to_string())
                    .join("content");
                if version.content_path != expected_path {
                    return Err(LocalPackageError::CorruptState(format!(
                        "content path for '{}@{}' is outside its immutable version directory",
                        package.name, version.version
                    )));
                }
                let candidate = inspect_source(&version.content_path).map_err(|error| {
                    LocalPackageError::InstalledContentIntegrity {
                        name: package.name.clone(),
                        version: version.version.clone(),
                        reason: error.to_string(),
                    }
                })?;
                if candidate.name != package.name
                    || candidate.kind != package.kind
                    || candidate.version != version.version
                    || candidate.provenance.content_digest != version.provenance.content_digest
                {
                    return Err(LocalPackageError::InstalledContentIntegrity {
                        name: package.name.clone(),
                        version: version.version.clone(),
                        reason: "manifest identity or calculated digest changed".to_owned(),
                    });
                }
            }
        }
        Ok(())
    }

    fn cleanup_abandoned_content(&self, state: &LocalState) -> Result<(), LocalPackageError> {
        cleanup_directory_contents(&self.root.join(STAGING_DIR))?;
        cleanup_directory_contents(&self.root.join(TRASH_DIR))?;
        let packages_root = self.root.join(PACKAGES_DIR);
        let entries = fs::read_dir(&packages_root)
            .map_err(|error| LocalPackageError::io(&packages_root, error))?;
        for entry in entries {
            let entry = entry.map_err(|error| LocalPackageError::io(&packages_root, error))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !state.packages.contains_key(&name) {
                let path = entry.path();
                if entry
                    .file_type()
                    .map_err(|error| LocalPackageError::io(&path, error))?
                    .is_dir()
                {
                    fs::remove_dir_all(&path)
                        .map_err(|error| LocalPackageError::io(&path, error))?;
                } else {
                    return Err(LocalPackageError::UnsupportedFileType(path));
                }
            }
        }
        Ok(())
    }
}

/// Parse, validate, and hash a local package source without opening a store.
pub fn inspect_local_package(
    source: impl AsRef<Path>,
) -> Result<PackageCandidate, LocalPackageError> {
    inspect_source(source.as_ref())
}

fn inspect_source(source: &Path) -> Result<PackageCandidate, LocalPackageError> {
    if !source.is_dir() {
        return Err(LocalPackageError::SourceNotDirectory(source.to_path_buf()));
    }
    let source_path = source
        .canonicalize()
        .map_err(|error| LocalPackageError::io(source, error))?;
    let plugin_path = source_path.join(PLUGIN_MANIFEST);
    let kit_path = source_path.join(KIT_MANIFEST);
    match (plugin_path.is_file(), kit_path.is_file()) {
        (true, false) => inspect_plugin(source_path, &plugin_path),
        (false, true) => inspect_kit(source_path, &kit_path),
        _ => Err(LocalPackageError::AmbiguousManifest),
    }
}

fn inspect_plugin(
    source_path: PathBuf,
    manifest_path: &Path,
) -> Result<PackageCandidate, LocalPackageError> {
    let content = fs::read_to_string(manifest_path)
        .map_err(|error| LocalPackageError::io(manifest_path, error))?;
    let manifest = PluginManifest::from_toml(&content)
        .map_err(|error| LocalPackageError::InvalidManifest(error.to_string()))?;
    let version = manifest
        .version()
        .map_err(|error| LocalPackageError::InvalidManifest(error.to_string()))?;
    let calculated = digest_tree(&source_path, Some(&manifest))?;
    let declared = manifest.provenance.content_digest.as_deref();
    let declared_digest_verified = match declared {
        Some(value) => {
            let normalized = normalize_digest(value)?;
            if normalized != calculated {
                return Err(LocalPackageError::DigestMismatch {
                    declared: normalized,
                    calculated,
                });
            }
            true
        }
        None => false,
    };
    let has_signature_claim = manifest.provenance.cosign_bundle.is_some()
        || manifest.provenance.signer_identity.is_some()
        || manifest.provenance.rekor_log_index.is_some()
        || manifest.provenance.slsa_provenance.is_some();
    Ok(PackageCandidate {
        name: manifest.plugin.name,
        kind: PackageKind::Plugin,
        version,
        source_path,
        provenance: PackageProvenance {
            status: if has_signature_claim {
                ProvenanceStatus::SignatureClaimUnverified
            } else {
                ProvenanceStatus::Unsigned
            },
            signer_identity: manifest.provenance.signer_identity,
            content_digest: calculated,
            declared_digest_verified,
            signature_cryptographically_verified: false,
        },
    })
}

fn inspect_kit(
    source_path: PathBuf,
    manifest_path: &Path,
) -> Result<PackageCandidate, LocalPackageError> {
    let content = fs::read_to_string(manifest_path)
        .map_err(|error| LocalPackageError::io(manifest_path, error))?;
    let manifest = KitManifest::from_toml(&content)
        .map_err(|error| LocalPackageError::InvalidManifest(error.to_string()))?;
    let version = manifest
        .version()
        .map_err(|error| LocalPackageError::InvalidManifest(error.to_string()))?;
    let calculated = digest_tree(&source_path, None)?;
    Ok(PackageCandidate {
        name: manifest.kit.name,
        kind: PackageKind::Kit,
        version,
        source_path,
        provenance: PackageProvenance {
            status: ProvenanceStatus::Unsigned,
            signer_identity: None,
            content_digest: calculated,
            declared_digest_verified: false,
            signature_cryptographically_verified: false,
        },
    })
}

/// Hash paths and content in lexical order. For plugin manifests the digest
/// field is cleared and the parsed manifest is hashed as canonical JSON,
/// avoiding a self-referential digest while still protecting the manifest.
fn digest_tree(
    root: &Path,
    plugin_manifest: Option<&PluginManifest>,
) -> Result<String, LocalPackageError> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    let mut hasher = blake3::Hasher::new();
    for relative in files {
        let relative_text = relative
            .to_str()
            .ok_or_else(|| LocalPackageError::UnsupportedPathEncoding(relative.clone()))?;
        let path_bytes = relative_text.as_bytes();
        hasher.update(&(path_bytes.len() as u64).to_le_bytes());
        hasher.update(path_bytes);
        let content = if relative == Path::new(PLUGIN_MANIFEST) {
            if let Some(manifest) = plugin_manifest {
                let mut canonical = manifest.clone();
                canonical.provenance.content_digest = None;
                serde_json::to_vec(&canonical)
                    .map_err(|error| LocalPackageError::InvalidManifest(error.to_string()))?
            } else {
                read_file(&root.join(&relative))?
            }
        } else {
            read_file(&root.join(&relative))?
        };
        hasher.update(&(content.len() as u64).to_le_bytes());
        hasher.update(&content);
    }
    Ok(format!("blake3:{}", hasher.finalize().to_hex()))
}

fn normalize_digest(value: &str) -> Result<String, LocalPackageError> {
    let hex = value.strip_prefix("blake3:").unwrap_or(value);
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(LocalPackageError::InvalidDigest(value.to_owned()));
    }
    Ok(format!("blake3:{}", hex.to_ascii_lowercase()))
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), LocalPackageError> {
    let entries =
        fs::read_dir(directory).map_err(|error| LocalPackageError::io(directory, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| LocalPackageError::io(directory, error))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| LocalPackageError::io(&path, error))?;
        if file_type.is_dir() {
            collect_files(root, &path, files)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|error| LocalPackageError::CorruptState(error.to_string()))?;
            files.push(relative.to_path_buf());
        } else {
            return Err(LocalPackageError::UnsupportedFileType(path));
        }
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), LocalPackageError> {
    create_dir(destination)?;
    let entries = fs::read_dir(source).map_err(|error| LocalPackageError::io(source, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| LocalPackageError::io(source, error))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| LocalPackageError::io(&source_path, error))?;
        if file_type.is_dir() {
            copy_tree(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &destination_path)
                .map_err(|error| LocalPackageError::io(&source_path, error))?;
        } else {
            return Err(LocalPackageError::UnsupportedFileType(source_path));
        }
    }
    Ok(())
}

fn create_dir(path: &Path) -> Result<(), LocalPackageError> {
    fs::create_dir_all(path).map_err(|error| LocalPackageError::io(path, error))
}

fn read_file(path: &Path) -> Result<Vec<u8>, LocalPackageError> {
    let mut file = File::open(path).map_err(|error| LocalPackageError::io(path, error))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| LocalPackageError::io(path, error))?;
    Ok(bytes)
}

fn cleanup_directory_contents(directory: &Path) -> Result<(), LocalPackageError> {
    let entries =
        fs::read_dir(directory).map_err(|error| LocalPackageError::io(directory, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| LocalPackageError::io(directory, error))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| LocalPackageError::io(&path, error))?;
        if file_type.is_dir() {
            fs::remove_dir_all(&path).map_err(|error| LocalPackageError::io(&path, error))?;
        } else if file_type.is_file() {
            fs::remove_file(&path).map_err(|error| LocalPackageError::io(&path, error))?;
        } else {
            return Err(LocalPackageError::UnsupportedFileType(path));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_plugin(directory: &Path, version: &str, body: &str, digest: Option<&str>) {
        fs::create_dir_all(directory).expect("create package");
        fs::write(directory.join("payload.txt"), body).expect("write payload");
        let digest_line = digest
            .map(|value| format!("content_digest = \"{value}\""))
            .unwrap_or_default();
        fs::write(
            directory.join(PLUGIN_MANIFEST),
            format!(
                r#"[plugin]
name = "test-plugin"
version = "{version}"

[provenance]
{digest_line}
"#
            ),
        )
        .expect("write manifest");
    }

    fn development_store(root: &Path) -> LocalPackageStore {
        LocalPackageStore::open(root, LocalInstallPolicy::development()).expect("open store")
    }

    fn write_kit(directory: &Path) {
        fs::create_dir_all(directory).expect("create kit");
        fs::write(
            directory.join(KIT_MANIFEST),
            r#"[kit]
name = "test-kit"
version = "2.0.0"
description = "local kit"

[skills]
skill-a = { version = "^1.0", role = "primary" }
"#,
        )
        .expect("write kit manifest");
    }

    #[test]
    fn install_persists_across_restart_and_is_idempotent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("source");
        let root = temp.path().join("registry");
        write_plugin(&source, "1.0.0", "hello", None);
        let store = development_store(&root);
        let installed = store.install(&source).expect("install");
        assert!(matches!(installed, InstallOutcome::Installed(_)));
        let repeated = store.install(&source).expect("repeat install");
        assert!(matches!(repeated, InstallOutcome::AlreadyInstalled(_)));
        drop(store);

        let reopened = development_store(&root);
        let package = reopened
            .get("test-plugin")
            .expect("read")
            .expect("installed");
        assert_eq!(package.active.version, Version::new(1, 0, 0));
        assert!(package.active.content_path.join("payload.txt").is_file());
        assert_eq!(reopened.list().expect("list").len(), 1);
    }

    #[test]
    fn update_and_rollback_survive_restart() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source_v1 = temp.path().join("v1");
        let source_v2 = temp.path().join("v2");
        let root = temp.path().join("registry");
        write_plugin(&source_v1, "1.0.0", "one", None);
        write_plugin(&source_v2, "1.1.0", "two", None);
        let store = development_store(&root);
        store.install(&source_v1).expect("install v1");
        let updated = store.update(&source_v2).expect("update");
        let UpdateOutcome::Updated(package) = updated else {
            panic!("expected update");
        };
        assert_eq!(package.active.version, Version::new(1, 1, 0));
        assert_eq!(package.history.len(), 1);
        drop(store);

        let reopened = development_store(&root);
        let rolled = reopened.rollback("test-plugin", None).expect("rollback");
        let RollbackOutcome::RolledBack(package) = rolled;
        assert_eq!(package.active.version, Version::new(1, 0, 0));
        assert_eq!(package.history.len(), 1);
        assert_eq!(package.history[0].version, Version::new(1, 1, 0));

        // Updating back to a retained newer version selects it without
        // leaving the active version duplicated in history.
        let updated = reopened.update(&source_v2).expect("reselect v2");
        let UpdateOutcome::Updated(package) = updated else {
            panic!("expected update");
        };
        assert_eq!(package.active.version, Version::new(1, 1, 0));
        assert_eq!(package.history.len(), 1);
        assert_eq!(package.history[0].version, Version::new(1, 0, 0));
        drop(reopened);

        let reopened = development_store(&root);
        let package = reopened
            .get("test-plugin")
            .expect("get")
            .expect("installed");
        assert_eq!(package.active.version, Version::new(1, 1, 0));
    }

    #[test]
    fn failed_update_preserves_active_version() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source_v1 = temp.path().join("v1");
        let source_old = temp.path().join("old");
        let root = temp.path().join("registry");
        write_plugin(&source_v1, "1.0.0", "one", None);
        write_plugin(&source_old, "0.9.0", "old", None);
        let store = development_store(&root);
        store.install(&source_v1).expect("install");
        assert!(matches!(
            store.update(&source_old),
            Err(LocalPackageError::NotNewer { .. })
        ));
        assert_eq!(
            store
                .get("test-plugin")
                .expect("get")
                .expect("installed")
                .active
                .version,
            Version::new(1, 0, 0)
        );
    }

    #[test]
    fn same_version_with_changed_content_is_rejected() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source_a = temp.path().join("a");
        let source_b = temp.path().join("b");
        let root = temp.path().join("registry");
        write_plugin(&source_a, "1.0.0", "one", None);
        write_plugin(&source_b, "1.0.0", "tampered", None);
        let store = development_store(&root);
        store.install(&source_a).expect("install");
        assert!(matches!(
            store.install(&source_b),
            Err(LocalPackageError::ImmutableVersionConflict { .. })
        ));
    }

    #[test]
    fn declared_digest_is_verified_and_tampering_fails() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("source");
        let root = temp.path().join("registry");
        write_plugin(&source, "1.0.0", "original", None);
        let probe = development_store(&root).inspect(&source).expect("inspect");
        write_plugin(
            &source,
            "1.0.0",
            "original",
            Some(&probe.provenance.content_digest),
        );
        let candidate = development_store(&root).inspect(&source).expect("verify");
        assert!(candidate.provenance.declared_digest_verified);
        fs::write(source.join("payload.txt"), "tampered").expect("tamper");
        assert!(matches!(
            development_store(&root).inspect(&source),
            Err(LocalPackageError::DigestMismatch { .. })
        ));
    }

    #[test]
    fn strict_policy_rejects_unsigned_before_materializing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("source");
        let root = temp.path().join("registry");
        write_plugin(&source, "1.0.0", "hello", None);
        let store = LocalPackageStore::open(&root, LocalInstallPolicy::strict()).expect("open");
        assert!(matches!(
            store.install(&source),
            Err(LocalPackageError::UnsignedRejected(_))
        ));
        assert!(store.list().expect("list").is_empty());
    }

    #[test]
    fn kit_manifest_uses_the_same_durable_lifecycle() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("kit");
        let root = temp.path().join("registry");
        write_kit(&source);
        let store = development_store(&root);
        let InstallOutcome::Installed(package) = store.install(&source).expect("install kit")
        else {
            panic!("expected install");
        };
        assert_eq!(package.kind, PackageKind::Kit);
        assert_eq!(package.active.version, Version::new(2, 0, 0));
    }

    #[test]
    fn unverified_signature_claim_is_never_reported_as_verified() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("source");
        let root = temp.path().join("registry");
        write_plugin(&source, "1.0.0", "hello", None);
        let path = source.join(PLUGIN_MANIFEST);
        let mut manifest = fs::read_to_string(&path).expect("read manifest");
        manifest.push_str("cosign_bundle = \"claim\"\nsigner_identity = \"publisher\"\n");
        fs::write(&path, manifest).expect("write claims");
        let candidate = development_store(&root).inspect(&source).expect("inspect");
        assert_eq!(
            candidate.provenance.status,
            ProvenanceStatus::SignatureClaimUnverified
        );
        assert!(!candidate.provenance.signature_cryptographically_verified);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_source_is_rejected() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("source");
        let root = temp.path().join("registry");
        write_plugin(&source, "1.0.0", "hello", None);
        symlink(source.join("payload.txt"), source.join("escape")).expect("symlink");
        assert!(matches!(
            development_store(&root).install(&source),
            Err(LocalPackageError::UnsupportedFileType(_))
        ));
    }

    #[test]
    fn uninstall_is_durable_and_idempotent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("source");
        let root = temp.path().join("registry");
        write_plugin(&source, "1.0.0", "hello", None);
        let store = development_store(&root);
        store.install(&source).expect("install");
        assert_eq!(
            store.uninstall("test-plugin").expect("uninstall"),
            UninstallOutcome::Uninstalled
        );
        assert_eq!(
            store.uninstall("test-plugin").expect("repeat"),
            UninstallOutcome::NotInstalled
        );
        drop(store);
        assert!(development_store(&root)
            .get("test-plugin")
            .expect("get")
            .is_none());
        assert!(!root.join(PACKAGES_DIR).join("test-plugin").exists());
    }

    #[test]
    fn separate_instances_do_not_overwrite_each_others_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("registry");
        let first_source = temp.path().join("first");
        let second_source = temp.path().join("second");
        write_plugin(&first_source, "1.0.0", "one", None);
        write_plugin(&second_source, "1.0.0", "two", None);
        fs::write(
            second_source.join(PLUGIN_MANIFEST),
            fs::read_to_string(second_source.join(PLUGIN_MANIFEST))
                .expect("read")
                .replace("test-plugin", "other-plugin"),
        )
        .expect("rename package");
        let first = development_store(&root);
        let second = development_store(&root);
        first.install(&first_source).expect("first install");
        second.install(&second_source).expect("second install");
        assert_eq!(first.list().expect("list").len(), 2);
    }

    #[test]
    fn concurrent_instances_serialize_state_changes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("registry");
        let first_source = temp.path().join("first");
        let second_source = temp.path().join("second");
        write_plugin(&first_source, "1.0.0", "one", None);
        write_plugin(&second_source, "1.0.0", "two", None);
        fs::write(
            second_source.join(PLUGIN_MANIFEST),
            fs::read_to_string(second_source.join(PLUGIN_MANIFEST))
                .expect("read")
                .replace("test-plugin", "other-plugin"),
        )
        .expect("rename package");
        let first = development_store(&root);
        let second = development_store(&root);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let barrier_one = barrier.clone();
        let first_thread = std::thread::spawn(move || {
            barrier_one.wait();
            first.install(first_source)
        });
        let barrier_two = barrier.clone();
        let second_thread = std::thread::spawn(move || {
            barrier_two.wait();
            second.install(second_source)
        });
        barrier.wait();
        first_thread.join().expect("first thread").expect("first");
        second_thread
            .join()
            .expect("second thread")
            .expect("second");
        assert_eq!(development_store(&root).list().expect("list").len(), 2);
    }

    #[test]
    fn installed_content_tampering_is_detected_on_restart() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("source");
        let root = temp.path().join("registry");
        write_plugin(&source, "1.0.0", "hello", None);
        let store = development_store(&root);
        let package = match store.install(&source).expect("install") {
            InstallOutcome::Installed(package) => package,
            InstallOutcome::AlreadyInstalled(_) => panic!("unexpected existing package"),
        };
        fs::write(package.active.content_path.join("payload.txt"), "tampered")
            .expect("tamper installed content");
        drop(store);
        assert!(matches!(
            LocalPackageStore::open(&root, LocalInstallPolicy::development()),
            Err(LocalPackageError::InstalledContentIntegrity { .. })
        ));
    }

    #[test]
    fn source_cannot_contain_store_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("source");
        let root = source.join("registry");
        write_plugin(&source, "1.0.0", "hello", None);
        let store = development_store(&root);
        assert!(matches!(
            store.install(&source),
            Err(LocalPackageError::SourceContainsStore { .. })
        ));
    }

    #[test]
    fn corrupt_state_fails_open_instead_of_resetting() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("registry");
        development_store(&root);
        fs::write(root.join(STATE_FILE), b"not-json").expect("corrupt");
        assert!(matches!(
            LocalPackageStore::open(&root, LocalInstallPolicy::development()),
            Err(LocalPackageError::CorruptState(_))
        ));
    }

    #[test]
    fn open_existing_does_not_create_a_missing_store() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("missing");
        assert!(matches!(
            LocalPackageStore::open_existing(&root, LocalInstallPolicy::development()),
            Err(LocalPackageError::SourceNotDirectory(_))
        ));
        assert!(!root.exists());
    }
}
