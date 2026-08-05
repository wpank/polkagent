//! Metadata service facade.
//!
//! [`MetadataService`] combines the metadata cache, pin store, and drift
//! detector into a single, ergonomic API for metadata management.

use std::time::Duration;

use tracing::{info, warn};

use crate::cache::MetadataCache;
use crate::drift::DriftDetector;
use crate::error::MetadataError;
use crate::pin::PinStore;
use crate::types::{ChainId, MetadataDrift, MetadataSnapshot, PinnedMetadata};

/// High-level metadata management service.
///
/// Composes cache, pin store, and drift detector to provide:
/// - Snapshot registration with automatic drift checking
/// - Metadata pinning for known-good hashes
/// - Staleness detection based on snapshot age
///
/// Thread-safe and cheaply cloneable (`Arc` internally).
#[derive(Debug, Clone)]
pub struct MetadataService {
    cache: MetadataCache,
    pins: PinStore,
    drift_detector: DriftDetector,
}

impl MetadataService {
    /// Create a new metadata service with default cache capacity (16 entries).
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache: MetadataCache::new(),
            pins: PinStore::new(),
            drift_detector: DriftDetector::new(),
        }
    }

    /// Create a new metadata service with a custom cache capacity.
    #[must_use]
    pub fn with_cache_capacity(max_entries: usize) -> Self {
        Self {
            cache: MetadataCache::with_capacity(max_entries),
            pins: PinStore::new(),
            drift_detector: DriftDetector::new(),
        }
    }

    /// Register a metadata snapshot.
    ///
    /// The snapshot is inserted into the cache and checked against any
    /// existing pins. If drift is detected, it is logged as a warning
    /// and returned.
    pub fn register_snapshot(&self, snapshot: MetadataSnapshot) -> Option<MetadataDrift> {
        let chain_id = snapshot.chain_id.clone();

        info!(
            chain = %chain_id,
            hash = %snapshot.hash,
            spec_version = snapshot.spec_version,
            "registering metadata snapshot"
        );

        // Check for drift before caching.
        let pins = self.pins.get_pinned(&chain_id);
        let drift = self.drift_detector.check(&chain_id, &snapshot, &pins);

        if let Some(ref d) = drift {
            warn!(
                chain = %d.chain_id,
                pinned = %d.pinned_hash,
                current = %d.current_hash,
                "metadata drift detected"
            );
        }

        self.cache.insert(snapshot);
        drift
    }

    /// Pin the currently cached metadata for a chain.
    ///
    /// Returns `Err(MetadataError::NothingToPin)` if no metadata is cached
    /// for the chain.
    pub fn pin_current(
        &self,
        chain_id: &ChainId,
        label: impl Into<String>,
    ) -> Result<(), MetadataError> {
        let snapshot =
            self.cache
                .get_latest(chain_id)
                .ok_or_else(|| MetadataError::NothingToPin {
                    chain_id: chain_id.clone(),
                })?;

        let label = label.into();
        info!(
            chain = %chain_id,
            hash = %snapshot.hash,
            label = %label,
            "pinning current metadata"
        );

        self.pins.pin(chain_id.clone(), snapshot.hash, label);
        Ok(())
    }

    /// Check for metadata drift on a chain.
    ///
    /// Compares the latest cached snapshot against pins. Returns `None` if
    /// no cached metadata exists or if no drift is detected.
    #[must_use]
    pub fn check_drift(&self, chain_id: &ChainId) -> Option<MetadataDrift> {
        let snapshot = self.cache.get_latest(chain_id)?;
        let pins = self.pins.get_pinned(chain_id);
        self.drift_detector.check(chain_id, &snapshot, &pins)
    }

    /// Get the latest cached metadata snapshot for a chain.
    #[must_use]
    pub fn get_snapshot(&self, chain_id: &ChainId) -> Option<MetadataSnapshot> {
        self.cache.get_latest(chain_id)
    }

    /// Get all pinned metadata records for a chain.
    #[must_use]
    pub fn get_pinned(&self, chain_id: &ChainId) -> Vec<PinnedMetadata> {
        self.pins.get_pinned(chain_id)
    }

    /// Check whether the cached metadata for a chain is older than `max_age`.
    ///
    /// Returns `true` if:
    /// - No metadata is cached for the chain, or
    /// - The cached metadata's `fetched_at` is older than `max_age` ago.
    #[must_use]
    pub fn is_stale(&self, chain_id: &ChainId, max_age: Duration) -> bool {
        let Some(snapshot) = self.cache.get_latest(chain_id) else {
            return true;
        };

        let age = chrono::Utc::now() - snapshot.fetched_at;
        let max_age_chrono = chrono::Duration::from_std(max_age).unwrap_or(chrono::TimeDelta::MAX);
        age > max_age_chrono
    }

    /// Get a reference to the underlying cache.
    #[must_use]
    pub fn cache(&self) -> &MetadataCache {
        &self.cache
    }

    /// Get a reference to the underlying pin store.
    #[must_use]
    pub fn pin_store(&self) -> &PinStore {
        &self.pins
    }
}

