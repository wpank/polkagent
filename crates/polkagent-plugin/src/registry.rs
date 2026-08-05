//! Plugin registry: stores loaded plugins and provides lookup by name or
//! capability.
//!
//! The [`PluginRegistry`] is the central index of all loaded plugins. It
//! stores plugin manifests alongside their lifecycle state and supports
//! querying by name or by required capability.

use std::collections::HashMap;

use parking_lot::RwLock;
use tracing::{debug, info};

use crate::capability::PluginCapability;
use crate::error::PluginError;
use crate::lifecycle::{PluginInstance, PluginState};
use crate::manifest::PluginManifest;

// ---------------------------------------------------------------------------
// RegisteredPlugin
// ---------------------------------------------------------------------------

/// A plugin that has been loaded and registered.
#[derive(Debug)]
pub struct RegisteredPlugin {
    /// The parsed manifest.
    pub manifest: PluginManifest,
    /// Lifecycle state tracking.
    pub instance: PluginInstance,
}

// ---------------------------------------------------------------------------
// PluginRegistry
// ---------------------------------------------------------------------------

/// A thread-safe registry of loaded plugins.
///
/// Plugins are registered by name and can be looked up individually or
/// filtered by capability.
#[derive(Debug)]
pub struct PluginRegistry {
    /// Plugins indexed by name.
    plugins: RwLock<HashMap<String, RegisteredPlugin>>,
}

impl PluginRegistry {
    /// Create a new, empty registry.
    pub fn new() -> Self {
        Self {
            plugins: RwLock::new(HashMap::new()),
        }
    }

    /// Register a plugin from its manifest.
    ///
    /// If a plugin with the same name is already registered, it is replaced.
    pub fn register(&self, manifest: PluginManifest) {
        let name = manifest.plugin.name.clone();
        info!(plugin = %name, version = %manifest.plugin.version, "registering plugin");

        let instance = PluginInstance::new(&name);
        let entry = RegisteredPlugin { manifest, instance };
        self.plugins.write().insert(name, entry);
    }

    /// Unregister a plugin by name.
    ///
    /// Returns `true` if the plugin was found and removed.
    pub fn unregister(&self, name: &str) -> bool {
        let removed = self.plugins.write().remove(name).is_some();
        if removed {
            debug!(plugin = %name, "unregistered plugin");
        }
        removed
    }

    /// Look up a plugin by name.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::NotFound`] if no plugin with the given name
    /// is registered.
    pub fn get(&self, name: &str) -> Result<PluginManifest, PluginError> {
        self.plugins
            .read()
            .get(name)
            .map(|entry| entry.manifest.clone())
            .ok_or_else(|| PluginError::NotFound {
                name: name.to_string(),
            })
    }

    /// Get the lifecycle state of a registered plugin.
    pub fn get_state(&self, name: &str) -> Option<PluginState> {
        self.plugins
            .read()
            .get(name)
            .map(|entry| entry.instance.state)
    }

    /// Update the lifecycle state of a registered plugin.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::NotFound`] if no plugin with the given name
    /// is registered.
    pub fn set_state(&self, name: &str, state: PluginState) -> Result<(), PluginError> {
        let mut plugins = self.plugins.write();
        match plugins.get_mut(name) {
            Some(entry) => {
                debug!(plugin = %name, from = %entry.instance.state, to = %state, "state transition");
                entry.instance.transition(state);
                Ok(())
            }
            None => Err(PluginError::NotFound {
                name: name.to_string(),
            }),
        }
    }

    /// Return all registered plugin names.
    pub fn names(&self) -> Vec<String> {
        self.plugins.read().keys().cloned().collect()
    }

    /// Return all registered plugin manifests.
    pub fn manifests(&self) -> Vec<PluginManifest> {
        self.plugins
            .read()
            .values()
            .map(|entry| entry.manifest.clone())
            .collect()
    }

    /// Find all plugins that declare a specific capability as required.
    pub fn find_by_capability(&self, capability: PluginCapability) -> Vec<PluginManifest> {
        let cap_str = capability.to_string();
        self.plugins
            .read()
            .values()
            .filter(|entry| entry.manifest.capabilities.required.contains(&cap_str))
            .map(|entry| entry.manifest.clone())
            .collect()
    }

    /// Return the number of registered plugins.
    pub fn len(&self) -> usize {
        self.plugins.read().len()
    }

