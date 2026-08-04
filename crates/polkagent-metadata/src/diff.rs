//! Metadata diff and upgrade impact analysis.
//!
//! Compares two [`RuntimeMetadata`] snapshots and produces a structured
//! [`MetadataDiff`] describing what changed between runtime versions.
//! This is used to assess the impact of a runtime upgrade before the agent
//! operates against a chain that has undergone a metadata change.
//!
//! # Example
//!
//! ```rust,ignore
//! use polkagent_metadata::diff::{diff_metadata, generate_impact_brief, is_breaking};
//!
//! let diff = diff_metadata(&old_metadata, &new_metadata);
//! if is_breaking(&diff) {
//!     println!("BREAKING: {}", generate_impact_brief(&diff));
//! }
//! ```

use std::collections::{BTreeSet, HashMap};

use polkagent_codec::{CallMetadata, ConstantMetadata, PalletMetadata, RuntimeMetadata};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// PalletDiff
// ---------------------------------------------------------------------------

/// Detailed diff for a single pallet that exists in both old and new metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PalletDiff {
    /// Pallet name.
    pub name: String,
    /// Calls added in the new version.
    pub added_calls: Vec<String>,
    /// Calls removed from the old version.
    pub removed_calls: Vec<String>,
    /// Calls whose signatures (field count or types) changed.
    pub changed_call_signatures: Vec<String>,
    /// Storage entries added.
    pub added_storage: Vec<String>,
    /// Storage entries removed.
    pub removed_storage: Vec<String>,
    /// Constants added in the new version.
    pub added_constants: Vec<String>,
    /// Constants removed from the old version.
    pub removed_constants: Vec<String>,
    /// Constants whose type_id changed.
    pub changed_constants: Vec<String>,
}

impl PalletDiff {
    /// Returns `true` if this pallet has no changes.
    pub fn is_empty(&self) -> bool {
        self.added_calls.is_empty()
            && self.removed_calls.is_empty()
            && self.changed_call_signatures.is_empty()
            && self.added_storage.is_empty()
            && self.removed_storage.is_empty()
            && self.added_constants.is_empty()
            && self.removed_constants.is_empty()
            && self.changed_constants.is_empty()
    }

    /// Returns `true` if any change in this pallet is breaking.
    pub fn has_breaking_changes(&self) -> bool {
        !self.removed_calls.is_empty()
            || !self.changed_call_signatures.is_empty()
            || !self.removed_storage.is_empty()
            || !self.removed_constants.is_empty()
            || !self.changed_constants.is_empty()
    }
}

// ---------------------------------------------------------------------------
// MetadataDiff
// ---------------------------------------------------------------------------

/// Structured diff between two [`RuntimeMetadata`] snapshots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataDiff {
    /// Pallets present in the new metadata but not in the old.
    pub added_pallets: Vec<String>,
    /// Pallets present in the old metadata but not in the new.
    pub removed_pallets: Vec<String>,
    /// Pallets present in both but with changes.
    pub modified_pallets: Vec<PalletDiff>,
    /// Calls added across all pallets (formatted as "Pallet.call_name").
    pub added_calls: Vec<String>,
    /// Calls removed across all pallets (formatted as "Pallet.call_name").
    pub removed_calls: Vec<String>,
    /// Events added (pallet names that gained events).
    pub added_events: Vec<String>,
    /// Events removed (pallet names that lost events).
    pub removed_events: Vec<String>,
    /// Storage entries added across all pallets (formatted as "Pallet.entry").
    pub added_storage: Vec<String>,
    /// Storage entries removed across all pallets (formatted as "Pallet.entry").
    pub removed_storage: Vec<String>,
    /// Constants added across all pallets (formatted as "Pallet.constant_name").
    pub added_constants: Vec<String>,
    /// Constants removed across all pallets (formatted as "Pallet.constant_name").
    pub removed_constants: Vec<String>,
    /// Constants whose type changed across all pallets (formatted as "Pallet.constant_name").
    pub changed_constants: Vec<String>,
    /// Human-readable descriptions of breaking changes.
    pub breaking_changes: Vec<String>,
}

impl MetadataDiff {
    /// Returns `true` if there are no differences.
    pub fn is_empty(&self) -> bool {
        self.added_pallets.is_empty()
            && self.removed_pallets.is_empty()
            && self.modified_pallets.is_empty()
            && self.added_calls.is_empty()
            && self.removed_calls.is_empty()
            && self.added_events.is_empty()
            && self.removed_events.is_empty()
            && self.added_storage.is_empty()
            && self.removed_storage.is_empty()
            && self.added_constants.is_empty()
            && self.removed_constants.is_empty()
            && self.changed_constants.is_empty()
            && self.breaking_changes.is_empty()
    }
}

// ---------------------------------------------------------------------------
// diff_metadata
// ---------------------------------------------------------------------------

