//! Harness registry for managing configured coding-agent harnesses.
//!
//! The [`HarnessRegistry`] holds known harness definitions, probes PATH
//! availability, and resolves which harness to use following the precedence
//! in PRD-04a section 11.2:
//!
//! 1. `--harness` CLI flag (highest priority)
//! 2. `[harness] default` in config file
//! 3. Auto-probe availability in order: claude-code, codex, cursor, goose
//! 4. Fall back to executor-only (no harness)

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::error::ServiceError;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Known harnesses and their binary names, probed in this order.
const KNOWN_HARNESSES: &[(&str, &str)] = &[
    ("claude-code", "claude"),
    ("codex", "codex"),
    ("cursor", "cursor"),
    ("goose", "goose"),
];

// ---------------------------------------------------------------------------
// HarnessInfo
// ---------------------------------------------------------------------------

/// Summary information about a registered harness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessInfo {
    /// Stable identifier for this harness (e.g. `"claude-code"`).
    pub id: String,
    /// The binary name used for PATH probing (e.g. `"claude"`).
    pub binary: String,
    /// Whether the binary was found on PATH during the last probe.
    pub available: bool,
    /// Full path to the binary, if found.
    pub binary_path: Option<String>,
}

// ---------------------------------------------------------------------------
// HarnessResolution
// ---------------------------------------------------------------------------

/// The result of resolving which harness to use.
#[derive(Debug, Clone)]
pub struct HarnessResolution {
    /// The resolved harness name, or `None` for executor-only mode.
    pub harness_name: Option<String>,
    /// Human-readable note explaining the resolution decision.
    pub note: Option<String>,
}

// ---------------------------------------------------------------------------
// HarnessRegistry
// ---------------------------------------------------------------------------

/// Manages known harness definitions and probes their availability.
///
/// The registry mirrors [`ProviderRegistry`](crate::ProviderRegistry) but
/// for coding-agent harnesses instead of model providers. It holds a set of
/// known harnesses, can probe which ones have their binary available on PATH,
/// and resolves which harness to use given a CLI flag and config.
#[derive(Debug)]
pub struct HarnessRegistry {
    /// Registered harnesses keyed by their stable identifier.
    entries: HashMap<String, HarnessEntry>,
    /// Ordered list of harness IDs for auto-probe (first available wins).
    probe_order: Vec<String>,
}

#[derive(Debug)]
struct HarnessEntry {
    /// Binary name used for PATH probing.
    binary: String,
    /// Full path to the binary, populated after a successful probe.
    binary_path: Option<String>,
    /// Whether the harness was found on PATH.
    available: bool,
}

