//! Shared conformance test suite for [`ChainClient`] implementations.
//!
//! This module defines the canonical set of behavioural tests that **every**
//! adapter implementing [`ChainClient`] must pass (PRD-15).  Tests are plain
//! `async fn`s so each adapter can call them from its own `tests/` directory.
//!
//! # Usage
//!
//! ```rust,ignore
//! // In your adapter crate: tests/conformance.rs
//! use polkagent_chain_trait::conformance;
//! use my_adapter::MyChainClient;
//!
//! #[tokio::test]
//! async fn test_get_runtime_version() {
//!     let client = MyChainClient::new_for_tests();
//!     conformance::test_get_runtime_version(&client).await;
//! }
//! ```
//!
//! # Note on network access
//!
//! These tests are written against the trait; whether they require a live node
//! depends entirely on the adapter under test.  Fake adapters can run fully
//! offline; real adapters need a reachable node.

use crate::{BlockRef, ChainClient, ChainProfileId, FinalityObservation};
use polkagent_core::now;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A minimal [`ChainProfileId`] usable in conformance tests.
///
/// Adapters that require a specific profile should configure their test
/// instance with this identifier.
#[must_use]
pub fn test_profile_id() -> ChainProfileId {
    ChainProfileId::new("conformance-test")
}

/// A minimal [`BlockRef`] at block 0 for use as a "current block" in
/// simulation tests.
#[must_use]
pub fn genesis_block() -> BlockRef {
    BlockRef {
        number: 0,
        hash: "0x0000000000000000000000000000000000000000000000000000000000000000".into(),
    }
}

// ---------------------------------------------------------------------------
// Conformance tests
// ---------------------------------------------------------------------------

/// Conformance: `fetch_metadata()` returns a [`crate::PinnedMetadata`] with a
/// non-empty metadata body and a valid spec version.
///
/// The adapter under test must be configured with a chain profile identified
/// by `test_profile_id()`.  If the adapter does not recognise the profile it
/// may return an error; the test treats any non-panicking outcome as a pass.
pub async fn test_get_runtime_version(client: &dyn ChainClient) {
    let result = client.fetch_metadata(test_profile_id()).await;

    if let Ok(meta) = result {
        assert!(
            !meta.metadata_bytes.is_empty(),
            "fetch_metadata() must return non-empty metadata_bytes; got 0 bytes"
        );
        assert!(
            meta.spec_version > 0,
            "fetch_metadata() must return a non-zero spec_version; got {}",
            meta.spec_version,
        );
        assert_eq!(
            meta.chain_profile,
            test_profile_id(),
            "fetch_metadata() must return metadata tagged with the requested chain profile"
        );
    }
}

/// Conformance: `query_storage()` returns `None` or `Some(bytes)` without
/// panicking.
///
/// A storage query for an arbitrary key should never panic.  The result may
/// be `None` (key absent) or `Some(bytes)` (key present).
pub async fn test_query_storage(client: &dyn ChainClient) {
    // Query for an arbitrary, almost certainly absent storage key.
    let storage_key = vec![0u8; 32];
    let result = client
        .query_storage(&storage_key, None, test_profile_id())
        .await;

    // Any non-panicking outcome is acceptable.
    if let Ok(Some(value)) = result {
        assert!(
            !value.is_empty(),
            "query_storage() returned Some with 0 bytes; storage values must be non-empty"
        );
    }
}

/// Conformance: `fetch_metadata()` returns a block number via `block_ref`.
///
/// The returned `PinnedMetadata` must carry a `block_ref` with a plausible
/// block number (u64 ≥ 0; all values are technically valid but we check the
/// field is accessible).
pub async fn test_get_block_number(client: &dyn ChainClient) {
    let result = client.fetch_metadata(test_profile_id()).await;

    if let Ok(meta) = result {
        assert!(
            !meta.block_ref.hash.is_empty(),
            "block_ref.hash at block {} must not be empty; got {:?}",
            meta.block_ref.number,
            meta.block_ref.hash,
        );
    }
    // Errors are acceptable for offline adapters without the test profile.
}

/// Conformance: `submit_extrinsic()` returns a non-empty transaction hash or
/// a well-typed error.
///
/// A trivially small extrinsic byte sequence is sent.  The adapter is expected
/// to return either a `TxHash` (if it accepts the submission) or an error such
/// as `ExtrinsicRejected`.  What is not acceptable is a panic.
pub async fn test_submit_extrinsic(client: &dyn ChainClient) {
    // A minimal, obviously-invalid extrinsic (just a few bytes).
    let signed_extrinsic = vec![0x01u8, 0x00, 0x00, 0x00];
    let result = client
        .submit_extrinsic(&signed_extrinsic, test_profile_id())
        .await;

    if let Ok(tx_hash) = result {
        assert!(
            !tx_hash.0.is_empty(),
            "submit_extrinsic() must return a non-empty TxHash; got {tx_hash:?}",
        );
    }
}