/// Compute a structured diff between two [`RuntimeMetadata`] snapshots.
///
/// The diff captures added/removed pallets, call changes, event changes,
/// storage changes, and identifies breaking changes.
pub fn diff_metadata(old: &RuntimeMetadata, new: &RuntimeMetadata) -> MetadataDiff {
    let old_pallets: HashMap<&str, &PalletMetadata> =
        old.pallets.iter().map(|p| (p.name.as_str(), p)).collect();
    let new_pallets: HashMap<&str, &PalletMetadata> =
        new.pallets.iter().map(|p| (p.name.as_str(), p)).collect();

    let old_names: BTreeSet<&str> = old_pallets.keys().copied().collect();
    let new_names: BTreeSet<&str> = new_pallets.keys().copied().collect();

    let added_pallets: Vec<String> = new_names
        .difference(&old_names)
        .map(|s| (*s).to_string())
        .collect();
    let removed_pallets: Vec<String> = old_names
        .difference(&new_names)
        .map(|s| (*s).to_string())
        .collect();

    let mut modified_pallets = Vec::new();
    let mut all_added_calls = Vec::new();
    let mut all_removed_calls = Vec::new();
    let mut added_events = Vec::new();
    let mut removed_events = Vec::new();
    let mut all_added_storage = Vec::new();
    let mut all_removed_storage = Vec::new();
    let mut all_added_constants = Vec::new();
    let mut all_removed_constants = Vec::new();
    let mut all_changed_constants = Vec::new();
    let mut breaking_changes = Vec::new();

    // Collect added pallet calls/events/storage/constants
    for name in &added_pallets {
        if let Some(p) = new_pallets.get(name.as_str()) {
            if let Some(calls) = &p.calls {
                for c in calls {
                    all_added_calls.push(format!("{name}.{}", c.name));
                }
            }
            if p.events.is_some() {
                added_events.push(name.clone());
            }
            if p.storage.is_some() {
                all_added_storage.push(format!("{name}.*"));
            }
            for c in &p.constants {
                all_added_constants.push(format!("{name}.{}", c.name));
            }
        }
    }

    // Collect removed pallet calls/events/storage/constants as breaking
    for name in &removed_pallets {
        breaking_changes.push(format!("Pallet `{name}` removed"));
        if let Some(p) = old_pallets.get(name.as_str()) {
            if let Some(calls) = &p.calls {
                for c in calls {
                    all_removed_calls.push(format!("{name}.{}", c.name));
                }
            }
            if p.events.is_some() {
                removed_events.push(name.clone());
            }
            if p.storage.is_some() {
                all_removed_storage.push(format!("{name}.*"));
            }
            for c in &p.constants {
                all_removed_constants.push(format!("{name}.{}", c.name));
            }
        }
    }

    // Diff pallets present in both
    let common: BTreeSet<&str> = old_names.intersection(&new_names).copied().collect();
    for name in &common {
        let old_p = old_pallets[name];
        let new_p = new_pallets[name];

        let pallet_diff = diff_pallet(old_p, new_p);

        // Aggregate call changes
        for c in &pallet_diff.added_calls {
            all_added_calls.push(format!("{name}.{c}"));
        }
        for c in &pallet_diff.removed_calls {
            all_removed_calls.push(format!("{name}.{c}"));
            breaking_changes.push(format!("Call `{name}.{c}` removed"));
        }
        for c in &pallet_diff.changed_call_signatures {
            breaking_changes.push(format!("Call `{name}.{c}` signature changed"));
        }

        // Aggregate storage changes
        for s in &pallet_diff.added_storage {
            all_added_storage.push(format!("{name}.{s}"));
        }
        for s in &pallet_diff.removed_storage {
            all_removed_storage.push(format!("{name}.{s}"));
            breaking_changes.push(format!("Storage `{name}.{s}` removed"));
        }

        // Aggregate constant changes
        for c in &pallet_diff.added_constants {
            all_added_constants.push(format!("{name}.{c}"));
        }
        for c in &pallet_diff.removed_constants {
            all_removed_constants.push(format!("{name}.{c}"));
            breaking_changes.push(format!("Constant `{name}.{c}` removed"));
        }
        for c in &pallet_diff.changed_constants {
            all_changed_constants.push(format!("{name}.{c}"));
            breaking_changes.push(format!("Constant `{name}.{c}` type changed"));
        }

        // Event changes
        match (&old_p.events, &new_p.events) {
            (None, Some(_)) => added_events.push((*name).to_string()),
            (Some(_), None) => {
                removed_events.push((*name).to_string());
                breaking_changes.push(format!("Events removed from pallet `{name}`"));
            }
            _ => {}
        }

        if !pallet_diff.is_empty() {
            modified_pallets.push(pallet_diff);
        }
    }

    MetadataDiff {
        added_pallets,
        removed_pallets,
        modified_pallets,
        added_calls: all_added_calls,
        removed_calls: all_removed_calls,
        added_events,
        removed_events,
        added_storage: all_added_storage,
        removed_storage: all_removed_storage,
        added_constants: all_added_constants,
        removed_constants: all_removed_constants,
        changed_constants: all_changed_constants,
        breaking_changes,
    }
}

// ---------------------------------------------------------------------------
// diff_pallet
// ---------------------------------------------------------------------------

fn diff_pallet(old: &PalletMetadata, new: &PalletMetadata) -> PalletDiff {
    let (added_calls, removed_calls, changed_call_signatures) = diff_calls(
        old.calls.as_deref().unwrap_or(&[]),
        new.calls.as_deref().unwrap_or(&[]),
    );

    let (added_storage, removed_storage) = diff_storage(old, new);
    let (added_constants, removed_constants, changed_constants) =
        diff_constants(&old.constants, &new.constants);

    PalletDiff {
        name: new.name.clone(),
        added_calls,
        removed_calls,
        changed_call_signatures,
        added_storage,
        removed_storage,
        added_constants,
        removed_constants,
        changed_constants,
    }
}