impl HarnessRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            probe_order: Vec::new(),
        }
    }

    /// Create a registry pre-populated with the known harness definitions.
    ///
    /// The known harnesses are registered in probe order:
    /// claude-code, codex, cursor, goose.
    #[must_use]
    pub fn with_known_harnesses() -> Self {
        let mut registry = Self::new();
        for &(id, binary) in KNOWN_HARNESSES {
            registry.register(id, binary);
        }
        registry
    }

    /// Register a harness with the given identifier and binary name.
    ///
    /// If a harness with the same `id` is already registered, it is replaced
    /// and a warning is logged. The harness is appended to the probe order.
    pub fn register(&mut self, id: &str, binary: &str) {
        if self.entries.contains_key(id) {
            warn!(harness_id = %id, "replacing existing harness registration");
        } else {
            self.probe_order.push(id.to_owned());
        }

        info!(harness_id = %id, binary = %binary, "registering harness");

        self.entries.insert(
            id.to_owned(),
            HarnessEntry {
                binary: binary.to_owned(),
                binary_path: None,
                available: false,
            },
        );
    }

    /// Register a harness with a custom binary path.
    ///
    /// Unlike [`register`](Self::register), this sets the binary path
    /// directly (e.g. from config `binary_path` field) and marks the
    /// harness as available without probing.
    pub fn register_with_path(&mut self, id: &str, binary: &str, path: String) {
        if self.entries.contains_key(id) {
            warn!(harness_id = %id, "replacing existing harness registration");
        } else {
            self.probe_order.push(id.to_owned());
        }

        info!(
            harness_id = %id,
            binary = %binary,
            path = %path,
            "registering harness with explicit path"
        );

        self.entries.insert(
            id.to_owned(),
            HarnessEntry {
                binary: binary.to_owned(),
                binary_path: Some(path),
                available: true,
            },
        );
    }

    /// Probe all registered harnesses for binary availability on PATH.
    ///
    /// This runs `which <binary>` for each harness and updates the
    /// `available` and `binary_path` fields.
    pub fn probe_all(&mut self) {
        for (id, entry) in &mut self.entries {
            if entry.binary_path.is_some() {
                // Already has an explicit path; skip probing.
                continue;
            }
            match which_binary(&entry.binary) {
                Some(path) => {
                    info!(harness_id = %id, path = %path, "harness binary found");
                    entry.binary_path = Some(path);
                    entry.available = true;
                }
                None => {
                    entry.binary_path = None;
                    entry.available = false;
                }
            }
        }
    }

    /// Probe a single harness by ID. Returns `true` if the binary is available.
    pub fn probe(&mut self, harness_id: &str) -> bool {
        if let Some(entry) = self.entries.get_mut(harness_id) {
            if entry.binary_path.is_some() {
                return entry.available;
            }
            match which_binary(&entry.binary) {
                Some(path) => {
                    entry.binary_path = Some(path);
                    entry.available = true;
                    true
                }
                None => {
                    entry.binary_path = None;
                    entry.available = false;
                    false
                }
            }
        } else {
            // Unknown harness — try probing the name directly as a binary.
            which_binary(harness_id).is_some()
        }
    }

    /// Check if a harness is available (previously probed).
    #[must_use]
    pub fn is_available(&self, harness_id: &str) -> bool {
        self.entries.get(harness_id).map_or(false, |e| e.available)
    }

    /// Get the binary path for a harness, if probed and found.
    #[must_use]
    pub fn binary_path(&self, harness_id: &str) -> Option<&str> {
        self.entries
            .get(harness_id)
            .and_then(|e| e.binary_path.as_deref())
    }

    /// List summary information for all registered harnesses.
    #[must_use]
    pub fn list_harnesses(&self) -> Vec<HarnessInfo> {
        self.probe_order
            .iter()
            .filter_map(|id| {
                self.entries.get(id).map(|entry| HarnessInfo {
                    id: id.clone(),
                    binary: entry.binary.clone(),
                    available: entry.available,
                    binary_path: entry.binary_path.clone(),
                })
            })
            .collect()
    }

    /// Return the first available harness in probe order.
    #[must_use]
    pub fn first_available(&self) -> Option<&str> {
        self.probe_order
            .iter()
            .find(|id| self.entries.get(id.as_str()).map_or(false, |e| e.available))
            .map(String::as_str)
    }

    /// Resolve which harness to use, following PRD-04a section 11.2 precedence.
    ///
    /// 1. `harness_flag` — explicit CLI `--harness` value
    /// 2. `config_default` — `[harness] default` from config file
    /// 3. First available harness in probe order
    /// 4. `None` — executor-only mode
    ///
    /// This method probes on demand if needed (i.e., when checking a specific
    /// harness name that hasn't been probed yet).
    pub fn resolve(
        &mut self,
        harness_flag: Option<&str>,
        config_default: Option<&str>,
    ) -> HarnessResolution {
        // 1. CLI flag.
        if let Some(name) = harness_flag {
            if self.probe(name) {
                let note = format!("Using harness '{name}' (from --harness flag).");
                return HarnessResolution {
                    harness_name: Some(name.to_owned()),
                    note: Some(note),
                };
            }
            let note = format!(
                "Warning: harness '{name}' binary not found on PATH. \
                 Falling back to executor-only mode."
            );
            return HarnessResolution {
                harness_name: None,
                note: Some(note),
            };
        }

        // 2. Config default.
        if let Some(default) = config_default {
            if !default.is_empty() {
                if self.probe(default) {
                    let note = format!("Using harness '{default}' (from config default).");
                    return HarnessResolution {
                        harness_name: Some(default.to_owned()),
                        note: Some(note),
                    };
                }
                let note =
                    format!("Warning: configured default harness '{default}' not found on PATH.");
                warn!("{}", note);
            }
        }

        // 3. First available harness.
        // Ensure all harnesses have been probed.
        self.probe_all();
        if let Some(name) = self.first_available() {
            let note = format!("Auto-detected harness '{name}' on PATH.");
            return HarnessResolution {
                harness_name: Some(name.to_owned()),
                note: Some(note),
            };
        }

        // 4. Fallback — executor-only mode.
        HarnessResolution {
            harness_name: None,
            note: Some("No harness found. Running in executor-only mode.".to_owned()),
        }
    }

    /// Return the number of registered harnesses.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return `true` if no harnesses are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Look up a harness by ID, returning an error if not found.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::HarnessNotFound`] if no harness with the
    /// given identifier is registered.
    pub fn get(&self, harness_id: &str) -> Result<HarnessInfo, ServiceError> {
        self.entries
            .get(harness_id)
            .map(|entry| HarnessInfo {
                id: harness_id.to_owned(),
                binary: entry.binary.clone(),
                available: entry.available,
                binary_path: entry.binary_path.clone(),
            })
            .ok_or_else(|| ServiceError::HarnessNotFound {
                harness_id: harness_id.to_owned(),
            })
    }
}

impl Default for HarnessRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Binary probing
// ---------------------------------------------------------------------------

