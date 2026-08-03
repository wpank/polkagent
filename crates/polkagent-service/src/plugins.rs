//! Plugin lifecycle management for the service layer.
//!
//! [`ServicePluginManager`] wraps the lower-level
//! [`polkagent_plugin::PluginManager`] and exposes a service-oriented API
//! for discovering, loading, enabling, and disabling plugins. It bridges
//! the plugin system's capability model with the service layer's grant
//! system.
//!
//! # Usage
//!
//! ```rust,no_run
//! use polkagent_service::plugins::ServicePluginManager;
//! use std::path::PathBuf;
//!
//! let manager = ServicePluginManager::new(PathBuf::from("/etc/polkagent/plugins"));
//! let loaded = manager.load_plugins().expect("should load");
//! println!("Loaded {} plugins", loaded.len());
//! ```

use std::collections::HashSet;
use std::path::PathBuf;

use parking_lot::RwLock;
use tracing::{debug, info, warn};

use polkagent_plugin::{
    PluginCapability, PluginManifest, PluginManager, PluginRegistry, PluginSandbox,
};

use crate::error::ServiceError;

// ---------------------------------------------------------------------------
// Allowed capabilities — the grant system whitelist
// ---------------------------------------------------------------------------

/// The default set of capabilities that the grant system permits plugins to
/// request. Capabilities outside this set are rejected during validation.
///
/// In a real deployment this would be loaded from the grant policy
/// configuration; here we hard-code a reasonable default.
fn default_allowed_capabilities() -> HashSet<PluginCapability> {
    [
        PluginCapability::ChainQuery,
        PluginCapability::MemoryAccess,
        PluginCapability::NetworkAccess,
        PluginCapability::ToolExecution,
        PluginCapability::ReadFileSystem,
    ]
    .into_iter()
    .collect()
}

// ---------------------------------------------------------------------------
// PluginInfo — lightweight view returned by list_plugins
// ---------------------------------------------------------------------------

/// Lightweight summary of a loaded plugin, returned by
/// [`ServicePluginManager::list_plugins`].
#[derive(Debug, Clone)]
pub struct PluginInfo {
    /// The plugin name.
    pub name: String,
    /// The plugin version string.
    pub version: String,
    /// Human-readable description.
    pub description: String,
    /// Whether the plugin is currently enabled.
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// ServicePluginManager
// ---------------------------------------------------------------------------

/// Service-layer wrapper around the plugin system.
///
/// Manages the full plugin lifecycle: discovery from a configured directory,
/// manifest validation against the grant system, loading into the plugin
/// registry, and enable/disable toggling.
#[derive(Debug)]
pub struct ServicePluginManager {
    /// The directory from which plugin manifests are discovered.
    plugin_dir: PathBuf,
    /// The underlying `polkagent-plugin` manager.
    inner: PluginManager,
    /// Capabilities that the grant system allows plugins to request.
    allowed_capabilities: HashSet<PluginCapability>,
    /// Set of plugin names that have been explicitly disabled.
    disabled: RwLock<HashSet<String>>,
}

impl ServicePluginManager {
    /// Create a new `ServicePluginManager` that loads plugins from the
    /// given directory.
    pub fn new(plugin_dir: PathBuf) -> Self {
        let inner = PluginManager::with_search_paths(vec![plugin_dir.clone()]);
        Self {
            plugin_dir,
            inner,
            allowed_capabilities: default_allowed_capabilities(),
            disabled: RwLock::new(HashSet::new()),
        }
    }

    /// Create a manager with a custom set of allowed capabilities.
    ///
    /// Only plugins whose *required* capabilities are a subset of
    /// `allowed` will pass validation.
    pub fn with_allowed_capabilities(
        plugin_dir: PathBuf,
        allowed: HashSet<PluginCapability>,
    ) -> Self {
        let inner = PluginManager::with_search_paths(vec![plugin_dir.clone()]);
        Self {
            plugin_dir,
            inner,
            allowed_capabilities: allowed,
            disabled: RwLock::new(HashSet::new()),
        }
    }

    /// Return the configured plugin directory.
    pub fn plugin_dir(&self) -> &PathBuf {
        &self.plugin_dir
    }

    // -----------------------------------------------------------------------
    // Validation
    // -----------------------------------------------------------------------

