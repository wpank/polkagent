//! Metadata drift detection.
//!
//! [`DriftDetector`] compares the current metadata snapshot for a chain
//! against a set of pinned (trusted) hashes to detect runtime upgrades
//! or unexpected metadata changes.

use polkagent_codec::parse_metadata;
use polkagent_core::now;
use tracing::debug;

use crate::diff::diff_metadata;
use crate::types::{ChainId, MetadataDrift, MetadataSnapshot, PinnedMetadata};

/// Detects metadata drift by comparing current metadata hashes against pins.
///
/// This is a stateless comparator — it does not store any state itself.
/// It receives the current snapshot and the set of pins and produces a
/// drift record if they do not match.
#[derive(Debug, Clone, Default)]
pub struct DriftDetector;

impl DriftDetector {
    /// Create a new drift detector.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Check whether the current snapshot's hash matches any of the provided pins.
    ///
    /// Returns `Some(MetadataDrift)` if no pin matches the current hash,
    /// or `None` if the metadata matches a pin (or if no pins are provided).
    ///
    /// When drift is detected with no pinned snapshot available for comparison,
    /// [`check_with_old_snapshot`](Self::check_with_old_snapshot) can be used
    /// instead to get full pallet-level diffs.
    #[must_use]
    pub fn check(
        &self,
        chain_id: &ChainId,
        current_snapshot: &MetadataSnapshot,
        pins: &[PinnedMetadata],
    ) -> Option<MetadataDrift> {
        self.check_with_old_snapshot(chain_id, current_snapshot, pins, None)
    }

    /// Like [`check`](Self::check), but accepts an optional old snapshot for
    /// pallet-level diffing.
    ///
    /// When `old_snapshot` is `Some`, the raw metadata bytes from both
    /// snapshots are SCALE-decoded and compared at the pallet level to produce
    /// a detailed list of affected pallets. When `old_snapshot` is `None`,
    /// only the current snapshot's pallet list is reported.
    #[must_use]
    pub fn check_with_old_snapshot(
        &self,
        chain_id: &ChainId,
        current_snapshot: &MetadataSnapshot,
        pins: &[PinnedMetadata],
        old_snapshot: Option<&MetadataSnapshot>,
    ) -> Option<MetadataDrift> {
        if pins.is_empty() {
            return None;
        }

        let current_hash = &current_snapshot.hash;

        if pins.iter().any(|p| &p.hash == current_hash) {
            return None;
        }

        // Use the first pin as the reference for the drift report.
        let first_pin = &pins[0];

        let affected_pallets = if let Some(old) = old_snapshot {
            Self::diff_pallets(old, current_snapshot)
        } else {
            Self::diff_pallets_from_current(current_snapshot)
        };

        Some(MetadataDrift {
            chain_id: chain_id.clone(),
            pinned_hash: first_pin.hash.clone(),
            current_hash: current_hash.clone(),
            detected_at: now(),
            affected_pallets,
        })
    }

    /// List pallet names affected by a metadata change between two snapshots.
    ///
    /// Both snapshots' raw bytes are SCALE-decoded and compared using
    /// [`diff_metadata`]. The result is a list of human-readable change
    /// descriptions such as `"added: NewPallet"`, `"removed: OldPallet"`, or
    /// `"modified: Balances (calls changed)"`.
    ///
    /// Falls back to a placeholder if either snapshot cannot be parsed.
    #[must_use]
    pub fn diff_pallets(
        old_snapshot: &MetadataSnapshot,
        new_snapshot: &MetadataSnapshot,
    ) -> Vec<String> {
        if old_snapshot.hash == new_snapshot.hash {
            return vec![];
        }

        let old_meta = match parse_metadata(&old_snapshot.raw_bytes) {
            Ok(m) => m,
            Err(e) => {
                debug!("failed to parse old metadata for diff: {e}");
                return vec!["(old metadata could not be parsed for pallet diff)".into()];
            }
        };

        let new_meta = match parse_metadata(&new_snapshot.raw_bytes) {
            Ok(m) => m,
            Err(e) => {
                debug!("failed to parse new metadata for diff: {e}");
                return vec!["(new metadata could not be parsed for pallet diff)".into()];
            }
        };

        let diff = diff_metadata(&old_meta, &new_meta);

        Self::format_diff_results(&diff)
    }

