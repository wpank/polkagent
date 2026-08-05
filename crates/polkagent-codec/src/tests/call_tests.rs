//! Tests for [`call`] module helpers.

use crate::call::{
    call_index, decode_batch_call, decode_proxy_call, extract_transfer_amount, is_batch_call,
    is_proxy_call, is_transfer_call, pallet_index,
};
use crate::decode::{DecodedExtrinsic, DecodedField, FieldValue};
use crate::scale::ScaleEncoder;

// ---------------------------------------------------------------------------
// Helper to build a DecodedExtrinsic with raw-bytes args
// ---------------------------------------------------------------------------

fn raw_ext(pallet: u8, call: u8, raw_args: Vec<u8>) -> DecodedExtrinsic {
    DecodedExtrinsic {
        pallet_index: pallet,
        call_index: call,
        pallet_name: None,
        call_name: None,
        args: vec![DecodedField::unnamed(FieldValue::Bytes(raw_args))],
    }
}

fn empty_ext(pallet: u8, call: u8) -> DecodedExtrinsic {
    DecodedExtrinsic {
        pallet_index: pallet,
        call_index: call,
        pallet_name: None,
        call_name: None,
        args: vec![],
    }
}

// ---------------------------------------------------------------------------
// Predicate tests
// ---------------------------------------------------------------------------

#[test]
fn is_batch_call_batch() {
    let ext = empty_ext(pallet_index::UTILITY, call_index::UTILITY_BATCH);
    assert!(is_batch_call(&ext));
}

#[test]
fn is_batch_call_batch_all() {
    let ext = empty_ext(pallet_index::UTILITY, call_index::UTILITY_BATCH_ALL);
    assert!(is_batch_call(&ext));
}

#[test]
fn is_batch_call_force_batch() {
    let ext = empty_ext(pallet_index::UTILITY, call_index::UTILITY_FORCE_BATCH);
    assert!(is_batch_call(&ext));
}

#[test]
fn is_batch_call_false_for_non_utility() {
    let ext = empty_ext(5, 0);
    assert!(!is_batch_call(&ext));
}

#[test]
fn is_proxy_call_true() {
    let ext = empty_ext(pallet_index::PROXY, call_index::PROXY_PROXY);
    assert!(is_proxy_call(&ext));
}

#[test]
fn is_proxy_call_false() {
    let ext = empty_ext(pallet_index::UTILITY, 0);
    assert!(!is_proxy_call(&ext));
}

#[test]
fn is_transfer_call_transfer() {
    let ext = empty_ext(pallet_index::BALANCES, call_index::BALANCES_TRANSFER);
    assert!(is_transfer_call(&ext));
}

#[test]
fn is_transfer_call_keep_alive() {
    let ext = empty_ext(
        pallet_index::BALANCES,
        call_index::BALANCES_TRANSFER_KEEP_ALIVE,
    );
    assert!(is_transfer_call(&ext));
}

#[test]
fn is_transfer_call_false() {
    let ext = empty_ext(0, 0);
    assert!(!is_transfer_call(&ext));
}

// ---------------------------------------------------------------------------
// decode_batch_call
// ---------------------------------------------------------------------------

/// Encode a batch of inner bare calls.
fn encode_batch_args(inner_calls: &[Vec<u8>]) -> Vec<u8> {
    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u32(inner_calls.len() as u32);
    for call in inner_calls {
        enc.encode_bytes(call);
    }
    enc.finish()
}

#[test]
fn decode_batch_call_empty() {
    let args = encode_batch_args(&[]);
    let ext = raw_ext(pallet_index::UTILITY, call_index::UTILITY_BATCH, args);
    let calls = decode_batch_call(&ext).expect("decode batch");
    assert!(calls.is_empty());
}

#[test]
fn decode_batch_call_single_inner() {
    let inner_call = vec![5u8, 3]; // pallet 5, call 3
    let args = encode_batch_args(&[inner_call]);
    let ext = raw_ext(pallet_index::UTILITY, call_index::UTILITY_BATCH, args);
    let calls = decode_batch_call(&ext).expect("decode batch single");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].pallet_index, 5);
    assert_eq!(calls[0].call_index, 3);
}