    /// Validate that every *required* capability declared by `manifest` is
    /// present in the allowed set (grant whitelist).
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Config`] if any required capability is not
    /// in the allowed set.
    fn validate_capabilities(&self, manifest: &PluginManifest) -> Result<(), ServiceError> {
        let required = manifest
            .required_capabilities()
            .map_err(|e| ServiceError::Config {
                message: format!(
                    "plugin '{}' declares invalid capabilities: {e}",
                    manifest.plugin.name
                ),
            })?;

        for cap in required.iter() {
            if !self.allowed_capabilities.contains(&cap) {
                return Err(ServiceError::Config {
                    message: format!(
                        "plugin '{}' requires capability '{}' which is not permitted by the grant system",
                        manifest.plugin.name, cap,
                    ),
                });
            }
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    /// Discover, validate, and load all plugins from the configured
    /// directory.
    ///
    /// Each plugin's manifest is validated against the grant system's
    /// allowed capabilities before being loaded. Plugins that fail
    /// validation are skipped with a warning.
    ///
    /// Returns the list of successfully loaded plugin names.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Config`] if the underlying plugin loader
    /// encounters a fatal error during dependency resolution.
    pub fn load_plugins(&self) -> Result<Vec<String>, ServiceError> {
        info!(dir = %self.plugin_dir.display(), "loading plugins");

        let discovered = self.inner.discover_plugins();
        if discovered.is_empty() {
            debug!("no plugins found in {}", self.plugin_dir.display());
            return Ok(Vec::new());
        }

        info!(count = discovered.len(), "discovered plugins");

        // Validate each manifest against the grant system.
        let mut valid: Vec<PluginManifest> = Vec::new();
        for manifest in discovered {
            match self.validate_capabilities(&manifest) {
                Ok(()) => {
                    debug!(plugin = %manifest.plugin.name, "plugin passed capability validation");
                    valid.push(manifest);
                }
                Err(e) => {
                    warn!(
                        plugin = %manifest.plugin.name,
                        error = %e,
                        "skipping plugin: capability validation failed"
                    );
                }
            }
        }

        // Load the validated manifests through the inner manager.
        let _order = self
            .inner
            .load_plugins(valid.clone())
            .map_err(|e| ServiceError::Config {
                message: format!("failed to load plugins: {e}"),
            })?;

        let names: Vec<String> = valid.iter().map(|m| m.plugin.name.clone()).collect();
        info!(loaded = ?names, "plugins loaded successfully");
        Ok(names)
    }

    /// List all currently loaded plugins with their status.
    pub fn list_plugins(&self) -> Vec<PluginInfo> {
        let disabled = self.disabled.read();
        self.inner
            .registry()
            .manifests()
            .into_iter()
            .map(|m| {
                let name = m.plugin.name.clone();
                PluginInfo {
                    name: name.clone(),
                    version: m.plugin.version,
                    description: m.plugin.description,
                    enabled: !disabled.contains(&name),
                }
            })
            .collect()
    }

    /// Enable a previously disabled plugin.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Config`] if the plugin is not registered.
    pub fn enable_plugin(&self, name: &str) -> Result<(), ServiceError> {
        // Verify the plugin exists in the registry.
        self.inner
            .registry()
            .get(name)
            .map_err(|_| ServiceError::Config {
                message: format!("plugin '{name}' is not loaded"),
            })?;

        let mut disabled = self.disabled.write();
        if disabled.remove(name) {
            info!(plugin = %name, "plugin enabled");
        } else {
            debug!(plugin = %name, "plugin was already enabled");
        }

        Ok(())
    }

    /// Disable a loaded plugin. The plugin remains in the registry but is
    /// marked as disabled and will be skipped during capability checks.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Config`] if the plugin is not registered.
    pub fn disable_plugin(&self, name: &str) -> Result<(), ServiceError> {
        // Verify the plugin exists in the registry.
        self.inner
            .registry()
            .get(name)
            .map_err(|_| ServiceError::Config {
                message: format!("plugin '{name}' is not loaded"),
            })?;

        let mut disabled = self.disabled.write();
        if disabled.insert(name.to_owned()) {
            info!(plugin = %name, "plugin disabled");
        } else {
            debug!(plugin = %name, "plugin was already disabled");
        }

        Ok(())
    }

    /// Check whether a specific plugin is currently enabled.
    pub fn is_enabled(&self, name: &str) -> bool {
        !self.disabled.read().contains(name)
    }

    /// Return a reference to the inner plugin registry.
    pub fn registry(&self) -> &PluginRegistry {
        self.inner.registry()
    }

    /// Return a reference to the inner plugin sandbox.
    pub fn sandbox(&self) -> &PluginSandbox {
        self.inner.sandbox()
    }

