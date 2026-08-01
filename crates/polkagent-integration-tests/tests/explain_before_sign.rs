//! Phase 2: "Explain Before Sign" end-to-end integration tests.
//!
//! Acceptance criteria covered:
//!
//! - **AC-P2-001** — Extrinsic decoded correctly against pinned metadata.
//! - **AC-P2-003** — Signer receives exact bytes (bitwise identical).
//! - **AC-P2-004** — Stale metadata produces explicit error.
//! - **AC-P2-005** — Wrong-network extrinsic rejected before signing.
//! - **AC-P2-006** — CLI and web show identical run state.
//!
//! The test flow mirrors the production path:
//!
//! ```text
//! Raw extrinsic bytes
//!   -> decode_extrinsic (polkagent-codec)
//!   -> DecodedExtrinsic (pallet_name, call_name, args)
//!   -> FakeChainClient.decode_call() for pinned-metadata decode
//!   -> ActionCardBuilder (polkagent-card) + payload_hash
//!   -> ActionCard (canonical sections carry pallet, call, amount, dest)
//!   -> user approval (simulated in tests)
//!   -> CanonicalSignRequest (polkagent-signer-trait)
//!   -> FakeSigner.sign()
//!   -> SignedPayload (signature, public_key, signed_extrinsic)
//!   -> FakeChainClient.submit_extrinsic()
//!   -> EffectPipeline.record_outcome (polkagent-effect)
//! ```

use std::sync::Arc;

use chrono::Utc;

use polkagent_card::{
    ActionCardBuilder, RiskFlag, RiskFlagType, SectionSource, Severity,
    render::render_text,
};
use polkagent_chain_fake::FakeChainClientBuilder;
use polkagent_chain_trait::{
    BlockRef, ChainClient, ChainProfileId as ChainChainProfileId, DecodedCall,
    MetadataDigest as ChainMetadataDigest, PinnedMetadata,
};
use polkagent_codec::{
    call::{
        call_index, decode_batch_call, extract_transfer_amount, is_batch_call,
        is_transfer_call, pallet_index,
    },
    decode::decode_extrinsic,
    scale::ScaleEncoder,
};
use polkagent_core::{
    EffectAttemptId, EffectOutcomeId, RunId, StepId, TurnId,
};
use polkagent_effect::{
    EffectIntentSpec, EffectKind, EffectOutcome, OutcomeResult, ResolutionHint,
};
use polkagent_metadata::{
    ChainId, MetadataService, MetadataSnapshot, MetadataVersion,
    validate_network, MetadataError,
};
use polkagent_signer_fake::FakeSigner;
use polkagent_signer_trait::{
    AccountRef, ApprovalId, CanonicalSignRequest, ChainProfileId, GrantDigest,
    MetadataDigest, Signer,
};
use polkagent_store_trait::{RunStatus, RunStore};

use polkagent_integration_tests::{make_effect_pipeline, MemRunStore};

// ---------------------------------------------------------------------------
// Shared fixtures and helpers
// ---------------------------------------------------------------------------

/// Construct a minimal unsigned extrinsic:
/// `[compact(total_len)] [version=0x04] [pallet] [call] [args...]`
fn build_unsigned_extrinsic(pallet: u8, call: u8, args: &[u8]) -> Vec<u8> {
    let mut payload = vec![0x04u8, pallet, call];
    payload.extend_from_slice(args);

    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u32(payload.len() as u32);
    let mut out = enc.finish();
    out.extend(payload);
    out
}

/// Construct a `Balances.transferKeepAlive` extrinsic:
/// - dest: `MultiAddress::Id([0u8; 31, 0x01])`
/// - value: compact(1_000_000_000_000) = 1 DOT
fn transfer_keep_alive_fixture() -> Vec<u8> {
    let dest = {
        let mut b = vec![0x00u8]; // MultiAddress::Id
        b.extend_from_slice(&[0u8; 31]);
        b.push(0x01);
        b
    };
    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u64(1_000_000_000_000u64); // 1 DOT in planck
    let value_bytes = enc.finish();

    let mut args = dest;
    args.extend(value_bytes);
    build_unsigned_extrinsic(
        pallet_index::BALANCES,
        call_index::BALANCES_TRANSFER_KEEP_ALIVE,
        &args,
    )
}

/// Construct a `Balances.transfer` extrinsic (pallet 5, call 0):
/// - dest: `MultiAddress::Id([0xAA; 32])`
/// - value: compact(5_000_000_000_000) = 5 DOT
fn transfer_fixture() -> Vec<u8> {
    let dest = {
        let mut b = vec![0x00u8]; // MultiAddress::Id
        b.extend_from_slice(&[0xAAu8; 32]);
        b
    };
    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u64(5_000_000_000_000u64); // 5 DOT in planck
    let value_bytes = enc.finish();

    let mut args = dest;
    args.extend(value_bytes);
    build_unsigned_extrinsic(
        pallet_index::BALANCES,
        call_index::BALANCES_TRANSFER,
        &args,
    )
}

/// Compute a BLAKE3 hex hash for a byte slice.
fn blake3_hex(data: &[u8]) -> String {
    let hash = blake3::hash(data);
    let bytes = hash.as_bytes();
    bytes
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            s.push_str(&format!("{b:02x}"));
            s
        })
}

