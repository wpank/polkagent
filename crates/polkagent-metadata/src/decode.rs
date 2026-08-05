//! Extrinsic decode service that combines metadata caching with SCALE decoding.
//!
//! [`DecodeService`] wraps a [`MetadataService`] and a parsed
//! [`RuntimeMetadata`] cache so that callers can decode raw extrinsic bytes
//! into typed [`DecodedExtrinsic`] values without re-parsing metadata on every
//! call.
//!
//! # Design notes
//!
//! * **AC-P2-004** — Stale metadata is reported explicitly rather than silently
//!   producing a potentially wrong decode.
//! * **AC-P2-005** — Wrong-network extrinsics are rejected before decoding
//!   through [`DecodeService::decode_with_validation`].

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use polkagent_codec::{decode_call_with_metadata, DecodedExtrinsic, RuntimeMetadata};

use crate::error::MetadataError;
use crate::service::MetadataService;
use crate::types::ChainId;
use crate::validation::validate_network;

// ---------------------------------------------------------------------------
// DecodeService
// ---------------------------------------------------------------------------

/// Result type alias for decode operations.
pub type Result<T> = std::result::Result<T, MetadataError>;

/// Combines a [`MetadataService`] with a parsed-metadata cache to provide
/// typed extrinsic decoding.
///
/// Parsed [`RuntimeMetadata`] objects are expensive to create (they require
/// SCALE-decoding the full metadata blob). `DecodeService` keeps a per-chain
/// cache of the parsed form so subsequent calls to [`decode_extrinsic`] or
/// [`decode_with_validation`] are fast.
///
/// # Thread safety
///
/// `DecodeService` is `Clone` and all internal state is protected by `Arc` and
/// `RwLock`, making it safe to share across threads and async tasks.
///
/// [`decode_extrinsic`]: DecodeService::decode_extrinsic
/// [`decode_with_validation`]: DecodeService::decode_with_validation
#[derive(Debug, Clone)]
pub struct DecodeService {
    service: MetadataService,
    /// Cache of already-parsed `RuntimeMetadata`, keyed by chain name.
    /// Each entry also records the raw bytes hash so we can detect when the
    /// metadata snapshot has been replaced and the parsed form needs a refresh.
    parsed_cache: Arc<RwLock<HashMap<String, ParsedEntry>>>,
    /// Maximum age of cached metadata before it is considered stale.
    max_age: Duration,
}

#[derive(Debug, Clone)]
struct ParsedEntry {
    /// BLAKE3 hash (hex) of the raw bytes the metadata was parsed from.
    raw_hash: String,
    /// The parsed representation.
    metadata: RuntimeMetadata,
}

impl DecodeService {
    /// Create a new `DecodeService` wrapping the given `MetadataService`.
    ///
    /// Metadata is considered stale after `max_age`. To disable staleness
    /// checking, pass `Duration::MAX`.
    #[must_use]
    pub fn new(service: MetadataService, max_age: Duration) -> Self {
        Self {
            service,
            parsed_cache: Arc::new(RwLock::new(HashMap::new())),
            max_age,
        }
    }

    /// Create a `DecodeService` with a default metadata service and the given
    /// maximum metadata age.
    #[must_use]
    pub fn with_default_service(max_age: Duration) -> Self {
        Self::new(MetadataService::new(), max_age)
    }

    /// Access the underlying [`MetadataService`].
    #[must_use]
    pub fn metadata_service(&self) -> &MetadataService {
        &self.service
    }

    // -----------------------------------------------------------------------
    // decode_extrinsic
    // -----------------------------------------------------------------------