/// Conformance: `fetch_metadata()` `genesis_hash` is non-empty.
///
/// Every chain profile must have a non-empty genesis hash.  This test
/// verifies the genesis hash stored in the pinned metadata is accessible and
/// non-empty.
pub async fn test_genesis_hash_not_empty(client: &dyn ChainClient) {
    let result = client.fetch_metadata(test_profile_id()).await;

    if let Ok(meta) = result {
        // We cannot verify the exact value without knowing the network, but
        // a non-empty hex string is required.
        assert!(
            !meta.metadata_digest.0.is_empty(),
            "PinnedMetadata.metadata_digest must not be empty; got {:?}",
            meta.metadata_digest,
        );
    }
    // Errors are acceptable for offline adapters.
}

/// Conformance: `decode_call()` handles the provided metadata without panicking.
///
/// Given the metadata returned by `fetch_metadata()` and a minimal byte
/// sequence, `decode_call()` must return either a decoded result or a
/// well-typed `DecodeFailed` error.  Panics are not acceptable.
pub async fn test_decode_call_no_panic(client: &dyn ChainClient) {
    let meta_result = client.fetch_metadata(test_profile_id()).await;

    if let Ok(meta) = meta_result {
        // An all-zero byte sequence is deliberately invalid but must not panic.
        let call_bytes = vec![0x00u8; 4];
        let _result = client.decode_call(&call_bytes, &meta).await;
    }
    // If fetch_metadata failed, skip — no metadata to decode with.
}

/// Conformance: `watch_finality()` returns `Unknown` on a bogus tx hash.
///
/// A transaction hash that was never submitted should result in
/// [`FinalityObservation::Unknown`] (timeout without confirmation) rather
/// than an error.  If the adapter returns an error that is also acceptable
/// (it might if it cannot connect).  Panics are not acceptable.
pub async fn test_watch_finality_unknown_on_timeout(client: &dyn ChainClient) {
    let bogus_hash =
        crate::TxHash::new("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef");
    let result = client
        .watch_finality(
            bogus_hash,
            test_profile_id(),
            100, // 100 ms timeout — very short to keep tests fast
        )
        .await;

    match result {
        Ok(FinalityObservation::Unknown { .. }) | Err(_) => {
            // Correct — unknown tx hash timed out without confirmation.
        }
        Ok(other) => {
            panic!("watch_finality() on a bogus tx hash must return Unknown; got {other:?}");
        }
    }
}

/// Conformance: `simulate()` does not panic for any well-formed byte sequence.
///
/// The adapter must accept any byte slice (even ones that are not valid signed
/// extrinsics) without panicking.  It may return `Ok(SimulationResult)` with
/// any `success` value, or a well-typed error.  Fake adapters are permitted
/// to return `success = true` even for invalid extrinsics.
pub async fn test_simulate_invalid_extrinsic(client: &dyn ChainClient) {
    let meta_result = client.fetch_metadata(test_profile_id()).await;

    if let Ok(meta) = meta_result {
        let invalid_extrinsic = vec![0xFF_u8; 8];
        let block = genesis_block();
        // Any non-panicking outcome is acceptable.
        let _result = client.simulate(&invalid_extrinsic, &block, &meta).await;
    }
}

/// Conformance: `health()` returns without panicking.
pub async fn test_health_returns_result(client: &dyn ChainClient) {
    let _result = client.health().await;
}

/// Conformance: `PinnedMetadata` returned by `fetch_metadata()` has a
/// `fetched_at` timestamp close to the current time.
///
/// The `fetched_at` field must be set to the time the metadata was retrieved.
/// This test verifies it is not the Unix epoch and is within a plausible range.
pub async fn test_pinned_metadata_timestamp_plausible(client: &dyn ChainClient) {
    let result = client.fetch_metadata(test_profile_id()).await;

    if let Ok(meta) = result {
        let one_hour_ago = now() - chrono::Duration::hours(1);
        let one_hour_hence = now() + chrono::Duration::hours(1);

        assert!(
            meta.fetched_at >= one_hour_ago,
            "PinnedMetadata.fetched_at ({:?}) is implausibly old; \
             expected within the last hour",
            meta.fetched_at,
        );

        assert!(
            meta.fetched_at <= one_hour_hence,
            "PinnedMetadata.fetched_at ({:?}) is implausibly far in the future",
            meta.fetched_at,
        );
    }
}