/// Build a `CanonicalSignRequest` from a payload, using test-only values.
fn canonical_request(payload: Vec<u8>) -> CanonicalSignRequest {
    CanonicalSignRequest {
        request_id: "explain-before-sign-test".into(),
        payload,
        account: AccountRef::from_bytes([0u8; 32]),
        chain_profile: ChainProfileId::new("polkadot"),
        metadata_hash: MetadataDigest(vec![0xAB; 32]),
        grant_digest: GrantDigest(vec![0xCD; 32]),
        approval_id: ApprovalId::new("approval-explain-before-sign"),
        expires_at: polkagent_core::now() + chrono::Duration::hours(1),
    }
}

/// Build fake pinned metadata for a FakeChainClient.
fn fake_pinned_metadata() -> PinnedMetadata {
    PinnedMetadata {
        chain_profile: ChainChainProfileId::new("polkadot"),
        spec_version: 1_003_000,
        metadata_digest: ChainMetadataDigest("0xdeadbeef".into()),
        metadata_bytes: vec![0u8; 32],
        block_ref: BlockRef {
            number: 1_000_000,
            hash: "0xabc".into(),
        },
        fetched_at: polkagent_core::now(),
    }
}

// ===========================================================================
// AC-P2-001 — Extrinsic decoded correctly against pinned metadata
// ===========================================================================

/// Encode a known Balances.transfer call, decode it using codec,
/// and verify the decoded pallet index and call index match.
#[test]
fn ac_p2_001_decode_transfer_pallet_and_call() {
    let bytes = transfer_fixture();
    let ext = decode_extrinsic(&bytes).expect("decode_extrinsic must succeed");

    assert_eq!(
        ext.pallet_index,
        pallet_index::BALANCES,
        "pallet_index must be Balances (5)"
    );
    assert_eq!(
        ext.call_index,
        call_index::BALANCES_TRANSFER,
        "call_index must be transfer (0)"
    );
}

/// Decode a transfer_keep_alive call and verify it is correctly identified by
/// the call helpers.
#[test]
fn ac_p2_001_decode_transfer_keep_alive_identified() {
    let bytes = transfer_keep_alive_fixture();
    let ext = decode_extrinsic(&bytes).expect("decode ok");

    assert!(
        is_transfer_call(&ext),
        "must be recognised as a transfer call"
    );
    assert_eq!(ext.pallet_index, pallet_index::BALANCES);
    assert_eq!(ext.call_index, call_index::BALANCES_TRANSFER_KEEP_ALIVE);
}

/// Decode a known call using FakeChainClient.decode_call() and verify the
/// decoded pallet name and call name match.
#[tokio::test]
async fn ac_p2_001_decode_via_fake_chain_client() {
    let client = FakeChainClientBuilder::polkadot().build();
    let metadata = fake_pinned_metadata();

    let call_bytes = vec![
        pallet_index::BALANCES,
        call_index::BALANCES_TRANSFER_KEEP_ALIVE,
        0x00, 0x01, 0x02, 0x03,
    ];

    let decoded: DecodedCall = client
        .decode_call(&call_bytes, &metadata)
        .await
        .expect("decode_call must succeed");

    assert_eq!(decoded.pallet, "Balances", "pallet must be Balances");
    assert_eq!(
        decoded.call_name, "transfer_keep_alive",
        "call_name must be transfer_keep_alive"
    );
    assert!(
        !decoded.arguments_json.is_empty(),
        "arguments_json must be non-empty"
    );
}

/// Verify that decoded arguments from FakeChainClient include the call bytes
/// hex in the JSON payload.
#[tokio::test]
async fn ac_p2_001_decode_call_arguments_contain_call_data() {
    let client = FakeChainClientBuilder::polkadot().build();
    let metadata = fake_pinned_metadata();

    let call_bytes = vec![0x05, 0x03, 0xDE, 0xAD, 0xBE, 0xEF];
    let decoded = client
        .decode_call(&call_bytes, &metadata)
        .await
        .expect("decode ok");

    // FakeChainClient embeds the first 8 bytes as hex in arguments_json
    // with a "0x" prefix.
    assert!(
        decoded.arguments_json.contains("0x0503deadbeef"),
        "arguments_json must contain hex of the call bytes"
    );
    // Also verify length is reported.
    assert!(
        decoded.arguments_json.contains(&call_bytes.len().to_string()),
        "arguments_json must report the call byte length"
    );
}

/// Build an action card from decoded extrinsic data and verify canonical fields
/// are populated correctly.
#[test]
fn ac_p2_001_action_card_canonical_fields_from_decode() {
    let bytes = transfer_keep_alive_fixture();
    let ext = decode_extrinsic(&bytes).expect("decode");

    let amount = extract_transfer_amount(&ext);
    assert!(amount.is_some(), "extract_transfer_amount must succeed");
    let amount_value = amount.unwrap();
    assert_eq!(amount_value, 1_000_000_000_000u128);

    let pallet_name = "Balances";
    let call_name = "transfer_keep_alive";
    let payload_hash = blake3_hex(&bytes);

    let card = ActionCardBuilder::new("Transfer 1 DOT")
        .add_canonical("Pallet", pallet_name, SectionSource::Metadata)
        .add_canonical("Call", call_name, SectionSource::Metadata)
        .add_canonical(
            "Amount",
            &format!("{amount_value} planck"),
            SectionSource::Metadata,
        )
        .with_payload_hash(payload_hash.clone())
        .build();

    assert_eq!(card.canonical_sections[0].value, "Balances");
    assert_eq!(card.canonical_sections[1].value, "transfer_keep_alive");
    assert!(card.canonical_sections[2].value.contains("1000000000000"));
    assert_eq!(card.payload_hash, payload_hash);
}

