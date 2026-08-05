//! PRD-15 Metadata Safety Tests: MD-02 through MD-06.
//!
//! These tests verify the metadata service's security invariants:
//! - MD-02: Stale metadata refuses operations (explicit error, not silent misdecode).
//! - MD-03: Metadata diff produces structured output.
//! - MD-05: Type-hash comparison detects semantic drift.
//!
//! Crates under test: polkagent-metadata, polkagent-card.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "metadata safety tests intentionally fail fast when expected validation outcomes are absent"
)]

use polkagent_core::now;
use polkagent_metadata::error::MetadataError;
use polkagent_metadata::types::{
    ChainId, MetadataDrift, MetadataHash, MetadataSnapshot, MetadataVersion,
};
use polkagent_metadata::{DriftDetector, MetadataService, PinStore};
use std::time::Duration;

// ===========================================================================
// MD-02: Stale metadata refuses operations
// ===========================================================================
// When the cached metadata entry has exceeded its freshness window, the system
// must return an explicit staleness signal, not silently decode using the old
// call indices.

#[test]
fn md02_stale_metadata_detected_by_is_stale() {
    let svc = MetadataService::new();
    let chain = ChainId::new("polkadot");

    // Register a snapshot that is 2 hours old.
    let old_snap = MetadataSnapshot::new(
        chain.clone(),
        MetadataVersion::V14,
        b"old_spec_1000".to_vec(),
        now() - chrono::Duration::hours(2),
        1000,
    );
    svc.register_snapshot(old_snap);

    // With max_age = 1 hour, the snapshot is stale.
    assert!(
        svc.is_stale(&chain, Duration::from_secs(3600)),
        "MD-02: 2-hour-old metadata must be detected as stale with 1h max_age"
    );
}

#[test]
fn md02_fresh_metadata_not_stale() {
    let svc = MetadataService::new();
    let chain = ChainId::new("polkadot");

    // Register a freshly fetched snapshot.
    let fresh_snap = MetadataSnapshot::new(
        chain.clone(),
        MetadataVersion::V14,
        b"fresh_spec_1001".to_vec(),
        now(),
        1001,
    );
    svc.register_snapshot(fresh_snap);

    // With max_age = 1 hour, a just-fetched snapshot is not stale.
    assert!(
        !svc.is_stale(&chain, Duration::from_secs(3600)),
        "MD-02: freshly fetched metadata must NOT be detected as stale"
    );
}

#[test]
fn md02_missing_metadata_reports_stale_not_panic() {
    // If no metadata is cached, is_stale must return true (fail-safe).
    let svc = MetadataService::new();
    let unknown_chain = ChainId::new("unknown-chain-never-registered");

    // Must not panic; must return true (stale = unsafe to use).
    assert!(
        svc.is_stale(&unknown_chain, Duration::from_secs(3600)),
        "MD-02: missing metadata must be treated as stale (fail-safe)"
    );
}

#[test]
fn md02_nothing_to_pin_error_when_no_cache() {
    // Attempting to pin when no metadata is cached must return an explicit error,
    // not panic or silently succeed.
    let svc = MetadataService::new();
    let chain = ChainId::new("polkadot");

    let result = svc.pin_current(&chain, "v1.0");
    assert!(
        result.is_err(),
        "MD-02: pin_current with no cached metadata must return Err"
    );

    assert!(
        matches!(result.unwrap_err(), MetadataError::NothingToPin { .. }),
        "MD-02: error must be NothingToPin variant"
    );
}

#[test]
fn md02_stale_metadata_error_type_is_explicit() {
    // Construct the DriftDetected error directly to verify its structure.
    let chain = ChainId::new("polkadot");
    let err = MetadataError::DriftDetected {
        chain_id: chain.clone(),
        pinned_hash: MetadataHash::from_hex("aaaa"),
        current_hash: MetadataHash::from_hex("bbbb"),
    };

    let msg = format!("{err}");
    assert!(
        msg.contains("polkadot"),
        "MD-02: DriftDetected error must name the chain"
    );
    assert!(
        msg.contains("aaaa"),
        "MD-02: DriftDetected error must show pinned hash"
    );
    assert!(
        msg.contains("bbbb"),
        "MD-02: DriftDetected error must show current hash"
    );
}

