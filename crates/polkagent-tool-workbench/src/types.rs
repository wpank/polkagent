//! Shared domain types for workbench tools.
//!
//! These types model the output of builder workflows such as storage
//! migration rehearsal. All types derive [`Serialize`] and [`Deserialize`]
//! so they can be returned as structured JSON from tool handlers.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// StorageKeyChange
// ---------------------------------------------------------------------------

/// Describes how a single storage key changed during a migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageKeyChange {
    /// The key existed before and after, but its value changed.
    Modified {
        /// Hex-encoded storage key.
        key: String,
        /// Hex-encoded value before the migration.
        old_value: String,
        /// Hex-encoded value after the migration.
        new_value: String,
    },
    /// The key was added by the migration (did not exist before).
    Added {
        /// Hex-encoded storage key.
        key: String,
        /// Hex-encoded value after the migration.
        value: String,
    },
    /// The key was removed by the migration (existed before, absent after).
    Removed {
        /// Hex-encoded storage key.
        key: String,
        /// Hex-encoded value before the migration.
        value: String,
    },
}

// ---------------------------------------------------------------------------
// StorageDiff
// ---------------------------------------------------------------------------

/// A structured diff of storage changes produced by a migration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageDiff {
    /// Keys whose values changed.
    pub modified: Vec<StorageKeyChange>,
    /// Keys that were added.
    pub added: Vec<StorageKeyChange>,
    /// Keys that were removed.
    pub removed: Vec<StorageKeyChange>,
}

impl StorageDiff {
    /// Total number of changed keys across all categories.
    #[must_use]
    pub fn total_changes(&self) -> usize {
        self.modified.len() + self.added.len() + self.removed.len()
    }

    /// Returns `true` if the migration produced no storage changes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.modified.is_empty() && self.added.is_empty() && self.removed.is_empty()
    }
}

// ---------------------------------------------------------------------------
// MigrationRehearsalReport
// ---------------------------------------------------------------------------

/// Structured report from a storage migration rehearsal.
///
/// Contains the migration parameters, the resulting storage diff, and
/// summary counts. This is a read-only analysis artifact — no state
/// was modified on any live chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationRehearsalReport {
    /// The path to the runtime WASM blob that was analyzed.
    pub wasm_blob_path: String,
    /// The block number at which the fork was taken.
    pub block_number: u64,
    /// Number of storage keys that were modified.
    pub modified_count: usize,
    /// Number of storage keys that were added.
    pub added_count: usize,
    /// Number of storage keys that were removed.
    pub removed_count: usize,
    /// Total number of storage key changes.
    pub total_changes: usize,
    /// The full storage diff.
    pub diff: StorageDiff,
}

// ---------------------------------------------------------------------------
// MetadataComparisonReport
// ---------------------------------------------------------------------------

/// Structured report from a metadata comparison between two runtime versions.
///
/// Enumerates every change across pallets, calls, events, storage items,
/// and constants with before/after detail. This is a read-only analysis
/// artifact — no state was modified on any live chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataComparisonReport {
    /// Identifier for the old metadata source (block number or file path).
    pub old_source: String,
    /// Identifier for the new metadata source (block number or file path).
    pub new_source: String,
    /// Whether the diff contains any breaking changes.
    pub is_breaking: bool,
    /// Pallets added in the new version.
    pub added_pallets: Vec<String>,
    /// Pallets removed from the old version.
    pub removed_pallets: Vec<String>,
    /// Pallets present in both but with changes.
    pub modified_pallets: Vec<PalletComparisonDetail>,
    /// Summary counts.
    pub summary: ComparisonSummary,
    /// Human-readable impact brief suitable for an action card.
    pub impact_brief: String,
}

/// Summary counts for a metadata comparison.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComparisonSummary {
    /// Total pallets added.
    pub pallets_added: usize,
    /// Total pallets removed.
    pub pallets_removed: usize,
    /// Total pallets modified.
    pub pallets_modified: usize,
    /// Total calls added across all pallets.
    pub calls_added: usize,
    /// Total calls removed across all pallets.
    pub calls_removed: usize,
    /// Total call signatures changed.
    pub calls_changed: usize,
    /// Total events added.
    pub events_added: usize,
    /// Total events removed.
    pub events_removed: usize,
    /// Total storage entries added.
    pub storage_added: usize,
    /// Total storage entries removed.
    pub storage_removed: usize,
    /// Total constants added.
    pub constants_added: usize,
    /// Total constants removed.
    pub constants_removed: usize,
    /// Total constants whose type changed.
    pub constants_changed: usize,
    /// Total breaking changes.
    pub breaking_change_count: usize,
}

