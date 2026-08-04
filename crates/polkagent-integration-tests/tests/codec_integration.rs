//! Integration tests for the codec subsystem.
//!
//! Exercises SCALE encode/decode, extrinsic decoding, batch recursion,
//! transfer amount extraction, and error handling — all wired through the
//! `polkagent-codec` crate boundary.

use polkagent_codec::{
    call::{call_index, pallet_index},
    decode_batch_call, decode_extrinsic, extract_transfer_amount, is_batch_call, is_proxy_call,
    is_transfer_call, DecodedField, FieldValue, ScaleDecoder, ScaleEncoder,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compact-encode a u32 length, returning the prefix bytes.
fn compact_len_bytes(len: usize) -> Vec<u8> {
    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u32(len as u32);
    enc.finish()
}

/// Encode a minimal *unsigned* extrinsic:
/// [compact length][version=0x04][pallet_index][call_index][...args]
///
/// We build this manually as a Vec<u8> so we can prepend the compact length
/// prefix without needing an `encode_bytes_raw` method.
fn make_unsigned_extrinsic(pallet: u8, call: u8, args: &[u8]) -> Vec<u8> {
    // Inner payload: [version=0x04][pallet][call][...args]
    let mut inner: Vec<u8> = Vec::with_capacity(3 + args.len());
    inner.push(0x04u8); // version byte (unsigned)
    inner.push(pallet);
    inner.push(call);
    inner.extend_from_slice(args);

    // Outer: compact length prefix + inner bytes
    let mut out = compact_len_bytes(inner.len());
    out.extend_from_slice(&inner);
    out
}

/// Encode a compact u64 as bytes.
fn compact_u64(v: u64) -> Vec<u8> {
    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u64(v);
    enc.finish()
}

/// Encode a MultiAddress Id variant: [0x00][32 bytes]
fn multi_address_id(addr: &[u8; 32]) -> Vec<u8> {
    let mut v = vec![0x00u8];
    v.extend_from_slice(addr);
    v
}

/// Build compact-length-prefixed inner call bytes for batch encoding.
/// Each inner call in a batch is encoded as [compact_byte_len][pallet][call][args].
fn batch_inner_call(pallet: u8, call: u8, args: &[u8]) -> Vec<u8> {
    let mut call_data: Vec<u8> = Vec::with_capacity(2 + args.len());
    call_data.push(pallet);
    call_data.push(call);
    call_data.extend_from_slice(args);
    // Length-prefix the call bytes using encode_bytes (compact len + data)
    let mut enc = ScaleEncoder::new();
    enc.encode_bytes(&call_data);
    enc.finish()
}

// ---------------------------------------------------------------------------
// IT-CODEC-01: Decode known extrinsic bytes → verify pallet/call
// ---------------------------------------------------------------------------

#[test]
fn decode_unsigned_balances_transfer_pallet_call_indices() {
    let dest = [1u8; 32];
    let mut args = multi_address_id(&dest);
    args.extend(compact_u64(1_000_000_000));

    let ext_bytes = make_unsigned_extrinsic(
        pallet_index::BALANCES,
        call_index::BALANCES_TRANSFER_KEEP_ALIVE,
        &args,
    );

    let decoded = decode_extrinsic(&ext_bytes).expect("decode ok");
    assert_eq!(decoded.pallet_index, pallet_index::BALANCES);
    assert_eq!(decoded.call_index, call_index::BALANCES_TRANSFER_KEEP_ALIVE);
}

#[test]
fn decode_unsigned_utility_batch_pallet_call_indices() {
    let mut args = ScaleEncoder::new();
    args.encode_compact_u32(0); // 0 inner calls

    let ext_bytes = make_unsigned_extrinsic(
        pallet_index::UTILITY,
        call_index::UTILITY_BATCH,
        &args.finish(),
    );

    let decoded = decode_extrinsic(&ext_bytes).expect("decode ok");
    assert_eq!(decoded.pallet_index, pallet_index::UTILITY);
    assert_eq!(decoded.call_index, call_index::UTILITY_BATCH);
}

#[test]
fn decode_extrinsic_pallet_name_is_none_without_metadata() {
    let ext_bytes = make_unsigned_extrinsic(5, 3, &[]);
    let decoded = decode_extrinsic(&ext_bytes).expect("decode ok");
    assert!(decoded.pallet_name.is_none());
    assert!(decoded.call_name.is_none());
}

#[test]
fn decode_extrinsic_args_captured_as_raw_bytes() {
    let raw_args = vec![0xAA, 0xBB, 0xCC, 0xDD];
    let ext_bytes = make_unsigned_extrinsic(10, 2, &raw_args);
    let decoded = decode_extrinsic(&ext_bytes).expect("decode ok");
    assert!(!decoded.args.is_empty());
    if let FieldValue::Bytes(ref b) = decoded.args[0].value {
        assert_eq!(b, &raw_args);
    } else {
        panic!("expected FieldValue::Bytes for raw args");
    }
}

#[test]
fn decoded_extrinsic_raw_args_bytes_returns_concatenated() {
    let raw_args = vec![0x01, 0x02, 0x03];
    let ext_bytes = make_unsigned_extrinsic(5, 0, &raw_args);
    let decoded = decode_extrinsic(&ext_bytes).expect("decode ok");
    let raw = decoded
        .raw_args_bytes()
        .expect("raw_args_bytes should return Some");
    assert_eq!(raw, raw_args);
}

#[test]
fn is_transfer_call_identifies_transfer() {
    let args = {
        let mut a = multi_address_id(&[0u8; 32]);
        a.extend(compact_u64(500_000));
        a
    };
    let ext_bytes =
        make_unsigned_extrinsic(pallet_index::BALANCES, call_index::BALANCES_TRANSFER, &args);
    let decoded = decode_extrinsic(&ext_bytes).expect("decode ok");
    assert!(is_transfer_call(&decoded));
    assert!(!is_batch_call(&decoded));
    assert!(!is_proxy_call(&decoded));
}

#[test]
fn is_transfer_call_identifies_transfer_keep_alive() {
    let args = {
        let mut a = multi_address_id(&[0u8; 32]);
        a.extend(compact_u64(500_000));
        a
    };
    let ext_bytes = make_unsigned_extrinsic(
        pallet_index::BALANCES,
        call_index::BALANCES_TRANSFER_KEEP_ALIVE,
        &args,
    );
    let decoded = decode_extrinsic(&ext_bytes).expect("decode ok");
    assert!(is_transfer_call(&decoded));
}

#[test]
fn is_batch_call_identifies_batch() {
    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u32(0);
    let ext_bytes = make_unsigned_extrinsic(
        pallet_index::UTILITY,
        call_index::UTILITY_BATCH,
        &enc.finish(),
    );
    let decoded = decode_extrinsic(&ext_bytes).expect("decode ok");
    assert!(is_batch_call(&decoded));
    assert!(!is_transfer_call(&decoded));
}

#[test]
fn is_batch_all_call_identified() {
    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u32(0);
    let ext_bytes = make_unsigned_extrinsic(
        pallet_index::UTILITY,
        call_index::UTILITY_BATCH_ALL,
        &enc.finish(),
    );
    let decoded = decode_extrinsic(&ext_bytes).expect("decode ok");
    assert!(is_batch_call(&decoded));
}

// ---------------------------------------------------------------------------
// IT-CODEC-02: Batch call recursive decode
// ---------------------------------------------------------------------------

#[test]
fn decode_batch_with_single_inner_call() {
    let inner = batch_inner_call(pallet_index::BALANCES, call_index::BALANCES_TRANSFER, &[]);
    // Build batch args: [compact count] [length-prefixed inner calls...]
    // batch_inner_call already returns length-prefixed bytes, so we just
    // prepend the count and concatenate the pre-encoded call bytes.
    let mut batch_args: Vec<u8> = compact_len_bytes(1); // count = 1
    batch_args.extend_from_slice(&inner);

    let ext_bytes = make_unsigned_extrinsic(
        pallet_index::UTILITY,
        call_index::UTILITY_BATCH,
        &batch_args,
    );

    let decoded = decode_extrinsic(&ext_bytes).expect("decode ext ok");
    assert!(is_batch_call(&decoded));

    let inner_calls = decode_batch_call(&decoded).expect("decode batch ok");
    assert_eq!(inner_calls.len(), 1);
    assert_eq!(inner_calls[0].pallet_index, pallet_index::BALANCES);
    assert_eq!(inner_calls[0].call_index, call_index::BALANCES_TRANSFER);
}

#[test]
fn decode_batch_with_multiple_inner_calls() {
    let call1 = batch_inner_call(pallet_index::BALANCES, call_index::BALANCES_TRANSFER, &[]);
    let call2 = batch_inner_call(
        pallet_index::BALANCES,
        call_index::BALANCES_TRANSFER_KEEP_ALIVE,
        &[],
    );
    let call3 = batch_inner_call(10, 5, &[0xDE, 0xAD]);

    // [compact count=3][call1 bytes][call2 bytes][call3 bytes]
    let mut batch_args: Vec<u8> = compact_len_bytes(3);
    batch_args.extend_from_slice(&call1);
    batch_args.extend_from_slice(&call2);
    batch_args.extend_from_slice(&call3);

    let ext_bytes = make_unsigned_extrinsic(
        pallet_index::UTILITY,
        call_index::UTILITY_BATCH,
        &batch_args,
    );

    let decoded = decode_extrinsic(&ext_bytes).expect("decode ok");
    let inner = decode_batch_call(&decoded).expect("decode batch ok");
    assert_eq!(inner.len(), 3);
    assert_eq!(inner[0].pallet_index, pallet_index::BALANCES);
    assert_eq!(
        inner[1].call_index,
        call_index::BALANCES_TRANSFER_KEEP_ALIVE
    );
    assert_eq!(inner[2].pallet_index, 10);
    assert_eq!(inner[2].call_index, 5);
}

#[test]
fn decode_empty_batch_returns_empty_vec() {
    let mut batch_args = ScaleEncoder::new();
    batch_args.encode_compact_u32(0);

    let ext_bytes = make_unsigned_extrinsic(
        pallet_index::UTILITY,
        call_index::UTILITY_BATCH,
        &batch_args.finish(),
    );
    let decoded = decode_extrinsic(&ext_bytes).expect("decode ok");
    let inner = decode_batch_call(&decoded).expect("decode batch ok");
    assert!(inner.is_empty());
}

#[test]
fn decode_batch_on_non_batch_extrinsic_returns_error() {
    let args = {
        let mut a = multi_address_id(&[0u8; 32]);
        a.extend(compact_u64(100));
        a
    };
    let ext_bytes =
        make_unsigned_extrinsic(pallet_index::BALANCES, call_index::BALANCES_TRANSFER, &args);
    let decoded = decode_extrinsic(&ext_bytes).expect("decode ok");
    let result = decode_batch_call(&decoded);
    assert!(result.is_err(), "decode_batch_call on non-batch must fail");
}

#[test]
fn decode_batch_all_works_same_as_batch() {
    let inner = batch_inner_call(pallet_index::BALANCES, call_index::BALANCES_TRANSFER, &[]);
    let mut batch_args: Vec<u8> = compact_len_bytes(1);
    batch_args.extend_from_slice(&inner);

    let ext_bytes = make_unsigned_extrinsic(
        pallet_index::UTILITY,
        call_index::UTILITY_BATCH_ALL,
        &batch_args,
    );
    let decoded = decode_extrinsic(&ext_bytes).expect("decode ok");
    let calls = decode_batch_call(&decoded).expect("batch_all decode ok");
    assert_eq!(calls.len(), 1);
}

// ---------------------------------------------------------------------------
// IT-CODEC-03: Transfer amount extraction
// ---------------------------------------------------------------------------

#[test]
fn extract_transfer_amount_from_transfer_call() {
    let amount_planck: u64 = 1_000_000_000_000; // 1 DOT
    let args = {
        let mut a = multi_address_id(&[2u8; 32]);
        a.extend(compact_u64(amount_planck));
        a
    };
    let ext_bytes =
        make_unsigned_extrinsic(pallet_index::BALANCES, call_index::BALANCES_TRANSFER, &args);
    let decoded = decode_extrinsic(&ext_bytes).expect("decode ok");
    let amount = extract_transfer_amount(&decoded);
    assert_eq!(amount, Some(amount_planck as u128));
}

#[test]
fn extract_transfer_amount_from_transfer_keep_alive() {
    let amount_planck: u64 = 5_000_000_000;
    let args = {
        let mut a = multi_address_id(&[3u8; 32]);
        a.extend(compact_u64(amount_planck));
        a
    };
    let ext_bytes = make_unsigned_extrinsic(
        pallet_index::BALANCES,
        call_index::BALANCES_TRANSFER_KEEP_ALIVE,
        &args,
    );
    let decoded = decode_extrinsic(&ext_bytes).expect("decode ok");
    let amount = extract_transfer_amount(&decoded);
    assert_eq!(amount, Some(amount_planck as u128));
}

#[test]
fn extract_transfer_amount_from_named_field() {
    use polkagent_codec::decode::DecodedExtrinsic;

    let ext = DecodedExtrinsic {
        pallet_index: pallet_index::BALANCES,
        call_index: call_index::BALANCES_TRANSFER,
        pallet_name: None,
        call_name: None,
        args: vec![DecodedField::named(
            "value",
            FieldValue::Compact(42_000_000_000),
        )],
    };
    let amount = extract_transfer_amount(&ext);
    assert_eq!(amount, Some(42_000_000_000));
}

#[test]
fn extract_transfer_amount_returns_none_for_non_transfer_with_no_named_field() {
    let ext_bytes =
        make_unsigned_extrinsic(pallet_index::UTILITY, call_index::UTILITY_BATCH, &[0x00]);
    let decoded = decode_extrinsic(&ext_bytes).expect("decode ok");
    let amount = extract_transfer_amount(&decoded);
    // Utility.batch is not a transfer and has no named amount field
    assert!(amount.is_none());
}

#[test]
fn extract_transfer_amount_named_u128_field_works() {
    use polkagent_codec::decode::DecodedExtrinsic;

    let ext = DecodedExtrinsic {
        pallet_index: pallet_index::BALANCES,
        call_index: call_index::BALANCES_TRANSFER,
        pallet_name: None,
        call_name: None,
        args: vec![DecodedField::named("value", FieldValue::U128(999_000))],
    };
    let amount = extract_transfer_amount(&ext);
    assert_eq!(amount, Some(999_000));
}

#[test]
fn small_transfer_amounts_survive_round_trip() {
    for amount in [0u64, 1, 100, u32::MAX as u64] {
        let args = {
            let mut a = multi_address_id(&[0u8; 32]);
            a.extend(compact_u64(amount));
            a
        };
        let ext_bytes =
            make_unsigned_extrinsic(pallet_index::BALANCES, call_index::BALANCES_TRANSFER, &args);
        let decoded = decode_extrinsic(&ext_bytes).expect("decode ok");
        let extracted = extract_transfer_amount(&decoded);
        assert_eq!(
            extracted,
            Some(amount as u128),
            "amount {amount} failed round-trip"
        );
    }
}

// ---------------------------------------------------------------------------
// IT-CODEC-04: Invalid bytes → clean error
// ---------------------------------------------------------------------------

#[test]
fn decode_extrinsic_empty_bytes_returns_error() {
    let result = decode_extrinsic(&[]);
    assert!(result.is_err(), "empty bytes must return an error");
}

#[test]
fn decode_extrinsic_truncated_bytes_returns_error() {
    // Compact length says 100 bytes but nothing follows
    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u32(100);
    let result = decode_extrinsic(&enc.finish());
    assert!(result.is_err(), "truncated extrinsic must return an error");
}

#[test]
fn decode_extrinsic_only_version_byte_no_pallet_returns_error() {
    // Compact(1) = [0x04], then version byte [0x04], then nothing
    let bytes = vec![0x04u8, 0x04];
    let result = decode_extrinsic(&bytes);
    assert!(
        result.is_err(),
        "extrinsic with only version byte must error"
    );
}

#[test]
fn decode_extrinsic_single_zero_byte_returns_error() {
    let result = decode_extrinsic(&[0x00]);
    assert!(result.is_err(), "single zero byte must error");
}

#[test]
fn decode_extrinsic_garbage_bytes_returns_error() {
    // Random high bytes that cannot form a valid compact prefix
    let bytes = vec![0xFF, 0xFF, 0xFF, 0xFF];
    let result = decode_extrinsic(&bytes);
    // Either an overflow error or an EOF — just must be an error
    assert!(result.is_err(), "garbage bytes must return error");
}

// ---------------------------------------------------------------------------
// IT-CODEC-05: Round-trip encode → decode → verify
// ---------------------------------------------------------------------------

#[test]
fn scale_encoder_decoder_round_trip_u8() {
    let mut enc = ScaleEncoder::new();
    enc.encode_u8(255);
    let bytes = enc.finish();
    let mut dec = ScaleDecoder::new(&bytes);
    assert_eq!(dec.decode_u8().expect("decode"), 255u8);
}

#[test]
fn scale_encoder_decoder_round_trip_u16() {
    let val = 0x1234u16;
    let mut enc = ScaleEncoder::new();
    enc.encode_u16(val);
    let bytes = enc.finish();
    let mut dec = ScaleDecoder::new(&bytes);
    assert_eq!(dec.decode_u16().expect("decode"), val);
}

#[test]
fn scale_encoder_decoder_round_trip_u32() {
    let mut enc = ScaleEncoder::new();
    enc.encode_u32(0xDEAD_BEEF);
    let bytes = enc.finish();
    let mut dec = ScaleDecoder::new(&bytes);
    assert_eq!(dec.decode_u32().expect("decode"), 0xDEAD_BEEF);
}

#[test]
fn scale_encoder_decoder_round_trip_u64() {
    let mut enc = ScaleEncoder::new();
    enc.encode_u64(u64::MAX);
    let bytes = enc.finish();
    let mut dec = ScaleDecoder::new(&bytes);
    assert_eq!(dec.decode_u64().expect("decode"), u64::MAX);
}

#[test]
fn scale_encoder_decoder_round_trip_u128() {
    let mut enc = ScaleEncoder::new();
    enc.encode_u128(u128::MAX);
    let bytes = enc.finish();
    let mut dec = ScaleDecoder::new(&bytes);
    assert_eq!(dec.decode_u128().expect("decode"), u128::MAX);
}

#[test]
fn scale_encoder_decoder_round_trip_bool() {
    for val in [true, false] {
        let mut enc = ScaleEncoder::new();
        enc.encode_bool(val);
        let bytes = enc.finish();
        let mut dec = ScaleDecoder::new(&bytes);
        assert_eq!(dec.decode_bool().expect("decode"), val);
    }
}

#[test]
fn scale_encoder_decoder_round_trip_compact_u32() {
    for &v in &[0u32, 1, 63, 64, 16383, 16384, 1_073_741_823] {
        let mut enc = ScaleEncoder::new();
        enc.encode_compact_u32(v);
        let bytes = enc.finish();
        let mut dec = ScaleDecoder::new(&bytes);
        assert_eq!(
            dec.decode_compact_u32().expect("decode"),
            v,
            "compact_u32 failed for {v}"
        );
    }
}

#[test]
fn scale_encoder_decoder_round_trip_compact_u64() {
    for &v in &[0u64, 1, 64, 16384, 1_073_741_824, u32::MAX as u64] {
        let mut enc = ScaleEncoder::new();
        enc.encode_compact_u64(v);
        let bytes = enc.finish();
        let mut dec = ScaleDecoder::new(&bytes);
        assert_eq!(
            dec.decode_compact_u64().expect("decode"),
            v,
            "compact_u64 failed for {v}"
        );
    }
}

#[test]
fn scale_encoder_decoder_round_trip_string() {
    let s = "Hello, Polkadot!";
    let mut enc = ScaleEncoder::new();
    enc.encode_string(s);
    let bytes = enc.finish();
    let mut dec = ScaleDecoder::new(&bytes);
    assert_eq!(dec.decode_string().expect("decode"), s);
}

#[test]
fn scale_encoder_decoder_round_trip_bytes() {
    let data = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01];
    let mut enc = ScaleEncoder::new();
    enc.encode_bytes(&data);
    let bytes = enc.finish();
    let mut dec = ScaleDecoder::new(&bytes);
    assert_eq!(dec.decode_bytes().expect("decode"), data);
}

