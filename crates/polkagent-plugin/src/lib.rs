//! `polkagent-plugin` — WASM-free plugin loading from TOML manifests with
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
//! | [`manifest`] | [`PluginManifest`] deserialization from TOML, [`PluginId`] type, validation. |
//! | [`capability`] | [`PluginCapability`] enum and [`CapabilitySet`] for intersection/union/gating. |
//! | [`loader`] | [`PluginLoader`] for discovering and loading manifests from the filesystem. |
//! | [`registry`] | [`PluginRegistry`] for registering, looking up, and managing plugins. |
//! | [`sandbox`] | [`PluginSandbox`] capability checker that gates plugin operations. |
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

pub mod capability;
pub mod dependency;
pub mod error;
pub mod lifecycle;
pub mod loader;
pub mod manifest;
pub mod registry;
pub mod sandbox;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use capability::{CapabilitySet, PluginCapability};
pub use dependency::DependencyResolver;
pub use error::PluginError;
pub use lifecycle::{HealthStatus, PluginLifecycle, PluginState};
pub use loader::PluginLoader;
pub use manifest::{PluginId, PluginManifest};
pub use registry::PluginRegistry;
pub use sandbox::PluginSandbox;

// ---------------------------------------------------------------------------
// PluginManager — top-level orchestrator
// ---------------------------------------------------------------------------

/// Top-level orchestrator that wires together loading, dependency resolution,
/// capability validation, and registry management.
///
/// The `PluginManager` provides a single entry point for common workflows:
/// discovering plugins, resolving their dependency graph, registering them,
/// and setting up sandboxing.
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
}

impl PluginManager {
    /// Create a new `PluginManager` with default search paths.
    pub fn new() -> Self {
        Self {
            loader: PluginLoader::with_default_paths(),
            resolver: DependencyResolver::new(),
            registry: PluginRegistry::new(),
            sandbox: PluginSandbox::new(),
        }
    }

    /// Create a new `PluginManager` with custom search paths.
    pub fn with_search_paths(search_paths: Vec<std::path::PathBuf>) -> Self {
        Self {
            loader: PluginLoader::new(search_paths),
            resolver: DependencyResolver::new(),
            registry: PluginRegistry::new(),
            sandbox: PluginSandbox::new(),
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
            self.sandbox.grant(&manifest.plugin.name, caps);
        }

        Ok(order)
    }

    /// Get a reference to the plugin registry.
    pub fn registry(&self) -> &PluginRegistry {
        &self.registry
    }

    /// Get a reference to the plugin sandbox.
    pub fn sandbox(&self) -> &PluginSandbox {
        &self.sandbox
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
        }
    }

    #[test]
    fn manager_load_and_check_capability() {
        let manager = PluginManager::with_search_paths(vec![]);

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
        let manager = PluginManager::with_search_paths(vec![]);

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
    }
}
