//! Cross-crate integration tests for the metadata service.
//!
//! These tests exercise the `polkagent-metadata` crate's cache, pin store,
//! drift detection, staleness checking, and cache eviction from outside
//! the crate boundary.

// This assertion-oriented integration target uses `expect`/`unwrap` to identify
// the exact cross-crate fixture step or behavioral contract that failed.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::Duration;

use polkagent_core::now;
use polkagent_metadata::{
    ChainId, DriftDetector, MetadataCache, MetadataHash, MetadataService, MetadataSnapshot,
    MetadataVersion, PinStore, PinnedMetadata,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn duration_minutes(minutes: u64) -> Duration {
    Duration::from_secs(minutes * 60)
}

fn duration_hours(hours: u64) -> Duration {
    duration_minutes(hours * 60)
}

fn make_snapshot(chain: &str, data: &[u8]) -> MetadataSnapshot {
    MetadataSnapshot::new(
        ChainId::new(chain),
        MetadataVersion::V14,
        data.to_vec(),
        now(),
        1_000_000,
    )
}

fn make_old_snapshot(chain: &str, data: &[u8], age: Duration) -> MetadataSnapshot {
    let ts =
        chrono::Utc::now() - chrono::Duration::from_std(age).unwrap_or(chrono::Duration::zero());
    MetadataSnapshot::new(
        ChainId::new(chain),
        MetadataVersion::V14,
        data.to_vec(),
        ts,
        1_000_000,
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

// ---------------------------------------------------------------------------
// Cache stores and retrieves snapshots
// ---------------------------------------------------------------------------

#[test]
fn cache_stores_and_retrieves_snapshot() {
    let cache = MetadataCache::new();
    let snap = make_snapshot("polkadot", b"runtime_metadata_v42");
    let hash = snap.hash.clone();
    let chain_id = snap.chain_id.clone();

    cache.insert(snap);

    let retrieved = cache.get(&chain_id, &hash);
    assert!(
        retrieved.is_some(),
        "snapshot should be retrievable by chain_id and hash"
    );

    let got = retrieved.expect("already checked Some");
    assert_eq!(got.hash, hash);
    assert_eq!(got.chain_id, chain_id);
    assert_eq!(got.raw_bytes, b"runtime_metadata_v42");
}

#[test]
fn cache_get_latest_returns_most_recent_for_chain() {
    let cache = MetadataCache::new();
    let chain = ChainId::new("polkadot");

    let snap_old = make_snapshot("polkadot", b"old_metadata");
    let snap_new = make_snapshot("polkadot", b"new_metadata");
    let expected_hash = snap_new.hash.clone();

    cache.insert(snap_old);
    cache.insert(snap_new);

    let latest = cache.get_latest(&chain);
    assert!(latest.is_some());
    assert_eq!(latest.expect("checked").hash, expected_hash);
}

#[test]
fn cache_returns_none_for_unknown_chain() {
    let cache = MetadataCache::new();
    assert!(cache.get_latest(&ChainId::new("unknown")).is_none());
    assert!(cache
        .get(&ChainId::new("unknown"), &MetadataHash::from_hex("abc"))
        .is_none());
}

// ---------------------------------------------------------------------------
// Pinning marks metadata as known-good
// ---------------------------------------------------------------------------

#[test]
fn pin_store_marks_metadata_as_known_good() {
    let pins = PinStore::new();
    let chain = ChainId::new("polkadot");
    let hash = MetadataHash::from_bytes(b"trusted_metadata");

    assert!(!pins.is_pinned(&chain, &hash));

    pins.pin(chain.clone(), hash.clone(), "v1.0.0-release");

    assert!(pins.is_pinned(&chain, &hash));

    let pinned = pins.get_pinned(&chain);
    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0].label, "v1.0.0-release");
    assert!(pinned[0].trusted);
}

#[test]
fn pin_store_pin_is_idempotent() {
    let pins = PinStore::new();
    let chain = ChainId::new("polkadot");
    let hash = MetadataHash::from_bytes(b"data");

    pins.pin(chain.clone(), hash.clone(), "first");
    pins.pin(chain.clone(), hash.clone(), "second");

    assert_eq!(
        pins.total_pins(),
        1,
        "duplicate pin should not add a second entry"
    );
}

#[test]
fn pin_store_unpin_removes_pin() {
    let pins = PinStore::new();
    let chain = ChainId::new("kusama");
    let hash = MetadataHash::from_bytes(b"ksm_metadata");

    pins.pin(chain.clone(), hash.clone(), "ksm-v1");
    assert!(pins.is_pinned(&chain, &hash));

    pins.unpin(&chain, &hash).expect("unpin should succeed");
    assert!(!pins.is_pinned(&chain, &hash));
}

#[test]
fn pin_store_verify_against_pin_ok_when_matching() {
    let pins = PinStore::new();
    let chain = ChainId::new("polkadot");
    let hash = MetadataHash::from_bytes(b"matched_data");

    pins.pin(chain.clone(), hash.clone(), "matching-pin");

    let result = pins.verify_against_pin(&chain, &hash);
    assert!(result.is_ok(), "matching pin should verify successfully");
}

#[test]
fn pin_store_verify_against_pin_returns_drift_on_mismatch() {
    let pins = PinStore::new();
    let chain = ChainId::new("polkadot");
    let pinned_hash = MetadataHash::from_bytes(b"pinned_version");
    let current_hash = MetadataHash::from_bytes(b"different_version");

    pins.pin(chain.clone(), pinned_hash.clone(), "old-pin");

    let result = pins.verify_against_pin(&chain, &current_hash);
    assert!(result.is_err(), "mismatched hash should produce drift");

    let drift = result.unwrap_err();
    assert_eq!(drift.chain_id, chain);
    assert_eq!(drift.pinned_hash, pinned_hash);
    assert_eq!(drift.current_hash, current_hash);
}

// ---------------------------------------------------------------------------
// Drift detection identifies changed metadata
// ---------------------------------------------------------------------------

#[test]
fn drift_detector_no_drift_when_hash_matches_pin() {
    let detector = DriftDetector::new();
    let snap = make_snapshot("polkadot", b"same_data");
    let pins = vec![make_pin("polkadot", b"same_data", "pinned")];

    let drift = detector.check(&ChainId::new("polkadot"), &snap, &pins);
    assert!(drift.is_none(), "matching hash should produce no drift");
}

#[test]
fn drift_detector_detects_drift_when_hash_differs() {
    let detector = DriftDetector::new();
    let snap = make_snapshot("polkadot", b"current_runtime");
    let pins = vec![make_pin("polkadot", b"old_runtime", "old-version")];

    let drift = detector.check(&ChainId::new("polkadot"), &snap, &pins);
    assert!(drift.is_some(), "different hash should produce drift");

    let d = drift.expect("checked");
    assert_eq!(d.chain_id, ChainId::new("polkadot"));
    assert_eq!(d.pinned_hash, MetadataHash::from_bytes(b"old_runtime"));
    assert_eq!(d.current_hash, MetadataHash::from_bytes(b"current_runtime"));
    assert!(!d.affected_pallets.is_empty());
}

#[test]
fn drift_detector_no_drift_when_no_pins_exist() {
    let detector = DriftDetector::new();
    let snap = make_snapshot("polkadot", b"any_data");

    let drift = detector.check(&ChainId::new("polkadot"), &snap, &[]);
    assert!(drift.is_none(), "no pins means nothing to drift against");
}

#[test]
fn drift_detector_matches_any_of_multiple_pins() {
    let detector = DriftDetector::new();
    let snap = make_snapshot("polkadot", b"v2_metadata");
    let pins = vec![
        make_pin("polkadot", b"v1_metadata", "v1"),
        make_pin("polkadot", b"v2_metadata", "v2"),
    ];

    let drift = detector.check(&ChainId::new("polkadot"), &snap, &pins);
    assert!(drift.is_none(), "matching any pin should suppress drift");
}

// ---------------------------------------------------------------------------
// Stale metadata is detected by age
// ---------------------------------------------------------------------------

#[test]
fn stale_metadata_detected_when_old() {
    let svc = MetadataService::new();
    let chain = ChainId::new("polkadot");

    // Register a snapshot that is 2 hours old.
    let old_snap = make_old_snapshot("polkadot", b"old_data", duration_hours(2));
    svc.register_snapshot(old_snap);

    // Check with a 1-hour max age -> should be stale.
    assert!(
        svc.is_stale(&chain, duration_hours(1)),
        "2-hour-old metadata should be stale with 1-hour threshold"
    );
}

#[test]
fn fresh_metadata_not_detected_as_stale() {
    let svc = MetadataService::new();
    let chain = ChainId::new("polkadot");

    // Register a fresh snapshot.
    let snap = make_snapshot("polkadot", b"fresh_data");
    svc.register_snapshot(snap);

    // Check with a 1-hour max age -> should not be stale.
    assert!(
        !svc.is_stale(&chain, duration_hours(1)),
        "just-registered metadata should not be stale"
    );
}

#[test]
fn no_cached_metadata_counts_as_stale() {
    let svc = MetadataService::new();
    assert!(
        svc.is_stale(&ChainId::new("polkadot"), duration_minutes(1)),
        "no cached metadata should be considered stale"
    );
}

// ---------------------------------------------------------------------------
// Cache eviction works when full
// ---------------------------------------------------------------------------

#[test]
fn cache_eviction_removes_lru_when_full() {
    let cache = MetadataCache::with_capacity(3);

    let snap1 = make_snapshot("chain-a", b"snap1");
    let snap2 = make_snapshot("chain-b", b"snap2");
    let snap3 = make_snapshot("chain-c", b"snap3");

    let hash1 = snap1.hash.clone();
    let chain1 = snap1.chain_id.clone();

    cache.insert(snap1);
    cache.insert(snap2);
    cache.insert(snap3);
    assert_eq!(cache.len(), 3);

    // Insert a 4th entry; snap1 (LRU) should be evicted.
    let snap4 = make_snapshot("chain-d", b"snap4");
    cache.insert(snap4);
    assert_eq!(cache.len(), 3, "cache should not exceed capacity");

    // snap1 should be gone.
    assert!(
        cache.get(&chain1, &hash1).is_none(),
        "LRU entry should have been evicted"
    );
}

#[test]
fn cache_access_promotes_entry_preventing_eviction() {
    let cache = MetadataCache::with_capacity(3);

    let snap1 = make_snapshot("chain-a", b"snap1");
    let snap2 = make_snapshot("chain-b", b"snap2");
    let snap3 = make_snapshot("chain-c", b"snap3");

    let hash1 = snap1.hash.clone();
    let chain1 = snap1.chain_id.clone();
    let hash2 = snap2.hash.clone();
    let chain2 = snap2.chain_id.clone();

    cache.insert(snap1);
    cache.insert(snap2);
    cache.insert(snap3);

    // Access snap1 to promote it to MRU.
    let _ = cache.get(&chain1, &hash1);

    // Insert snap4; snap2 should be evicted (it is now LRU).
    let snap4 = make_snapshot("chain-d", b"snap4");
    cache.insert(snap4);

    // snap1 should still exist (promoted by get).
    assert!(
        cache.get(&chain1, &hash1).is_some(),
        "accessed entry should survive eviction"
    );

    // snap2 should be evicted.
    assert!(
        cache.get(&chain2, &hash2).is_none(),
        "LRU entry (snap2) should have been evicted"
    );
}

#[test]
fn cache_explicit_evict_removes_entry() {
    let cache = MetadataCache::new();
    let snap = make_snapshot("polkadot", b"to_evict");
    let hash = snap.hash.clone();
    let chain_id = snap.chain_id.clone();

    cache.insert(snap);
    assert_eq!(cache.len(), 1);

    let removed = cache.evict(&chain_id, &hash);
    assert!(removed, "evict should return true for existing entry");
    assert_eq!(cache.len(), 0);
    assert!(cache.get(&chain_id, &hash).is_none());
}

#[test]
fn cache_evict_returns_false_for_missing_entry() {
    let cache = MetadataCache::new();
    let removed = cache.evict(&ChainId::new("nonexistent"), &MetadataHash::from_hex("abc"));
    assert!(!removed, "evict should return false for non-existent entry");
}

// ---------------------------------------------------------------------------
// Full MetadataService workflow
// ---------------------------------------------------------------------------

#[test]
fn metadata_service_full_workflow() {
    let svc = MetadataService::new();
    let chain = ChainId::new("polkadot");

    // 1. Register initial metadata -- no drift (no pins yet).
    let snap_v1 = make_snapshot("polkadot", b"v1_runtime");
    let drift = svc.register_snapshot(snap_v1);
    assert!(drift.is_none(), "no pins yet, so no drift expected");

    // 2. Pin the current metadata.
    svc.pin_current(&chain, "v1-release")
        .expect("pin should succeed");
    assert_eq!(svc.get_pinned(&chain).len(), 1);

    // 3. Check drift -- should be none (current matches pin).
    assert!(svc.check_drift(&chain).is_none());

    // 4. Register new metadata -- drift detected.
    let snap_v2 = make_snapshot("polkadot", b"v2_runtime");
    let drift = svc.register_snapshot(snap_v2);
    assert!(drift.is_some(), "new metadata should trigger drift");

    let d = drift.expect("checked");
    assert_eq!(d.pinned_hash, MetadataHash::from_bytes(b"v1_runtime"));
    assert_eq!(d.current_hash, MetadataHash::from_bytes(b"v2_runtime"));

    // 5. Pin the new version.
    svc.pin_current(&chain, "v2-release")
        .expect("pin v2 should succeed");
    assert_eq!(svc.get_pinned(&chain).len(), 2);

    // 6. Check drift -- should be clear now.
    assert!(svc.check_drift(&chain).is_none());

    // 7. Metadata is still fresh.
    assert!(!svc.is_stale(&chain, duration_hours(1)));
}

#[test]
fn metadata_service_pin_current_fails_without_cache() {
    let svc = MetadataService::new();
    let result = svc.pin_current(&ChainId::new("polkadot"), "label");
    assert!(
        result.is_err(),
        "pinning without cached metadata should fail"
    );
}

#[test]
fn metadata_service_multiple_chains_are_independent() {
    let svc = MetadataService::new();
    let polkadot = ChainId::new("polkadot");
    let kusama = ChainId::new("kusama");

    svc.register_snapshot(make_snapshot("polkadot", b"pdot_meta"));
    svc.register_snapshot(make_snapshot("kusama", b"ksm_meta"));

    svc.pin_current(&polkadot, "pdot-pin").expect("pin pdot");

    // Kusama has no pins, so no drift.
    assert!(svc.check_drift(&kusama).is_none());

    // Polkadot matches its pin.
    assert!(svc.check_drift(&polkadot).is_none());

    // Register new polkadot metadata -- drift on polkadot only.
    svc.register_snapshot(make_snapshot("polkadot", b"pdot_meta_v2"));
    assert!(svc.check_drift(&polkadot).is_some());
    assert!(svc.check_drift(&kusama).is_none());
}
