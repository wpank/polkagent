//! Tests for [`ScaleDecoder`] and [`ScaleEncoder`].

use crate::error::CodecError;
use crate::scale::{ScaleDecoder, ScaleEncoder};

// ---------------------------------------------------------------------------
// Compact encoding round-trips
// ---------------------------------------------------------------------------

fn compact_rt(v: u32) {
    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u32(v);
    let bytes = enc.finish();
    let mut dec = ScaleDecoder::new(&bytes);
    let decoded = dec.decode_compact_u32().expect("compact decode failed");
    assert_eq!(decoded, v, "compact round-trip failed for {v}");
}

fn compact64_rt(v: u64) {
    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u64(v);
    let bytes = enc.finish();
    let mut dec = ScaleDecoder::new(&bytes);
    let decoded = dec.decode_compact_u64().expect("compact64 decode failed");
    assert_eq!(decoded, v, "compact64 round-trip failed for {v}");
}

#[test]
fn compact_zero() {
    compact_rt(0);
}

#[test]
fn compact_single_byte_max() {
    compact_rt(63);
}

#[test]
fn compact_two_byte_min() {
    compact_rt(64);
}

#[test]
fn compact_two_byte_max() {
    compact_rt(16_383);
}

#[test]
fn compact_four_byte_min() {
    compact_rt(16_384);
}

#[test]
fn compact_four_byte_max() {
    compact_rt(0x3FFF_FFFF);
}

#[test]
fn compact_u32_max() {
    compact_rt(u32::MAX);
}

#[test]
fn compact_u64_round_trip_small() {
    compact64_rt(0);
    compact64_rt(63);
    compact64_rt(64);
    compact64_rt(16_383);
    compact64_rt(16_384);
    compact64_rt(u64::from(0x3FFF_FFFFu32));
}

#[test]
fn compact_u64_large_values() {
    compact64_rt(u64::from(u32::MAX));
    compact64_rt(u64::MAX);
}

/// Verify the specific byte encodings from the SCALE spec.
#[test]
fn compact_spec_known_bytes() {
    // 0 → single-byte: 0x00
    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u32(0);
    assert_eq!(enc.finish(), vec![0x00]);

    // 1 → single-byte: 0x04
    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u32(1);
    assert_eq!(enc.finish(), vec![0x04]);

    // 63 → single-byte: 0xFC
    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u32(63);
    assert_eq!(enc.finish(), vec![0xFC]);

    // 64 → two-byte: 0x01 0x01
    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u32(64);
    assert_eq!(enc.finish(), vec![0x01, 0x01]);

    // 16384 → four-byte: 0x02 0x00 0x01 0x00
    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u32(16_384);
    assert_eq!(enc.finish(), vec![0x02, 0x00, 0x01, 0x00]);
}

// ---------------------------------------------------------------------------
// Primitive round-trips
// ---------------------------------------------------------------------------

#[test]
fn u8_round_trip() {
    for v in [0u8, 1, 127, 128, 255] {
        let mut enc = ScaleEncoder::new();
        enc.encode_u8(v);
        let bytes = enc.finish();
        let mut dec = ScaleDecoder::new(&bytes);
        assert_eq!(dec.decode_u8().expect("u8 decode"), v);
    }
}

#[test]
fn u16_round_trip() {
    for v in [0u16, 1, 256, 1000, 0xFFFF] {
        let mut enc = ScaleEncoder::new();
        enc.encode_u16(v);
        let bytes = enc.finish();
        let mut dec = ScaleDecoder::new(&bytes);
        assert_eq!(dec.decode_u16().expect("u16 decode"), v);
    }
}

#[test]
fn u32_round_trip() {
    for v in [0u32, 1, 0xDEAD_BEEF, u32::MAX] {
        let mut enc = ScaleEncoder::new();
        enc.encode_u32(v);
        let bytes = enc.finish();
        assert_eq!(bytes.len(), 4);
        let mut dec = ScaleDecoder::new(&bytes);
        assert_eq!(dec.decode_u32().expect("u32 decode"), v);
    }
}

