//! Capability-based sandbox for plugins.
//!
//! The [`PluginSandbox`] acts as a capability checker that gates plugin
//! operations. Each plugin is assigned a [`CapabilitySet`] at load time,
//! and every operation must pass a capability check before being
//! dispatched.
//!
//! The [`WasmtimeSandbox`] provides Wasmtime Component Model based
//! sandboxing for marketplace packages (PRD-12 §8.5). It enforces
//! capability declarations via WIT host-call gating, with fuel metering,
//! epoch interruption, and `ResourceLimiter`-based memory caps.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, info, warn};

use crate::capability::{CapabilitySet, PluginCapability};
use crate::error::PluginError;
use crate::manifest::SandboxTier;

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
    pub fn check(
        &self,
        plugin_name: &str,
        capability: PluginCapability,
    ) -> Result<(), PluginError> {
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
// SandboxResourceLimits — configurable bounds for WASM execution
// ---------------------------------------------------------------------------

/// Resource limits enforced by the Wasmtime sandbox.
///
/// See PRD-12 §8.4 for default values.
#[derive(Debug, Clone)]
pub struct SandboxResourceLimits {
    /// Fuel budget per invocation (Wasmtime instruction-level metering).
    pub fuel_budget: u64,
    /// Maximum linear memory in bytes.
    pub max_memory_bytes: usize,
    /// Wall-clock timeout per invocation.
    pub timeout: Duration,
    /// Maximum number of outbound network requests per invocation.
    pub max_network_requests: u32,
    /// Maximum network bandwidth per invocation in bytes.
    pub max_network_bytes: usize,
    /// Maximum filesystem read in bytes.
    pub max_fs_read_bytes: usize,
    /// Maximum filesystem write in bytes.
    pub max_fs_write_bytes: usize,
}

impl Default for SandboxResourceLimits {
    fn default() -> Self {
        Self {
            fuel_budget: 1_000_000,
            max_memory_bytes: 256 * 1024 * 1024, // 256 MB
            timeout: Duration::from_secs(30),
            max_network_requests: 100,
            max_network_bytes: 10 * 1024 * 1024,  // 10 MB
            max_fs_read_bytes: 100 * 1024 * 1024, // 100 MB
            max_fs_write_bytes: 10 * 1024 * 1024, // 10 MB
        }
    }
}

// ---------------------------------------------------------------------------
// SandboxConfig — configuration for a sandboxed plugin
// ---------------------------------------------------------------------------

/// Configuration for a sandboxed plugin execution environment.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// The sandbox tier to enforce.
    pub tier: SandboxTier,
    /// The capabilities granted to the plugin.
    pub granted_capabilities: CapabilitySet,
    /// Resource limits for WASM execution.
    pub resource_limits: SandboxResourceLimits,
    /// Allowed network hosts (supports wildcards like `"*.parity.io"`).
    pub allowed_hosts: Vec<String>,
}