    /// Check whether a plugin is allowed to perform an operation requiring
    /// the given capability.
    ///
    /// This checks both the sandbox grants and the enabled/disabled status.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::Config`] if the plugin is disabled or does
    /// not have the required capability.
    pub fn check_capability(
        &self,
        plugin_name: &str,
        capability: PluginCapability,
    ) -> Result<(), ServiceError> {
        if !self.is_enabled(plugin_name) {
            return Err(ServiceError::Config {
                message: format!("plugin '{plugin_name}' is disabled"),
            });
        }

        self.inner
            .check_capability(plugin_name, capability)
            .map_err(|e| ServiceError::Config {
                message: e.to_string(),
            })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_plugin::loader::MANIFEST_FILENAME;
    use std::fs;

    /// Helper: create a temp directory with zero plugin subdirectories.
    fn empty_plugin_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("should create temp dir")
    }

    /// Helper: write a valid plugin.toml into a subdirectory of `root`.
    fn write_manifest(root: &std::path::Path, dir_name: &str, toml: &str) {
        let plugin_dir = root.join(dir_name);
        fs::create_dir_all(&plugin_dir).expect("should create plugin dir");
        fs::write(plugin_dir.join(MANIFEST_FILENAME), toml).expect("should write manifest");
    }

    const VALID_MANIFEST: &str = r#"
[plugin]
name = "test-plugin"
version = "1.0.0"
description = "A test plugin"

[capabilities]
required = ["chain.query", "memory.read"]
"#;

    const VALID_MANIFEST_B: &str = r#"
[plugin]
name = "helper-plugin"
version = "2.0.0"
description = "A helper plugin"

[capabilities]
required = ["network.http"]
"#;

    // -------------------------------------------------------------------
    // Test 1: Plugin manager can be registered with AppService
    // -------------------------------------------------------------------

    #[test]
    fn plugin_manager_registered_with_app_service() {
        use crate::app::AppServiceBuilder;
        use polkagent_config::Config;
        use polkagent_event::EventRecorder;
        use polkagent_store_trait::event::EventStore;
        use std::sync::Arc;

        // Minimal fake event store for the recorder.
        #[derive(Debug, Default)]
        struct FakeEventStore;

        #[async_trait::async_trait]
        impl EventStore for FakeEventStore {
            async fn append_durable(
                &self,
                event: polkagent_store_trait::event::StoredEvent,
            ) -> Result<
                polkagent_store_trait::event::StoredEvent,
                polkagent_store_trait::event::EventStoreError,
            > {
                Ok(event)
            }

            async fn append_diagnostic(
                &self,
                _event: polkagent_store_trait::event::StoredEvent,
                _expires_at: String,
            ) -> Result<(), polkagent_store_trait::event::EventStoreError> {
                Ok(())
            }

            async fn read_from_cursor(
                &self,
                _cursor: u64,
                _limit: usize,
            ) -> Result<
                Vec<polkagent_store_trait::event::StoredEvent>,
                polkagent_store_trait::event::EventStoreError,
            > {
                Ok(vec![])
            }

            async fn read_run_events(
                &self,
                _run_id: polkagent_core::RunId,
            ) -> Result<
                Vec<polkagent_store_trait::event::StoredEvent>,
                polkagent_store_trait::event::EventStoreError,
            > {
                Ok(vec![])
            }

            async fn query(
                &self,
                _filter: polkagent_store_trait::event::EventFilter,
            ) -> Result<
                Vec<polkagent_store_trait::event::StoredEvent>,
                polkagent_store_trait::event::EventStoreError,
            > {
                Ok(vec![])
            }

            async fn max_sequence(
                &self,
                _run_id: polkagent_core::RunId,
            ) -> Result<u64, polkagent_store_trait::event::EventStoreError> {
                Ok(0)
            }

            async fn has_terminal_event(
                &self,
                _run_id: polkagent_core::RunId,
            ) -> Result<bool, polkagent_store_trait::event::EventStoreError> {
                Ok(false)
            }
        }

        // Minimal fake run store.
        #[derive(Debug, Default)]
        struct FakeRunStore;

        #[async_trait::async_trait]
        impl polkagent_store_trait::RunStore for FakeRunStore {
            async fn create(
                &self,
                _run_id: polkagent_core::RunId,
                _agent_id: &str,
                _status: polkagent_store_trait::RunStatus,
            ) -> Result<(), polkagent_store_trait::StoreError> {
                Ok(())
            }

            async fn get(
                &self,
                run_id: polkagent_core::RunId,
            ) -> Result<polkagent_store_trait::RunSummary, polkagent_store_trait::StoreError> {
                Err(polkagent_store_trait::StoreError::NotFound {
                    resource_type: "Run",
                    id: run_id.to_string(),
                })
            }

            async fn update_state(
                &self,
                _run_id: polkagent_core::RunId,
                _new_status: polkagent_store_trait::RunStatus,
            ) -> Result<(), polkagent_store_trait::StoreError> {
                Ok(())
            }

            async fn list_by_agent(
                &self,
                _agent_id: &str,
                _limit: u32,
                _offset: u32,
            ) -> Result<Vec<polkagent_store_trait::RunSummary>, polkagent_store_trait::StoreError>
            {
                Ok(vec![])
            }

            async fn list_by_state(
                &self,
                _status: polkagent_store_trait::RunStatus,
                _limit: u32,
                _offset: u32,
            ) -> Result<Vec<polkagent_store_trait::RunSummary>, polkagent_store_trait::StoreError>
            {
                Ok(vec![])
            }

            async fn insert_turn(
                &self,
                _turn_id: polkagent_core::TurnId,
                _run_id: polkagent_core::RunId,
                _sequence: u32,
                _role: &str,
                _started_at: &str,
                _completed_at: Option<&str>,
                _input_tokens: u32,
                _output_tokens: u32,
            ) -> Result<(), polkagent_store_trait::StoreError> {
                Ok(())
            }
        }

        let dir = empty_plugin_dir();
        let manager = ServicePluginManager::new(dir.path().to_path_buf());

        let event_store: Arc<dyn EventStore + Send + Sync> =
            Arc::new(FakeEventStore);
        let recorder = EventRecorder::new(event_store, Default::default());

        let app = AppServiceBuilder::new()
            .with_config(Config::default())
            .with_run_store(Arc::new(FakeRunStore))
            .with_event_recorder(recorder)
            .with_plugin_manager(manager)
            .build()
            .expect("should build");

        // The plugin manager should be accessible.
        let pm = app.plugin_manager().expect("should have plugin manager");
        assert!(pm.list_plugins().is_empty());
    }

