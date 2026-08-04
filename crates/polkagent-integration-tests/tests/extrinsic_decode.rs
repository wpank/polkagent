//! Integration tests for SCALE extrinsic decoding against real fixtures.
//!
//! Acceptance criteria covered:
//!
//! - **AC-P2-001** -- Extrinsic decoded correctly against pinned metadata.
//! - **AC-P2-004** -- Wrong metadata version produces explicit error.
//!
//! The fixture at `fixtures/extrinsics/polkadot_balance_transfer.hex` contains a
//! hex-encoded unsigned Polkadot `Balances::transfer_keep_alive` extrinsic:
//!
//! ```text
//! [compact(42)] [0x04 unsigned] [0x05 Balances] [0x03 transfer_keep_alive]
//! [0x00 MultiAddress::Id] [32-byte dest (Alice)] [compact(10_000_000_000)]
//! ```

use polkagent_codec::call::{call_index, extract_transfer_amount, is_transfer_call, pallet_index};
use polkagent_codec::metadata::build_minimal_metadata_v14;
use polkagent_codec::{decode_call_with_metadata, decode_extrinsic, ScaleDecoder, ScaleEncoder};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The fixture file path relative to the workspace root.
const FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/extrinsics/polkadot_balance_transfer.hex"
);

/// Alice's public key (well-known Substrate dev account).
const ALICE_PUBKEY: [u8; 32] = [
    0xd4, 0x35, 0x93, 0xc7, 0x15, 0xfd, 0xd3, 0x1c, 0x61, 0x14, 0x1a, 0xbd, 0x04, 0xa9, 0x9f, 0xd6,
    0x82, 0x2c, 0x85, 0x58, 0x85, 0x4c, 0xcd, 0xe3, 0x9a, 0x56, 0x84, 0xe7, 0xa5, 0x6d, 0xa2, 0x7d,
];

/// 1 DOT = 10^10 planck on Polkadot.
const ONE_DOT_PLANCK: u64 = 10_000_000_000;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Load the fixture hex file and decode to raw bytes.
fn load_fixture() -> Vec<u8> {
    let hex_str = std::fs::read_to_string(FIXTURE_PATH)
        .unwrap_or_else(|e| panic!("failed to read fixture at {FIXTURE_PATH}: {e}"));
    let hex_str = hex_str.trim();
    hex_decode(hex_str)
}

/// Decode a hex string to bytes (no 0x prefix).
fn hex_decode(s: &str) -> Vec<u8> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    assert!(s.len() % 2 == 0, "hex string must have even length");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex digit"))
        .collect()
}

/// Build an unsigned extrinsic byte sequence:
/// `[compact(len)] [0x04 version] [pallet] [call] [args...]`
fn build_unsigned_extrinsic(pallet: u8, call: u8, args: &[u8]) -> Vec<u8> {
    let mut payload = vec![0x04u8, pallet, call];
    payload.extend_from_slice(args);

    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u32(payload.len() as u32);
    let mut out = enc.finish();
    out.extend(payload);
    out
}

/// Strip extrinsic framing and return bare call bytes.
///
/// Mirrors the production `DecodeService::strip_extrinsic_framing` logic for
/// unsigned extrinsics: skip compact length prefix and version byte.
fn strip_unsigned_framing(bytes: &[u8]) -> Vec<u8> {
    let mut dec = ScaleDecoder::new(bytes);
    let _len = dec.decode_compact_u32().expect("compact length");
    let version = dec.decode_u8().expect("version byte");
    assert_eq!(version & 0x80, 0, "expected unsigned extrinsic");
    dec.remaining_bytes().to_vec()
}

// ---------------------------------------------------------------------------
// Test 1: Decode fixture -- verify pallet and call indices (AC-P2-001)
// ---------------------------------------------------------------------------

