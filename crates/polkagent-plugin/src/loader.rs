//! Plugin loader: discovers and loads plugin manifests from the filesystem.
//!
//! The loader searches well-known directories for plugin packages and parses
//! their `plugin.toml` manifests. Search paths follow a precedence order:
//!
//! 1. **Local project plugins** `.polkagent/plugins/` (relative to CWD)
//! 2. **User-level plugins** `~/.polkagent/plugins/`
//!
//! Each plugin is a directory containing at minimum a `plugin.toml` file.

use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

use crate::error::PluginError;
use crate::manifest::PluginManifest;

/// The conventional manifest filename inside a plugin directory.
pub const MANIFEST_FILENAME: &str = "plugin.toml";

/// Loads plugin manifests from the filesystem.
#[derive(Debug)]
pub struct PluginLoader {
    /// Ordered list of directories to search for plugin packages.
    search_paths: Vec<PathBuf>,
}

impl PluginLoader {
    /// Create a loader with explicit search paths.
    pub fn new(search_paths: Vec<PathBuf>) -> Self {
        Self { search_paths }
    }

    /// Create a loader with the default search paths:
    ///
    /// 1. `.polkagent/plugins/` (relative to CWD)
    /// 2. `~/.polkagent/plugins/`
    pub fn with_default_paths() -> Self {
        let mut paths = Vec::new();

        // Local project plugins
        paths.push(PathBuf::from(".polkagent/plugins"));

        // User-level plugins
        if let Some(home) = dirs::home_dir() {
            paths.push(home.join(".polkagent/plugins"));
        }

        Self {
            search_paths: paths,
        }
    }

    /// Return the configured search paths.
    pub fn search_paths(&self) -> &[PathBuf] {
        &self.search_paths
    }

    /// Load a single plugin manifest from a directory.
    ///
    /// The directory must contain a `plugin.toml` file.
    pub fn load_from_dir(path: &Path) -> Result<PluginManifest, PluginError> {
        let manifest_path = path.join(MANIFEST_FILENAME);
        debug!(path = %manifest_path.display(), "loading plugin manifest");

        let content = std::fs::read_to_string(&manifest_path)
            .map_err(|e| PluginError::io(&manifest_path, &e))?;

        Self::load_from_toml(&content)
    }

    /// Parse a plugin manifest from a TOML string.
    pub fn load_from_toml(content: &str) -> Result<PluginManifest, PluginError> {
        PluginManifest::from_toml(content)
    }