    /// Extract pallet names from a single (current) snapshot.
    ///
    /// Used when we detect drift but don't have the old snapshot's raw bytes
    /// (only its hash from a pin). Lists all pallets in the current metadata
    /// as potentially affected.
    fn diff_pallets_from_current(snapshot: &MetadataSnapshot) -> Vec<String> {
        match parse_metadata(&snapshot.raw_bytes) {
            Ok(meta) => {
                if meta.pallets.is_empty() {
                    return vec!["(runtime contains no pallets)".into()];
                }
                let pallet_names: Vec<&str> =
                    meta.pallets.iter().map(|p| p.name.as_str()).collect();
                vec![format!(
                    "(pinned metadata unavailable; current runtime has {} pallets: {})",
                    pallet_names.len(),
                    pallet_names.join(", ")
                )]
            }
            Err(e) => {
                debug!("failed to parse current metadata for diff: {e}");
                vec!["(metadata could not be parsed for pallet diff)".into()]
            }
        }
    }

    /// Format a [`MetadataDiff`] into a `Vec<String>` of human-readable
    /// change descriptions.
    fn format_diff_results(diff: &crate::diff::MetadataDiff) -> Vec<String> {
        if diff.is_empty() {
            return vec![];
        }

        let mut results = Vec::new();

        for name in &diff.added_pallets {
            results.push(format!("added: {name}"));
        }

        for name in &diff.removed_pallets {
            results.push(format!("removed: {name}"));
        }

        for pallet_diff in &diff.modified_pallets {
            let mut changes = Vec::new();

            if !pallet_diff.added_calls.is_empty() || !pallet_diff.removed_calls.is_empty() {
                changes.push("calls changed");
            }
            if !pallet_diff.changed_call_signatures.is_empty() {
                changes.push("call signatures changed");
            }
            if !pallet_diff.added_storage.is_empty() || !pallet_diff.removed_storage.is_empty() {
                changes.push("storage changed");
            }
            if !pallet_diff.added_constants.is_empty()
                || !pallet_diff.removed_constants.is_empty()
                || !pallet_diff.changed_constants.is_empty()
            {
                changes.push("constants changed");
            }

            if changes.is_empty() {
                results.push(format!("modified: {}", pallet_diff.name));
            } else {
                results.push(format!(
                    "modified: {} ({})",
                    pallet_diff.name,
                    changes.join(", ")
                ));
            }
        }

        results
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Drift tests intentionally panic when an expected drift fixture is absent so
// detection regressions remain easy to diagnose.
#[allow(
    clippy::expect_used,
    reason = "metadata drift assertions intentionally panic with focused diagnostics"
)]
mod tests {
    use super::*;
    use crate::types::{ChainId, MetadataHash, MetadataVersion};
    use polkagent_codec::metadata::build_minimal_metadata_v14;
    use polkagent_core::now;

    /// Create a snapshot with arbitrary (non-parseable) raw bytes.
    fn make_snapshot(chain: &str, data: &[u8]) -> MetadataSnapshot {
        MetadataSnapshot::new(
            ChainId::new(chain),
            MetadataVersion::V14,
            data.to_vec(),
            now(),
            1,
        )
    }

    /// Create a snapshot whose `raw_bytes` are valid minimal v14 metadata.
    fn make_valid_snapshot(chain: &str, pallets: &[(&str, u8)]) -> MetadataSnapshot {
        let raw = build_minimal_metadata_v14(pallets);
        MetadataSnapshot::new(ChainId::new(chain), MetadataVersion::V14, raw, now(), 1)
    }

    fn make_pin(chain: &str, data: &[u8], label: &str) -> PinnedMetadata {
        PinnedMetadata {
            chain_id: ChainId::new(chain),
            hash: MetadataHash::from_bytes(data),
            pinned_at: now(),
            label: label.into(),
            trusted: true,
        }
    }