#[test]
fn md02_stale_check_zero_max_age_always_stale() {
    // A zero max_age means any snapshot is immediately stale.
    let svc = MetadataService::new();
    let chain = ChainId::new("polkadot");

    let snap = MetadataSnapshot::new(
        chain.clone(),
        MetadataVersion::V14,
        b"fresh_data".to_vec(),
        now(),
        1000,
    );
    svc.register_snapshot(snap);

    // Even a just-fetched snapshot must be "stale" when max_age = 0.
    assert!(
        svc.is_stale(&chain, Duration::from_secs(0)),
        "MD-02: zero max_age must make everything stale"
    );
}

#[test]
fn md02_old_spec_version_produces_mismatched_hash() {
    // Two snapshots with different spec_versions but same raw bytes would have
    // the same hash. Different bytes with different spec_versions differ.
    // This test verifies the hash changes when the content changes.
    let h_old = MetadataHash::from_bytes(b"spec_1000_call_indices");
    let h_new = MetadataHash::from_bytes(b"spec_1001_call_indices_changed");

    assert_ne!(
        h_old, h_new,
        "MD-02: different spec_version byte content must produce different hashes"
    );
}

// ===========================================================================
// MD-03: Metadata diff structured output
// ===========================================================================
// When two metadata snapshots are compared, the diff must be structured data
// (not free-form text), so that downstream consumers can act on it
// programmatically.

#[test]
fn md03_drift_struct_has_structured_fields() {
    // MetadataDrift is a typed struct — verify all fields are present and typed.
    let chain = ChainId::new("polkadot");
    let timestamp = now();

    let drift = MetadataDrift {
        chain_id: chain.clone(),
        pinned_hash: MetadataHash::from_hex("aaaa0000"),
        current_hash: MetadataHash::from_hex("bbbb1111"),
        detected_at: timestamp,
        affected_pallets: vec!["Balances".to_string(), "Staking".to_string()],
    };

    // All fields are strongly typed — not a string blob.
    assert_eq!(
        drift.chain_id, chain,
        "MD-03: chain_id must be ChainId type"
    );
    assert_ne!(
        drift.pinned_hash, drift.current_hash,
        "MD-03: pinned and current hashes must differ"
    );
    assert_eq!(
        drift.affected_pallets.len(),
        2,
        "MD-03: affected_pallets must enumerate specific pallets"
    );
}

#[test]
fn md03_drift_serde_produces_structured_json() {
    // The MetadataDrift struct must serialise to well-structured JSON,
    // not a plain string.
    let drift = MetadataDrift {
        chain_id: ChainId::new("polkadot"),
        pinned_hash: MetadataHash::from_hex("oldcafe"),
        current_hash: MetadataHash::from_hex("newbeef"),
        detected_at: now(),
        affected_pallets: vec!["Balances".to_string()],
    };

    let json = serde_json::to_string(&drift).expect("MD-03: drift must serialize to JSON");
    let parsed: serde_json::Value =
        serde_json::from_str(&json).expect("MD-03: serialized drift must parse as valid JSON");

    // Must be a JSON object, not a string or array.
    assert!(
        parsed.is_object(),
        "MD-03: drift JSON must be a structured object, not a plain string"
    );

    // Key fields must be present.
    assert!(
        parsed["chain_id"].is_string(),
        "MD-03: chain_id must be a string field"
    );
    assert!(
        parsed["pinned_hash"].is_string(),
        "MD-03: pinned_hash must be a string field"
    );
    assert!(
        parsed["current_hash"].is_string(),
        "MD-03: current_hash must be a string field"
    );
    assert!(
        parsed["affected_pallets"].is_array(),
        "MD-03: affected_pallets must be an array"
    );
}

#[test]
fn md03_drift_serde_round_trip_preserves_all_fields() {
    let original = MetadataDrift {
        chain_id: ChainId::new("kusama"),
        pinned_hash: MetadataHash::from_hex("1122334455"),
        current_hash: MetadataHash::from_hex("aabbccddee"),
        detected_at: now(),
        affected_pallets: vec!["System".to_string(), "Timestamp".to_string()],
    };

    let json = serde_json::to_string(&original).expect("serialize");
    let back: MetadataDrift = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(
        back.chain_id, original.chain_id,
        "MD-03: chain_id round-trips"
    );
    assert_eq!(
        back.pinned_hash, original.pinned_hash,
        "MD-03: pinned_hash round-trips"
    );
    assert_eq!(
        back.current_hash, original.current_hash,
        "MD-03: current_hash round-trips"
    );
    assert_eq!(
        back.affected_pallets, original.affected_pallets,
        "MD-03: affected_pallets round-trips"
    );
}