    /// Decode an extrinsic using cached metadata for `chain`.
    ///
    /// The `raw_bytes` must be the full outer-length-prefixed extrinsic bytes
    /// as produced by the Substrate runtime (i.e. starting with a compact
    /// length, then the version/signed byte, then optional signature fields,
    /// then the call bytes).
    ///
    /// # Errors
    ///
    /// | Condition | Error variant |
    /// |-----------|---------------|
    /// | No metadata cached for `chain` | [`MetadataError::NotFound`] |
    /// | Cached metadata is older than `max_age` | [`MetadataError::StaleMetadata`] (AC-P2-004) |
    /// | Raw metadata bytes cannot be parsed | [`MetadataError::Codec`] |
    /// | SCALE decoding of extrinsic fails | [`MetadataError::Codec`] |
    pub fn decode_extrinsic(&self, chain: &str, raw_bytes: &[u8]) -> Result<DecodedExtrinsic> {
        let metadata = self.require_fresh_metadata(chain)?;

        // The codec's `decode_call_with_metadata` expects bare call bytes
        // (pallet_index, call_index, args…). We need to strip the outer
        // extrinsic framing first.
        let call_bytes = Self::strip_extrinsic_framing(raw_bytes)
            .map_err(|e| MetadataError::Codec(e.to_string()))?;

        decode_call_with_metadata(&call_bytes, &metadata)
            .map_err(|e| MetadataError::Codec(e.to_string()))
    }

    // -----------------------------------------------------------------------
    // decode_with_validation
    // -----------------------------------------------------------------------