impl SandboxConfig {
    /// Create a new `SandboxConfig` for the given tier and capabilities.
    pub fn new(tier: SandboxTier, granted_capabilities: CapabilitySet) -> Self {
        Self {
            tier,
            granted_capabilities,
            resource_limits: SandboxResourceLimits::default(),
            allowed_hosts: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// WasmtimeSandbox — Wasmtime Component Model sandbox (PRD-12 §8.5)
// ---------------------------------------------------------------------------

/// Wasmtime Component Model sandbox for marketplace packages.
///
/// Enforces capability declarations at the WASM host boundary using
/// fuel metering, epoch interruption, and memory limits. Each host-call
/// (HTTP, filesystem, chain query, tool invocation) is checked against
/// the plugin's granted [`CapabilitySet`] before dispatch.
///
/// This struct manages the sandbox configuration and host-call
/// enforcement policy. Actual Wasmtime `Engine`/`Store`/`Component`
/// instantiation requires the `wasmtime` crate dependency, which is
/// wired at the application level. This module provides the
/// capability-gating and resource-limit logic that the host uses.
#[derive(Debug)]
pub struct WasmtimeSandbox {
    /// Per-plugin sandbox configurations, keyed by plugin name.
    configs: RwLock<HashMap<String, SandboxConfig>>,
}

impl WasmtimeSandbox {
    /// Create a new, empty `WasmtimeSandbox`.
    pub fn new() -> Self {
        Self {
            configs: RwLock::new(HashMap::new()),
        }
    }

    /// Register a plugin with its sandbox configuration.
    pub fn register(&self, plugin_name: impl Into<String>, config: SandboxConfig) {
        let name = plugin_name.into();
        info!(
            plugin = %name,
            tier = %config.tier,
            fuel = config.resource_limits.fuel_budget,
            max_memory_mb = config.resource_limits.max_memory_bytes / (1024 * 1024),
            "registered plugin in wasmtime sandbox"
        );
        self.configs.write().insert(name, config);
    }

    /// Remove a plugin from the sandbox.
    pub fn unregister(&self, plugin_name: &str) {
        debug!(plugin = %plugin_name, "unregistered from wasmtime sandbox");
        self.configs.write().remove(plugin_name);
    }

    /// Check whether a host-call operation is allowed for the given plugin.
    ///
    /// This is called at the WIT host boundary before dispatching any
    /// host function. If the plugin's granted capabilities do not include
    /// the required capability, the call is denied.
    pub fn check_host_call(
        &self,
        plugin_name: &str,
        required_capability: PluginCapability,
    ) -> Result<(), PluginError> {
        let configs = self.configs.read();
        match configs.get(plugin_name) {
            Some(config) => {
                if config.granted_capabilities.contains(required_capability) {
                    debug!(
                        plugin = %plugin_name,
                        capability = %required_capability,
                        "host call allowed"
                    );
                    Ok(())
                } else {
                    warn!(
                        plugin = %plugin_name,
                        capability = %required_capability,
                        "host call denied: capability not granted"
                    );
                    Err(PluginError::SandboxDenied {
                        plugin_name: plugin_name.to_string(),
                        operation: format!(
                            "host call requiring {required_capability} is not in granted capabilities"
                        ),
                    })
                }
            }
            None => {
                warn!(
                    plugin = %plugin_name,
                    "host call denied: plugin not registered in sandbox"
                );
                Err(PluginError::SandboxDenied {
                    plugin_name: plugin_name.to_string(),
                    operation: "plugin not registered in wasmtime sandbox".to_string(),
                })
            }
        }
    }

    /// Check whether an HTTP request to a specific host is allowed.
    ///
    /// In addition to checking the `NetworkAccess` capability, this
    /// validates the target host against the plugin's allowed host list.
    pub fn check_http_host(&self, plugin_name: &str, target_host: &str) -> Result<(), PluginError> {
        self.check_host_call(plugin_name, PluginCapability::NetworkAccess)?;

        let configs = self.configs.read();
        if let Some(config) = configs.get(plugin_name) {
            if config.allowed_hosts.is_empty() {
                return Ok(());
            }
            let allowed = config.allowed_hosts.iter().any(|pattern| {
                if let Some(suffix) = pattern.strip_prefix("*.") {
                    target_host == suffix || target_host.ends_with(&format!(".{suffix}"))
                } else {
                    pattern == target_host
                }
            });
            if allowed {
                Ok(())
            } else {
                warn!(
                    plugin = %plugin_name,
                    host = %target_host,
                    "HTTP host denied: not in allowed list"
                );
                Err(PluginError::SandboxDenied {
                    plugin_name: plugin_name.to_string(),
                    operation: format!("HTTP request to host '{target_host}' is not allowed"),
                })
            }
        } else {
            Ok(())
        }
    }

    /// Get the resource limits for a registered plugin.
    pub fn resource_limits(&self, plugin_name: &str) -> Option<SandboxResourceLimits> {
        self.configs
            .read()
            .get(plugin_name)
            .map(|c| c.resource_limits.clone())
    }

    /// Get the sandbox tier for a registered plugin.
    pub fn sandbox_tier(&self, plugin_name: &str) -> Option<SandboxTier> {
        self.configs.read().get(plugin_name).map(|c| c.tier)
    }

    /// Return the number of plugins registered in the sandbox.
    pub fn plugin_count(&self) -> usize {
        self.configs.read().len()
    }
}

impl Default for WasmtimeSandbox {
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

    // -----------------------------------------------------------------------
    // WasmtimeSandbox tests
    // -----------------------------------------------------------------------

    #[test]
    fn wasmtime_sandbox_register_and_check() {
        let sandbox = WasmtimeSandbox::new();
        let config = SandboxConfig::new(
            SandboxTier::Wasm,
            CapabilitySet::from_iter([
                PluginCapability::ChainQuery,
                PluginCapability::MemoryAccess,
            ]),
        );
        sandbox.register("wasm-plugin", config);

        assert!(sandbox
            .check_host_call("wasm-plugin", PluginCapability::ChainQuery)
            .is_ok());
        assert!(sandbox
            .check_host_call("wasm-plugin", PluginCapability::MemoryAccess)
            .is_ok());
    }

    #[test]
    fn wasmtime_sandbox_denies_undeclared_capability() {
        let sandbox = WasmtimeSandbox::new();
        let config = SandboxConfig::new(
            SandboxTier::Wasm,
            CapabilitySet::from_iter([PluginCapability::ChainQuery]),
        );
        sandbox.register("wasm-plugin", config);

        let result = sandbox.check_host_call("wasm-plugin", PluginCapability::WriteFileSystem);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("wasm-plugin"));
        assert!(msg.contains("denied"));
    }

    #[test]
    fn wasmtime_sandbox_denies_unregistered_plugin() {
        let sandbox = WasmtimeSandbox::new();
        let result = sandbox.check_host_call("unknown", PluginCapability::ChainQuery);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("not registered"));
    }

    #[test]
    fn wasmtime_sandbox_http_host_filtering() {
        let sandbox = WasmtimeSandbox::new();
        let mut config = SandboxConfig::new(
            SandboxTier::Wasm,
            CapabilitySet::from_iter([PluginCapability::NetworkAccess]),
        );
        config.allowed_hosts = vec!["api.polkadot.io".to_string(), "*.parity.io".to_string()];
        sandbox.register("net-plugin", config);

        // Allowed: exact match.
        assert!(sandbox
            .check_http_host("net-plugin", "api.polkadot.io")
            .is_ok());

        // Allowed: wildcard match.
        assert!(sandbox
            .check_http_host("net-plugin", "rpc.parity.io")
            .is_ok());

        // Denied: not in allowed list.
        let result = sandbox.check_http_host("net-plugin", "evil.com");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("evil.com"));
        assert!(msg.contains("not allowed"));
    }