impl Default for MetadataService {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Service tests intentionally panic at pinning and drift contract boundaries so
// malformed metadata fixtures remain easy to diagnose.
#[allow(
    clippy::expect_used,
    reason = "metadata service assertions intentionally panic with focused diagnostics"
)]
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

    fn make_old_snapshot(chain: &str, data: &[u8], age: Duration) -> MetadataSnapshot {
        let ts = chrono::Utc::now()
            - chrono::Duration::from_std(age).unwrap_or(chrono::Duration::zero());
        MetadataSnapshot::new(
            ChainId::new(chain),
            MetadataVersion::V14,
            data.to_vec(),
            ts,
            1,
        )
    }

    #[test]
    fn register_and_retrieve_snapshot() {
        let svc = MetadataService::new();
        let snap = make_snapshot("polkadot", b"metadata_v1");
        let chain = ChainId::new("polkadot");

        let drift = svc.register_snapshot(snap.clone());
        assert!(drift.is_none()); // No pins, so no drift.

        let got = svc.get_snapshot(&chain);
        assert!(got.is_some());
        assert_eq!(got.as_ref().map(|s| &s.hash), Some(&snap.hash));
    }

    #[test]
    fn pin_current_succeeds() {
        let svc = MetadataService::new();
        let snap = make_snapshot("polkadot", b"metadata_v1");
        let chain = ChainId::new("polkadot");
        let hash = snap.hash.clone();

        svc.register_snapshot(snap);
        svc.pin_current(&chain, "v1.0").expect("pin should succeed");

        let pins = svc.get_pinned(&chain);
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].hash, hash);
        assert_eq!(pins[0].label, "v1.0");
    }

    #[test]
    fn pin_current_fails_when_no_cache() {
        let svc = MetadataService::new();
        let result = svc.pin_current(&ChainId::new("polkadot"), "label");
        assert!(result.is_err());
    }

    #[test]
    fn drift_detected_after_pin_and_new_snapshot() {
        let svc = MetadataService::new();
        let chain = ChainId::new("polkadot");

        // Register and pin v1.
        let snap_v1 = make_snapshot("polkadot", b"v1_metadata");
        svc.register_snapshot(snap_v1);
        svc.pin_current(&chain, "v1").expect("pin v1");

        // Register v2 — should detect drift.
        let snap_v2 = make_snapshot("polkadot", b"v2_metadata");
        let drift = svc.register_snapshot(snap_v2);
        assert!(drift.is_some());

        let d = drift.expect("drift should be present");
        assert_eq!(d.chain_id, chain);
        assert_eq!(d.pinned_hash, MetadataHash::from_bytes(b"v1_metadata"));
        assert_eq!(d.current_hash, MetadataHash::from_bytes(b"v2_metadata"));
    }

    #[test]
    fn no_drift_when_snapshot_matches_pin() {
        let svc = MetadataService::new();
        let chain = ChainId::new("polkadot");

        let snap = make_snapshot("polkadot", b"same_metadata");
        svc.register_snapshot(snap.clone());
        svc.pin_current(&chain, "pinned").expect("pin");

        // Register the same metadata again — no drift.
        let snap2 = make_snapshot("polkadot", b"same_metadata");
        let drift = svc.register_snapshot(snap2);
        assert!(drift.is_none());
    }

    #[test]
    fn check_drift_returns_none_when_no_cache() {
        let svc = MetadataService::new();
        let result = svc.check_drift(&ChainId::new("polkadot"));
        assert!(result.is_none());
    }

    #[test]
    fn check_drift_returns_none_when_no_pins() {
        let svc = MetadataService::new();
        let snap = make_snapshot("polkadot", b"data");
        svc.register_snapshot(snap);

        let result = svc.check_drift(&ChainId::new("polkadot"));
        assert!(result.is_none());
    }

    #[test]
    fn check_drift_returns_drift_when_mismatch() {
        let svc = MetadataService::new();
        let chain = ChainId::new("polkadot");

        let snap_v1 = make_snapshot("polkadot", b"v1");
        svc.register_snapshot(snap_v1);
        svc.pin_current(&chain, "v1").expect("pin");

        let snap_v2 = make_snapshot("polkadot", b"v2");
        svc.register_snapshot(snap_v2);

        let drift = svc.check_drift(&chain);
        assert!(drift.is_some());
    }

    #[test]
    fn is_stale_true_when_no_cache() {
        let svc = MetadataService::new();
        assert!(svc.is_stale(&ChainId::new("polkadot"), Duration::from_secs(60)));
    }

    #[test]
    fn is_stale_false_for_fresh_snapshot() {
        let svc = MetadataService::new();
        let snap = make_snapshot("polkadot", b"fresh");
        svc.register_snapshot(snap);

        assert!(!svc.is_stale(&ChainId::new("polkadot"), Duration::from_secs(60)));
    }

    #[test]
    fn is_stale_true_for_old_snapshot() {
        let svc = MetadataService::new();
        let snap = make_old_snapshot("polkadot", b"old", Duration::from_secs(120));
        svc.register_snapshot(snap);

        assert!(svc.is_stale(&ChainId::new("polkadot"), Duration::from_secs(60)));
    }

    #[test]
    fn get_pinned_returns_empty_for_unknown_chain() {
        let svc = MetadataService::new();
        let pins = svc.get_pinned(&ChainId::new("unknown"));
        assert!(pins.is_empty());
    }

    #[test]
    fn full_workflow_register_pin_drift_stale() {
        let svc = MetadataService::new();
        let chain = ChainId::new("polkadot");

        // 1. Register initial metadata.
        let snap = make_snapshot("polkadot", b"initial");
        assert!(svc.register_snapshot(snap).is_none());

        // 2. Pin it.
        svc.pin_current(&chain, "initial").expect("pin");
        assert_eq!(svc.get_pinned(&chain).len(), 1);

        // 3. Check drift — should be none.
        assert!(svc.check_drift(&chain).is_none());

        // 4. Metadata is fresh.
        assert!(!svc.is_stale(&chain, Duration::from_secs(300)));

        // 5. Register new metadata — drift detected.
        let snap2 = make_snapshot("polkadot", b"upgraded");
        let drift = svc.register_snapshot(snap2);
        assert!(drift.is_some());

        // 6. Pin the new version.
        svc.pin_current(&chain, "upgraded").expect("pin v2");
        assert_eq!(svc.get_pinned(&chain).len(), 2);

        // 7. Drift check should pass now (current matches v2 pin).
        assert!(svc.check_drift(&chain).is_none());
    }

    #[test]
    fn custom_cache_capacity() {
        let svc = MetadataService::with_cache_capacity(4);
        assert_eq!(svc.cache().capacity(), 4);
    }

    #[test]
    fn multiple_chains_independent() {
        let svc = MetadataService::new();
        let polkadot = ChainId::new("polkadot");
        let kusama = ChainId::new("kusama");

        svc.register_snapshot(make_snapshot("polkadot", b"pdot"));
        svc.register_snapshot(make_snapshot("kusama", b"ksm"));

        svc.pin_current(&polkadot, "pdot-pin").expect("pin pdot");

        // Kusama has no pins, so no drift possible.
        assert!(svc.check_drift(&kusama).is_none());

        // Polkadot matches its pin.
        assert!(svc.check_drift(&polkadot).is_none());

        // Register new polkadot metadata — drift.
        svc.register_snapshot(make_snapshot("polkadot", b"pdot_v2"));
        assert!(svc.check_drift(&polkadot).is_some());

        // Kusama is still fine.
        assert!(svc.check_drift(&kusama).is_none());
    }
}
