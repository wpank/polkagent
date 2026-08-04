//! Tests for extrinsic decoding ([`decode_extrinsic`], [`decode_call_with_metadata`]).

use crate::decode::{decode_extrinsic, DecodedExtrinsic, DecodedField, FieldValue};
use crate::scale::ScaleEncoder;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a minimal unsigned extrinsic byte sequence:
/// [compact(total_len)] [version=4 (no sign flag)] [pallet] [call] [args...]
fn build_unsigned_extrinsic(pallet: u8, call: u8, args: &[u8]) -> Vec<u8> {
    // Inner payload: version byte + pallet + call + args
    let mut payload = vec![0x04, pallet, call];
    payload.extend_from_slice(args);

    // Outer: compact length prefix.
    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u32(payload.len() as u32);
    let mut out = enc.finish();
    out.extend(payload);
    out
}

// ---------------------------------------------------------------------------
// Balances.transferKeepAlive fixture
//
// Pallet 5, Call 3 (transferKeepAlive)
// Args:
//   dest: MultiAddress::Id(0x0000...0001) = 0x00 + [32 bytes]
//   value: compact(1_000_000_000_000) = 10^12 planck = 1 DOT
// ---------------------------------------------------------------------------

fn transfer_keep_alive_fixture() -> Vec<u8> {
    let dest_address = {
        let mut b = vec![0x00u8]; // MultiAddress::Id variant
        b.extend_from_slice(&[0u8; 31]);
        b.push(0x01); // last byte = 0x01 for a non-zero address
        b
    };
    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u64(1_000_000_000_000u64); // 1 DOT in planck
    let value_bytes = enc.finish();

    let mut args = dest_address;
    args.extend(value_bytes);

    build_unsigned_extrinsic(5, 3, &args)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn decode_transfer_keep_alive_fixture() {
    let ext_bytes = transfer_keep_alive_fixture();
    let ext = decode_extrinsic(&ext_bytes).expect("decode transfer_keep_alive");
    assert_eq!(ext.pallet_index, 5);
    assert_eq!(ext.call_index, 3);
    assert!(ext.pallet_name.is_none());
    assert!(ext.call_name.is_none());
    // Raw args blob is present
    assert!(!ext.args.is_empty());
}

#[test]
fn decode_extrinsic_unsigned() {
    let bytes = build_unsigned_extrinsic(1, 2, &[0xAA, 0xBB]);
    let ext = decode_extrinsic(&bytes).expect("decode unsigned");
    assert_eq!(ext.pallet_index, 1);
    assert_eq!(ext.call_index, 2);
    assert_eq!(
        ext.args,
        vec![DecodedField::unnamed(FieldValue::Bytes(vec![0xAA, 0xBB]))]
    );
}

#[test]
fn decode_extrinsic_no_args() {
    let bytes = build_unsigned_extrinsic(7, 0, &[]);
    let ext = decode_extrinsic(&bytes).expect("decode no-args");
    assert_eq!(ext.pallet_index, 7);
    assert_eq!(ext.call_index, 0);
    assert!(ext.args.is_empty());
}

#[test]
fn decode_extrinsic_truncated_error() {
    // Too short to contain a valid extrinsic
    let bytes = vec![0x08, 0x04]; // compact(2) + version, missing pallet+call
    let result = decode_extrinsic(&bytes);
    assert!(result.is_err());
}

#[test]
fn decode_extrinsic_empty_error() {
    let result = decode_extrinsic(&[]);
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// FieldValue Display
// ---------------------------------------------------------------------------

#[test]
fn field_value_display_u8() {
    assert_eq!(format!("{}", FieldValue::U8(42)), "42");
}

#[test]
fn field_value_display_u128() {
    assert_eq!(
        format!("{}", FieldValue::U128(1_000_000_000_000u128)),
        "1000000000000"
    );
}

#[test]
fn field_value_display_bool() {
    assert_eq!(format!("{}", FieldValue::Bool(true)), "true");
    assert_eq!(format!("{}", FieldValue::Bool(false)), "false");
}

#[test]
fn field_value_display_string() {
    assert_eq!(
        format!("{}", FieldValue::String("hello".to_string())),
        "\"hello\""
    );
}

#[test]
fn field_value_display_bytes() {
    assert_eq!(
        format!("{}", FieldValue::Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF])),
        "0xdeadbeef"
    );
}

#[test]
fn field_value_display_compact() {
    assert_eq!(format!("{}", FieldValue::Compact(1024)), "compact(1024)");
}

#[test]
fn field_value_display_sequence() {
    let seq = FieldValue::Sequence(vec![FieldValue::U8(1), FieldValue::U8(2)]);
    assert_eq!(format!("{seq}"), "[1, 2]");
}

#[test]
fn field_value_display_composite() {
    let comp = FieldValue::Composite(vec![
        DecodedField::named("a", FieldValue::U32(10)),
        DecodedField::named("b", FieldValue::Bool(true)),
    ]);
    assert_eq!(format!("{comp}"), "{a: 10, b: true}");
}

#[test]
fn field_value_display_variant_named() {
    let v = FieldValue::Variant {
        index: 1,
        name: Some("Transfer".to_string()),
        fields: vec![],
    };
    assert_eq!(format!("{v}"), "Transfer");
}

#[test]
fn field_value_display_variant_unnamed() {
    let v = FieldValue::Variant {
        index: 3,
        name: None,
        fields: vec![],
    };
    assert_eq!(format!("{v}"), "#3");
}

#[test]
fn field_value_display_account_id() {
    let id = [0u8; 32];
    let v = FieldValue::AccountId(id);
    assert_eq!(format!("{v}"), format!("0x{}", "00".repeat(32)));
}

// ---------------------------------------------------------------------------
// DecodedField constructors
// ---------------------------------------------------------------------------

#[test]
fn decoded_field_named() {
    let f = DecodedField::named("foo", FieldValue::U8(99));
    assert_eq!(f.name, Some("foo".to_string()));
    assert_eq!(f.value, FieldValue::U8(99));
}

#[test]
fn decoded_field_unnamed() {
    let f = DecodedField::unnamed(FieldValue::Bool(false));
    assert!(f.name.is_none());
    assert_eq!(f.value, FieldValue::Bool(false));
}

// ---------------------------------------------------------------------------
// Debug impls (compile-check)
// ---------------------------------------------------------------------------

#[test]
fn field_value_debug_impl() {
    let v = FieldValue::U64(42);
    let s = format!("{v:?}");
    assert!(s.contains("U64"));
}

#[test]
fn decoded_extrinsic_debug_impl() {
    let ext = DecodedExtrinsic {
        pallet_index: 0,
        call_index: 0,
        pallet_name: None,
        call_name: None,
        args: vec![],
    };
    let s = format!("{ext:?}");
    assert!(s.contains("DecodedExtrinsic"));
}