#[test]
fn fixture_decode_produces_correct_pallet_and_call() {
    let bytes = load_fixture();
    let decoded = decode_extrinsic(&bytes).expect("fixture bytes must decode successfully");

    assert_eq!(
        decoded.pallet_index,
        pallet_index::BALANCES,
        "pallet index should be 5 (Balances)"
    );
    assert_eq!(
        decoded.call_index,
        call_index::BALANCES_TRANSFER_KEEP_ALIVE,
        "call index should be 3 (transfer_keep_alive)"
    );
    assert!(
        is_transfer_call(&decoded),
        "decoded extrinsic should be recognised as a transfer call"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Decode fixture with metadata -- verify pallet name (AC-P2-001)
// ---------------------------------------------------------------------------

#[test]
fn fixture_decode_with_metadata_resolves_pallet_name() {
    let bytes = load_fixture();

    // Build minimal metadata that knows about the Balances pallet.
    let meta_bytes =
        build_minimal_metadata_v14(&[("System", 0), ("Balances", pallet_index::BALANCES)]);
    let metadata =
        polkagent_codec::parse_metadata(&meta_bytes).expect("minimal metadata must parse");

    // Strip framing to get bare call bytes for decode_call_with_metadata.
    let call_bytes = strip_unsigned_framing(&bytes);
    let decoded = decode_call_with_metadata(&call_bytes, &metadata)
        .expect("decode with metadata must succeed");

    assert_eq!(decoded.pallet_index, pallet_index::BALANCES);
    assert_eq!(decoded.call_index, call_index::BALANCES_TRANSFER_KEEP_ALIVE);
    assert_eq!(
        decoded.pallet_name.as_deref(),
        Some("Balances"),
        "pallet name should be resolved from metadata"
    );
}

// ---------------------------------------------------------------------------
// Test 3: Decode fixture -- verify transfer amount extraction
// ---------------------------------------------------------------------------

#[test]
fn fixture_decode_extracts_transfer_amount() {
    let bytes = load_fixture();
    let decoded = decode_extrinsic(&bytes).expect("fixture decode must succeed");

    let amount = extract_transfer_amount(&decoded)
        .expect("transfer amount should be extractable from raw args");
    assert_eq!(
        amount,
        u128::from(ONE_DOT_PLANCK),
        "transfer amount should be 1 DOT (10^10 planck)"
    );
}

// ---------------------------------------------------------------------------
// Test 4: Decode with wrong metadata version returns error (AC-P2-004)
// ---------------------------------------------------------------------------

#[test]
fn decode_with_wrong_metadata_version_returns_error() {
    // Build v14 metadata bytes, then try to parse them as v15.
    let v14_bytes = build_minimal_metadata_v14(&[("Balances", 5)]);
    let result = polkagent_codec::parse_metadata_v15(&v14_bytes);

    assert!(result.is_err(), "parsing v14 metadata as v15 should fail");
    let err = result.expect_err("should be an error");
    let msg = format!("{err}");
    assert!(
        msg.contains("unsupported") || msg.contains("version"),
        "error should mention version mismatch: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Test 5: Decode truncated bytes returns error
// ---------------------------------------------------------------------------

#[test]
fn decode_truncated_bytes_returns_error() {
    let bytes = load_fixture();

    // Truncate to just the length prefix and version byte (not enough for
    // pallet + call indices).
    let truncated = &bytes[..2];
    let result = decode_extrinsic(truncated);
    assert!(
        result.is_err(),
        "truncated extrinsic (2 bytes) should fail to decode"
    );

    // Truncate in the middle of the arguments.
    let half = bytes.len() / 2;
    let partial = &bytes[..half];
    // This may or may not error depending on where the cut falls,
    // but the decoded args should be incomplete compared to the original.
    // At minimum, verify that decoding does not panic.
    let _ = decode_extrinsic(partial);
}

// ---------------------------------------------------------------------------
// Test 6: Decode empty bytes returns error
// ---------------------------------------------------------------------------

#[test]
fn decode_empty_bytes_returns_error() {
    let result = decode_extrinsic(&[]);
    assert!(result.is_err(), "decoding empty bytes must return an error");
    let err = result.expect_err("should be error");
    let msg = format!("{err}");
    assert!(
        msg.contains("unexpected") || msg.contains("eof") || msg.contains("end"),
        "error should indicate unexpected EOF: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Test 7: Round-trip -- encode then decode produces same call data
// ---------------------------------------------------------------------------

#[test]
fn round_trip_encode_then_decode_produces_same_call_data() {
    // Build a transfer_keep_alive extrinsic from scratch.
    let pallet = pallet_index::BALANCES;
    let call = call_index::BALANCES_TRANSFER_KEEP_ALIVE;

    // Args: MultiAddress::Id(Alice) + compact(1 DOT)
    let mut args = vec![0x00u8]; // MultiAddress::Id variant
    args.extend_from_slice(&ALICE_PUBKEY);
    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u64(ONE_DOT_PLANCK);
    args.extend(enc.finish());

    let ext_bytes = build_unsigned_extrinsic(pallet, call, &args);

    // Decode and verify the round-trip.
    let decoded = decode_extrinsic(&ext_bytes).expect("round-trip decode must succeed");

    assert_eq!(decoded.pallet_index, pallet);
    assert_eq!(decoded.call_index, call);

    // The raw args should match the original args.
    let raw_args = decoded.raw_args_bytes().expect("args should be raw bytes");
    assert_eq!(
        raw_args, args,
        "round-trip raw args must match original args"
    );
}

// ---------------------------------------------------------------------------
// Test 8: Fixture matches hand-constructed extrinsic
// ---------------------------------------------------------------------------

#[test]
fn fixture_matches_hand_constructed_extrinsic() {
    let fixture_bytes = load_fixture();

    // Re-construct the same extrinsic from known parameters.
    let mut args = vec![0x00u8]; // MultiAddress::Id
    args.extend_from_slice(&ALICE_PUBKEY);
    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u64(ONE_DOT_PLANCK);
    args.extend(enc.finish());

    let constructed = build_unsigned_extrinsic(
        pallet_index::BALANCES,
        call_index::BALANCES_TRANSFER_KEEP_ALIVE,
        &args,
    );

    assert_eq!(
        fixture_bytes, constructed,
        "fixture bytes must be bitwise identical to hand-constructed extrinsic"
    );
}

// ---------------------------------------------------------------------------
// Test 9: Decode fixture bare call bytes with metadata (no framing)
// ---------------------------------------------------------------------------

#[test]
fn decode_bare_call_bytes_matches_framed_decode() {
    let bytes = load_fixture();

    // Full structural decode (with framing).
    let framed = decode_extrinsic(&bytes).expect("framed decode");

    // Bare call decode (no framing).
    let call_bytes = strip_unsigned_framing(&bytes);
    let meta_bytes =
        build_minimal_metadata_v14(&[("System", 0), ("Balances", pallet_index::BALANCES)]);
    let metadata = polkagent_codec::parse_metadata(&meta_bytes).expect("parse metadata");
    let bare = decode_call_with_metadata(&call_bytes, &metadata).expect("bare call decode");

    // Both should agree on pallet and call indices.
    assert_eq!(framed.pallet_index, bare.pallet_index);
    assert_eq!(framed.call_index, bare.call_index);

    // Bare decode with metadata should have the pallet name resolved.
    assert_eq!(bare.pallet_name.as_deref(), Some("Balances"));
}

// ---------------------------------------------------------------------------
// Test 10: Decode single-byte extrinsic returns error
// ---------------------------------------------------------------------------

#[test]
fn decode_single_byte_returns_error() {
    // A single byte is a valid compact length (0 or small value) but not
    // enough for a full extrinsic.
    let result = decode_extrinsic(&[0x04]);
    assert!(
        result.is_err(),
        "single byte should not decode to a valid extrinsic"
    );
}

// ---------------------------------------------------------------------------
// Test 11: Unknown pallet index still decodes structurally
// ---------------------------------------------------------------------------

#[test]
fn unknown_pallet_decodes_structurally_without_metadata() {
    // Pallet 255 with call 127 -- not a real pallet, but should still
    // decode structurally.
    let ext_bytes = build_unsigned_extrinsic(255, 127, &[0xCA, 0xFE]);
    let decoded =
        decode_extrinsic(&ext_bytes).expect("structural decode of unknown pallet should succeed");

    assert_eq!(decoded.pallet_index, 255);
    assert_eq!(decoded.call_index, 127);
    assert!(decoded.pallet_name.is_none());
    assert!(decoded.call_name.is_none());
}