/// Detailed comparison for a single pallet that was modified.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PalletComparisonDetail {
    /// Pallet name.
    pub name: String,
    /// Calls added.
    pub added_calls: Vec<String>,
    /// Calls removed.
    pub removed_calls: Vec<String>,
    /// Calls whose signatures changed.
    pub changed_call_signatures: Vec<String>,
    /// Storage entries added.
    pub added_storage: Vec<String>,
    /// Storage entries removed.
    pub removed_storage: Vec<String>,
    /// Constants added.
    pub added_constants: Vec<String>,
    /// Constants removed.
    pub removed_constants: Vec<String>,
    /// Constants whose type changed.
    pub changed_constants: Vec<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_diff() -> StorageDiff {
        StorageDiff {
            modified: vec![StorageKeyChange::Modified {
                key: "0xabcd".to_string(),
                old_value: "0x01".to_string(),
                new_value: "0x02".to_string(),
            }],
            added: vec![StorageKeyChange::Added {
                key: "0x1234".to_string(),
                value: "0xff".to_string(),
            }],
            removed: vec![StorageKeyChange::Removed {
                key: "0x5678".to_string(),
                value: "0xaa".to_string(),
            }],
        }
    }

    #[test]
    fn storage_diff_total_changes() {
        let diff = sample_diff();
        assert_eq!(diff.total_changes(), 3);
    }

    #[test]
    fn storage_diff_is_empty_false() {
        let diff = sample_diff();
        assert!(!diff.is_empty());
    }

    #[test]
    fn storage_diff_is_empty_true() {
        let diff = StorageDiff {
            modified: vec![],
            added: vec![],
            removed: vec![],
        };
        assert!(diff.is_empty());
    }

    #[test]
    fn storage_key_change_serde_modified() {
        let change = StorageKeyChange::Modified {
            key: "0xab".to_string(),
            old_value: "0x01".to_string(),
            new_value: "0x02".to_string(),
        };
        let json = serde_json::to_value(&change).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(json["modified"]["key"], "0xab");
        let back: StorageKeyChange = serde_json::from_value(json).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(back, change);
    }

    #[test]
    fn storage_key_change_serde_added() {
        let change = StorageKeyChange::Added {
            key: "0xcd".to_string(),
            value: "0xff".to_string(),
        };
        let json = serde_json::to_value(&change).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(json["added"]["key"], "0xcd");
    }

    #[test]
    fn storage_key_change_serde_removed() {
        let change = StorageKeyChange::Removed {
            key: "0xef".to_string(),
            value: "0xaa".to_string(),
        };
        let json = serde_json::to_value(&change).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(json["removed"]["key"], "0xef");
    }

    #[test]
    fn migration_report_serializes() {
        let report = MigrationRehearsalReport {
            wasm_blob_path: "/tmp/runtime.wasm".to_string(),
            block_number: 12_345_678,
            modified_count: 1,
            added_count: 1,
            removed_count: 1,
            total_changes: 3,
            diff: sample_diff(),
        };
        let json = serde_json::to_value(&report).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(json["block_number"], 12_345_678);
        assert_eq!(json["total_changes"], 3);
        assert_eq!(json["wasm_blob_path"], "/tmp/runtime.wasm");
    }

    #[test]
    fn migration_report_deserializes() {
        let json = serde_json::json!({
            "wasm_blob_path": "/tmp/rt.wasm",
            "block_number": 100,
            "modified_count": 0,
            "added_count": 0,
            "removed_count": 0,
            "total_changes": 0,
            "diff": {
                "modified": [],
                "added": [],
                "removed": []
            }
        });
        let report: MigrationRehearsalReport =
            serde_json::from_value(json).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(report.block_number, 100);
        assert!(report.diff.is_empty());
    }
}