/// Narrative sections always come after canonical sections, regardless of
/// builder call order, and always carry the AI disclaimer.
#[test]
fn ac_p2_001_narrative_after_canonical_with_disclaimer() {
    let card = ActionCardBuilder::new("Transfer 1 DOT")
        .add_narrative("Why?", "Agent rebalancing staking position.")
        .add_canonical("Pallet", "Balances", SectionSource::Metadata)
        .add_canonical("Call", "transfer_keep_alive", SectionSource::Metadata)
        .with_payload_hash("abc123")
        .build();

    assert_eq!(card.canonical_sections.len(), 2);
    assert_eq!(card.narrative_sections.len(), 1);
    assert_eq!(card.canonical_sections[0].label, "Pallet");
    assert_eq!(
        card.narrative_sections[0].disclaimer,
        "AI-generated explanation"
    );
}

// ===========================================================================
// AC-P2-003 — Signer receives exact bytes (bitwise identical)
// ===========================================================================

/// Create call bytes, pass through the decode pipeline, and verify the bytes
/// handed to the signer are bitwise identical to the original.
#[tokio::test]
async fn ac_p2_003_signer_receives_bitwise_identical_bytes() {
    let raw_bytes = transfer_keep_alive_fixture();

    // Decode to verify structure (does not modify the bytes).
    let ext = decode_extrinsic(&raw_bytes).expect("decode");
    assert!(is_transfer_call(&ext));

    // Build the sign request with the original bytes.
    let sign_request = canonical_request(raw_bytes.clone());

    let signer = FakeSigner::new();
    let signed = signer.sign(sign_request).await.expect("sign ok");

    // The FakeSigner constructs signed_extrinsic = payload || signature.
    let payload_from_signed = &signed.signed_extrinsic[..raw_bytes.len()];
    assert_eq!(
        payload_from_signed, &raw_bytes[..],
        "bytes in signed_extrinsic must be bitwise identical to original (AC-P2-003)"
    );
}

/// BLAKE3 hash of the bytes the signer receives matches the card's
/// payload_hash.
#[tokio::test]
async fn ac_p2_003_hash_binding_matches_card() {
    let raw_bytes = transfer_keep_alive_fixture();
    let expected_hash = blake3_hex(&raw_bytes);

    let card = ActionCardBuilder::new("Transfer 1 DOT")
        .add_canonical("Pallet", "Balances", SectionSource::Metadata)
        .with_payload_hash(expected_hash.clone())
        .build();

    let sign_request = canonical_request(raw_bytes.clone());
    let signer = FakeSigner::new();
    let signed = signer.sign(sign_request).await.expect("sign ok");

    let payload_from_signed = &signed.signed_extrinsic[..raw_bytes.len()];
    let hash_from_signed = blake3_hex(payload_from_signed);
    assert_eq!(
        hash_from_signed, card.payload_hash,
        "BLAKE3 hash from signed payload must match card.payload_hash"
    );
}

/// Tampering with one byte produces a different BLAKE3 hash.
#[test]
fn ac_p2_003_tampered_bytes_produce_different_hash() {
    let raw_bytes = transfer_keep_alive_fixture();
    let original_hash = blake3_hex(&raw_bytes);

    let mut tampered = raw_bytes.clone();
    if let Some(b) = tampered.last_mut() {
        *b = b.wrapping_add(1);
    }
    let tampered_hash = blake3_hex(&tampered);

    assert_ne!(
        original_hash, tampered_hash,
        "tampered bytes must produce a different hash"
    );
}

/// The signer records the exact request it was called with and the payload
/// matches.
#[tokio::test]
async fn ac_p2_003_signer_records_exact_request() {
    let raw_bytes = transfer_keep_alive_fixture();
    let sign_request = canonical_request(raw_bytes.clone());
    let request_id = sign_request.request_id.clone();

    let signer = FakeSigner::new();
    signer.sign(sign_request).await.expect("sign ok");

    let captured = signer.last_request().expect("request must be recorded");
    assert_eq!(captured.request_id, request_id);
    assert_eq!(
        captured.payload, raw_bytes,
        "signer must have received the exact extrinsic bytes"
    );
}

/// Multiple sign calls with different payloads each receive the correct bytes.
#[tokio::test]
async fn ac_p2_003_multiple_sign_calls_each_receive_correct_bytes() {
    let bytes_a = transfer_keep_alive_fixture();
    let bytes_b = transfer_fixture();

    let signer = FakeSigner::new();

    let req_a = canonical_request(bytes_a.clone());
    let signed_a = signer.sign(req_a).await.expect("sign A");
    assert_eq!(
        &signed_a.signed_extrinsic[..bytes_a.len()],
        &bytes_a[..],
        "first sign call must receive bytes_a"
    );

    let req_b = canonical_request(bytes_b.clone());
    let signed_b = signer.sign(req_b).await.expect("sign B");
    assert_eq!(
        &signed_b.signed_extrinsic[..bytes_b.len()],
        &bytes_b[..],
        "second sign call must receive bytes_b"
    );

    assert_eq!(signer.sign_call_count(), 2);
}

// ===========================================================================
// AC-P2-004 — Stale metadata produces explicit error
// ===========================================================================