    fn make_pin_from_snapshot(snap: &MetadataSnapshot, label: &str) -> PinnedMetadata {
        PinnedMetadata {
            chain_id: snap.chain_id.clone(),
            hash: snap.hash.clone(),
            pinned_at: now(),
            label: label.into(),
            trusted: true,
        }
    }

    #[test]
    fn no_drift_when_matching_pin() {
        let detector = DriftDetector::new();
        let snap = make_snapshot("polkadot", b"metadata_v1");
        let pins = vec![make_pin("polkadot", b"metadata_v1", "v1")];

        let result = detector.check(&ChainId::new("polkadot"), &snap, &pins);
        assert!(result.is_none());
    }

    #[test]
    fn no_drift_when_no_pins() {
        let detector = DriftDetector::new();
        let snap = make_snapshot("polkadot", b"anything");

        let result = detector.check(&ChainId::new("polkadot"), &snap, &[]);
        assert!(result.is_none());
    }

    #[test]
    fn drift_detected_when_hash_mismatch() {
        let detector = DriftDetector::new();
        let snap = make_snapshot("polkadot", b"new_metadata");
        let pins = vec![make_pin("polkadot", b"old_metadata", "old")];

        let result = detector.check(&ChainId::new("polkadot"), &snap, &pins);
        assert!(result.is_some());

        let drift = result.expect("drift should be present");
        assert_eq!(drift.chain_id, ChainId::new("polkadot"));
        assert_eq!(drift.pinned_hash, MetadataHash::from_bytes(b"old_metadata"));
        assert_eq!(
            drift.current_hash,
            MetadataHash::from_bytes(b"new_metadata")
        );
        assert!(!drift.affected_pallets.is_empty());
    }

    #[test]
    fn no_drift_when_matching_one_of_multiple_pins() {
        let detector = DriftDetector::new();
        let snap = make_snapshot("polkadot", b"v2_metadata");
        let pins = vec![
            make_pin("polkadot", b"v1_metadata", "v1"),
            make_pin("polkadot", b"v2_metadata", "v2"),
        ];

        let result = detector.check(&ChainId::new("polkadot"), &snap, &pins);
        assert!(result.is_none());
    }

    #[test]
    fn drift_when_no_pin_matches() {
        let detector = DriftDetector::new();
        let snap = make_snapshot("polkadot", b"v3_metadata");
        let pins = vec![
            make_pin("polkadot", b"v1_metadata", "v1"),
            make_pin("polkadot", b"v2_metadata", "v2"),
        ];

        let result = detector.check(&ChainId::new("polkadot"), &snap, &pins);
        assert!(result.is_some());
    }

    #[test]
    fn diff_pallets_same_hash() {
        let snap1 = make_valid_snapshot("polkadot", &[("System", 0), ("Balances", 5)]);
        let snap2 = make_valid_snapshot("polkadot", &[("System", 0), ("Balances", 5)]);
        // Same pallets => same raw bytes => same hash => empty diff.
        let diff = DriftDetector::diff_pallets(&snap1, &snap2);
        assert!(diff.is_empty());
    }

    #[test]
    fn diff_pallets_detects_added_pallet() {
        let old = make_valid_snapshot("polkadot", &[("System", 0)]);
        let new = make_valid_snapshot("polkadot", &[("System", 0), ("Balances", 5)]);
        let diff = DriftDetector::diff_pallets(&old, &new);
        assert!(!diff.is_empty(), "should detect added Balances pallet");
        let joined = diff.join(" ");
        assert!(
            joined.contains("added") && joined.contains("Balances"),
            "diff should mention added Balances, got: {joined}"
        );
    }

    #[test]
    fn diff_pallets_detects_removed_pallet() {
        let old = make_valid_snapshot("polkadot", &[("System", 0), ("Staking", 7)]);
        let new = make_valid_snapshot("polkadot", &[("System", 0)]);
        let diff = DriftDetector::diff_pallets(&old, &new);
        assert!(!diff.is_empty(), "should detect removed Staking pallet");
        let joined = diff.join(" ");
        assert!(
            joined.contains("removed") && joined.contains("Staking"),
            "diff should mention removed Staking, got: {joined}"
        );
    }

