//! Capability-based sandbox for plugins.
//!
//! The [`PluginSandbox`] acts as a capability checker that gates plugin
//! operations. There is no actual process isolation or WASM sandboxing;
//! instead, each plugin is assigned a [`CapabilitySet`] at load time,
//! and every operation must pass a capability check before being
//! dispatched.
//!
//! This provides a defense-in-depth layer: even if a plugin's code path
//! reaches an operation handler, the sandbox will reject the call if the
//! plugin's manifest did not declare the required capability.

use parking_lot::RwLock;
use std::collections::HashMap;
use tracing::{debug, warn};

use crate::capability::{CapabilitySet, PluginCapability};
use crate::error::PluginError;

// ---------------------------------------------------------------------------
// PluginSandbox
// ---------------------------------------------------------------------------

/// A capability checker that gates plugin operations.
///
/// The sandbox maintains a mapping from plugin names to their granted
/// capability sets. Before dispatching any operation on behalf of a
/// plugin, the caller should invoke [`check`](Self::check) to verify
/// the plugin has the required capability.
#[derive(Debug)]
pub struct PluginSandbox {
    /// Granted capabilities per plugin, keyed by plugin name.
    grants: RwLock<HashMap<String, CapabilitySet>>,
}

impl PluginSandbox {
    /// Create a new, empty sandbox.
    pub fn new() -> Self {
        Self {
            grants: RwLock::new(HashMap::new()),
        }
    }

    /// Grant a set of capabilities to a plugin.
    ///
    /// If the plugin already has grants, they are replaced entirely.
    pub fn grant(&self, plugin_name: impl Into<String>, capabilities: CapabilitySet) {
        let name = plugin_name.into();
        debug!(plugin = %name, capabilities = %capabilities, "granting capabilities");
        self.grants.write().insert(name, capabilities);
    }

    /// Revoke all capabilities from a plugin.
    pub fn revoke(&self, plugin_name: &str) {
        debug!(plugin = %plugin_name, "revoking all capabilities");
        self.grants.write().remove(plugin_name);
    }

    /// Check whether a plugin has a specific capability.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::CapabilityDenied`] if the plugin does not
    /// have the required capability, or if the plugin is not known to the
    /// sandbox.
    pub fn check(&self, plugin_name: &str, capability: PluginCapability) -> Result<(), PluginError> {
        let grants = self.grants.read();
        match grants.get(plugin_name) {
            Some(set) if set.contains(capability) => {
                debug!(
                    plugin = %plugin_name,
                    capability = %capability,
                    "capability check passed"
                );
                Ok(())
            }
            Some(_) => {
                warn!(
                    plugin = %plugin_name,
                    capability = %capability,
                    "capability denied"
                );
                Err(PluginError::CapabilityDenied {
                    plugin_name: plugin_name.to_string(),
                    capability: capability.to_string(),
                })
            }
            None => {
                warn!(
                    plugin = %plugin_name,
                    capability = %capability,
                    "plugin not registered in sandbox"
                );
                Err(PluginError::CapabilityDenied {
                    plugin_name: plugin_name.to_string(),
                    capability: capability.to_string(),
                })
            }
        }
    }

    /// Check whether a plugin has all of the capabilities in a set.
    ///
    /// Returns the first missing capability as an error, or `Ok(())` if
    /// all are granted.
    pub fn check_all(
        &self,
        plugin_name: &str,
        required: &CapabilitySet,
    ) -> Result<(), PluginError> {
        for cap in required.iter() {
            self.check(plugin_name, cap)?;
        }
        Ok(())
    }

    /// Return the granted capabilities for a plugin (if any).
    pub fn get_grants(&self, plugin_name: &str) -> Option<CapabilitySet> {
        self.grants.read().get(plugin_name).cloned()
    }

    /// Return the number of plugins registered in the sandbox.
    pub fn plugin_count(&self) -> usize {
        self.grants.read().len()
    }
}

impl Default for PluginSandbox {
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