/// Create metadata with version N, then register version N+1. The
/// MetadataService must detect the drift.
#[test]
fn ac_p2_004_stale_metadata_drift_detection() {
    let svc = MetadataService::new();
    let chain = ChainId::new("polkadot");

    // Register and pin "v1" metadata (version N).
    let snap_v1 = MetadataSnapshot::new(
        chain.clone(),
        MetadataVersion::V14,
        b"polkadot-runtime-v1".to_vec(),
        polkagent_core::now(),
        1_000_000,
    );
    let v1_hash = snap_v1.hash.clone();
    assert!(
        svc.register_snapshot(snap_v1).is_none(),
        "no pins yet, so no drift"
    );
    svc.pin_current(&chain, "v1.0.0").expect("pin v1 ok");

    // Runtime upgrade: register "v2" metadata (version N+1).
    let snap_v2 = MetadataSnapshot::new(
        chain.clone(),
        MetadataVersion::V14,
        b"polkadot-runtime-v2-upgraded".to_vec(),
        polkagent_core::now(),
        1_001_000,
    );
    let v2_hash = snap_v2.hash.clone();
    let drift = svc.register_snapshot(snap_v2);

    // Drift must be detected.
    assert!(
        drift.is_some(),
        "MetadataService must detect drift after runtime upgrade (AC-P2-004)"
    );
    let d = drift.unwrap();
    assert_eq!(d.chain_id, chain);
    assert_eq!(d.pinned_hash, v1_hash, "pinned hash must be v1");
    assert_eq!(d.current_hash, v2_hash, "current hash must be v2");
}

/// When no metadata is cached for a chain, `is_stale` returns true.
#[test]
fn ac_p2_004_absent_metadata_is_stale() {
    let svc = MetadataService::new();
    let chain = ChainId::new("polkadot");

    assert!(
        svc.is_stale(&chain, std::time::Duration::from_secs(3600)),
        "absent metadata must be treated as stale (AC-P2-004)"
    );
}

/// Drift record carries enough identifying information to block signing.
#[test]
fn ac_p2_004_drift_carries_identifying_info() {
    let svc = MetadataService::new();
    let chain = ChainId::new("polkadot");

    let snap_v1 = MetadataSnapshot::new(
        chain.clone(),
        MetadataVersion::V14,
        b"meta-version-A".to_vec(),
        polkagent_core::now(),
        9_000,
    );
    svc.register_snapshot(snap_v1);
    svc.pin_current(&chain, "vA").expect("pin");

    let snap_v2 = MetadataSnapshot::new(
        chain.clone(),
        MetadataVersion::V14,
        b"meta-version-B-upgraded".to_vec(),
        polkagent_core::now(),
        9_001,
    );
    let drift = svc
        .register_snapshot(snap_v2)
        .expect("drift must be detected");

    assert_eq!(drift.chain_id.0, "polkadot");
    assert!(!drift.pinned_hash.0.is_empty());
    assert!(!drift.current_hash.0.is_empty());
    assert_ne!(
        drift.pinned_hash, drift.current_hash,
        "stale metadata hashes must differ"
    );
}

/// FakeChainClient decode_call with fault injection produces an explicit error.
#[tokio::test]
async fn ac_p2_004_fake_chain_decode_call_fault_returns_explicit_error() {
    let client = FakeChainClientBuilder::polkadot()
        .fail_next_n(1)
        .build();
    let metadata = fake_pinned_metadata();
    let call_bytes = vec![0x05, 0x03, 0x00, 0x01];

    let result = client.decode_call(&call_bytes, &metadata).await;
    assert!(
        result.is_err(),
        "decode_call with fault injection must fail with explicit error"
    );
    let err = result.unwrap_err();
    let err_msg = format!("{err}");
    assert!(
        err_msg.contains("simulated fault") || err_msg.contains("RPC"),
        "error message must indicate the failure reason"
    );
}

// ===========================================================================
// AC-P2-005 — Wrong-network extrinsic rejected before signing
// ===========================================================================