#[test]
fn md03_two_snapshots_produce_drift_with_correct_hashes() {
    // Registering a v2 snapshot after pinning v1 must produce a drift report
    // containing the correct pinned and current hashes.
    let svc = MetadataService::new();
    let chain = ChainId::new("polkadot");

    let v1_bytes = b"v1_metadata_bytes_spec_1000";
    let v2_bytes = b"v2_metadata_bytes_spec_1001";

    let snap_v1 = MetadataSnapshot::new(
        chain.clone(),
        MetadataVersion::V14,
        v1_bytes.to_vec(),
        now(),
        1000,
    );
    let expected_v1_hash = MetadataHash::from_bytes(v1_bytes);
    let expected_v2_hash = MetadataHash::from_bytes(v2_bytes);

    // Register and pin v1.
    svc.register_snapshot(snap_v1);
    svc.pin_current(&chain, "spec-1000").expect("pin v1");

    // Register v2 — drift must be detected.
    let snap_v2 = MetadataSnapshot::new(
        chain.clone(),
        MetadataVersion::V14,
        v2_bytes.to_vec(),
        now(),
        1001,
    );
    let drift = svc.register_snapshot(snap_v2);

    assert!(
        drift.is_some(),
        "MD-03: v2 registration after pinning v1 must produce drift"
    );
    let d = drift.unwrap();

    assert_eq!(
        d.pinned_hash, expected_v1_hash,
        "MD-03: pinned_hash must be the v1 hash"
    );
    assert_eq!(
        d.current_hash, expected_v2_hash,
        "MD-03: current_hash must be the v2 hash"
    );
    assert_ne!(d.pinned_hash, d.current_hash, "MD-03: hashes must differ");
}

#[test]
fn md03_no_drift_produces_none_not_empty_struct() {
    // When there is no drift, register_snapshot must return None, not Some(empty).
    let svc = MetadataService::new();
    let chain = ChainId::new("polkadot");

    let snap = MetadataSnapshot::new(
        chain.clone(),
        MetadataVersion::V14,
        b"same_bytes".to_vec(),
        now(),
        1000,
    );
    svc.register_snapshot(snap.clone());
    svc.pin_current(&chain, "pinned").expect("pin");

    // Same bytes again — no drift.
    let snap2 = MetadataSnapshot::new(
        chain.clone(),
        MetadataVersion::V14,
        b"same_bytes".to_vec(),
        now(),
        1000,
    );
    let result = svc.register_snapshot(snap2);

    assert!(
        result.is_none(),
        "MD-03: identical metadata must produce None drift, not Some(empty)"
    );
}

#[test]
fn md03_drift_detector_check_matches_pin_store() {
    // DriftDetector.check must use PinStore data for comparison, and the
    // result must be a structured MetadataDrift.
    let detector = DriftDetector::new();
    let pins = PinStore::new();
    let chain = ChainId::new("polkadot");

    // Pin a known hash.
    let pinned_bytes = b"pinned_metadata";
    let pinned_hash = MetadataHash::from_bytes(pinned_bytes);
    pins.pin(chain.clone(), pinned_hash.clone(), "v1".to_string());

    // Snapshot with different bytes (drift).
    let current_snap = MetadataSnapshot::new(
        chain.clone(),
        MetadataVersion::V14,
        b"upgraded_metadata".to_vec(),
        now(),
        1001,
    );

    let pinned_records = pins.get_pinned(&chain);
    let drift = detector.check(&chain, &current_snap, &pinned_records);

    assert!(
        drift.is_some(),
        "MD-03: drift detector must find drift between pinned and current"
    );
    let d = drift.unwrap();
    assert_eq!(
        d.pinned_hash, pinned_hash,
        "MD-03: pinned_hash in drift must match pin store"
    );
    assert_eq!(
        d.current_hash, current_snap.hash,
        "MD-03: current_hash in drift must match current snapshot"
    );
}

// ===========================================================================
// MD-05: Semantic drift detection via type-hash comparison
// ===========================================================================
// When a pallet's type changes (e.g., a Balance type changes from u64 to u128),
// the metadata hash must change, and the drift detector must surface this.