    /// Check whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.plugins.read().is_empty()
    }

    /// Return all plugins currently in a specific lifecycle state.
    pub fn find_by_state(&self, state: PluginState) -> Vec<PluginManifest> {
        self.plugins
            .read()
            .values()
            .filter(|entry| entry.instance.state == state)
            .map(|entry| entry.manifest.clone())
            .collect()
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "registry tests intentionally panic at lookup, transition, and rejection fixture boundaries"
)]
mod tests {
    use super::*;
    use crate::manifest::{CapabilitiesSection, PluginSection, ProvenanceSection};

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
            provenance: ProvenanceSection::default(),
        }
    }

    #[test]
    fn register_and_get() {
        let registry = PluginRegistry::new();
        let manifest = make_manifest("my-plugin", "1.0.0", &[]);
        registry.register(manifest);

        let retrieved = registry.get("my-plugin").expect("should find");
        assert_eq!(retrieved.plugin.name, "my-plugin");
        assert_eq!(retrieved.plugin.version, "1.0.0");
    }

    #[test]
    fn get_not_found() {
        let registry = PluginRegistry::new();
        let result = registry.get("nonexistent");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("nonexistent"));
    }

    #[test]
    fn unregister() {
        let registry = PluginRegistry::new();
        registry.register(make_manifest("to-remove", "1.0.0", &[]));
        assert_eq!(registry.len(), 1);

        assert!(registry.unregister("to-remove"));
        assert_eq!(registry.len(), 0);
        assert!(registry.get("to-remove").is_err());
    }

    #[test]
    fn unregister_nonexistent() {
        let registry = PluginRegistry::new();
        assert!(!registry.unregister("nope"));
    }

    #[test]
    fn find_by_capability() {
        let registry = PluginRegistry::new();
        registry.register(make_manifest("a", "1.0.0", &["chain.query"]));
        registry.register(make_manifest("b", "1.0.0", &["chain.query", "memory.read"]));
        registry.register(make_manifest("c", "1.0.0", &["network.http"]));

        let chain_plugins = registry.find_by_capability(PluginCapability::ChainQuery);
        assert_eq!(chain_plugins.len(), 2);
        let names: Vec<&str> = chain_plugins
            .iter()
            .map(|m| m.plugin.name.as_str())
            .collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));

        let network_plugins = registry.find_by_capability(PluginCapability::NetworkAccess);
        assert_eq!(network_plugins.len(), 1);
        assert_eq!(network_plugins[0].plugin.name, "c");

        let fs_plugins = registry.find_by_capability(PluginCapability::ReadFileSystem);
        assert!(fs_plugins.is_empty());
    }

    #[test]
    fn lifecycle_state_management() {
        let registry = PluginRegistry::new();
        registry.register(make_manifest("plugin", "1.0.0", &[]));

        assert_eq!(registry.get_state("plugin"), Some(PluginState::Discovered));

        registry
            .set_state("plugin", PluginState::Initialized)
            .expect("should succeed");
        assert_eq!(registry.get_state("plugin"), Some(PluginState::Initialized));

        registry
            .set_state("plugin", PluginState::Running)
            .expect("should succeed");
        assert_eq!(registry.get_state("plugin"), Some(PluginState::Running));
    }

    #[test]
    fn set_state_not_found() {
        let registry = PluginRegistry::new();
        let result = registry.set_state("nope", PluginState::Running);
        assert!(result.is_err());
    }

    #[test]
    fn find_by_state() {
        let registry = PluginRegistry::new();
        registry.register(make_manifest("a", "1.0.0", &[]));
        registry.register(make_manifest("b", "1.0.0", &[]));

        registry
            .set_state("a", PluginState::Running)
            .expect("should succeed");

        let running = registry.find_by_state(PluginState::Running);
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].plugin.name, "a");

        let discovered = registry.find_by_state(PluginState::Discovered);
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].plugin.name, "b");
    }

    #[test]
    fn names_and_manifests() {
        let registry = PluginRegistry::new();
        registry.register(make_manifest("x", "1.0.0", &[]));
        registry.register(make_manifest("y", "2.0.0", &[]));

        let mut names = registry.names();
        names.sort();
        assert_eq!(names, vec!["x", "y"]);

        let manifests = registry.manifests();
        assert_eq!(manifests.len(), 2);
    }

    #[test]
    fn register_replaces_existing() {
        let registry = PluginRegistry::new();
        registry.register(make_manifest("dup", "1.0.0", &[]));
        registry.register(make_manifest("dup", "2.0.0", &[]));

        assert_eq!(registry.len(), 1);
        let retrieved = registry.get("dup").expect("should find");
        assert_eq!(retrieved.plugin.version, "2.0.0");
    }

    #[test]
    fn empty_registry() {
        let registry = PluginRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(registry.names().is_empty());
        assert!(registry.manifests().is_empty());
    }
}