#[test]
fn u64_round_trip() {
    for v in [0u64, 1, u64::MAX / 2, u64::MAX] {
        let mut enc = ScaleEncoder::new();
        enc.encode_u64(v);
        let bytes = enc.finish();
        assert_eq!(bytes.len(), 8);
        let mut dec = ScaleDecoder::new(&bytes);
        assert_eq!(dec.decode_u64().expect("u64 decode"), v);
    }
}

#[test]
fn u128_round_trip() {
    for v in [0u128, 1, u128::MAX / 2, u128::MAX] {
        let mut enc = ScaleEncoder::new();
        enc.encode_u128(v);
        let bytes = enc.finish();
        assert_eq!(bytes.len(), 16);
        let mut dec = ScaleDecoder::new(&bytes);
        assert_eq!(dec.decode_u128().expect("u128 decode"), v);
    }
}

#[test]
fn bool_round_trip() {
    for v in [false, true] {
        let mut enc = ScaleEncoder::new();
        enc.encode_bool(v);
        let bytes = enc.finish();
        assert_eq!(bytes.len(), 1);
        let mut dec = ScaleDecoder::new(&bytes);
        assert_eq!(dec.decode_bool().expect("bool decode"), v);
    }
}

#[test]
fn bool_invalid_byte_error() {
    let bytes = vec![0x02];
    let mut dec = ScaleDecoder::new(&bytes);
    assert!(matches!(
        dec.decode_bool(),
        Err(CodecError::InvalidType { .. })
    ));
}

// ---------------------------------------------------------------------------
// Vec / Option / String round-trips
// ---------------------------------------------------------------------------

#[test]
fn vec_round_trip() {
    let items = vec![1u32, 2, 3, 42, 1000];
    let mut enc = ScaleEncoder::new();
    enc.encode_vec(&items, |e, v| e.encode_u32(*v));
    let bytes = enc.finish();
    let mut dec = ScaleDecoder::new(&bytes);
    let decoded = dec.decode_vec(|d| d.decode_u32()).expect("vec decode");
    assert_eq!(decoded, items);
}

#[test]
fn empty_vec_round_trip() {
    let items: Vec<u8> = vec![];
    let mut enc = ScaleEncoder::new();
    enc.encode_vec(&items, |e, v| e.encode_u8(*v));
    let bytes = enc.finish();
    assert_eq!(bytes, vec![0x00]); // compact 0
    let mut dec = ScaleDecoder::new(&bytes);
    let decoded = dec.decode_vec(|d| d.decode_u8()).expect("empty vec decode");
    assert!(decoded.is_empty());
}

#[test]
fn option_some_round_trip() {
    let mut enc = ScaleEncoder::new();
    enc.encode_option(Some(&42u32), |e, v| e.encode_u32(*v));
    let bytes = enc.finish();
    assert_eq!(bytes[0], 0x01);
    let mut dec = ScaleDecoder::new(&bytes);
    let decoded = dec
        .decode_option(|d| d.decode_u32())
        .expect("option decode");
    assert_eq!(decoded, Some(42u32));
}

#[test]
fn option_none_round_trip() {
    let mut enc = ScaleEncoder::new();
    enc.encode_option::<u32, _>(None, |e, v| e.encode_u32(*v));
    let bytes = enc.finish();
    assert_eq!(bytes, vec![0x00]);
    let mut dec = ScaleDecoder::new(&bytes);
    let decoded = dec.decode_option(|d| d.decode_u32()).expect("none decode");
    assert_eq!(decoded, None);
}

#[test]
fn string_round_trip() {
    for s in ["", "hello", "Polkadot", "unicode: \u{1F600}"] {
        let mut enc = ScaleEncoder::new();
        enc.encode_string(s);
        let bytes = enc.finish();
        let mut dec = ScaleDecoder::new(&bytes);
        let decoded = dec.decode_string().expect("string decode");
        assert_eq!(decoded, s);
    }
}