    // -------------------------------------------------------------------
    // Test 2: Loading from empty directory returns empty list
    // -------------------------------------------------------------------

    #[test]
    fn load_from_empty_directory_returns_empty() {
        let dir = empty_plugin_dir();
        let manager = ServicePluginManager::new(dir.path().to_path_buf());

        let loaded = manager.load_plugins().expect("should succeed");
        assert!(loaded.is_empty(), "expected no plugins from empty dir");
        assert!(manager.list_plugins().is_empty());
    }

    // -------------------------------------------------------------------
    // Test 3: Valid manifest is loaded correctly
    // -------------------------------------------------------------------

    #[test]
    fn valid_manifest_loaded_correctly() {
        let dir = empty_plugin_dir();
        write_manifest(dir.path(), "test-plugin", VALID_MANIFEST);

        let manager = ServicePluginManager::new(dir.path().to_path_buf());
        let loaded = manager.load_plugins().expect("should load");

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], "test-plugin");

        let plugins = manager.list_plugins();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "test-plugin");
        assert_eq!(plugins[0].version, "1.0.0");
        assert_eq!(plugins[0].description, "A test plugin");
        assert!(plugins[0].enabled);
    }

    // -------------------------------------------------------------------
    // Test 4: Invalid manifest is rejected
    // -------------------------------------------------------------------

    #[test]
    fn invalid_manifest_rejected() {
        let dir = empty_plugin_dir();

        // Plugin requiring a forbidden capability (chain.submit is not in
        // the default allowed set).
        let forbidden_toml = r#"
[plugin]
name = "forbidden-plugin"
version = "1.0.0"

[capabilities]
required = ["chain.submit"]
"#;
        write_manifest(dir.path(), "forbidden-plugin", forbidden_toml);

        // Also add a valid one to show selective loading.
        write_manifest(dir.path(), "test-plugin", VALID_MANIFEST);

        let manager = ServicePluginManager::new(dir.path().to_path_buf());
        let loaded = manager.load_plugins().expect("should succeed (partial)");

        // Only the valid plugin should have been loaded.
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], "test-plugin");

        // The forbidden plugin should not appear in the list.
        let plugins = manager.list_plugins();
        let names: Vec<&str> = plugins
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert!(!names.contains(&"forbidden-plugin"));
    }

    // -------------------------------------------------------------------
    // Test 5: Plugin can be enabled/disabled
    // -------------------------------------------------------------------

    #[test]
    fn plugin_enable_disable_lifecycle() {
        let dir = empty_plugin_dir();
        write_manifest(dir.path(), "test-plugin", VALID_MANIFEST);

        let manager = ServicePluginManager::new(dir.path().to_path_buf());
        manager.load_plugins().expect("should load");

        // Initially enabled.
        assert!(manager.is_enabled("test-plugin"));

        // Disable it.
        manager
            .disable_plugin("test-plugin")
            .expect("should disable");
        assert!(!manager.is_enabled("test-plugin"));

        // list_plugins should reflect disabled status.
        let plugins = manager.list_plugins();
        assert!(!plugins[0].enabled);

        // Re-enable it.
        manager
            .enable_plugin("test-plugin")
            .expect("should enable");
        assert!(manager.is_enabled("test-plugin"));

        // list_plugins should reflect enabled status.
        let plugins = manager.list_plugins();
        assert!(plugins[0].enabled);
    }

    // -------------------------------------------------------------------
    // Test 6: Enable/disable non-existent plugin errors
    // -------------------------------------------------------------------

    #[test]
    fn enable_nonexistent_plugin_errors() {
        let dir = empty_plugin_dir();
        let manager = ServicePluginManager::new(dir.path().to_path_buf());

        let result = manager.enable_plugin("ghost");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("ghost"));
    }

    #[test]
    fn disable_nonexistent_plugin_errors() {
        let dir = empty_plugin_dir();
        let manager = ServicePluginManager::new(dir.path().to_path_buf());

        let result = manager.disable_plugin("ghost");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("ghost"));
    }

    // -------------------------------------------------------------------
    // Test 7: Capability check respects disabled state
    // -------------------------------------------------------------------

    #[test]
    fn capability_check_respects_disabled() {
        let dir = empty_plugin_dir();
        write_manifest(dir.path(), "test-plugin", VALID_MANIFEST);

        let manager = ServicePluginManager::new(dir.path().to_path_buf());
        manager.load_plugins().expect("should load");

        // Should pass when enabled.
        assert!(manager
            .check_capability("test-plugin", PluginCapability::ChainQuery)
            .is_ok());

        // Disable and check again.
        manager
            .disable_plugin("test-plugin")
            .expect("should disable");
        let result = manager.check_capability("test-plugin", PluginCapability::ChainQuery);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("disabled"));
    }

    // -------------------------------------------------------------------
    // Test 8: Multiple plugins loaded
    // -------------------------------------------------------------------

    #[test]
    fn multiple_plugins_loaded() {
        let dir = empty_plugin_dir();
        write_manifest(dir.path(), "test-plugin", VALID_MANIFEST);
        write_manifest(dir.path(), "helper-plugin", VALID_MANIFEST_B);

        let manager = ServicePluginManager::new(dir.path().to_path_buf());
        let loaded = manager.load_plugins().expect("should load");

        assert_eq!(loaded.len(), 2);
        let plugins = manager.list_plugins();
        assert_eq!(plugins.len(), 2);

        let names: Vec<&str> = plugins.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"test-plugin"));
        assert!(names.contains(&"helper-plugin"));
    }

    // -------------------------------------------------------------------
    // Test 9: Custom allowed capabilities restrict loading
    // -------------------------------------------------------------------

    #[test]
    fn custom_allowed_capabilities() {
        let dir = empty_plugin_dir();
        write_manifest(dir.path(), "test-plugin", VALID_MANIFEST);

        // Only allow chain.query — memory.read is missing from allowed set,
        // so the plugin should be rejected.
        let allowed: HashSet<PluginCapability> = [PluginCapability::ChainQuery].into_iter().collect();
        let manager =
            ServicePluginManager::with_allowed_capabilities(dir.path().to_path_buf(), allowed);

        let loaded = manager.load_plugins().expect("should succeed");
        assert!(loaded.is_empty(), "plugin should be rejected due to missing memory.read");
    }

    // -------------------------------------------------------------------
    // Test 10: Plugin directory accessor
    // -------------------------------------------------------------------

    #[test]
    fn plugin_dir_accessor() {
        let dir = empty_plugin_dir();
        let manager = ServicePluginManager::new(dir.path().to_path_buf());
        assert_eq!(manager.plugin_dir(), dir.path());
    }
}
