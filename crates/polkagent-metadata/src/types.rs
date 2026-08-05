//! Core metadata types for the Polkagent metadata service.
//!
//! These types represent chain metadata snapshots, hashes, pinning records,
//! and drift information. They are pure data types with no I/O dependencies.

use polkagent_core::Timestamp;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// ChainId
// ---------------------------------------------------------------------------

/// Identifier for a Polkadot-family chain (e.g., "polkadot", "kusama", "westend").
///
/// This is a lightweight string wrapper used as the primary key for metadata
/// lookups, caching, and pinning operations.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChainId(pub String);

impl ChainId {
    /// Create a new `ChainId` from any string-like value.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for ChainId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// MetadataVersion
// ---------------------------------------------------------------------------

/// Wrapper around the runtime metadata version number (e.g., 14 for V14, 15 for V15).
///
/// Substrate runtimes expose metadata in versioned formats; this type captures
/// which version was fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MetadataVersion(pub u32);

impl MetadataVersion {
    /// Metadata V14 (introduced in Substrate for runtime metadata).
    pub const V14: Self = Self(14);
    /// Metadata V15 (extended metadata format).
    pub const V15: Self = Self(15);
}

impl std::fmt::Display for MetadataVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "V{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// MetadataHash
// ---------------------------------------------------------------------------

/// BLAKE3 hash of raw metadata bytes, stored as a hex string.
///
/// Used for integrity verification, cache keys, and drift detection.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MetadataHash(pub String);

impl MetadataHash {
    /// Compute a `MetadataHash` from raw metadata bytes using BLAKE3.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let hash = blake3::hash(bytes);
        Self(hex::encode(hash.as_bytes()))
    }

    /// Create a `MetadataHash` from a pre-computed hex string.
    #[must_use]
    pub fn from_hex(hex: impl Into<String>) -> Self {
        Self(hex.into())
    }
}

impl std::fmt::Display for MetadataHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// MetadataSnapshot
// ---------------------------------------------------------------------------

/// A point-in-time snapshot of a chain's runtime metadata.
///
/// Contains the raw bytes, computed hash, version info, and when it was
/// fetched. This is what gets stored in the metadata cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataSnapshot {
    /// Which chain this metadata belongs to.
    pub chain_id: ChainId,
    /// The runtime metadata format version.
    pub version: MetadataVersion,
    /// BLAKE3 hash of `raw_bytes`.
    pub hash: MetadataHash,
    /// The raw SCALE-encoded metadata bytes.
    pub raw_bytes: Vec<u8>,
    /// When this snapshot was fetched from the chain.
    pub fetched_at: Timestamp,
    /// Runtime spec version at the time of fetch.
    pub spec_version: u32,
}

impl MetadataSnapshot {
    /// Create a new `MetadataSnapshot`, computing the BLAKE3 hash from `raw_bytes`.
    #[must_use]
    pub fn new(
        chain_id: ChainId,
        version: MetadataVersion,
        raw_bytes: Vec<u8>,
        fetched_at: Timestamp,
        spec_version: u32,
    ) -> Self {
        let hash = MetadataHash::from_bytes(&raw_bytes);
        Self {
            chain_id,
            version,
            hash,
            raw_bytes,
            fetched_at,
            spec_version,
        }
    }
}

// ---------------------------------------------------------------------------
// PinnedMetadata
// ---------------------------------------------------------------------------

/// A pinned (known-good) metadata hash for a specific chain.
///
/// Pinning a hash declares it as trusted. The drift detector compares
/// current metadata against pinned hashes to detect unexpected runtime
/// upgrades.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinnedMetadata {
    /// The chain this pin applies to.
    pub chain_id: ChainId,
    /// The pinned BLAKE3 hash of the known-good metadata.
    pub hash: MetadataHash,
    /// When this pin was created.
    pub pinned_at: Timestamp,
    /// Human-readable label for this pin (e.g., "polkadot-v1.2.0").
    pub label: String,
    /// Whether this pin is marked as trusted.
    pub trusted: bool,
}

