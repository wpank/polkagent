//! Metadata drift detection.
//!
//! [`DriftDetector`] compares the current metadata snapshot for a chain
//! against a set of pinned (trusted) hashes to detect runtime upgrades
//! or unexpected metadata changes.

use polkagent_core::now;

use crate::types::{ChainId, MetadataDrift, MetadataHash, MetadataSnapshot, PinnedMetadata};

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
    #[must_use]
    pub fn check(
        &self,
        chain_id: &ChainId,
        current_snapshot: &MetadataSnapshot,
        pins: &[PinnedMetadata],
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
        Some(MetadataDrift {
            chain_id: chain_id.clone(),
            pinned_hash: first_pin.hash.clone(),
            current_hash: current_hash.clone(),
            detected_at: now(),
            affected_pallets: Self::diff_pallets_by_hash(&first_pin.hash, current_hash),
        })
    }

    /// List pallet names affected by a metadata change.
    ///
    /// This is a stub implementation that compares hashes. Accurate
    /// pallet-level diffing requires SCALE decoding of the metadata,
    /// which is future work.
    ///
    /// Currently: if the hashes differ, returns a placeholder message.
    /// If they are the same, returns an empty list.
    #[must_use]
    pub fn diff_pallets(
        old_snapshot: &MetadataSnapshot,
        new_snapshot: &MetadataSnapshot,
    ) -> Vec<String> {
        Self::diff_pallets_by_hash(&old_snapshot.hash, &new_snapshot.hash)
    }

    /// Internal helper for pallet diff by hash comparison.
    fn diff_pallets_by_hash(old_hash: &MetadataHash, new_hash: &MetadataHash) -> Vec<String> {
        if old_hash == new_hash {
            vec![]
        } else {
            vec!["(full pallet diff requires SCALE decoding)".into()]
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChainId, MetadataHash, MetadataVersion};
    use polkagent_core::now;

    fn make_snapshot(chain: &str, data: &[u8]) -> MetadataSnapshot {
        MetadataSnapshot::new(
            ChainId::new(chain),
            MetadataVersion::V14,
            data.to_vec(),
            now(),
            1,
        )
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
        let snap1 = make_snapshot("polkadot", b"same");
        let snap2 = make_snapshot("polkadot", b"same");
        let diff = DriftDetector::diff_pallets(&snap1, &snap2);
        assert!(diff.is_empty());
    }

    #[test]
    fn diff_pallets_different_hash() {
        let snap1 = make_snapshot("polkadot", b"old");
        let snap2 = make_snapshot("polkadot", b"new");
        let diff = DriftDetector::diff_pallets(&snap1, &snap2);
        assert!(!diff.is_empty());
        assert!(diff[0].contains("SCALE decoding"));
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