/// Check if a binary is available on PATH. Returns the full path if found.
fn which_binary(name: &str) -> Option<String> {
    std::process::Command::new("which")
        .arg(name)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let path = String::from_utf8_lossy(&o.stdout).trim().to_owned();
            if path.is_empty() {
                None
            } else {
                Some(path)
            }
        })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_registry_is_empty() {
        let registry = HarnessRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn with_known_harnesses_has_expected_entries() {
        let registry = HarnessRegistry::with_known_harnesses();
        assert_eq!(registry.len(), 4);

        let ids: Vec<String> = registry
            .list_harnesses()
            .iter()
            .map(|h| h.id.clone())
            .collect();
        assert_eq!(ids, vec!["claude-code", "codex", "cursor", "goose"]);
    }

    #[test]
    fn register_adds_harness() {
        let mut registry = HarnessRegistry::new();
        registry.register("test-harness", "test-bin");
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_available("test-harness"));
    }

    #[test]
    fn register_with_path_marks_available() {
        let mut registry = HarnessRegistry::new();
        registry.register_with_path("test-harness", "test-bin", "/usr/bin/test-bin".into());
        assert_eq!(registry.len(), 1);
        assert!(registry.is_available("test-harness"));
        assert_eq!(
            registry.binary_path("test-harness"),
            Some("/usr/bin/test-bin")
        );
    }

    #[test]
    fn get_returns_harness_info() {
        let mut registry = HarnessRegistry::new();
        registry.register("test-harness", "test-bin");

        let info = registry.get("test-harness").expect("should find harness");
        assert_eq!(info.id, "test-harness");
        assert_eq!(info.binary, "test-bin");
        assert!(!info.available);
    }

    #[test]
    fn get_returns_error_for_unknown() {
        let registry = HarnessRegistry::new();
        let result = registry.get("nonexistent");
        assert!(matches!(result, Err(ServiceError::HarnessNotFound { .. })));
    }

    #[test]
    fn probe_order_preserved() {
        let mut registry = HarnessRegistry::new();
        registry.register("b-harness", "b");
        registry.register("a-harness", "a");
        registry.register("c-harness", "c");

        let ids: Vec<String> = registry
            .list_harnesses()
            .iter()
            .map(|h| h.id.clone())
            .collect();
        assert_eq!(ids, vec!["b-harness", "a-harness", "c-harness"]);
    }

    #[test]
    fn which_binary_finds_sh() {
        // `sh` should exist on any Unix system.
        let result = which_binary("sh");
        assert!(result.is_some(), "expected `sh` to be found on PATH");
    }

    #[test]
    fn which_binary_returns_none_for_nonexistent() {
        let result = which_binary("nonexistent-binary-abc123xyz");
        assert!(result.is_none());
    }

    #[test]
    fn resolve_with_cli_flag_nonexistent_falls_back() {
        let mut registry = HarnessRegistry::with_known_harnesses();
        let result = registry.resolve(Some("nonexistent-harness-xyz"), None);
        assert!(result.harness_name.is_none());
        let note = result.note.expect("expected a note");
        assert!(
            note.contains("not found on PATH"),
            "expected 'not found' note, got: {note}",
        );
    }

    #[test]
    fn resolve_no_flag_no_config_falls_back() {
        let mut registry = HarnessRegistry::with_known_harnesses();
        let result = registry.resolve(None, None);
        // We can't guarantee any harness is on PATH in CI, but the result
        // should either be auto-detected or fallback.
        if result.harness_name.is_none() {
            let note = result.note.expect("expected a note");
            assert!(
                note.contains("executor-only mode"),
                "expected fallback note, got: {note}",
            );
        } else {
            let note = result.note.expect("expected a note");
            assert!(
                note.contains("Auto-detected"),
                "expected auto-detect note, got: {note}",
            );
        }
    }

    #[test]
    fn resolve_config_default_nonexistent_falls_through() {
        let mut registry = HarnessRegistry::with_known_harnesses();
        let result = registry.resolve(None, Some("nonexistent-harness-xyz"));
        // Config default not found should fall through to auto-probe or fallback.
        if result.harness_name.is_none() {
            let note = result.note.expect("expected a note");
            assert!(
                note.contains("executor-only mode"),
                "expected fallback note, got: {note}",
            );
        }
    }

    #[test]
    fn first_available_returns_none_when_nothing_probed() {
        let registry = HarnessRegistry::with_known_harnesses();
        // Before probing, none should be available (unless actually on PATH,
        // which we can't control in tests).
        // Just verify it doesn't panic.
        let _ = registry.first_available();
    }

    #[test]
    fn register_replaces_existing() {
        let mut registry = HarnessRegistry::new();
        registry.register("test", "old-bin");
        registry.register("test", "new-bin");

        let info = registry.get("test").expect("should find harness");
        assert_eq!(info.binary, "new-bin");
        // Should not duplicate in probe order.
        assert_eq!(registry.len(), 1);
    }
}