/// Create an extrinsic for Polkadot genesis hash, try to submit to a
/// Kusama-configured chain, and verify rejection with a wrong-network error.
#[test]
fn ac_p2_005_wrong_network_genesis_hash_mismatch() {
    // Polkadot genesis hash (from polkagent-chain-fake builder).
    let polkadot_genesis: [u8; 32] = [
        0x91, 0xb1, 0x71, 0xbb, 0x15, 0x8e, 0x2d, 0x38, 0x48, 0xfa, 0x23,
        0xa9, 0x7b, 0x64, 0x48, 0x77, 0x58, 0x08, 0x55, 0x00, 0x7d, 0x04,
        0x47, 0x87, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    // Kusama genesis hash (from polkagent-chain-fake builder).
    let kusama_genesis: [u8; 32] = [
        0xb0, 0xa8, 0xd4, 0x93, 0x28, 0x5c, 0x2d, 0xf7, 0x32, 0x90, 0xdf,
        0xb7, 0xe6, 0x1f, 0x87, 0x0f, 0x17, 0xb4, 0x18, 0x01, 0x19, 0x7a,
        0x14, 0x9c, 0xa9, 0x36, 0x54, 0x99, 0x9e, 0xbc, 0xae, 0x88,
    ];

    // Extrinsic was built for Polkadot; try to validate against Kusama.
    let result = validate_network(
        "kusama",
        &polkadot_genesis,  // extrinsic genesis
        &kusama_genesis,    // expected genesis (Kusama)
    );

    assert!(
        result.is_err(),
        "extrinsic for Polkadot must be rejected when validated against Kusama"
    );
    let err = result.unwrap_err();
    let err_msg = format!("{err}");
    assert!(
        err_msg.contains("kusama"),
        "error must mention the expected chain name"
    );
    assert!(
        matches!(err, MetadataError::WrongNetwork { .. }),
        "error must be WrongNetwork variant (AC-P2-005)"
    );
}

/// Validating the correct genesis hash succeeds.
#[test]
fn ac_p2_005_correct_network_genesis_hash_accepted() {
    let polkadot_genesis: [u8; 32] = [
        0x91, 0xb1, 0x71, 0xbb, 0x15, 0x8e, 0x2d, 0x38, 0x48, 0xfa, 0x23,
        0xa9, 0x7b, 0x64, 0x48, 0x77, 0x58, 0x08, 0x55, 0x00, 0x7d, 0x04,
        0x47, 0x87, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    let result = validate_network(
        "polkadot",
        &polkadot_genesis,
        &polkadot_genesis,
    );
    assert!(
        result.is_ok(),
        "matching genesis hashes must pass validation"
    );
}

/// The MetadataService drift mechanism detects wrong-network metadata
/// masquerading as the target chain.
#[test]
fn ac_p2_005_wrong_network_detected_via_metadata_drift() {
    let svc = MetadataService::new();
    let polkadot = ChainId::new("polkadot");

    // Pin Polkadot metadata.
    let polkadot_meta = MetadataSnapshot::new(
        polkadot.clone(),
        MetadataVersion::V14,
        b"polkadot-real-metadata".to_vec(),
        polkagent_core::now(),
        1_003_000,
    );
    svc.register_snapshot(polkadot_meta);
    svc.pin_current(&polkadot, "polkadot-v1").expect("pin");

    // Attempt to register Kusama metadata in the Polkadot chain slot.
    let kusama_meta_as_polkadot = MetadataSnapshot::new(
        polkadot.clone(), // wrong: Kusama masquerading as Polkadot
        MetadataVersion::V14,
        b"kusama-different-genesis-metadata".to_vec(),
        polkagent_core::now(),
        1_000_000,
    );
    let drift = svc.register_snapshot(kusama_meta_as_polkadot);

    assert!(
        drift.is_some(),
        "wrong-network metadata must be detected as drift"
    );
    let d = drift.unwrap();
    assert_eq!(d.chain_id, polkadot);
    assert_ne!(d.pinned_hash, d.current_hash);
}

/// Independent chains do not affect each other's drift state.
#[test]
fn ac_p2_005_independent_chains_isolated() {
    let svc = MetadataService::new();
    let polkadot = ChainId::new("polkadot");
    let kusama = ChainId::new("kusama");

    let pdot = MetadataSnapshot::new(
        polkadot.clone(),
        MetadataVersion::V14,
        b"pdot-meta".to_vec(),
        polkagent_core::now(),
        1_003_000,
    );
    svc.register_snapshot(pdot);
    svc.pin_current(&polkadot, "pdot-v1").expect("pin");

    // Register Kusama metadata (separate chain slot).
    let ksm = MetadataSnapshot::new(
        kusama.clone(),
        MetadataVersion::V14,
        b"ksm-meta".to_vec(),
        polkagent_core::now(),
        1_000_000,
    );
    svc.register_snapshot(ksm);

    // Polkadot's drift state must be unaffected.
    assert!(
        svc.check_drift(&polkadot).is_none(),
        "Polkadot drift must be unaffected by Kusama registration"
    );
    assert!(
        svc.check_drift(&kusama).is_none(),
        "Kusama with no pins must have no drift"
    );
}

/// A rejecting signer returns UserRejected when the user declines signing.
#[tokio::test]
async fn ac_p2_005_user_rejects_signing_returns_explicit_error() {
    let raw_bytes = transfer_keep_alive_fixture();
    let sign_request = canonical_request(raw_bytes);

    let signer = FakeSigner::rejecting();
    let result = signer.sign(sign_request).await;
    assert!(
        matches!(
            result,
            Err(polkagent_signer_trait::SignerError::UserRejected)
        ),
        "rejecting signer must return UserRejected"
    );
}

// ===========================================================================
// AC-P2-006 — CLI and web show identical run state
// ===========================================================================

/// Create a run via the store layer, query state through RunStore, and verify
/// the state representation is identical regardless of access path.
#[tokio::test]
async fn ac_p2_006_run_state_identical_via_different_access_paths() {
    let store = Arc::new(MemRunStore::default());
    let run_id = RunId::new();
    let agent_id = "agent-test-001";

    // Create a run with "queued" status.
    store
        .create(run_id, agent_id, RunStatus::new("queued"))
        .await
        .expect("create run");

    // Access path 1: get by run_id.
    let summary_by_id = store.get(run_id).await.expect("get by id");

    // Access path 2: list by agent.
    let runs_by_agent = store
        .list_by_agent(agent_id, 100, 0)
        .await
        .expect("list by agent");
    let summary_by_agent = &runs_by_agent[0];

    // Access path 3: list by state.
    let runs_by_state = store
        .list_by_state(RunStatus::new("queued"), 100, 0)
        .await
        .expect("list by state");
    let summary_by_state = runs_by_state
        .iter()
        .find(|r| r.id == run_id)
        .expect("run must appear in state listing");

    // Verify all three access paths return identical state.
    assert_eq!(
        summary_by_id.status, summary_by_agent.status,
        "get-by-id and list-by-agent must return the same status"
    );
    assert_eq!(
        summary_by_id.status, summary_by_state.status,
        "get-by-id and list-by-state must return the same status"
    );
    assert_eq!(
        summary_by_id.agent_id, summary_by_agent.agent_id,
        "agent_id must be identical across access paths"
    );
    assert_eq!(
        summary_by_id.id, summary_by_state.id,
        "run_id must be identical across access paths"
    );
}

/// Verify that state transitions are visible through all query paths.
#[tokio::test]
async fn ac_p2_006_state_transition_visible_through_all_paths() {
    let store = Arc::new(MemRunStore::default());
    let run_id = RunId::new();
    let agent_id = "agent-transition";

    store
        .create(run_id, agent_id, RunStatus::new("queued"))
        .await
        .expect("create");

    // Transition to "executing".
    store
        .update_state(run_id, RunStatus::new("executing"))
        .await
        .expect("update to executing");

    // All access paths must reflect "executing".
    let by_id = store.get(run_id).await.expect("get");
    assert_eq!(by_id.status.as_str(), "executing");

    let by_agent = store
        .list_by_agent(agent_id, 10, 0)
        .await
        .expect("list by agent");
    assert_eq!(by_agent[0].status.as_str(), "executing");

    let by_state = store
        .list_by_state(RunStatus::new("executing"), 10, 0)
        .await
        .expect("list by state");
    assert!(
        by_state.iter().any(|r| r.id == run_id),
        "run must appear in 'executing' state list"
    );

    // The old state must no longer contain this run.
    let old_state = store
        .list_by_state(RunStatus::new("queued"), 10, 0)
        .await
        .expect("list old state");
    assert!(
        !old_state.iter().any(|r| r.id == run_id),
        "run must not appear in 'queued' state list after transition"
    );
}

/// The JSON serialization of RunSummary is identical regardless of access path.
#[tokio::test]
async fn ac_p2_006_json_representation_identical() {
    let store = Arc::new(MemRunStore::default());
    let run_id = RunId::new();
    let agent_id = "agent-json";

    store
        .create(run_id, agent_id, RunStatus::new("running"))
        .await
        .expect("create");

    let by_id = store.get(run_id).await.expect("get");
    let by_agent = store
        .list_by_agent(agent_id, 10, 0)
        .await
        .expect("list")
        .into_iter()
        .find(|r| r.id == run_id)
        .expect("found");

    let json_by_id = serde_json::to_string(&by_id).expect("serialize by-id");
    let json_by_agent =
        serde_json::to_string(&by_agent).expect("serialize by-agent");

    assert_eq!(
        json_by_id, json_by_agent,
        "JSON serialization must be identical across access paths (AC-P2-006)"
    );
}

// ===========================================================================
// Additional edge case tests
// ===========================================================================

/// Signing a request that has already expired is rejected before signing.
#[tokio::test]
async fn expired_sign_request_rejected() {
    let raw_bytes = transfer_keep_alive_fixture();
    let mut req = canonical_request(raw_bytes);
    req.expires_at = polkagent_core::now() - chrono::Duration::seconds(10);

    let signer = FakeSigner::new();
    let result = signer.sign(req).await;
    assert!(
        matches!(
            result,
            Err(polkagent_signer_trait::SignerError::Expired { .. })
        ),
        "expired request must be rejected"
    );
    assert_eq!(signer.sign_call_count(), 1);
}

/// Multiple accounts sign different requests; each result's public key matches
/// the signing account.
#[tokio::test]
async fn multiple_accounts_sign_independently() {
    let account_a = AccountRef::from_bytes([0xAA; 32]);
    let account_b = AccountRef::from_bytes([0xBB; 32]);

    let signer = FakeSigner::with_accounts(vec![
        account_a.clone(),
        account_b.clone(),
    ]);

    let raw_bytes = transfer_keep_alive_fixture();

    let mut req_a = canonical_request(raw_bytes.clone());
    req_a.account = account_a.clone();

    let mut req_b = canonical_request(raw_bytes.clone());
    req_b.account = account_b.clone();

    let signed_a = signer.sign(req_a).await.expect("sign A");
    let signed_b = signer.sign(req_b).await.expect("sign B");

    assert_eq!(signed_a.public_key, account_a.account_id.to_vec());
    assert_eq!(signed_b.public_key, account_b.account_id.to_vec());
    assert_ne!(signed_a.public_key, signed_b.public_key);
    assert_eq!(signer.sign_call_count(), 2);
}

/// Risk flags on an action card drive the risk level inference correctly.
#[test]
fn risk_flags_drive_risk_level() {
    let bytes = transfer_keep_alive_fixture();
    let payload_hash = blake3_hex(&bytes);

    let card = ActionCardBuilder::new("Transfer 100 DOT")
        .add_canonical("Pallet", "Balances", SectionSource::Metadata)
        .add_canonical("Call", "transfer_keep_alive", SectionSource::Metadata)
        .add_risk_flag(RiskFlag::new(
            RiskFlagType::HighValue,
            "Amount exceeds 50 DOT threshold",
            Severity::High,
        ))
        .add_risk_flag(RiskFlag::new(
            RiskFlagType::FirstTimeRecipient,
            "Recipient not seen before",
            Severity::Medium,
        ))
        .with_payload_hash(payload_hash)
        .build();

    assert_eq!(card.risk_level, polkagent_card::RiskLevel::High);
    assert_eq!(card.risk_flags.len(), 2);

    // Serialise for audit.
    let json = serde_json::to_string(&card).expect("serialize");
    assert!(json.contains("high_value"));
    assert!(json.contains("Balances"));
}

/// Action card expiry check works correctly before and after the deadline.
#[test]
fn action_card_expiry_check() {
    use chrono::Duration;

    let bytes = transfer_keep_alive_fixture();
    let past = Utc::now() - Duration::seconds(60);
    let future = Utc::now() + Duration::seconds(300);

    let expired_card = ActionCardBuilder::new("Transfer")
        .with_payload_hash(blake3_hex(&bytes))
        .with_expires_at(past)
        .build();

    let valid_card = ActionCardBuilder::new("Transfer")
        .with_payload_hash(blake3_hex(&bytes))
        .with_expires_at(future)
        .build();

    assert!(expired_card.is_expired(Utc::now()));
    assert!(!valid_card.is_expired(Utc::now()));
}

/// Batch call is fully decoded with inner call count verified.
#[test]
fn batch_call_fully_decoded() {
    let make_inner = |dest_byte: u8, amount: u64| -> Vec<u8> {
        let mut dest = vec![0x00u8];
        dest.extend_from_slice(&[0u8; 31]);
        dest.push(dest_byte);

        let mut enc = ScaleEncoder::new();
        enc.encode_compact_u64(amount);
        let value_bytes = enc.finish();

        let mut bare =
            vec![pallet_index::BALANCES, call_index::BALANCES_TRANSFER_KEEP_ALIVE];
        bare.extend(dest);
        bare.extend(value_bytes);
        bare
    };

    let inner1 = make_inner(0x01, 1_000_000_000_000);
    let inner2 = make_inner(0x02, 2_000_000_000_000);

    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u32(2);
    enc.encode_bytes(&inner1);
    enc.encode_bytes(&inner2);
    let batch_args = enc.finish();

    let bytes = build_unsigned_extrinsic(
        pallet_index::UTILITY,
        call_index::UTILITY_BATCH,
        &batch_args,
    );

    let ext = decode_extrinsic(&bytes).expect("decode batch");
    assert!(is_batch_call(&ext));

    let inner_calls = decode_batch_call(&ext).expect("decode inner calls");
    assert_eq!(inner_calls.len(), 2);

    for inner in &inner_calls {
        assert!(is_transfer_call(inner));
    }
}

/// Full end-to-end lifecycle: decode -> card -> sign -> pipeline -> outcome.
#[tokio::test]
async fn full_lifecycle_intent_to_outcome() {
    // Step 1: Decode.
    let raw_bytes = transfer_keep_alive_fixture();
    let ext = decode_extrinsic(&raw_bytes).expect("decode");
    assert_eq!(ext.pallet_index, pallet_index::BALANCES);

    // Step 2: Build action card.
    let payload_hash = blake3_hex(&raw_bytes);
    let card = ActionCardBuilder::new("Transfer 1 DOT")
        .add_canonical("Pallet", "Balances", SectionSource::Metadata)
        .add_canonical("Call", "transfer_keep_alive", SectionSource::Metadata)
        .with_payload_hash(payload_hash.clone())
        .build();

    assert!(!card.card_id.is_empty());
    assert_eq!(card.payload_hash, payload_hash);

    // Step 3: Sign.
    let approval_id = ApprovalId::new("approval-lifecycle-001");
    let sign_request = CanonicalSignRequest {
        request_id: "lifecycle-sign-001".into(),
        payload: raw_bytes.clone(),
        account: AccountRef::from_bytes([0u8; 32]),
        chain_profile: ChainProfileId::new("polkadot"),
        metadata_hash: MetadataDigest(vec![0xAB; 32]),
        grant_digest: GrantDigest(vec![0xCD; 32]),
        approval_id: approval_id.clone(),
        expires_at: polkagent_core::now() + chrono::Duration::hours(1),
    };

    let signer = FakeSigner::new();
    let signed = signer.sign(sign_request).await.expect("sign ok");
    assert!(!signed.signature.is_empty());
    assert_eq!(signed.public_key.len(), 32);

    // Step 4: Propose effect intent.
    let (pipeline, store) = make_effect_pipeline();
    let run_id = RunId::new();
    let intent_id = pipeline
        .propose(EffectIntentSpec {
            run_id,
            turn_id: TurnId::new(),
            step_id: StepId::new(),
            kind: EffectKind::SignatureRequest,
            sequence: 1,
            idempotency_key: None,
            payload: serde_json::json!({
                "card_id": card.card_id,
                "payload_hash": card.payload_hash,
                "approval_id": approval_id.0,
            }),
            retry_class: None,
            priority: None,
            max_attempts: None,
            action_card: None,
        })
        .await
        .expect("propose");

    // Intent must start in "pending" state.
    {
        let intents = store.intents.lock().expect("lock");
        let intent = intents.get(&intent_id).expect("exists");
        assert_eq!(intent.state, "pending");
    }

    // Step 5: Claim.
    let guard = pipeline
        .claim()
        .await
        .expect("claim ok")
        .expect("must find pending");

    // Step 6: Record outcome.
    let outcome = EffectOutcome {
        id: EffectOutcomeId::new(),
        attempt_id: EffectAttemptId::new(),
        intent_id,
        run_id,
        result: OutcomeResult::Success {
            data: serde_json::json!({
                "tx_hash": "0xdeadbeef",
                "signed_extrinsic_len": signed.signed_extrinsic.len(),
            }),
        },
        observed_at: Utc::now(),
        digest: [0u8; 32],
    };

    pipeline
        .record_outcome(intent_id, &outcome)
        .await
        .expect("record_outcome ok");
    let _ = guard.complete();

    // Step 7: Verify resolved.
    {
        let intents = store.intents.lock().expect("lock");
        let intent = intents.get(&intent_id).expect("exists");
        assert_eq!(intent.state, "resolved");
    }

    let outcomes = store.outcomes.lock().expect("lock");
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].intent_id, intent_id);
    assert_eq!(outcomes[0].run_id, run_id);
}