// ---------------------------------------------------------------------------
// diff_calls
// ---------------------------------------------------------------------------

fn diff_calls(
    old: &[CallMetadata],
    new: &[CallMetadata],
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let old_map: HashMap<&str, &CallMetadata> = old.iter().map(|c| (c.name.as_str(), c)).collect();
    let new_map: HashMap<&str, &CallMetadata> = new.iter().map(|c| (c.name.as_str(), c)).collect();

    let old_names: BTreeSet<&str> = old_map.keys().copied().collect();
    let new_names: BTreeSet<&str> = new_map.keys().copied().collect();

    let added: Vec<String> = new_names
        .difference(&old_names)
        .map(|s| (*s).to_string())
        .collect();
    let removed: Vec<String> = old_names
        .difference(&new_names)
        .map(|s| (*s).to_string())
        .collect();

    let mut changed = Vec::new();
    for name in old_names.intersection(&new_names) {
        let old_call = old_map[name];
        let new_call = new_map[name];
        if !calls_signature_equal(old_call, new_call) {
            changed.push((*name).to_string());
        }
    }

    (added, removed, changed)
}

fn calls_signature_equal(a: &CallMetadata, b: &CallMetadata) -> bool {
    if a.fields.len() != b.fields.len() {
        return false;
    }
    for (fa, fb) in a.fields.iter().zip(b.fields.iter()) {
        if fa.name != fb.name || fa.type_id != fb.type_id {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// diff_storage
// ---------------------------------------------------------------------------

fn diff_storage(old: &PalletMetadata, new: &PalletMetadata) -> (Vec<String>, Vec<String>) {
    // The current StorageMetadata only has a prefix string. We compare
    // presence/absence of storage and the prefix name. A more detailed
    // comparison would require per-entry parsing (future work).
    let old_prefix = old.storage.as_ref().map(|s| s.prefix.as_str());
    let new_prefix = new.storage.as_ref().map(|s| s.prefix.as_str());

    match (old_prefix, new_prefix) {
        (None, Some(p)) => (vec![p.to_string()], vec![]),
        (Some(p), None) => (vec![], vec![p.to_string()]),
        _ => (vec![], vec![]),
    }
}

// ---------------------------------------------------------------------------
// diff_constants
// ---------------------------------------------------------------------------

fn diff_constants(
    old: &[ConstantMetadata],
    new: &[ConstantMetadata],
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let old_map: HashMap<&str, &ConstantMetadata> =
        old.iter().map(|c| (c.name.as_str(), c)).collect();
    let new_map: HashMap<&str, &ConstantMetadata> =
        new.iter().map(|c| (c.name.as_str(), c)).collect();

    let old_names: BTreeSet<&str> = old_map.keys().copied().collect();
    let new_names: BTreeSet<&str> = new_map.keys().copied().collect();

    let added: Vec<String> = new_names
        .difference(&old_names)
        .map(|s| (*s).to_string())
        .collect();
    let removed: Vec<String> = old_names
        .difference(&new_names)
        .map(|s| (*s).to_string())
        .collect();

    let mut changed = Vec::new();
    for name in old_names.intersection(&new_names) {
        let old_c = old_map[name];
        let new_c = new_map[name];
        if old_c.type_id != new_c.type_id {
            changed.push((*name).to_string());
        }
    }

    (added, removed, changed)
}

// ---------------------------------------------------------------------------
// is_breaking
// ---------------------------------------------------------------------------

/// Returns `true` if the diff contains any breaking changes.
///
/// A change is considered breaking if it removes or modifies existing
/// functionality that consumers may depend on:
/// - Removed pallets
/// - Removed calls
/// - Changed call signatures
/// - Removed storage entries
/// - Removed events
pub fn is_breaking(diff: &MetadataDiff) -> bool {
    !diff.breaking_changes.is_empty()
}

// ---------------------------------------------------------------------------
// generate_impact_brief
// ---------------------------------------------------------------------------

/// Generate a plain-language summary of the metadata diff.
///
/// Returns a human-readable string describing what changed and whether the
/// changes are breaking. Suitable for display in logs, alerts, or agent
/// decision-making context.
pub fn generate_impact_brief(diff: &MetadataDiff) -> String {
    if diff.is_empty() {
        return "No metadata changes detected.".to_string();
    }

    let mut lines = Vec::new();

    // Header
    let breaking = is_breaking(diff);
    if breaking {
        lines.push("BREAKING runtime upgrade detected.".to_string());
    } else {
        lines.push("Non-breaking runtime upgrade detected.".to_string());
    }

    // Pallet summary
    if !diff.added_pallets.is_empty() {
        lines.push(format!("Added pallets: {}", diff.added_pallets.join(", ")));
    }
    if !diff.removed_pallets.is_empty() {
        lines.push(format!(
            "Removed pallets: {}",
            diff.removed_pallets.join(", ")
        ));
    }
    if !diff.modified_pallets.is_empty() {
        let names: Vec<&str> = diff
            .modified_pallets
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        lines.push(format!("Modified pallets: {}", names.join(", ")));
    }

    // Call summary
    if !diff.added_calls.is_empty() {
        lines.push(format!(
            "Added calls ({}): {}",
            diff.added_calls.len(),
            diff.added_calls.join(", ")
        ));
    }
    if !diff.removed_calls.is_empty() {
        lines.push(format!(
            "Removed calls ({}): {}",
            diff.removed_calls.len(),
            diff.removed_calls.join(", ")
        ));
    }

    // Event summary
    if !diff.added_events.is_empty() {
        lines.push(format!("Added events in: {}", diff.added_events.join(", ")));
    }
    if !diff.removed_events.is_empty() {
        lines.push(format!(
            "Removed events from: {}",
            diff.removed_events.join(", ")
        ));
    }

    // Storage summary
    if !diff.added_storage.is_empty() {
        lines.push(format!(
            "Added storage ({}): {}",
            diff.added_storage.len(),
            diff.added_storage.join(", ")
        ));
    }
    if !diff.removed_storage.is_empty() {
        lines.push(format!(
            "Removed storage ({}): {}",
            diff.removed_storage.len(),
            diff.removed_storage.join(", ")
        ));
    }

    // Constants summary
    if !diff.added_constants.is_empty() {
        lines.push(format!(
            "Added constants ({}): {}",
            diff.added_constants.len(),
            diff.added_constants.join(", ")
        ));
    }
    if !diff.removed_constants.is_empty() {
        lines.push(format!(
            "Removed constants ({}): {}",
            diff.removed_constants.len(),
            diff.removed_constants.join(", ")
        ));
    }
    if !diff.changed_constants.is_empty() {
        lines.push(format!(
            "Changed constants ({}): {}",
            diff.changed_constants.len(),
            diff.changed_constants.join(", ")
        ));
    }

    // Breaking change details
    if !diff.breaking_changes.is_empty() {
        lines.push(format!(
            "Breaking changes ({}):",
            diff.breaking_changes.len()
        ));
        for bc in &diff.breaking_changes {
            lines.push(format!("  - {bc}"));
        }
    }

    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_codec::{
        CallMetadata, EventMetadata, FieldMetadata, PalletMetadata, RuntimeMetadata,
        StorageMetadata, TypeRegistry,
    };

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn empty_metadata() -> RuntimeMetadata {
        RuntimeMetadata {
            version: 14,
            pallets: vec![],
            types: TypeRegistry::new(),
        }
    }

    fn make_pallet(name: &str, index: u8) -> PalletMetadata {
        PalletMetadata {
            name: name.to_string(),
            index,
            calls: None,
            storage: None,
            events: None,
            constants: vec![],
        }
    }

    fn make_pallet_with_calls(name: &str, index: u8, calls: Vec<CallMetadata>) -> PalletMetadata {
        PalletMetadata {
            name: name.to_string(),
            index,
            calls: Some(calls),
            storage: None,
            events: None,
            constants: vec![],
        }
    }

    fn make_call(name: &str, index: u8, fields: Vec<FieldMetadata>) -> CallMetadata {
        CallMetadata {
            name: name.to_string(),
            index,
            fields,
        }
    }

    fn make_field(name: &str, type_id: u32) -> FieldMetadata {
        FieldMetadata {
            name: Some(name.to_string()),
            type_id,
        }
    }

    fn make_pallet_with_storage(name: &str, index: u8, prefix: &str) -> PalletMetadata {
        PalletMetadata {
            name: name.to_string(),
            index,
            calls: None,
            storage: Some(StorageMetadata {
                prefix: prefix.to_string(),
            }),
            events: None,
            constants: vec![],
        }
    }

    fn make_pallet_with_events(name: &str, index: u8, type_id: u32) -> PalletMetadata {
        PalletMetadata {
            name: name.to_string(),
            index,
            calls: None,
            storage: None,
            events: Some(EventMetadata { type_id }),
            constants: vec![],
        }
    }

    fn metadata_with_pallets(pallets: Vec<PalletMetadata>) -> RuntimeMetadata {
        RuntimeMetadata {
            version: 14,
            pallets,
            types: TypeRegistry::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Test: identical metadata produces empty diff
    // -----------------------------------------------------------------------

    #[test]
    fn identical_metadata_empty_diff() {
        let meta =
            metadata_with_pallets(vec![make_pallet("System", 0), make_pallet("Balances", 5)]);
        let diff = diff_metadata(&meta, &meta);
        assert!(diff.is_empty());
        assert!(!is_breaking(&diff));
    }

    // -----------------------------------------------------------------------
    // Test: both empty → empty diff
    // -----------------------------------------------------------------------

    #[test]
    fn both_empty_metadata() {
        let diff = diff_metadata(&empty_metadata(), &empty_metadata());
        assert!(diff.is_empty());
        assert!(!is_breaking(&diff));
    }

    // -----------------------------------------------------------------------
    // Test: added pallet detected
    // -----------------------------------------------------------------------

    #[test]
    fn added_pallet_detected() {
        let old = metadata_with_pallets(vec![make_pallet("System", 0)]);
        let new = metadata_with_pallets(vec![make_pallet("System", 0), make_pallet("Balances", 5)]);
        let diff = diff_metadata(&old, &new);
        assert_eq!(diff.added_pallets, vec!["Balances"]);
        assert!(diff.removed_pallets.is_empty());
        assert!(!is_breaking(&diff));
    }

    // -----------------------------------------------------------------------
    // Test: removed pallet is breaking
    // -----------------------------------------------------------------------

    #[test]
    fn removed_pallet_is_breaking() {
        let old = metadata_with_pallets(vec![make_pallet("System", 0), make_pallet("Balances", 5)]);
        let new = metadata_with_pallets(vec![make_pallet("System", 0)]);
        let diff = diff_metadata(&old, &new);
        assert_eq!(diff.removed_pallets, vec!["Balances"]);
        assert!(is_breaking(&diff));
        assert!(diff
            .breaking_changes
            .iter()
            .any(|c| c.contains("Balances") && c.contains("removed")));
    }

    // -----------------------------------------------------------------------
    // Test: added call in existing pallet
    // -----------------------------------------------------------------------

    #[test]
    fn added_call_in_existing_pallet() {
        let old = metadata_with_pallets(vec![make_pallet_with_calls(
            "Balances",
            5,
            vec![make_call("transfer", 0, vec![])],
        )]);
        let new = metadata_with_pallets(vec![make_pallet_with_calls(
            "Balances",
            5,
            vec![
                make_call("transfer", 0, vec![]),
                make_call("transfer_keep_alive", 1, vec![]),
            ],
        )]);
        let diff = diff_metadata(&old, &new);
        assert!(diff
            .added_calls
            .contains(&"Balances.transfer_keep_alive".to_string()));
        assert!(diff.removed_calls.is_empty());
        assert!(!is_breaking(&diff));
    }

    // -----------------------------------------------------------------------
    // Test: removed call is breaking
    // -----------------------------------------------------------------------

    #[test]
    fn removed_call_is_breaking() {
        let old = metadata_with_pallets(vec![make_pallet_with_calls(
            "Balances",
            5,
            vec![
                make_call("transfer", 0, vec![]),
                make_call("set_balance", 1, vec![]),
            ],
        )]);
        let new = metadata_with_pallets(vec![make_pallet_with_calls(
            "Balances",
            5,
            vec![make_call("transfer", 0, vec![])],
        )]);
        let diff = diff_metadata(&old, &new);
        assert!(diff
            .removed_calls
            .contains(&"Balances.set_balance".to_string()));
        assert!(is_breaking(&diff));
    }

    // -----------------------------------------------------------------------
    // Test: changed call signature is breaking
    // -----------------------------------------------------------------------

    #[test]
    fn changed_call_signature_is_breaking() {
        let old = metadata_with_pallets(vec![make_pallet_with_calls(
            "Balances",
            5,
            vec![make_call(
                "transfer",
                0,
                vec![make_field("dest", 1), make_field("value", 2)],
            )],
        )]);
        let new = metadata_with_pallets(vec![make_pallet_with_calls(
            "Balances",
            5,
            vec![make_call(
                "transfer",
                0,
                vec![
                    make_field("dest", 1),
                    make_field("value", 2),
                    make_field("memo", 3),
                ],
            )],
        )]);
        let diff = diff_metadata(&old, &new);
        assert!(is_breaking(&diff));
        assert!(diff
            .breaking_changes
            .iter()
            .any(|c| c.contains("transfer") && c.contains("signature")));
        assert_eq!(diff.modified_pallets.len(), 1);
        assert!(diff.modified_pallets[0]
            .changed_call_signatures
            .contains(&"transfer".to_string()));
    }

    // -----------------------------------------------------------------------
    // Test: field type_id change is signature change
    // -----------------------------------------------------------------------

    #[test]
    fn field_type_id_change_is_breaking() {
        let old = metadata_with_pallets(vec![make_pallet_with_calls(
            "Balances",
            5,
            vec![make_call("transfer", 0, vec![make_field("value", 2)])],
        )]);
        let new = metadata_with_pallets(vec![make_pallet_with_calls(
            "Balances",
            5,
            vec![make_call("transfer", 0, vec![make_field("value", 99)])],
        )]);
        let diff = diff_metadata(&old, &new);
        assert!(is_breaking(&diff));
        assert!(diff.modified_pallets[0]
            .changed_call_signatures
            .contains(&"transfer".to_string()));
    }

    // -----------------------------------------------------------------------
    // Test: added events
    // -----------------------------------------------------------------------

    #[test]
    fn added_events_detected() {
        let old = metadata_with_pallets(vec![make_pallet("Balances", 5)]);
        let new = metadata_with_pallets(vec![make_pallet_with_events("Balances", 5, 42)]);
        let diff = diff_metadata(&old, &new);
        assert!(diff.added_events.contains(&"Balances".to_string()));
        assert!(diff.removed_events.is_empty());
        assert!(!is_breaking(&diff));
    }

    // -----------------------------------------------------------------------
    // Test: removed events is breaking
    // -----------------------------------------------------------------------

    #[test]
    fn removed_events_is_breaking() {
        let old = metadata_with_pallets(vec![make_pallet_with_events("Balances", 5, 42)]);
        let new = metadata_with_pallets(vec![make_pallet("Balances", 5)]);
        let diff = diff_metadata(&old, &new);
        assert!(diff.removed_events.contains(&"Balances".to_string()));
        assert!(is_breaking(&diff));
    }

    // -----------------------------------------------------------------------
    // Test: added storage
    // -----------------------------------------------------------------------

    #[test]
    fn added_storage_detected() {
        let old = metadata_with_pallets(vec![make_pallet("Balances", 5)]);
        let new = metadata_with_pallets(vec![make_pallet_with_storage("Balances", 5, "Balances")]);
        let diff = diff_metadata(&old, &new);
        assert!(!diff.added_storage.is_empty());
        assert!(diff.removed_storage.is_empty());
        assert!(!is_breaking(&diff));
    }

    // -----------------------------------------------------------------------
    // Test: removed storage is breaking
    // -----------------------------------------------------------------------

    #[test]
    fn removed_storage_is_breaking() {
        let old = metadata_with_pallets(vec![make_pallet_with_storage("Balances", 5, "Balances")]);
        let new = metadata_with_pallets(vec![make_pallet("Balances", 5)]);
        let diff = diff_metadata(&old, &new);
        assert!(!diff.removed_storage.is_empty());
        assert!(is_breaking(&diff));
    }

    // -----------------------------------------------------------------------
    // Test: multiple pallet changes
    // -----------------------------------------------------------------------

    #[test]
    fn multiple_pallet_changes() {
        let old = metadata_with_pallets(vec![
            make_pallet("System", 0),
            make_pallet("Balances", 5),
            make_pallet("Staking", 7),
        ]);
        let new = metadata_with_pallets(vec![
            make_pallet("System", 0),
            make_pallet("Staking", 7),
            make_pallet("Governance", 10),
        ]);
        let diff = diff_metadata(&old, &new);
        assert_eq!(diff.added_pallets, vec!["Governance"]);
        assert_eq!(diff.removed_pallets, vec!["Balances"]);
        assert!(is_breaking(&diff));
    }

    // -----------------------------------------------------------------------
    // Test: generate_impact_brief for empty diff
    // -----------------------------------------------------------------------

    #[test]
    fn impact_brief_empty_diff() {
        let diff = diff_metadata(&empty_metadata(), &empty_metadata());
        let brief = generate_impact_brief(&diff);
        assert_eq!(brief, "No metadata changes detected.");
    }

    // -----------------------------------------------------------------------
    // Test: generate_impact_brief for non-breaking
    // -----------------------------------------------------------------------

    #[test]
    fn impact_brief_non_breaking() {
        let old = metadata_with_pallets(vec![make_pallet("System", 0)]);
        let new =
            metadata_with_pallets(vec![make_pallet("System", 0), make_pallet("NewPallet", 99)]);
        let diff = diff_metadata(&old, &new);
        let brief = generate_impact_brief(&diff);
        assert!(brief.contains("Non-breaking"));
        assert!(brief.contains("NewPallet"));
    }

    // -----------------------------------------------------------------------
    // Test: generate_impact_brief for breaking
    // -----------------------------------------------------------------------

    #[test]
    fn impact_brief_breaking() {
        let old = metadata_with_pallets(vec![make_pallet("System", 0), make_pallet("Balances", 5)]);
        let new = metadata_with_pallets(vec![make_pallet("System", 0)]);
        let diff = diff_metadata(&old, &new);
        let brief = generate_impact_brief(&diff);
        assert!(brief.contains("BREAKING"));
        assert!(brief.contains("Balances"));
        assert!(brief.contains("removed"));
    }

    // -----------------------------------------------------------------------
    // Test: is_breaking false for additive-only changes
    // -----------------------------------------------------------------------

    #[test]
    fn is_breaking_false_for_additive_changes() {
        let old = metadata_with_pallets(vec![make_pallet_with_calls(
            "Balances",
            5,
            vec![make_call("transfer", 0, vec![])],
        )]);
        let new = metadata_with_pallets(vec![make_pallet_with_calls(
            "Balances",
            5,
            vec![
                make_call("transfer", 0, vec![]),
                make_call("transfer_all", 1, vec![]),
            ],
        )]);
        let diff = diff_metadata(&old, &new);
        assert!(!is_breaking(&diff));
    }

    // -----------------------------------------------------------------------
    // Test: PalletDiff is_empty
    // -----------------------------------------------------------------------

    #[test]
    fn pallet_diff_is_empty() {
        let pd = PalletDiff {
            name: "Test".to_string(),
            added_calls: vec![],
            removed_calls: vec![],
            changed_call_signatures: vec![],
            added_storage: vec![],
            removed_storage: vec![],
            added_constants: vec![],
            removed_constants: vec![],
            changed_constants: vec![],
        };
        assert!(pd.is_empty());
        assert!(!pd.has_breaking_changes());
    }

    // -----------------------------------------------------------------------
    // Test: PalletDiff has_breaking_changes
    // -----------------------------------------------------------------------

    #[test]
    fn pallet_diff_has_breaking_removed_call() {
        let pd = PalletDiff {
            name: "Test".to_string(),
            added_calls: vec![],
            removed_calls: vec!["foo".to_string()],
            changed_call_signatures: vec![],
            added_storage: vec![],
            removed_storage: vec![],
            added_constants: vec![],
            removed_constants: vec![],
            changed_constants: vec![],
        };
        assert!(!pd.is_empty());
        assert!(pd.has_breaking_changes());
    }

    #[test]
    fn pallet_diff_has_breaking_changed_signature() {
        let pd = PalletDiff {
            name: "Test".to_string(),
            added_calls: vec![],
            removed_calls: vec![],
            changed_call_signatures: vec!["bar".to_string()],
            added_storage: vec![],
            removed_storage: vec![],
            added_constants: vec![],
            removed_constants: vec![],
            changed_constants: vec![],
        };
        assert!(pd.has_breaking_changes());
    }

    #[test]
    fn pallet_diff_has_breaking_removed_storage() {
        let pd = PalletDiff {
            name: "Test".to_string(),
            added_calls: vec![],
            removed_calls: vec![],
            changed_call_signatures: vec![],
            added_storage: vec![],
            removed_storage: vec!["Account".to_string()],
            added_constants: vec![],
            removed_constants: vec![],
            changed_constants: vec![],
        };
        assert!(pd.has_breaking_changes());
    }

    // -----------------------------------------------------------------------
    // Test: calls going from None to Some
    // -----------------------------------------------------------------------

    #[test]
    fn calls_none_to_some() {
        let old = metadata_with_pallets(vec![make_pallet("Balances", 5)]);
        let new = metadata_with_pallets(vec![make_pallet_with_calls(
            "Balances",
            5,
            vec![make_call("transfer", 0, vec![])],
        )]);
        let diff = diff_metadata(&old, &new);
        assert!(diff.added_calls.contains(&"Balances.transfer".to_string()));
        assert!(!is_breaking(&diff));
    }

    // -----------------------------------------------------------------------
    // Test: calls going from Some to None (breaking)
    // -----------------------------------------------------------------------

    #[test]
    fn calls_some_to_none() {
        let old = metadata_with_pallets(vec![make_pallet_with_calls(
            "Balances",
            5,
            vec![make_call("transfer", 0, vec![])],
        )]);
        let new = metadata_with_pallets(vec![make_pallet("Balances", 5)]);
        let diff = diff_metadata(&old, &new);
        assert!(diff
            .removed_calls
            .contains(&"Balances.transfer".to_string()));
        assert!(is_breaking(&diff));
    }

    // -----------------------------------------------------------------------
    // Test: MetadataDiff serialization round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn metadata_diff_serde_round_trip() {
        let old = metadata_with_pallets(vec![make_pallet("System", 0)]);
        let new = metadata_with_pallets(vec![make_pallet("System", 0), make_pallet("Balances", 5)]);
        let diff = diff_metadata(&old, &new);
        let json = serde_json::to_string(&diff).expect("serialize");
        let back: MetadataDiff = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.added_pallets, diff.added_pallets);
        assert_eq!(back.removed_pallets, diff.removed_pallets);
    }

    // -----------------------------------------------------------------------
    // Test: PalletDiff serialization round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn pallet_diff_serde_round_trip() {
        let pd = PalletDiff {
            name: "Balances".to_string(),
            added_calls: vec!["transfer_all".to_string()],
            removed_calls: vec!["set_balance".to_string()],
            changed_call_signatures: vec!["transfer".to_string()],
            added_storage: vec!["Locks".to_string()],
            removed_storage: vec![],
            added_constants: vec!["NewConst".to_string()],
            removed_constants: vec![],
            changed_constants: vec![],
        };
        let json = serde_json::to_string(&pd).expect("serialize");
        let back: PalletDiff = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.name, pd.name);
        assert_eq!(back.added_calls, pd.added_calls);
        assert_eq!(back.removed_calls, pd.removed_calls);
    }

    // -----------------------------------------------------------------------
    // Test: new pallet with calls shows up in added_calls
    // -----------------------------------------------------------------------

    #[test]
    fn new_pallet_with_calls_adds_to_added_calls() {
        let old = metadata_with_pallets(vec![make_pallet("System", 0)]);
        let new = metadata_with_pallets(vec![
            make_pallet("System", 0),
            make_pallet_with_calls(
                "Governance",
                10,
                vec![
                    make_call("propose", 0, vec![]),
                    make_call("vote", 1, vec![]),
                ],
            ),
        ]);
        let diff = diff_metadata(&old, &new);
        assert!(diff.added_calls.contains(&"Governance.propose".to_string()));
        assert!(diff.added_calls.contains(&"Governance.vote".to_string()));
    }

    // -----------------------------------------------------------------------
    // Test: removed pallet with calls shows up in removed_calls
    // -----------------------------------------------------------------------

    #[test]
    fn removed_pallet_with_calls_adds_to_removed_calls() {
        let old = metadata_with_pallets(vec![
            make_pallet("System", 0),
            make_pallet_with_calls("Governance", 10, vec![make_call("propose", 0, vec![])]),
        ]);
        let new = metadata_with_pallets(vec![make_pallet("System", 0)]);
        let diff = diff_metadata(&old, &new);
        assert!(diff
            .removed_calls
            .contains(&"Governance.propose".to_string()));
    }

    // -----------------------------------------------------------------------
    // Test: unchanged call signature not flagged
    // -----------------------------------------------------------------------

    #[test]
    fn unchanged_call_signature_not_flagged() {
        let calls = vec![make_call(
            "transfer",
            0,
            vec![make_field("dest", 1), make_field("value", 2)],
        )];
        let old = metadata_with_pallets(vec![make_pallet_with_calls("Balances", 5, calls.clone())]);
        let new = metadata_with_pallets(vec![make_pallet_with_calls("Balances", 5, calls)]);
        let diff = diff_metadata(&old, &new);
        assert!(diff.is_empty());
        assert!(!is_breaking(&diff));
    }

    // -----------------------------------------------------------------------
    // Test: field name change is signature change
    // -----------------------------------------------------------------------

    #[test]
    fn field_name_change_is_breaking() {
        let old = metadata_with_pallets(vec![make_pallet_with_calls(
            "Balances",
            5,
            vec![make_call("transfer", 0, vec![make_field("dest", 1)])],
        )]);
        let new = metadata_with_pallets(vec![make_pallet_with_calls(
            "Balances",
            5,
            vec![make_call("transfer", 0, vec![make_field("target", 1)])],
        )]);
        let diff = diff_metadata(&old, &new);
        assert!(is_breaking(&diff));
    }

    // -----------------------------------------------------------------------
    // Test: impact brief contains modified pallets
    // -----------------------------------------------------------------------

    #[test]
    fn impact_brief_contains_modified_pallets() {
        let old = metadata_with_pallets(vec![make_pallet_with_calls(
            "Balances",
            5,
            vec![make_call("transfer", 0, vec![])],
        )]);
        let new = metadata_with_pallets(vec![make_pallet_with_calls(
            "Balances",
            5,
            vec![
                make_call("transfer", 0, vec![]),
                make_call("transfer_all", 1, vec![]),
            ],
        )]);
        let diff = diff_metadata(&old, &new);
        let brief = generate_impact_brief(&diff);
        assert!(brief.contains("Modified pallets"));
        assert!(brief.contains("Balances"));
    }

    // -----------------------------------------------------------------------
    // Test: impact brief contains call counts
    // -----------------------------------------------------------------------

    #[test]
    fn impact_brief_contains_call_counts() {
        let old = metadata_with_pallets(vec![make_pallet_with_calls("B", 5, vec![])]);
        let new = metadata_with_pallets(vec![make_pallet_with_calls(
            "B",
            5,
            vec![make_call("a", 0, vec![]), make_call("b", 1, vec![])],
        )]);
        let diff = diff_metadata(&old, &new);
        let brief = generate_impact_brief(&diff);
        assert!(brief.contains("Added calls (2)"));
    }

    // -----------------------------------------------------------------------
    // Test: new pallet with events shows in added_events
    // -----------------------------------------------------------------------

    #[test]
    fn new_pallet_with_events_in_added_events() {
        let old = metadata_with_pallets(vec![make_pallet("System", 0)]);
        let new = metadata_with_pallets(vec![
            make_pallet("System", 0),
            make_pallet_with_events("Balances", 5, 42),
        ]);
        let diff = diff_metadata(&old, &new);
        assert!(diff.added_events.contains(&"Balances".to_string()));
    }

    // -----------------------------------------------------------------------
    // Test: removed pallet with events shows in removed_events
    // -----------------------------------------------------------------------

    #[test]
    fn removed_pallet_with_events_in_removed_events() {
        let old = metadata_with_pallets(vec![
            make_pallet("System", 0),
            make_pallet_with_events("Balances", 5, 42),
        ]);
        let new = metadata_with_pallets(vec![make_pallet("System", 0)]);
        let diff = diff_metadata(&old, &new);
        assert!(diff.removed_events.contains(&"Balances".to_string()));
    }

    // -----------------------------------------------------------------------
    // Test: new pallet with storage shows in added_storage
    // -----------------------------------------------------------------------

    #[test]
    fn new_pallet_with_storage_in_added_storage() {
        let old = metadata_with_pallets(vec![make_pallet("System", 0)]);
        let new = metadata_with_pallets(vec![
            make_pallet("System", 0),
            make_pallet_with_storage("Balances", 5, "Balances"),
        ]);
        let diff = diff_metadata(&old, &new);
        assert!(!diff.added_storage.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test: removed pallet with storage shows in removed_storage
    // -----------------------------------------------------------------------

    #[test]
    fn removed_pallet_with_storage_in_removed_storage() {
        let old = metadata_with_pallets(vec![
            make_pallet("System", 0),
            make_pallet_with_storage("Balances", 5, "Balances"),
        ]);
        let new = metadata_with_pallets(vec![make_pallet("System", 0)]);
        let diff = diff_metadata(&old, &new);
        assert!(!diff.removed_storage.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test: impact brief mentions breaking change count
    // -----------------------------------------------------------------------

    #[test]
    fn impact_brief_shows_breaking_change_count() {
        let old = metadata_with_pallets(vec![make_pallet("A", 1), make_pallet("B", 2)]);
        let new = metadata_with_pallets(vec![]);
        let diff = diff_metadata(&old, &new);
        let brief = generate_impact_brief(&diff);
        assert!(brief.contains("Breaking changes (2)"));
    }
}