    #[test]
    fn grant_and_check_capability() {
        let sandbox = PluginSandbox::new();
        let caps = CapabilitySet::from_iter([
            PluginCapability::ChainQuery,
            PluginCapability::MemoryAccess,
        ]);
        sandbox.grant("my-plugin", caps);

        assert!(sandbox
            .check("my-plugin", PluginCapability::ChainQuery)
            .is_ok());
        assert!(sandbox
            .check("my-plugin", PluginCapability::MemoryAccess)
            .is_ok());
    }

    #[test]
    fn deny_ungrantable_capability() {
        let sandbox = PluginSandbox::new();
        let caps = CapabilitySet::from_iter([PluginCapability::ChainQuery]);
        sandbox.grant("my-plugin", caps);

        let result = sandbox.check("my-plugin", PluginCapability::WriteFileSystem);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("my-plugin"));
        assert!(msg.contains("fs.write"));
    }

    #[test]
    fn deny_unknown_plugin() {
        let sandbox = PluginSandbox::new();
        let result = sandbox.check("unknown-plugin", PluginCapability::ChainQuery);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("unknown-plugin"));
    }

    #[test]
    fn revoke_capabilities() {
        let sandbox = PluginSandbox::new();
        sandbox.grant(
            "my-plugin",
            CapabilitySet::from_iter([PluginCapability::ChainQuery]),
        );
        assert!(sandbox
            .check("my-plugin", PluginCapability::ChainQuery)
            .is_ok());

        sandbox.revoke("my-plugin");
        assert!(sandbox
            .check("my-plugin", PluginCapability::ChainQuery)
            .is_err());
    }

    #[test]
    fn check_all_success() {
        let sandbox = PluginSandbox::new();
        let caps = CapabilitySet::from_iter([
            PluginCapability::ChainQuery,
            PluginCapability::MemoryAccess,
        ]);
        sandbox.grant("my-plugin", caps.clone());

        assert!(sandbox.check_all("my-plugin", &caps).is_ok());
    }

    #[test]
    fn check_all_partial_failure() {
        let sandbox = PluginSandbox::new();
        sandbox.grant(
            "my-plugin",
            CapabilitySet::from_iter([PluginCapability::ChainQuery]),
        );

        let required = CapabilitySet::from_iter([
            PluginCapability::ChainQuery,
            PluginCapability::NetworkAccess,
        ]);
        let result = sandbox.check_all("my-plugin", &required);
        assert!(result.is_err());
    }

    #[test]
    fn get_grants() {
        let sandbox = PluginSandbox::new();
        assert!(sandbox.get_grants("nope").is_none());

        let caps = CapabilitySet::from_iter([PluginCapability::ChainQuery]);
        sandbox.grant("my-plugin", caps.clone());

        let retrieved = sandbox.get_grants("my-plugin").expect("should exist");
        assert_eq!(retrieved, caps);
    }

    #[test]
    fn plugin_count() {
        let sandbox = PluginSandbox::new();
        assert_eq!(sandbox.plugin_count(), 0);

        sandbox.grant(
            "a",
            CapabilitySet::from_iter([PluginCapability::ChainQuery]),
        );
        sandbox.grant(
            "b",
            CapabilitySet::from_iter([PluginCapability::MemoryAccess]),
        );
        assert_eq!(sandbox.plugin_count(), 2);

        sandbox.revoke("a");
        assert_eq!(sandbox.plugin_count(), 1);
    }

    #[test]
    fn grant_replaces_previous() {
        let sandbox = PluginSandbox::new();
        sandbox.grant(
            "my-plugin",
            CapabilitySet::from_iter([PluginCapability::ChainQuery]),
        );
        assert!(sandbox
            .check("my-plugin", PluginCapability::ChainQuery)
            .is_ok());

        // Replace with a different set.
        sandbox.grant(
            "my-plugin",
            CapabilitySet::from_iter([PluginCapability::NetworkAccess]),
        );
        assert!(sandbox
            .check("my-plugin", PluginCapability::ChainQuery)
            .is_err());
        assert!(sandbox
            .check("my-plugin", PluginCapability::NetworkAccess)
            .is_ok());
    }
}