    #[test]
    fn wasmtime_sandbox_http_denied_without_network_cap() {
        let sandbox = WasmtimeSandbox::new();
        let config = SandboxConfig::new(
            SandboxTier::Wasm,
            CapabilitySet::from_iter([PluginCapability::ChainQuery]),
        );
        sandbox.register("no-net", config);

        let result = sandbox.check_http_host("no-net", "anything.com");
        assert!(result.is_err());
    }

    #[test]
    fn wasmtime_sandbox_resource_limits_defaults() {
        let limits = SandboxResourceLimits::default();
        assert_eq!(limits.fuel_budget, 1_000_000);
        assert_eq!(limits.max_memory_bytes, 256 * 1024 * 1024);
        assert_eq!(limits.timeout, Duration::from_secs(30));
        assert_eq!(limits.max_network_requests, 100);
    }

    #[test]
    fn wasmtime_sandbox_get_resource_limits() {
        let sandbox = WasmtimeSandbox::new();
        assert!(sandbox.resource_limits("nope").is_none());

        let mut config = SandboxConfig::new(SandboxTier::Wasm, CapabilitySet::empty());
        config.resource_limits.fuel_budget = 500;
        sandbox.register("limited", config);

        let limits = sandbox.resource_limits("limited").expect("should exist");
        assert_eq!(limits.fuel_budget, 500);
    }

    #[test]
    fn wasmtime_sandbox_tier_lookup() {
        let sandbox = WasmtimeSandbox::new();
        let config = SandboxConfig::new(SandboxTier::Wasm, CapabilitySet::empty());
        sandbox.register("plugin", config);

        assert_eq!(sandbox.sandbox_tier("plugin"), Some(SandboxTier::Wasm));
        assert_eq!(sandbox.sandbox_tier("nope"), None);
    }

    #[test]
    fn wasmtime_sandbox_unregister() {
        let sandbox = WasmtimeSandbox::new();
        let config = SandboxConfig::new(SandboxTier::Wasm, CapabilitySet::empty());
        sandbox.register("to-remove", config);
        assert_eq!(sandbox.plugin_count(), 1);

        sandbox.unregister("to-remove");
        assert_eq!(sandbox.plugin_count(), 0);
    }
}
