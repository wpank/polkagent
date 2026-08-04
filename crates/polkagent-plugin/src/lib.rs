//! `polkagent-plugin` — plugin loading from TOML manifests with
//! capability-based sandboxing for the Polkagent platform.
//!
//! This crate implements the plugin system that allows agents to discover,
//! load, validate, and manage modular plugins. Plugins are declarative
//! packages described by a `plugin.toml` manifest that declares the
//! plugin's identity, version, required capabilities, and dependencies.
//!
//! # Module overview
//!
//! | Module | Responsibility |
//! |--------|---------------|
//! | [`manifest`] | [`PluginManifest`] deserialization from TOML, [`PluginId`] type, trust/sandbox tiers. |
//! | [`capability`] | [`PluginCapability`] enum and [`CapabilitySet`] for intersection/union/gating. |
//! | [`loader`] | [`PluginLoader`] for discovering and loading manifests from the filesystem. |
//! | [`registry`] | [`PluginRegistry`] for registering, looking up, and managing plugins. |
//! | [`sandbox`] | [`PluginSandbox`] capability checker + [`WasmtimeSandbox`] WASM host-call gating. |
//! | [`verification`] | [`PackageVerifier`] cosign v3 keyless signature verification. |
//! | [`attestation`] | [`AttestationChecker`] SLSA Build L2 provenance attestation. |
//! | [`lifecycle`] | [`PluginLifecycle`] trait with init/start/stop/health_check hooks. |
//! | [`dependency`] | [`DependencyResolver`] for topological sort and version compat checks. |
//! | [`error`] | [`PluginError`] enum covering all failure modes. |
//!
//! # Quick start
//!
//! ```rust,no_run
//! use polkagent_plugin::PluginManager;
//!
//! // Create a plugin manager and discover plugins from the default paths.
//! let manager = PluginManager::new();
//! let manifests = manager.discover_plugins();
//!
//! println!("Discovered {} plugins", manifests.len());
//! for m in &manifests {
//!     println!("  {} v{}", m.plugin.name, m.plugin.version);
//! }
//! ```

#![forbid(unsafe_code)]
#![warn(
    missing_docs,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used
)]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod attestation;
pub mod capability;
pub mod dependency;
pub mod error;
pub mod lifecycle;
pub mod loader;
pub mod manifest;
pub mod registry;
pub mod sandbox;
pub mod verification;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use attestation::{AttestationChecker, AttestationPolicy, SlsaBuildLevel};
pub use capability::{CapabilitySet, PluginCapability};
pub use dependency::DependencyResolver;
pub use error::PluginError;
pub use lifecycle::{HealthStatus, PluginLifecycle, PluginState};
pub use loader::PluginLoader;
pub use manifest::{PluginId, PluginManifest, SandboxTier, TrustTier};
pub use registry::PluginRegistry;
pub use sandbox::{PluginSandbox, SandboxConfig, SandboxResourceLimits, WasmtimeSandbox};
pub use verification::{PackageVerifier, VerificationPolicy};

// ---------------------------------------------------------------------------
// PluginManager — top-level orchestrator
// ---------------------------------------------------------------------------

/// Top-level orchestrator that wires together loading, dependency resolution,
/// capability validation, signature verification, SLSA attestation, and
/// sandbox management.
///
/// The `PluginManager` provides a single entry point for common workflows:
/// discovering plugins, verifying their supply-chain provenance, resolving
/// their dependency graph, registering them, and setting up sandboxing.
#[derive(Debug)]
pub struct PluginManager {
    /// The plugin loader for filesystem discovery.
    loader: PluginLoader,
    /// The dependency resolver.
    resolver: DependencyResolver,
    /// The plugin registry.
    registry: PluginRegistry,
    /// The capability sandbox.
    sandbox: PluginSandbox,
    /// Wasmtime Component Model sandbox for WASM-tier plugins.
    wasm_sandbox: WasmtimeSandbox,
    /// Cosign v3 keyless signature verifier.
    verifier: PackageVerifier,
    /// SLSA Build Level attestation checker.
    attestation_checker: AttestationChecker,
}