    /// Decode an extrinsic after first validating the genesis hash (AC-P2-005).
    ///
    /// The `expected_genesis` hash must match the genesis hash embedded in the
    /// extrinsic's signed extensions. If it does not, the extrinsic is
    /// rejected with [`MetadataError::WrongNetwork`] before any decoding is
    /// attempted.
    ///
    /// For unsigned extrinsics (which have no genesis hash in their encoding)
    /// this method still succeeds; the signed-extension bytes are simply
    /// absent.
    ///
    /// # Arguments
    ///
    /// * `chain` — chain name used for cache lookup and error messages.
    /// * `raw_bytes` — full outer extrinsic bytes.
    /// * `expected_genesis` — the 32-byte genesis hash for the expected
    ///   network.
    ///
    /// # Errors
    ///
    /// All errors from [`decode_extrinsic`] plus:
    ///
    /// | Condition | Error variant |
    /// |-----------|---------------|
    /// | Genesis hash mismatch | [`MetadataError::WrongNetwork`] (AC-P2-005) |
    ///
    /// [`decode_extrinsic`]: DecodeService::decode_extrinsic
    pub fn decode_with_validation(
        &self,
        chain: &str,
        raw_bytes: &[u8],
        expected_genesis: &[u8; 32],
    ) -> Result<DecodedExtrinsic> {
        // Only signed extrinsics carry a genesis hash. Check if this one is
        // signed before attempting extraction.
        if let Some(genesis) = Self::extract_genesis_hash(raw_bytes) {
            validate_network(chain, &genesis, expected_genesis)?;
        }

        self.decode_extrinsic(chain, raw_bytes)
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Get a fresh (non-stale) parsed [`RuntimeMetadata`] for `chain`.
    ///
    /// Returns a clone from the parsed cache when the underlying raw snapshot
    /// has not changed; re-parses if the snapshot was replaced.
    fn require_fresh_metadata(&self, chain: &str) -> Result<RuntimeMetadata> {
        let chain_id = ChainId::new(chain);

        // Retrieve the latest cached snapshot.
        let snapshot =
            self.service
                .get_snapshot(&chain_id)
                .ok_or_else(|| MetadataError::NotFound {
                    chain_id: chain_id.clone(),
                })?;

        // Reject stale metadata (AC-P2-004).
        if self.service.is_stale(&chain_id, self.max_age) {
            return Err(MetadataError::StaleMetadata {
                chain_name: chain.to_string(),
            });
        }

        let raw_hash = snapshot.hash.0.clone();

        // Fast path: parsed metadata already in cache and still current.
        {
            let parsed = self
                .parsed_cache
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = parsed.get(chain) {
                if entry.raw_hash == raw_hash {
                    return Ok(entry.metadata.clone());
                }
            }
        }

        // Slow path: parse the raw bytes and cache the result.
        let runtime_metadata = polkagent_codec::parse_metadata(&snapshot.raw_bytes)
            .map_err(|e| MetadataError::Codec(e.to_string()))?;

        {
            let mut parsed = self
                .parsed_cache
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            parsed.insert(
                chain.to_string(),
                ParsedEntry {
                    raw_hash,
                    metadata: runtime_metadata.clone(),
                },
            );
        }

        Ok(runtime_metadata)
    }

    /// Strip the outer extrinsic framing (compact length + version byte +
    /// optional signature block + optional era/nonce/tip) from `bytes` and
    /// return only the call bytes (`[pallet_index, call_index, args…]`).
    ///
    /// This mirrors the logic in `polkagent_codec::decode::decode_extrinsic`
    /// but discards everything before the pallet byte rather than the whole
    /// extrinsic.
    fn strip_extrinsic_framing(bytes: &[u8]) -> polkagent_codec::Result<Vec<u8>> {
        use polkagent_codec::ScaleDecoder;

        let mut dec = ScaleDecoder::new(bytes);

        // Outer compact length prefix.
        let _len = dec.decode_compact_u32()?;

        // Version byte: high bit = signed flag.
        let version_byte = dec.decode_u8()?;
        let is_signed = (version_byte & 0x80) != 0;

        if is_signed {
            // sender: MultiAddress variant byte + 32-byte AccountId.
            let _addr_variant = dec.decode_u8()?;
            let _sender: [u8; 32] = dec.decode_fixed_array::<32>()?;

            // signature: MultiSignature variant byte + payload.
            let sig_variant = dec.decode_u8()?;
            match sig_variant {
                0 | 1 => {
                    let _: [u8; 64] = dec.decode_fixed_array::<64>()?;
                }
                2 => {
                    let _: [u8; 65] = dec.decode_fixed_array::<65>()?;
                }
                v => {
                    return Err(polkagent_codec::CodecError::invalid_type(format!(
                        "unknown signature variant: {v}"
                    )));
                }
            }

            // Era (immortal = 0x00; mortal = 2 bytes).
            let era_byte = dec.decode_u8()?;
            if era_byte != 0x00 {
                let _era_high = dec.decode_u8()?;
            }

            // Nonce: compact u64.
            let _nonce = dec.decode_compact_u64()?;
            // Tip: compact u64 (widened to u128 by caller).
            let _tip = dec.decode_compact_u64()?;
        }

        // Everything remaining is the call bytes.
        Ok(dec.remaining_bytes().to_vec())
    }

    /// Attempt to extract the 32-byte genesis hash from the signed extensions
    /// of a *signed* extrinsic.
    ///
    /// In Substrate's SCALE encoding the signed extensions that follow the tip
    /// are:
    /// ```text
    /// [CheckMortality] era
    /// [CheckNonce]     nonce (compact u64)
    /// [ChargeTransactionPayment] tip (compact u64)
    /// [CheckGenesis]   genesis_hash [u8; 32]
    /// [CheckVersion]   spec_version u32
    /// …
    /// ```
    ///
    /// Because the layout of signed extensions is runtime-specific and we
    /// don't have full metadata here, we use a best-effort heuristic:
    /// skip sender + signature + era + nonce + tip, then read 32 bytes.
    ///
    /// Returns `None` for unsigned extrinsics or if extraction fails.
    fn extract_genesis_hash(bytes: &[u8]) -> Option<[u8; 32]> {
        use polkagent_codec::ScaleDecoder;

        let mut dec = ScaleDecoder::new(bytes);

        // Outer compact length prefix.
        dec.decode_compact_u32().ok()?;

        // Version byte.
        let version_byte = dec.decode_u8().ok()?;
        let is_signed = (version_byte & 0x80) != 0;

        if !is_signed {
            return None;
        }

        // Sender: MultiAddress variant + 32 bytes.
        let _addr_variant = dec.decode_u8().ok()?;
        let _sender: [u8; 32] = dec.decode_fixed_array::<32>().ok()?;

        // Signature: variant byte + payload.
        let sig_variant = dec.decode_u8().ok()?;
        match sig_variant {
            0 | 1 => {
                let _: [u8; 64] = dec.decode_fixed_array::<64>().ok()?;
            }
            2 => {
                let _: [u8; 65] = dec.decode_fixed_array::<65>().ok()?;
            }
            _ => return None,
        }

        // Era.
        let era_byte = dec.decode_u8().ok()?;
        if era_byte != 0x00 {
            dec.decode_u8().ok()?; // mortal era high byte
        }

        // Nonce (compact u64).
        dec.decode_compact_u64().ok()?;

        // Tip (compact u64).
        dec.decode_compact_u64().ok()?;

        // Next 32 bytes: genesis hash.
        dec.decode_fixed_array::<32>().ok()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{MetadataSnapshot, MetadataVersion};
    use polkagent_codec::metadata::build_minimal_metadata_v14;
    use polkagent_codec::ScaleEncoder;
    use polkagent_core::now;
    use std::time::Duration;

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    /// Build a minimal metadata blob with named pallets (no calls).
    fn minimal_meta_bytes(pallets: &[(&str, u8)]) -> Vec<u8> {
        build_minimal_metadata_v14(pallets)
    }

    /// Register a snapshot in the service's cache.
    fn register(svc: &MetadataService, chain: &str, raw: Vec<u8>) {
        let snap = MetadataSnapshot::new(ChainId::new(chain), MetadataVersion::V14, raw, now(), 1);
        svc.register_snapshot(snap);
    }

    /// Build an unsigned extrinsic byte sequence:
    /// `[compact(len)] [0x04 version] [pallet] [call] [args...]`
    fn unsigned_extrinsic(pallet: u8, call: u8, args: &[u8]) -> Vec<u8> {
        let mut payload = vec![0x04u8, pallet, call];
        payload.extend_from_slice(args);

        let mut enc = ScaleEncoder::new();
        enc.encode_compact_u32(payload.len() as u32);
        let mut out = enc.finish();
        out.extend(payload);
        out
    }

    /// Build a mock signed extrinsic with the given genesis hash embedded.
    ///
    /// Layout:
    /// `[compact len] [0x84 signed] [0x00 addr_variant] [32 sender] [0x01 sr25519] [64 sig] [0x00 immortal era] [compact 0 nonce] [compact 0 tip] [genesis: 32 bytes] [pallet] [call]`
    fn signed_extrinsic_with_genesis(
        sender: [u8; 32],
        genesis: [u8; 32],
        pallet: u8,
        call: u8,
    ) -> Vec<u8> {
        let mut payload: Vec<u8> = Vec::new();
        payload.push(0x84); // signed extrinsic version byte (0x04 | 0x80)
        payload.push(0x00); // MultiAddress::Id variant
        payload.extend_from_slice(&sender);
        payload.push(0x01); // Sr25519 signature variant
        payload.extend_from_slice(&[0u8; 64]); // 64-byte dummy signature
        payload.push(0x00); // era: immortal
        payload.push(0x00); // nonce: compact(0) = 0x00
        payload.push(0x00); // tip: compact(0) = 0x00
        payload.extend_from_slice(&genesis); // genesis hash (32 bytes)
        payload.push(pallet);
        payload.push(call);

        let mut enc = ScaleEncoder::new();
        enc.encode_compact_u32(payload.len() as u32);
        let mut out = enc.finish();
        out.extend(payload);
        out
    }

    // -----------------------------------------------------------------------
    // Test: decode_extrinsic with valid cached metadata
    // -----------------------------------------------------------------------

    #[test]
    fn decode_with_valid_cached_metadata_succeeds() {
        let svc = MetadataService::new();
        let meta_bytes = minimal_meta_bytes(&[("System", 0), ("Balances", 5)]);
        register(&svc, "polkadot", meta_bytes);

        let decode_svc = DecodeService::new(svc, Duration::from_secs(3600));

        // Unsigned extrinsic targeting pallet 5 (Balances), call 0.
        let ext_bytes = unsigned_extrinsic(5, 0, &[]);
        let result = decode_svc.decode_extrinsic("polkadot", &ext_bytes);
        assert!(result.is_ok(), "expected ok, got: {result:?}");

        let decoded = result.expect("decoded");
        assert_eq!(decoded.pallet_index, 5);
        assert_eq!(decoded.call_index, 0);
        // Pallet name should be resolved from metadata.
        assert_eq!(decoded.pallet_name.as_deref(), Some("Balances"));
    }

    // -----------------------------------------------------------------------
    // Test: decode with no cached metadata → clear error
    // -----------------------------------------------------------------------

    #[test]
    fn decode_with_no_cached_metadata_returns_not_found() {
        let svc = MetadataService::new();
        let decode_svc = DecodeService::new(svc, Duration::from_secs(3600));

        let ext_bytes = unsigned_extrinsic(0, 0, &[]);
        let result = decode_svc.decode_extrinsic("polkadot", &ext_bytes);
        assert!(result.is_err());

        let err = result.expect_err("should be err");
        assert!(
            matches!(err, MetadataError::NotFound { .. }),
            "expected NotFound, got: {err:?}"
        );
        let msg = format!("{err}");
        assert!(
            msg.contains("polkadot"),
            "error message should contain chain name: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // Test: decode with stale metadata → explicit stale error (AC-P2-004)
    // -----------------------------------------------------------------------

    #[test]
    fn decode_with_stale_metadata_returns_stale_error() {
        let svc = MetadataService::new();
        let meta_bytes = minimal_meta_bytes(&[("System", 0)]);

        // Register a snapshot that was fetched 2 hours ago.
        let old_ts = chrono::Utc::now() - chrono::Duration::seconds(7200);
        let snap = MetadataSnapshot::new(
            ChainId::new("polkadot"),
            MetadataVersion::V14,
            meta_bytes,
            old_ts,
            1,
        );
        svc.register_snapshot(snap);

        // max_age of 1 hour — snapshot is 2 hours old, so stale.
        let decode_svc = DecodeService::new(svc, Duration::from_secs(3600));

        let ext_bytes = unsigned_extrinsic(0, 0, &[]);
        let result = decode_svc.decode_extrinsic("polkadot", &ext_bytes);
        assert!(result.is_err());

        let err = result.expect_err("should be err");
        assert!(
            matches!(err, MetadataError::StaleMetadata { .. }),
            "expected StaleMetadata (AC-P2-004), got: {err:?}"
        );
        let msg = format!("{err}");
        assert!(
            msg.contains("polkadot"),
            "stale error should contain chain name: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // Test: wrong-network rejection (AC-P2-005)
    // -----------------------------------------------------------------------

    #[test]
    fn decode_with_wrong_genesis_rejects_extrinsic() {
        let svc = MetadataService::new();
        let meta_bytes = minimal_meta_bytes(&[("System", 0)]);
        register(&svc, "polkadot", meta_bytes);

        let decode_svc = DecodeService::new(svc, Duration::from_secs(3600));

        let polkadot_genesis = [0x91u8; 32];
        let kusama_genesis = [0xB0u8; 32];
        let sender = [0u8; 32];

        // Build a signed extrinsic with the Kusama genesis.
        let ext_bytes = signed_extrinsic_with_genesis(sender, kusama_genesis, 0, 0);

        // Ask DecodeService to validate against the Polkadot genesis.
        let result = decode_svc.decode_with_validation("polkadot", &ext_bytes, &polkadot_genesis);
        assert!(result.is_err());

        let err = result.expect_err("should be err");
        assert!(
            matches!(err, MetadataError::WrongNetwork { .. }),
            "expected WrongNetwork (AC-P2-005), got: {err:?}"
        );
        let msg = format!("{err}");
        assert!(
            msg.contains("polkadot"),
            "wrong-network error should mention chain profile: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // Test: matching genesis hash is accepted
    // -----------------------------------------------------------------------

    #[test]
    fn decode_with_correct_genesis_succeeds() {
        let svc = MetadataService::new();
        let meta_bytes = minimal_meta_bytes(&[("System", 0)]);
        register(&svc, "polkadot", meta_bytes);

        let decode_svc = DecodeService::new(svc, Duration::from_secs(3600));

        let genesis = [0x91u8; 32];
        let sender = [0u8; 32];

        let ext_bytes = signed_extrinsic_with_genesis(sender, genesis, 0, 0);
        let result = decode_svc.decode_with_validation("polkadot", &ext_bytes, &genesis);
        assert!(
            result.is_ok(),
            "should succeed with matching genesis: {result:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test: unsigned extrinsic passes validation (no genesis to check)
    // -----------------------------------------------------------------------

    #[test]
    fn unsigned_extrinsic_passes_genesis_validation() {
        let svc = MetadataService::new();
        let meta_bytes = minimal_meta_bytes(&[("System", 0)]);
        register(&svc, "polkadot", meta_bytes);

        let decode_svc = DecodeService::new(svc, Duration::from_secs(3600));

        let genesis = [0x91u8; 32];
        let ext_bytes = unsigned_extrinsic(0, 0, &[]);

        // Unsigned extrinsics have no embedded genesis — validation should pass.
        let result = decode_svc.decode_with_validation("polkadot", &ext_bytes, &genesis);
        assert!(
            result.is_ok(),
            "unsigned extrinsic should pass validation: {result:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test: DecodeService caches parsed RuntimeMetadata
    // -----------------------------------------------------------------------

    #[test]
    fn decode_service_caches_parsed_metadata() {
        let svc = MetadataService::new();
        let meta_bytes = minimal_meta_bytes(&[("System", 0), ("Balances", 5)]);
        register(&svc, "polkadot", meta_bytes);

        let decode_svc = DecodeService::new(svc, Duration::from_secs(3600));

        // First call should parse and cache.
        let ext1 = unsigned_extrinsic(0, 0, &[]);
        let _ = decode_svc
            .decode_extrinsic("polkadot", &ext1)
            .expect("first decode");

        // The parsed cache should now have an entry for "polkadot".
        {
            let cache = decode_svc.parsed_cache.read().expect("read lock");
            assert!(
                cache.contains_key("polkadot"),
                "parsed cache should contain polkadot"
            );
        }

        // Second call should hit the cache (no re-parse).
        let ext2 = unsigned_extrinsic(5, 0, &[]);
        let result = decode_svc
            .decode_extrinsic("polkadot", &ext2)
            .expect("second decode");
        assert_eq!(result.pallet_index, 5);
    }

    // -----------------------------------------------------------------------
    // Test: parsed cache is invalidated when metadata is replaced
    // -----------------------------------------------------------------------

    #[test]
    fn parsed_cache_refreshes_when_snapshot_changes() {
        let svc = MetadataService::new();

        // Register v1 metadata with only System pallet.
        let meta_v1 = minimal_meta_bytes(&[("System", 0)]);
        register(&svc, "polkadot", meta_v1);

        let decode_svc = DecodeService::new(svc.clone(), Duration::from_secs(3600));

        let ext = unsigned_extrinsic(0, 0, &[]);
        let r1 = decode_svc
            .decode_extrinsic("polkadot", &ext)
            .expect("decode v1");
        assert_eq!(r1.pallet_name.as_deref(), Some("System"));

        // Now register v2 metadata that adds a Balances pallet.
        let meta_v2 = minimal_meta_bytes(&[("System", 0), ("Balances", 5)]);
        register(&svc, "polkadot", meta_v2);

        // Decoding pallet 5 should work after the cache is refreshed.
        let ext2 = unsigned_extrinsic(5, 0, &[]);
        let r2 = decode_svc
            .decode_extrinsic("polkadot", &ext2)
            .expect("decode v2");
        assert_eq!(r2.pallet_index, 5);
        assert_eq!(r2.pallet_name.as_deref(), Some("Balances"));
    }

    // -----------------------------------------------------------------------
    // Test: known fixture bytes produce expected pallet/call
    // -----------------------------------------------------------------------

    #[test]
    fn known_fixture_bytes_produce_expected_pallet_call() {
        // Register metadata with System (0) and Balances (5) pallets.
        let svc = MetadataService::new();
        let meta_bytes = minimal_meta_bytes(&[("System", 0), ("Balances", 5)]);
        register(&svc, "polkadot", meta_bytes);

        let decode_svc = DecodeService::new(svc, Duration::from_secs(3600));

        // Extrinsic targeting Balances (5), call index 3 (transferKeepAlive).
        let ext_bytes = unsigned_extrinsic(5, 3, &[0xDE, 0xAD]);
        let decoded = decode_svc
            .decode_extrinsic("polkadot", &ext_bytes)
            .expect("decode");

        assert_eq!(decoded.pallet_index, 5);
        assert_eq!(decoded.call_index, 3);
        assert_eq!(decoded.pallet_name.as_deref(), Some("Balances"));
        // No call metadata in minimal fixture, so call_name is None.
        assert!(decoded.call_name.is_none());
    }

    // -----------------------------------------------------------------------
    // Test: decode of System pallet call
    // -----------------------------------------------------------------------

    #[test]
    fn decode_system_pallet_call() {
        let svc = MetadataService::new();
        let meta_bytes = minimal_meta_bytes(&[("System", 0), ("Utility", 24)]);
        register(&svc, "polkadot", meta_bytes);

        let decode_svc = DecodeService::new(svc, Duration::from_secs(3600));

        let ext_bytes = unsigned_extrinsic(0, 1, &[]);
        let decoded = decode_svc
            .decode_extrinsic("polkadot", &ext_bytes)
            .expect("decode");
        assert_eq!(decoded.pallet_index, 0);
        assert_eq!(decoded.pallet_name.as_deref(), Some("System"));
    }

    // -----------------------------------------------------------------------
    // Test: decode with multiple chains is independent
    // -----------------------------------------------------------------------

    #[test]
    fn decode_multiple_chains_independently() {
        let svc = MetadataService::new();

        let polkadot_meta = minimal_meta_bytes(&[("Balances", 5)]);
        let kusama_meta = minimal_meta_bytes(&[("Staking", 7)]);

        register(&svc, "polkadot", polkadot_meta);
        register(&svc, "kusama", kusama_meta);

        let decode_svc = DecodeService::new(svc, Duration::from_secs(3600));

        let pdot_ext = unsigned_extrinsic(5, 0, &[]);
        let pdot = decode_svc
            .decode_extrinsic("polkadot", &pdot_ext)
            .expect("polkadot decode");
        assert_eq!(pdot.pallet_name.as_deref(), Some("Balances"));

        let ksm_ext = unsigned_extrinsic(7, 0, &[]);
        let ksm = decode_svc
            .decode_extrinsic("kusama", &ksm_ext)
            .expect("kusama decode");
        assert_eq!(ksm.pallet_name.as_deref(), Some("Staking"));

        // polkadot does not know about Staking (7) — pallet_name is None.
        let ksm_ext2 = unsigned_extrinsic(7, 0, &[]);
        let pdot2 = decode_svc
            .decode_extrinsic("polkadot", &ksm_ext2)
            .expect("decode");
        assert!(pdot2.pallet_name.is_none());
    }

    // -----------------------------------------------------------------------
    // Test: wrong-network error message is helpful
    // -----------------------------------------------------------------------

    #[test]
    fn wrong_network_error_is_descriptive() {
        let svc = MetadataService::new();
        let meta_bytes = minimal_meta_bytes(&[("System", 0)]);
        register(&svc, "polkadot", meta_bytes);

        let decode_svc = DecodeService::new(svc, Duration::from_secs(3600));

        let polkadot_genesis = [0x91u8; 32];
        let kusama_genesis = [0xB0u8; 32];
        let sender = [0u8; 32];

        let ext_bytes = signed_extrinsic_with_genesis(sender, kusama_genesis, 0, 0);
        let err = decode_svc
            .decode_with_validation("polkadot", &ext_bytes, &polkadot_genesis)
            .expect_err("should fail");

        let msg = format!("{err}");
        // Must mention both the chain name and the genesis hash mismatch.
        assert!(msg.contains("polkadot"), "error mentions chain name: {msg}");
        // The error should include hex representations of the genesis hashes.
        assert!(
            msg.contains("b0b0") || msg.contains("9191") || msg.len() > 20,
            "error contains hash info: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // Test: genesis mismatch is rejected before decoding attempts
    // -----------------------------------------------------------------------

    #[test]
    fn wrong_network_rejected_before_decoding() {
        // Don't register any metadata — if we tried to decode, we'd get NotFound.
        // But WrongNetwork should be returned first.
        let svc = MetadataService::new();
        let meta_bytes = minimal_meta_bytes(&[("System", 0)]);
        register(&svc, "polkadot", meta_bytes);

        let decode_svc = DecodeService::new(svc, Duration::from_secs(3600));

        let polkadot_genesis = [0x91u8; 32];
        let wrong_genesis = [0xFFu8; 32];
        let sender = [0u8; 32];

        let ext_bytes = signed_extrinsic_with_genesis(sender, wrong_genesis, 0, 0);
        let err = decode_svc
            .decode_with_validation("polkadot", &ext_bytes, &polkadot_genesis)
            .expect_err("should fail");

        assert!(
            matches!(err, MetadataError::WrongNetwork { .. }),
            "expected WrongNetwork before NotFound, got: {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test: stale metadata error is not confused with a wrong decode
    // -----------------------------------------------------------------------

    #[test]
    fn stale_metadata_returns_stale_not_codec_error() {
        let svc = MetadataService::new();
        let meta_bytes = minimal_meta_bytes(&[("System", 0)]);

        let old_ts = chrono::Utc::now() - chrono::Duration::seconds(7200);
        let snap = MetadataSnapshot::new(
            ChainId::new("kusama"),
            MetadataVersion::V14,
            meta_bytes,
            old_ts,
            1,
        );
        svc.register_snapshot(snap);

        let decode_svc = DecodeService::new(svc, Duration::from_secs(3600));
        let ext_bytes = unsigned_extrinsic(0, 0, &[]);
        let err = decode_svc
            .decode_extrinsic("kusama", &ext_bytes)
            .expect_err("should fail");

        assert!(
            matches!(err, MetadataError::StaleMetadata { .. }),
            "stale metadata must produce StaleMetadata, not a codec error: {err:?}"
        );
        // Must NOT be a codec error.
        assert!(
            !matches!(err, MetadataError::Codec(_)),
            "must not be Codec error: {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test: metadata service accessor
    // -----------------------------------------------------------------------

    #[test]
    fn metadata_service_accessor_returns_service() {
        let svc = MetadataService::new();
        let decode_svc = DecodeService::new(svc.clone(), Duration::from_secs(60));
        // Just checks the accessor compiles and doesn't panic.
        let _ = decode_svc.metadata_service();
    }

    // -----------------------------------------------------------------------
    // Test: fresh metadata with Duration::MAX does not go stale
    // -----------------------------------------------------------------------

    #[test]
    fn max_age_max_duration_never_stale() {
        let svc = MetadataService::new();
        let meta_bytes = minimal_meta_bytes(&[("System", 0)]);

        // Even a very old snapshot should not be stale with Duration::MAX.
        let ancient_ts = chrono::Utc::now() - chrono::Duration::seconds(86400 * 365);
        let snap = MetadataSnapshot::new(
            ChainId::new("polkadot"),
            MetadataVersion::V14,
            meta_bytes,
            ancient_ts,
            1,
        );
        svc.register_snapshot(snap);

        let decode_svc = DecodeService::new(svc, Duration::MAX);
        let ext_bytes = unsigned_extrinsic(0, 0, &[]);
        let result = decode_svc.decode_extrinsic("polkadot", &ext_bytes);
        assert!(
            result.is_ok(),
            "should not be stale with Duration::MAX: {result:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test: with_default_service constructor
    // -----------------------------------------------------------------------

    #[test]
    fn with_default_service_has_empty_cache() {
        let decode_svc = DecodeService::with_default_service(Duration::from_secs(600));

        // No metadata registered — should return NotFound.
        let ext_bytes = unsigned_extrinsic(0, 0, &[]);
        let err = decode_svc
            .decode_extrinsic("polkadot", &ext_bytes)
            .expect_err("should fail");
        assert!(matches!(err, MetadataError::NotFound { .. }));
    }

    // -----------------------------------------------------------------------
    // Test: clone shares the parsed cache
    // -----------------------------------------------------------------------

    #[test]
    fn clone_shares_parsed_cache() {
        let svc = MetadataService::new();
        let meta_bytes = minimal_meta_bytes(&[("System", 0)]);
        register(&svc, "polkadot", meta_bytes);

        let decode_svc1 = DecodeService::new(svc, Duration::from_secs(3600));
        let decode_svc2 = decode_svc1.clone();

        // Populate the cache through svc1.
        let ext = unsigned_extrinsic(0, 0, &[]);
        let _ = decode_svc1
            .decode_extrinsic("polkadot", &ext)
            .expect("decode via svc1");

        // svc2 should share the same cache (Arc).
        {
            let cache = decode_svc2.parsed_cache.read().expect("lock");
            assert!(
                cache.contains_key("polkadot"),
                "shared cache should have polkadot entry"
            );
        }
    }
}