// ---------------------------------------------------------------------------
// PalletInfo
// ---------------------------------------------------------------------------

/// Summary information about a single pallet extracted from metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PalletInfo {
    /// Name of the pallet (e.g., "Balances", "System").
    pub name: String,
    /// Pallet index in the runtime.
    pub index: u8,
    /// Number of dispatchable calls.
    pub calls: u32,
    /// Number of events.
    pub events: u32,
    /// Number of storage entries.
    pub storage_entries: u32,
    /// Number of constants.
    pub constants: u32,
}

// ---------------------------------------------------------------------------
// CallInfo
// ---------------------------------------------------------------------------

/// Information about a single dispatchable call within a pallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallInfo {
    /// Name of the pallet containing this call.
    pub pallet: String,
    /// Name of the call (e.g., "`transfer_keep_alive`").
    pub name: String,
    /// Call index within the pallet.
    pub index: u8,
    /// Arguments as (name, `type_name`) pairs.
    pub args: Vec<(String, String)>,
}

// ---------------------------------------------------------------------------
// MetadataDrift
// ---------------------------------------------------------------------------

/// A detected metadata drift: the current on-chain metadata no longer
/// matches a pinned (trusted) hash.
///
/// This signals a runtime upgrade or unexpected metadata change that
/// requires attention before the agent can safely operate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataDrift {
    /// The chain where drift was detected.
    pub chain_id: ChainId,
    /// The pinned (expected) hash.
    pub pinned_hash: MetadataHash,
    /// The current (observed) hash.
    pub current_hash: MetadataHash,
    /// When the drift was detected.
    pub detected_at: Timestamp,
    /// Names of pallets that may be affected by the drift.
    ///
    /// Note: accurate pallet-level diffing requires SCALE decoding of the
    /// metadata, which is future work. For now this may just contain a
    /// placeholder indicating a full diff is needed.
    pub affected_pallets: Vec<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Wire-type tests intentionally panic at serialization boundaries so malformed
// metadata round-trip fixtures remain easy to diagnose.
#[allow(
    clippy::expect_used,
    reason = "metadata wire-type assertions intentionally panic with focused diagnostics"
)]
mod tests {
    use super::*;
    use polkagent_core::now;

    #[test]
    fn chain_id_display() {
        let id = ChainId::new("polkadot");
        assert_eq!(format!("{id}"), "polkadot");
    }