#[test]
fn decode_batch_call_multiple_inner() {
    let inner_calls = vec![
        vec![5u8, 3],  // Balances.transferKeepAlive
        vec![1u8, 0],  // pallet 1, call 0
        vec![24u8, 0], // Utility.batch
    ];
    let args = encode_batch_args(&inner_calls);
    let ext = raw_ext(pallet_index::UTILITY, call_index::UTILITY_BATCH_ALL, args);
    let calls = decode_batch_call(&ext).expect("decode batch multi");
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].pallet_index, 5);
    assert_eq!(calls[1].pallet_index, 1);
    assert_eq!(calls[2].pallet_index, 24);
}

#[test]
fn decode_batch_call_wrong_pallet_error() {
    let ext = empty_ext(5, 0); // Balances.transfer, not a batch
    let err = decode_batch_call(&ext).expect_err("should fail");
    assert!(err.to_string().contains("batch"));
}

// ---------------------------------------------------------------------------
// decode_proxy_call
// ---------------------------------------------------------------------------

/// Build minimal `Proxy.proxy` args:
/// `dest = MultiAddress::Id(32 zero bytes)`
/// `force_proxy_type = None`
/// `inner_call = [pallet, call]`
fn build_proxy_args(inner_pallet: u8, inner_call: u8) -> Vec<u8> {
    let mut enc = ScaleEncoder::new();
    // real: MultiAddress::Id
    enc.encode_u8(0x00);
    enc.encode_bytes(&[0u8; 32]); // 32 zero bytes + compact length prefix = wrong!
                                  // Actually MultiAddress::Id is a fixed 32-byte AccountId, not length-prefixed.
                                  // Re-do manually.
    let _ = enc; // discard

    let mut buf = Vec::new();
    buf.push(0x00u8); // MultiAddress::Id variant
    buf.extend_from_slice(&[0u8; 32]); // 32-byte AccountId (no length prefix)
    buf.push(0x00); // force_proxy_type: None

    // inner call: length-prefixed
    let call_bytes = vec![inner_pallet, inner_call];
    let mut enc2 = ScaleEncoder::new();
    enc2.encode_bytes(&call_bytes);
    buf.extend(enc2.finish());

    buf
}

#[test]
fn decode_proxy_call_basic() {
    let args = build_proxy_args(5, 3);
    let ext = raw_ext(pallet_index::PROXY, call_index::PROXY_PROXY, args);
    let inner = decode_proxy_call(&ext).expect("decode proxy");
    assert_eq!(inner.pallet_index, 5);
    assert_eq!(inner.call_index, 3);
}

#[test]
fn decode_proxy_call_wrong_pallet_error() {
    let ext = empty_ext(5, 0);
    let err = decode_proxy_call(&ext).expect_err("should fail");
    assert!(err.to_string().contains("Proxy"));
}

// ---------------------------------------------------------------------------
// extract_transfer_amount
// ---------------------------------------------------------------------------

#[test]
fn extract_transfer_amount_from_named_field() {
    let ext = DecodedExtrinsic {
        pallet_index: pallet_index::BALANCES,
        call_index: call_index::BALANCES_TRANSFER_KEEP_ALIVE,
        pallet_name: Some("Balances".to_string()),
        call_name: Some("transferKeepAlive".to_string()),
        args: vec![
            DecodedField::named("dest", FieldValue::AccountId([0u8; 32])),
            DecodedField::named("value", FieldValue::Compact(1_000_000_000_000u128)),
        ],
    };
    let amount = extract_transfer_amount(&ext).expect("extract amount");
    assert_eq!(amount, 1_000_000_000_000u128);
}

#[test]
fn extract_transfer_amount_named_u128() {
    let ext = DecodedExtrinsic {
        pallet_index: pallet_index::BALANCES,
        call_index: call_index::BALANCES_TRANSFER,
        pallet_name: None,
        call_name: None,
        args: vec![
            DecodedField::named("dest", FieldValue::AccountId([0u8; 32])),
            DecodedField::named("value", FieldValue::U128(5_000_000_000u128)),
        ],
    };
    let amount = extract_transfer_amount(&ext).expect("u128 amount");
    assert_eq!(amount, 5_000_000_000u128);
}

#[test]
fn extract_transfer_amount_none_for_non_transfer() {
    let ext = empty_ext(0, 0);
    assert!(extract_transfer_amount(&ext).is_none());
}