#[test]
fn scale_encoder_decoder_round_trip_fixed_array() {
    let arr: [u8; 32] = {
        let mut a = [0u8; 32];
        for (i, b) in a.iter_mut().enumerate() {
            *b = i as u8;
        }
        a
    };
    // SCALE fixed arrays are encoded as raw bytes with no length prefix.
    // ScaleEncoder has no encode_fixed_array; we push the bytes directly.
    let bytes: Vec<u8> = arr.to_vec();
    let mut dec = ScaleDecoder::new(&bytes);
    let decoded = dec.decode_fixed_array::<32>().expect("decode");
    assert_eq!(decoded, arr);
}

#[test]
fn scale_decoder_remaining_bytes_advances_correctly() {
    let data = vec![0x01, 0x02, 0x03, 0x04];
    let mut dec = ScaleDecoder::new(&data);
    dec.decode_u8().expect("read one byte");
    let remaining = dec.remaining_bytes();
    assert_eq!(remaining, &[0x02, 0x03, 0x04]);
}

#[test]
fn scale_decoder_eof_returns_error() {
    let data = vec![0x01, 0x02];
    let mut dec = ScaleDecoder::new(&data);
    dec.decode_u8().expect("first byte");
    dec.decode_u8().expect("second byte");
    let result = dec.decode_u8();
    assert!(result.is_err(), "reading past EOF must error");
}

#[test]
fn extrinsic_round_trip_preserves_pallet_and_call() {
    for (pallet, call) in [(5u8, 3u8), (24, 0), (29, 0), (1, 1)] {
        let args = vec![0xAA, 0xBB];
        let ext_bytes = make_unsigned_extrinsic(pallet, call, &args);
        let decoded = decode_extrinsic(&ext_bytes).expect("decode ok");
        assert_eq!(
            decoded.pallet_index, pallet,
            "pallet mismatch for {pallet}/{call}"
        );
        assert_eq!(
            decoded.call_index, call,
            "call mismatch for {pallet}/{call}"
        );
    }
}

#[test]
fn empty_extrinsic_no_args_decodes_correctly() {
    let ext_bytes = make_unsigned_extrinsic(pallet_index::UTILITY, call_index::UTILITY_BATCH, &[]);
    let decoded = decode_extrinsic(&ext_bytes).expect("decode ok");
    assert_eq!(decoded.pallet_index, pallet_index::UTILITY);
    assert!(
        decoded.args.is_empty(),
        "empty args extrinsic should have no args"
    );
}