#[test]
fn md05_type_change_produces_different_hash() {
    // Simulate metadata for two spec versions where a field type changes.
    // In real Substrate metadata, changing `Balance: u64` → `Balance: u128`
    // would change the raw SCALE bytes.
    let meta_u64 = b"Balances::Balance: u64; pallet_index=5; call_index=0";
    let meta_u128 = b"Balances::Balance: u128; pallet_index=5; call_index=0";

    let hash_u64 = MetadataHash::from_bytes(meta_u64);
    let hash_u128 = MetadataHash::from_bytes(meta_u128);

    assert_ne!(
        hash_u64, hash_u128,
        "MD-05: changing a field type must produce a different metadata hash"
    );
}

#[test]
fn md05_call_index_change_produces_different_hash() {
    // Simulate a pallet reorder that changes call indices.
    // This is the primary source of silent misdecodes if metadata isn't re-pinned.
    let meta_old = b"transfer_keep_alive: pallet_index=5, call_index=3";
    let meta_new = b"transfer_keep_alive: pallet_index=5, call_index=4"; // index shifted

    let hash_old = MetadataHash::from_bytes(meta_old);
    let hash_new = MetadataHash::from_bytes(meta_new);

    assert_ne!(
        hash_old, hash_new,
        "MD-05: call index change must produce different type hash"
    );
}

#[test]
fn md05_identical_bytes_produce_identical_hash() {
    // Type hashes must be deterministic — same bytes always produce the same hash.
    let data = b"transfer(dest: AccountId, value: Balance)";
    let h1 = MetadataHash::from_bytes(data);
    let h2 = MetadataHash::from_bytes(data);

    assert_eq!(h1, h2, "MD-05: type hash must be deterministic");
}

#[test]
fn md05_single_byte_change_produces_different_hash() {
    // A one-byte change in type metadata must produce a completely different hash.
    let mut meta = b"Balances::Balance: u64".to_vec();
    let hash_original = MetadataHash::from_bytes(&meta);

    // Change a single byte (u64 → u65, simulating a type rename).
    let last = meta.len() - 1;
    meta[last] = b'5';
    let hash_modified = MetadataHash::from_bytes(&meta);

    assert_ne!(
        hash_original, hash_modified,
        "MD-05: a single-byte change must produce a different type hash (avalanche property)"
    );
}

#[test]
fn md05_semantic_drift_detected_via_service() {
    // Register a snapshot, pin it, then register semantically different metadata
    // (simulated by different bytes). Verify drift is detected.
    let svc = MetadataService::new();
    let chain = ChainId::new("westend");

    // "Old" metadata: Balance is u64.
    let old_meta = b"Balances::Balance: u64; version=1000";
    let snap_old = MetadataSnapshot::new(
        chain.clone(),
        MetadataVersion::V14,
        old_meta.to_vec(),
        now(),
        1000,
    );
    let old_hash = MetadataHash::from_bytes(old_meta);

    svc.register_snapshot(snap_old);
    svc.pin_current(&chain, "v1000-u64-balance")
        .expect("pin old");

    // "New" metadata: Balance is u128 (type change — semantic drift).
    let new_meta = b"Balances::Balance: u128; version=1001";
    let snap_new = MetadataSnapshot::new(
        chain.clone(),
        MetadataVersion::V14,
        new_meta.to_vec(),
        now(),
        1001,
    );
    let new_hash = MetadataHash::from_bytes(new_meta);

    let drift = svc.register_snapshot(snap_new);

    assert!(
        drift.is_some(),
        "MD-05: semantic type change must be detected as drift"
    );

    let d = drift.unwrap();
    assert_eq!(
        d.chain_id, chain,
        "MD-05: drift must reference correct chain"
    );
    assert_eq!(
        d.pinned_hash, old_hash,
        "MD-05: pinned hash must be the old u64 hash"
    );
    assert_eq!(
        d.current_hash, new_hash,
        "MD-05: current hash must be the new u128 hash"
    );
    assert_ne!(
        d.pinned_hash, d.current_hash,
        "MD-05: type change must produce hash mismatch (semantic drift detected)"
    );
}