#[test]
fn bytes_round_trip() {
    let data: Vec<u8> = (0..=255).collect();
    let mut enc = ScaleEncoder::new();
    enc.encode_bytes(&data);
    let bytes = enc.finish();
    let mut dec = ScaleDecoder::new(&bytes);
    let decoded = dec.decode_bytes().expect("bytes decode");
    assert_eq!(decoded, data);
}

#[test]
fn fixed_array_round_trip() {
    let arr: [u8; 32] = [0xAB; 32];
    let mut dec = ScaleDecoder::new(&arr);
    let decoded: [u8; 32] = dec.decode_fixed_array().expect("fixed array decode");
    assert_eq!(decoded, arr);
}

// ---------------------------------------------------------------------------
// Error cases
// ---------------------------------------------------------------------------

#[test]
fn truncated_input_u32() {
    let bytes = vec![0x01, 0x00]; // only 2 bytes, need 4
    let mut dec = ScaleDecoder::new(&bytes);
    assert!(matches!(
        dec.decode_u32(),
        Err(CodecError::UnexpectedEof { .. })
    ));
}

#[test]
fn truncated_compact_two_byte() {
    // First byte signals two-byte mode but there is no second byte.
    let bytes = vec![0x01]; // low bits = 01 → two-byte mode, but truncated
    let mut dec = ScaleDecoder::new(&bytes);
    assert!(matches!(
        dec.decode_compact_u32(),
        Err(CodecError::UnexpectedEof { .. })
    ));
}

#[test]
fn truncated_compact_four_byte() {
    // First byte signals four-byte mode but only 1 extra byte follows.
    let bytes = vec![0x02, 0x00]; // low bits = 10 → four-byte mode, truncated
    let mut dec = ScaleDecoder::new(&bytes);
    assert!(matches!(
        dec.decode_compact_u32(),
        Err(CodecError::UnexpectedEof { .. })
    ));
}

#[test]
fn empty_input_error() {
    let bytes: Vec<u8> = vec![];
    let mut dec = ScaleDecoder::new(&bytes);
    assert!(matches!(
        dec.decode_u8(),
        Err(CodecError::UnexpectedEof { .. })
    ));
}

#[test]
fn decoder_position_tracks_correctly() {
    let mut enc = ScaleEncoder::new();
    enc.encode_u8(1);
    enc.encode_u16(2);
    enc.encode_u32(3);
    let bytes = enc.finish();
    let mut dec = ScaleDecoder::new(&bytes);
    assert_eq!(dec.position(), 0);
    dec.decode_u8().expect("u8");
    assert_eq!(dec.position(), 1);
    dec.decode_u16().expect("u16");
    assert_eq!(dec.position(), 3);
    dec.decode_u32().expect("u32");
    assert_eq!(dec.position(), 7);
    assert!(dec.is_empty());
}

#[test]
fn decoder_remaining_tracks_correctly() {
    let bytes = vec![1u8, 2, 3, 4];
    let mut dec = ScaleDecoder::new(&bytes);
    assert_eq!(dec.remaining(), 4);
    dec.decode_u8().expect("u8");
    assert_eq!(dec.remaining(), 3);
    dec.decode_u16().expect("u16");
    assert_eq!(dec.remaining(), 1);
}

#[test]
fn encoder_default_is_empty() {
    let enc = ScaleEncoder::default();
    assert!(enc.as_bytes().is_empty());
}

#[test]
fn encoder_with_capacity() {
    let enc = ScaleEncoder::with_capacity(64);
    assert!(enc.as_bytes().is_empty());
}

#[test]
fn little_endian_byte_order() {
    let mut enc = ScaleEncoder::new();
    enc.encode_u32(0x0102_0304);
    assert_eq!(enc.finish(), vec![0x04, 0x03, 0x02, 0x01]);
}

#[test]
fn compact_big_integer_u32_max_decode() {
    // u32::MAX in big-integer mode: mode byte + 4 bytes
    let mut enc = ScaleEncoder::new();
    enc.encode_compact_u32(u32::MAX);
    let bytes = enc.finish();
    let mut dec = ScaleDecoder::new(&bytes);
    let v = dec.decode_compact_u32().expect("compact u32::MAX");
    assert_eq!(v, u32::MAX);
}