impl PluginManager {
    /// Create a new `PluginManager` with default search paths and
    /// strict verification (unsigned packages are rejected).
    pub fn new() -> Self {
        Self {
            loader: PluginLoader::with_default_paths(),
            resolver: DependencyResolver::new(),
            registry: PluginRegistry::new(),
            sandbox: PluginSandbox::new(),
            wasm_sandbox: WasmtimeSandbox::new(),
            verifier: PackageVerifier::strict(),
            attestation_checker: AttestationChecker::strict(),
        }
    }

    /// Create a new `PluginManager` with custom search paths and
    /// strict verification.
    pub fn with_search_paths(search_paths: Vec<std::path::PathBuf>) -> Self {
        Self {
            loader: PluginLoader::new(search_paths),
            resolver: DependencyResolver::new(),
            registry: PluginRegistry::new(),
            sandbox: PluginSandbox::new(),
            wasm_sandbox: WasmtimeSandbox::new(),
            verifier: PackageVerifier::strict(),
            attestation_checker: AttestationChecker::strict(),
        }
    }

    /// Create a permissive `PluginManager` that accepts unsigned
    /// packages and does not require SLSA attestation. For development
    /// and sideloading workflows.
    pub fn permissive(search_paths: Vec<std::path::PathBuf>) -> Self {
        Self {
            loader: PluginLoader::new(search_paths),
            resolver: DependencyResolver::new(),
            registry: PluginRegistry::new(),
            sandbox: PluginSandbox::new(),
            wasm_sandbox: WasmtimeSandbox::new(),
            verifier: PackageVerifier::permissive(),
            attestation_checker: AttestationChecker::permissive(),
        }
    }

    /// Discover all plugins from the configured search paths.
    pub fn discover_plugins(&self) -> Vec<PluginManifest> {
        self.loader.discover_plugins()
    }

    /// Resolve the dependency graph for a set of plugin manifests.
    ///
    /// Returns plugins in topological (dependency-first) order.
    pub fn resolve_dependencies(
        &self,
        manifests: &[PluginManifest],
    ) -> Result<Vec<PluginId>, PluginError> {
        self.resolver.resolve(manifests)
    }

    /// Load and register a set of plugins.
    ///
    /// This resolves dependencies, registers each plugin in the registry,
    /// and sets up sandbox capability grants based on each plugin's
    /// declared required capabilities.
    ///
    /// Returns the plugins in dependency order.
    pub fn load_plugins(
        &self,
        manifests: Vec<PluginManifest>,
    ) -> Result<Vec<PluginId>, PluginError> {
        let order = self.resolver.resolve(&manifests)?;

        // Register and sandbox each plugin.
        for manifest in &manifests {
            let caps = manifest.required_capabilities()?;
            self.registry.register(manifest.clone());
            self.sandbox.grant(&manifest.plugin.name, caps.clone());

            // Set up Wasmtime sandbox for WASM-tier plugins.
            let tier = manifest.sandbox_tier();
            if tier == SandboxTier::Wasm {
                let config = SandboxConfig::new(tier, caps);
                self.wasm_sandbox.register(&manifest.plugin.name, config);
            }
        }

        Ok(order)
    }

    /// Load plugins with full supply-chain verification.
    ///
    /// Each manifest is verified for cosign signature and SLSA
    /// attestation before being registered. Unsigned packages or
    /// packages below the required SLSA level are rejected.
    pub fn load_plugins_verified(
        &self,
        manifests: Vec<PluginManifest>,
    ) -> Result<Vec<PluginId>, PluginError> {
        // Verify each manifest before loading.
        for manifest in &manifests {
            self.verifier.verify(manifest)?;
            self.attestation_checker.check(manifest)?;
        }

        self.load_plugins(manifests)
    }

    /// Get a reference to the plugin registry.
    pub fn registry(&self) -> &PluginRegistry {
        &self.registry
    }

    /// Get a reference to the plugin sandbox.
    pub fn sandbox(&self) -> &PluginSandbox {
        &self.sandbox
    }

    /// Get a reference to the Wasmtime sandbox.
    pub fn wasm_sandbox(&self) -> &WasmtimeSandbox {
        &self.wasm_sandbox
    }

    /// Get a reference to the package verifier.
    pub fn verifier(&self) -> &PackageVerifier {
        &self.verifier
    }

    /// Get a reference to the attestation checker.
    pub fn attestation_checker(&self) -> &AttestationChecker {
        &self.attestation_checker
    }