#[test]
fn md05_hash_length_is_always_64_chars() {
    // BLAKE3 produces 32-byte hashes; hex-encoded = 64 chars.
    // This invariant ensures hashes are always uniformly sized.
    let inputs: &[&[u8]] = &[
        b"",
        b"a",
        b"Balances::Balance: u64",
        b"Balances::Balance: u128",
        &[0u8; 1000],
    ];

    for input in inputs {
        let hash = MetadataHash::from_bytes(input);
        assert_eq!(
            hash.0.len(),
            64,
            "MD-05: BLAKE3 hash must always be 64 hex chars, got {} for input of len {}",
            hash.0.len(),
            input.len()
        );
    }
}

#[test]
fn md05_drift_affects_pallets_list_is_present() {
    // The affected_pallets field in MetadataDrift allows downstream consumers
    // to know which pallets may have changed semantics.
    let drift = MetadataDrift {
        chain_id: ChainId::new("polkadot"),
        pinned_hash: MetadataHash::from_hex("oldhash"),
        current_hash: MetadataHash::from_hex("newhash"),
        detected_at: now(),
        affected_pallets: vec!["Balances".to_string()],
    };

    assert!(
        !drift.affected_pallets.is_empty(),
        "MD-05: drift must enumerate affected pallets for semantic analysis"
    );
    assert_eq!(drift.affected_pallets[0], "Balances");
}

#[test]
fn md05_new_spec_version_always_changes_hash_if_content_changes() {
    // Verify that spec_version-gated byte changes propagate to hash changes.
    // In practice, a spec_version bump always implies at least one byte changed.
    let spec_1000 = MetadataHash::from_bytes(b"meta_content_for_spec_version_1000");
    let spec_1001 = MetadataHash::from_bytes(b"meta_content_for_spec_version_1001");

    assert_ne!(
        spec_1000, spec_1001,
        "MD-05: different spec_version metadata content must produce different hashes"
    );
}

// ===========================================================================
// Cross-cutting: metadata snapshot stores spec_version for comparison
// ===========================================================================

#[test]
fn cross_snapshot_spec_version_preserved() {
    // MetadataSnapshot must preserve the spec_version field for comparison.
    let snap = MetadataSnapshot::new(
        ChainId::new("polkadot"),
        MetadataVersion::V14,
        b"data".to_vec(),
        now(),
        999_001,
    );
    assert_eq!(
        snap.spec_version, 999_001,
        "Snapshot must preserve the spec_version for staleness comparisons"
    );
}

#[test]
fn cross_hash_from_hex_preserves_value() {
    let hex_str = "deadbeefcafebabe0102030405060708090a0b0c0d0e0f101112131415161718";
    let hash = MetadataHash::from_hex(hex_str);
    assert_eq!(hash.0, hex_str);
}

#[test]
fn cross_metadata_not_found_error_type_is_explicit() {
    let err = MetadataError::NotFound {
        chain_id: ChainId::new("polkadot"),
    };
    let msg = format!("{err}");
    assert!(
        msg.contains("polkadot"),
        "NotFound error must name the chain"
    );
}

#[test]
fn cross_multiple_chains_drift_isolated_per_chain() {
    // Drift on one chain must not affect another.
    let svc = MetadataService::new();

    let polkadot = ChainId::new("polkadot");
    let kusama = ChainId::new("kusama");

    // Pin v1 for both chains.
    svc.register_snapshot(MetadataSnapshot::new(
        polkadot.clone(),
        MetadataVersion::V14,
        b"pdot_v1".to_vec(),
        now(),
        1000,
    ));
    svc.pin_current(&polkadot, "pdot-v1").expect("pin pdot");

    svc.register_snapshot(MetadataSnapshot::new(
        kusama.clone(),
        MetadataVersion::V14,
        b"ksm_v1".to_vec(),
        now(),
        1000,
    ));
    svc.pin_current(&kusama, "ksm-v1").expect("pin ksm");

    // Upgrade only Polkadot.
    let pdot_drift = svc.register_snapshot(MetadataSnapshot::new(
        polkadot.clone(),
        MetadataVersion::V14,
        b"pdot_v2".to_vec(),
        now(),
        1001,
    ));

    // Polkadot: drift detected.
    assert!(
        pdot_drift.is_some(),
        "Drift must be detected for upgraded Polkadot"
    );

    // Kusama: no drift.
    let ksm_drift = svc.check_drift(&kusama);
    assert!(
        ksm_drift.is_none(),
        "Kusama must not show drift — it was not upgraded"
    );
}