/// MetadataService pin_and_retrieve workflow.
#[test]
fn metadata_service_pin_and_retrieve() {
    let svc = MetadataService::new();
    let chain = ChainId::new("polkadot");

    let snap = MetadataSnapshot::new(
        chain.clone(),
        MetadataVersion::V14,
        b"runtime-v42".to_vec(),
        polkagent_core::now(),
        42_000,
    );
    let expected_hash = snap.hash.clone();

    svc.register_snapshot(snap);
    svc.pin_current(&chain, "v42").expect("pin ok");

    let pins = svc.get_pinned(&chain);
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0].hash, expected_hash);
    assert_eq!(pins[0].label, "v42");
    assert!(pins[0].trusted);
}

/// Unknown outcome is preserved and still resolves the intent.
#[tokio::test]
async fn unknown_outcome_preserved_and_resolves_intent() {
    let (pipeline, store) = make_effect_pipeline();
    let run_id = RunId::new();

    let intent_id = pipeline
        .propose(EffectIntentSpec {
            run_id,
            turn_id: TurnId::new(),
            step_id: StepId::new(),
            kind: EffectKind::Broadcast,
            sequence: 1,
            idempotency_key: None,
            payload: serde_json::json!({"tx": "0xdeadbeef"}),
            retry_class: None,
            priority: None,
            max_attempts: None,
            action_card: None,
        })
        .await
        .expect("propose");

    let _guard = pipeline.claim().await.expect("claim").expect("guard");

    let outcome = EffectOutcome {
        id: EffectOutcomeId::new(),
        attempt_id: EffectAttemptId::new(),
        intent_id,
        run_id,
        result: OutcomeResult::Unknown {
            context: "finality not observed within timeout".into(),
            resolution_hint: ResolutionHint::CheckChain,
        },
        observed_at: Utc::now(),
        digest: [0u8; 32],
    };

    pipeline
        .record_outcome(intent_id, &outcome)
        .await
        .expect("record Unknown outcome");

    let intents = store.intents.lock().expect("lock");
    let stored = intents.get(&intent_id).expect("intent");
    assert_eq!(
        stored.state, "resolved",
        "Unknown outcome must still resolve the intent"
    );
}