    /// Discover all plugins across the configured search paths.
    ///
    /// Each immediate subdirectory of each search path is treated as a
    /// potential plugin directory. Directories without a valid `plugin.toml`
    /// are silently skipped (with a warning log).
    ///
    /// Plugins from earlier search paths take precedence when multiple
    /// plugins share the same name: the first occurrence wins.
    pub fn discover_plugins(&self) -> Vec<PluginManifest> {
        let mut manifests = Vec::new();
        let mut seen_names = std::collections::HashSet::new();

        for search_path in &self.search_paths {
            if !search_path.is_dir() {
                debug!(
                    path = %search_path.display(),
                    "search path does not exist or is not a directory, skipping"
                );
                continue;
            }

            let entries = match std::fs::read_dir(search_path) {
                Ok(entries) => entries,
                Err(e) => {
                    warn!(
                        path = %search_path.display(),
                        error = %e,
                        "failed to read plugin search directory"
                    );
                    continue;
                }
            };

            for entry in entries {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        warn!(error = %e, "failed to read directory entry");
                        continue;
                    }
                };

                let entry_path = entry.path();
                if !entry_path.is_dir() {
                    continue;
                }

                match Self::load_from_dir(&entry_path) {
                    Ok(manifest) => {
                        let name = manifest.plugin.name.clone();
                        if seen_names.contains(&name) {
                            debug!(
                                plugin = %name,
                                path = %entry_path.display(),
                                "duplicate plugin name, skipping (earlier occurrence takes precedence)"
                            );
                        } else {
                            info!(
                                plugin = %name,
                                version = %manifest.plugin.version,
                                path = %entry_path.display(),
                                "discovered plugin"
                            );
                            seen_names.insert(name);
                            manifests.push(manifest);
                        }
                    }
                    Err(e) => {
                        debug!(
                            path = %entry_path.display(),
                            error = %e,
                            "skipping directory (not a valid plugin)"
                        );
                    }
                }
            }
        }

        manifests
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_TOML: &str = r#"
[plugin]
name = "test-plugin"
version = "0.1.0"
description = "A test plugin"
"#;

    #[test]
    fn load_from_toml_success() {
        let manifest = PluginLoader::load_from_toml(SAMPLE_TOML).expect("should parse");
        assert_eq!(manifest.plugin.name, "test-plugin");
        assert_eq!(manifest.plugin.version, "0.1.0");
    }

    #[test]
    fn load_from_toml_invalid() {
        let result = PluginLoader::load_from_toml("not valid toml {{{");
        assert!(result.is_err());
    }

    #[test]
    fn load_from_dir_success() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let manifest_path = dir.path().join(MANIFEST_FILENAME);
        std::fs::write(&manifest_path, SAMPLE_TOML).expect("should write");

        let manifest = PluginLoader::load_from_dir(dir.path()).expect("should load");
        assert_eq!(manifest.plugin.name, "test-plugin");
    }

    #[test]
    fn load_from_dir_missing_manifest() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let result = PluginLoader::load_from_dir(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn discover_plugins_finds_valid_plugins() {
        let root = tempfile::tempdir().expect("should create temp dir");

        let plugin_a = root.path().join("plugin-a");
        std::fs::create_dir(&plugin_a).expect("should create dir");
        std::fs::write(
            plugin_a.join(MANIFEST_FILENAME),
            r#"
[plugin]
name = "plugin-a"
version = "1.0.0"
"#,
        )
        .expect("should write");

        let plugin_b = root.path().join("plugin-b");
        std::fs::create_dir(&plugin_b).expect("should create dir");
        std::fs::write(
            plugin_b.join(MANIFEST_FILENAME),
            r#"
[plugin]
name = "plugin-b"
version = "2.0.0"
"#,
        )
        .expect("should write");

        // An invalid directory (no manifest)
        let invalid = root.path().join("not-a-plugin");
        std::fs::create_dir(&invalid).expect("should create dir");

        let loader = PluginLoader::new(vec![root.path().to_path_buf()]);
        let plugins = loader.discover_plugins();

        assert_eq!(plugins.len(), 2);
        let names: Vec<&str> = plugins.iter().map(|p| p.plugin.name.as_str()).collect();
        assert!(names.contains(&"plugin-a"));
        assert!(names.contains(&"plugin-b"));
    }

    #[test]
    fn discover_plugins_skips_nonexistent_paths() {
        let loader = PluginLoader::new(vec![PathBuf::from("/nonexistent/path/12345")]);
        let plugins = loader.discover_plugins();
        assert!(plugins.is_empty());
    }

    #[test]
    fn discover_plugins_precedence() {
        let first = tempfile::tempdir().expect("should create temp dir");
        let second = tempfile::tempdir().expect("should create temp dir");

        let plugin_first = first.path().join("dup");
        std::fs::create_dir(&plugin_first).expect("should create dir");
        std::fs::write(
            plugin_first.join(MANIFEST_FILENAME),
            r#"
[plugin]
name = "dup-plugin"
version = "1.0.0"
description = "first"
"#,
        )
        .expect("should write");

        let plugin_second = second.path().join("dup");
        std::fs::create_dir(&plugin_second).expect("should create dir");
        std::fs::write(
            plugin_second.join(MANIFEST_FILENAME),
            r#"
[plugin]
name = "dup-plugin"
version = "2.0.0"
description = "second"
"#,
        )
        .expect("should write");

        let loader = PluginLoader::new(vec![
            first.path().to_path_buf(),
            second.path().to_path_buf(),
        ]);
        let plugins = loader.discover_plugins();

        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].plugin.version, "1.0.0");
        assert_eq!(plugins[0].plugin.description, "first");
    }

    #[test]
    fn default_paths_construction() {
        let loader = PluginLoader::with_default_paths();
        assert!(!loader.search_paths().is_empty());
        assert!(loader
            .search_paths()
            .iter()
            .any(|p| p.ends_with(".polkagent/plugins")));
    }

    #[test]
    fn discover_skips_files_in_search_dir() {
        let root = tempfile::tempdir().expect("should create temp dir");
        std::fs::write(root.path().join("readme.txt"), "hello").expect("should write");

        let loader = PluginLoader::new(vec![root.path().to_path_buf()]);
        let plugins = loader.discover_plugins();
        assert!(plugins.is_empty());
    }
}