    /// Check whether a plugin is allowed to perform an operation requiring
    /// the given capability.
    pub fn check_capability(
        &self,
        plugin_name: &str,
        capability: PluginCapability,
    ) -> Result<(), PluginError> {
        self.sandbox.check(plugin_name, capability)
    }

    /// Check a Wasmtime host-call for a sandboxed plugin.
    pub fn check_host_call(
        &self,
        plugin_name: &str,
        capability: PluginCapability,
    ) -> Result<(), PluginError> {
        self.wasm_sandbox.check_host_call(plugin_name, capability)
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{CapabilitiesSection, PluginSection};
    use std::collections::HashMap;

    fn make_manifest(name: &str, version: &str, required_caps: &[&str]) -> PluginManifest {
        PluginManifest {
            plugin: PluginSection {
                name: name.to_string(),
                version: version.to_string(),
                description: String::new(),
                author: String::new(),
                license: String::new(),
                entry_point: String::new(),
            },
            capabilities: CapabilitiesSection {
                required: required_caps.iter().map(|s| (*s).to_string()).collect(),
                optional: Vec::new(),
            },
            dependencies: HashMap::new(),
            provenance: Default::default(),
        }
    }

    #[test]
    fn manager_load_and_check_capability() {
        let manager = PluginManager::permissive(vec![]);

        let manifests = vec![
            make_manifest("plugin-a", "1.0.0", &["chain.query", "memory.read"]),
            make_manifest("plugin-b", "1.0.0", &["network.http"]),
        ];

        let order = manager.load_plugins(manifests).expect("should load");
        assert_eq!(order.len(), 2);

        // plugin-a should have chain.query
        assert!(manager
            .check_capability("plugin-a", PluginCapability::ChainQuery)
            .is_ok());
        // plugin-a should NOT have network.http
        assert!(manager
            .check_capability("plugin-a", PluginCapability::NetworkAccess)
            .is_err());

        // plugin-b should have network.http
        assert!(manager
            .check_capability("plugin-b", PluginCapability::NetworkAccess)
            .is_ok());
    }

    #[test]
    fn manager_registry_populated() {
        let manager = PluginManager::permissive(vec![]);

        let manifests = vec![
            make_manifest("alpha", "1.0.0", &[]),
            make_manifest("beta", "2.0.0", &[]),
        ];

        manager.load_plugins(manifests).expect("should load");

        assert_eq!(manager.registry().len(), 2);
        assert!(manager.registry().get("alpha").is_ok());
        assert!(manager.registry().get("beta").is_ok());
    }

    #[test]
    fn manager_default_construction() {
        let manager = PluginManager::default();
        assert!(manager.registry().is_empty());
        assert_eq!(manager.sandbox().plugin_count(), 0);
        assert_eq!(manager.wasm_sandbox().plugin_count(), 0);
    }

    #[test]
    fn manager_verified_load_rejects_unsigned() {
        let manager = PluginManager::with_search_paths(vec![]);
        let manifests = vec![make_manifest("unsigned-pkg", "1.0.0", &[])];

        let result = manager.load_plugins_verified(manifests);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("unsigned"));
    }

    #[test]
    fn manager_wasm_sandbox_for_signed_plugins() {
        use crate::manifest::ProvenanceSection;

        let manager = PluginManager::permissive(vec![]);

        // Create a signed manifest (cosign bundle present, no SLSA → Signed → Wasm tier).
        let mut manifest = make_manifest("wasm-tool", "1.0.0", &["chain.query"]);
        manifest.provenance = ProvenanceSection {
            cosign_bundle: Some("eyJhbGciOi...".to_string()),
            signer_identity: None,
            rekor_log_index: None,
            slsa_provenance: None,
            slsa_build_level: None,
            content_digest: None,
        };
        assert_eq!(manifest.sandbox_tier(), SandboxTier::Wasm);

        manager.load_plugins(vec![manifest]).expect("should load");

        // The wasm sandbox should have the plugin registered.
        assert_eq!(manager.wasm_sandbox().plugin_count(), 1);
        assert!(manager
            .check_host_call("wasm-tool", PluginCapability::ChainQuery)
            .is_ok());
        assert!(manager
            .check_host_call("wasm-tool", PluginCapability::WriteFileSystem)
            .is_err());
    }
}