/// The render_text output for an action card contains all key elements for
/// both CLI and web consumption (AC-P2-006: same data regardless of surface).
#[test]
fn ac_p2_006_render_text_contains_all_key_elements() {
    let card = ActionCardBuilder::new("Transfer 10 DOT")
        .add_canonical("Pallet", "Balances", SectionSource::Metadata)
        .add_canonical("Amount", "10 DOT", SectionSource::Metadata)
        .add_narrative("Why?", "Rebalancing staking position.")
        .add_risk_flag(RiskFlag::new(
            RiskFlagType::FirstTimeRecipient,
            "Recipient not seen before",
            Severity::Medium,
        ))
        .with_payload_hash("cafebabe1234")
        .build();

    let text = render_text(&card);

    // Verify all key elements are present in the text rendering.
    assert!(text.contains("Transfer 10 DOT"), "title must appear");
    assert!(text.contains("CANONICAL DATA"), "canonical marker");
    assert!(text.contains("Balances"), "pallet name");
    assert!(text.contains("10 DOT"), "amount");
    assert!(text.contains("AI-GENERATED"), "narrative marker");
    assert!(text.contains("cafebabe1234"), "payload hash");
    assert!(
        text.contains("Recipient not seen before"),
        "risk flag message"
    );

    // The card's JSON representation is also used for web surfaces.
    let json = serde_json::to_string(&card).expect("serialize");
    assert!(json.contains("Balances"));
    assert!(json.contains("10 DOT"));
    assert!(json.contains("cafebabe1234"));
    assert!(json.contains("first_time_recipient"));
}

/// FakeChainClient decode_call is deterministic -- calling twice with the
/// same bytes returns the same result.
#[tokio::test]
async fn fake_chain_client_decode_call_deterministic() {
    let client = FakeChainClientBuilder::polkadot().build();
    let metadata = fake_pinned_metadata();
    let call_bytes = vec![0x05, 0x03, 0xAA, 0xBB];

    let result1 = client
        .decode_call(&call_bytes, &metadata)
        .await
        .expect("decode 1");
    let result2 = client
        .decode_call(&call_bytes, &metadata)
        .await
        .expect("decode 2");

    assert_eq!(result1.pallet, result2.pallet);
    assert_eq!(result1.call_name, result2.call_name);
    assert_eq!(result1.arguments_json, result2.arguments_json);
}