    #[test]
    fn diff_pallets_no_change_returns_empty() {
        let old = make_valid_snapshot("polkadot", &[("System", 0), ("Balances", 5)]);
        // Build identical metadata
        let new_raw = build_minimal_metadata_v14(&[("System", 0), ("Balances", 5)]);
        // Use different spec_version but same raw bytes to ensure hash match is
        // tested by content, not by construction.
        let new = MetadataSnapshot::new(
            ChainId::new("polkadot"),
            MetadataVersion::V14,
            new_raw,
            now(),
            2,
        );
        let diff = DriftDetector::diff_pallets(&old, &new);
        assert!(diff.is_empty(), "identical metadata should produce no diff");
    }

    #[test]
    fn diff_pallets_fallback_on_invalid_old_bytes() {
        let old = make_snapshot("polkadot", b"not valid SCALE metadata");
        let new = make_valid_snapshot("polkadot", &[("System", 0)]);
        let diff = DriftDetector::diff_pallets(&old, &new);
        assert!(!diff.is_empty());
        assert!(
            diff[0].contains("could not be parsed"),
            "should fall back gracefully, got: {}",
            diff[0]
        );
    }

    #[test]
    fn diff_pallets_fallback_on_invalid_new_bytes() {
        let old = make_valid_snapshot("polkadot", &[("System", 0)]);
        let new = make_snapshot("polkadot", b"not valid SCALE metadata");
        let diff = DriftDetector::diff_pallets(&old, &new);
        assert!(!diff.is_empty());
        assert!(
            diff[0].contains("could not be parsed"),
            "should fall back gracefully, got: {}",
            diff[0]
        );
    }

    #[test]
    fn check_with_old_snapshot_produces_real_diff() {
        let detector = DriftDetector::new();
        let old = make_valid_snapshot("polkadot", &[("System", 0)]);
        let new = make_valid_snapshot("polkadot", &[("System", 0), ("Balances", 5)]);
        let pins = vec![make_pin_from_snapshot(&old, "v1")];

        let drift = detector
            .check_with_old_snapshot(&ChainId::new("polkadot"), &new, &pins, Some(&old))
            .expect("drift expected");

        let joined = drift.affected_pallets.join(" ");
        assert!(
            joined.contains("added") && joined.contains("Balances"),
            "drift should show added Balances pallet, got: {joined}"
        );
    }

    #[test]
    fn check_without_old_snapshot_lists_current_pallets() {
        let detector = DriftDetector::new();
        let new = make_valid_snapshot("polkadot", &[("System", 0), ("Balances", 5)]);
        let pins = vec![make_pin("polkadot", b"some_other_hash", "old")];

        let drift = detector
            .check(&ChainId::new("polkadot"), &new, &pins)
            .expect("drift expected");

        let joined = drift.affected_pallets.join(" ");
        assert!(
            joined.contains("System") && joined.contains("Balances"),
            "should list current pallet names, got: {joined}"
        );
    }

    #[test]
    fn drift_contains_affected_pallets() {
        let detector = DriftDetector::new();
        let snap = make_snapshot("polkadot", b"current");
        let pins = vec![make_pin("polkadot", b"pinned", "trusted")];

        let drift = detector
            .check(&ChainId::new("polkadot"), &snap, &pins)
            .expect("drift expected");
        assert!(!drift.affected_pallets.is_empty());
    }

    #[test]
    fn drift_detected_at_is_recent() {
        let before = now();
        let detector = DriftDetector::new();
        let snap = make_snapshot("polkadot", b"current");
        let pins = vec![make_pin("polkadot", b"old", "old")];

        let drift = detector
            .check(&ChainId::new("polkadot"), &snap, &pins)
            .expect("drift expected");
        assert!(drift.detected_at >= before);
    }
}