    #[test]
    fn chain_id_equality() {
        let a = ChainId::new("kusama");
        let b = ChainId::new("kusama");
        let c = ChainId::new("westend");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn chain_id_serde_round_trip() {
        let id = ChainId::new("polkadot");
        let json = serde_json::to_string(&id).expect("serialize");
        let back: ChainId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }

    #[test]
    fn metadata_version_display() {
        assert_eq!(format!("{}", MetadataVersion::V14), "V14");
        assert_eq!(format!("{}", MetadataVersion::V15), "V15");
        assert_eq!(format!("{}", MetadataVersion(16)), "V16");
    }

    #[test]
    fn metadata_version_constants() {
        assert_eq!(MetadataVersion::V14.0, 14);
        assert_eq!(MetadataVersion::V15.0, 15);
    }

    #[test]
    fn metadata_hash_from_bytes() {
        let data = b"some metadata bytes";
        let hash = MetadataHash::from_bytes(data);
        assert!(!hash.0.is_empty());
        // BLAKE3 produces 32-byte hashes, hex-encoded = 64 chars
        assert_eq!(hash.0.len(), 64);
    }

    #[test]
    fn metadata_hash_deterministic() {
        let data = b"deterministic input";
        let h1 = MetadataHash::from_bytes(data);
        let h2 = MetadataHash::from_bytes(data);
        assert_eq!(h1, h2);
    }

    #[test]
    fn metadata_hash_different_inputs() {
        let h1 = MetadataHash::from_bytes(b"aaa");
        let h2 = MetadataHash::from_bytes(b"bbb");
        assert_ne!(h1, h2);
    }

    #[test]
    fn metadata_hash_from_hex() {
        let hash = MetadataHash::from_hex("abcdef0123456789");
        assert_eq!(hash.0, "abcdef0123456789");
    }

    #[test]
    fn metadata_hash_display() {
        let hash = MetadataHash::from_hex("deadbeef");
        assert_eq!(format!("{hash}"), "deadbeef");
    }

    #[test]
    fn metadata_snapshot_computes_hash() {
        let raw = vec![1, 2, 3, 4, 5];
        let expected_hash = MetadataHash::from_bytes(&raw);
        let snap = MetadataSnapshot::new(
            ChainId::new("polkadot"),
            MetadataVersion::V14,
            raw,
            now(),
            1_000_000,
        );
        assert_eq!(snap.hash, expected_hash);
        assert_eq!(snap.chain_id, ChainId::new("polkadot"));
        assert_eq!(snap.spec_version, 1_000_000);
    }

    #[test]
    fn metadata_snapshot_serde_round_trip() {
        let snap = MetadataSnapshot::new(
            ChainId::new("kusama"),
            MetadataVersion::V15,
            vec![10, 20, 30],
            now(),
            42,
        );
        let json = serde_json::to_string(&snap).expect("serialize");
        let back: MetadataSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.chain_id, snap.chain_id);
        assert_eq!(back.hash, snap.hash);
        assert_eq!(back.spec_version, snap.spec_version);
    }

    #[test]
    fn pinned_metadata_fields() {
        let pin = PinnedMetadata {
            chain_id: ChainId::new("polkadot"),
            hash: MetadataHash::from_hex("aabbccdd"),
            pinned_at: now(),
            label: "v1.0.0".into(),
            trusted: true,
        };
        assert!(pin.trusted);
        assert_eq!(pin.label, "v1.0.0");
    }

    #[test]
    fn pallet_info_fields() {
        let info = PalletInfo {
            name: "Balances".into(),
            index: 5,
            calls: 7,
            events: 3,
            storage_entries: 10,
            constants: 2,
        };
        assert_eq!(info.name, "Balances");
        assert_eq!(info.index, 5);
    }

    #[test]
    fn call_info_fields() {
        let call = CallInfo {
            pallet: "Balances".into(),
            name: "transfer_keep_alive".into(),
            index: 3,
            args: vec![
                ("dest".into(), "AccountId".into()),
                ("value".into(), "Balance".into()),
            ],
        };
        assert_eq!(call.pallet, "Balances");
        assert_eq!(call.args.len(), 2);
        assert_eq!(call.args[0].0, "dest");
    }

    #[test]
    fn metadata_drift_fields() {
        let drift = MetadataDrift {
            chain_id: ChainId::new("polkadot"),
            pinned_hash: MetadataHash::from_hex("aaa"),
            current_hash: MetadataHash::from_hex("bbb"),
            detected_at: now(),
            affected_pallets: vec!["Balances".into(), "Staking".into()],
        };
        assert_eq!(drift.affected_pallets.len(), 2);
        assert_ne!(drift.pinned_hash, drift.current_hash);
    }

    #[test]
    fn metadata_drift_serde_round_trip() {
        let drift = MetadataDrift {
            chain_id: ChainId::new("westend"),
            pinned_hash: MetadataHash::from_hex("111"),
            current_hash: MetadataHash::from_hex("222"),
            detected_at: now(),
            affected_pallets: vec!["System".into()],
        };
        let json = serde_json::to_string(&drift).expect("serialize");
        let back: MetadataDrift = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.chain_id, drift.chain_id);
        assert_eq!(back.affected_pallets, drift.affected_pallets);
    }
}
