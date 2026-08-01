//! Criterion benchmarks for performance-critical SCALE codec paths.
//!
//! Covers:
//! - Compact u32 decode (single-byte, two-byte, four-byte modes)
//! - Extrinsic decode (Balances.transferKeepAlive fixture)
//! - Metadata parsing (minimal v14 fixture)
//!
//! PRD-15 performance benchmarks.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use polkagent_codec::{
    metadata::build_minimal_metadata_v14,
    parse_metadata,
    scale::{ScaleDecoder, ScaleEncoder},
    decode_extrinsic,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Build an unsigned extrinsic for Balances.transferKeepAlive.
///
/// Layout:
///   [compact length] [version=0x04] [pallet=5] [call=3] [dest: 00 ++ 32 bytes] [value: compact u128]
fn make_transfer_keep_alive_extrinsic() -> Vec<u8> {
    let mut enc = ScaleEncoder::new();

    // pallet 5 = Balances, call 3 = transfer_keep_alive
    let pallet: u8 = 5;
    let call: u8 = 3;

    // Unsigned extrinsic body (no signature)
    let mut body = ScaleEncoder::new();
    body.encode_u8(0x04); // version byte (unsigned)
    body.encode_u8(pallet);
    body.encode_u8(call);
    // dest: MultiAddress::Id(0x00 prefix + 32-byte AccountId)
    body.encode_u8(0x00); // MultiAddress variant = Id
    body.encode_bytes(&[0xdeu8; 32]); // 32-byte account (placeholder)
    // value: compact-encoded u128 (e.g. 1_000_000_000 planck)
    body.encode_compact_u64(1_000_000_000u64);

    let body_bytes = body.finish();
    // Outer compact length prefix
    enc.encode_compact_u32(body_bytes.len() as u32);
    let mut out = enc.finish();
    out.extend_from_slice(&body_bytes);
    out
}

/// Build a minimal v14 metadata fixture with two pallets.
fn make_minimal_metadata() -> Vec<u8> {
    build_minimal_metadata_v14(&[
        ("System", 0),
        ("Balances", 5),
    ])
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

fn bench_compact_u32_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("compact_u32_decode");

    // Single-byte mode (value 0–63)
    let single_byte: Vec<u8> = {
        let mut enc = ScaleEncoder::new();
        enc.encode_compact_u32(42);
        enc.finish()
    };

    // Two-byte mode (value 64–16383)
    let two_byte: Vec<u8> = {
        let mut enc = ScaleEncoder::new();
        enc.encode_compact_u32(1000);
        enc.finish()
    };

    // Four-byte mode (value 16384–2^30−1)
    let four_byte: Vec<u8> = {
        let mut enc = ScaleEncoder::new();
        enc.encode_compact_u32(100_000);
        enc.finish()
    };

    group.bench_function(BenchmarkId::new("single_byte", "42"), |b| {
        b.iter(|| {
            let mut dec = ScaleDecoder::new(&single_byte);
            let v = dec.decode_compact_u32().expect("decode");
            criterion::black_box(v);
        });
    });

    group.bench_function(BenchmarkId::new("two_byte", "1000"), |b| {
        b.iter(|| {
            let mut dec = ScaleDecoder::new(&two_byte);
            let v = dec.decode_compact_u32().expect("decode");
            criterion::black_box(v);
        });
    });

    group.bench_function(BenchmarkId::new("four_byte", "100000"), |b| {
        b.iter(|| {
            let mut dec = ScaleDecoder::new(&four_byte);
            let v = dec.decode_compact_u32().expect("decode");
            criterion::black_box(v);
        });
    });

    // 1000-iteration batch (matches PRD-15 requirement)
    group.bench_function("batch_1000", |b| {
        b.iter(|| {
            for _ in 0..1000 {
                let mut dec = ScaleDecoder::new(&four_byte);
                let v = dec.decode_compact_u32().expect("decode");
                criterion::black_box(v);
            }
        });
    });

    group.finish();
}

fn bench_extrinsic_decode(c: &mut Criterion) {
    let extrinsic = make_transfer_keep_alive_extrinsic();

    c.bench_function("extrinsic_decode_transfer_keep_alive", |b| {
        b.iter(|| {
            let result = decode_extrinsic(&extrinsic).expect("decode");
            criterion::black_box(result);
        });
    });
}

fn bench_metadata_parse(c: &mut Criterion) {
    let metadata_bytes = make_minimal_metadata();

    c.bench_function("metadata_parse_minimal_v14", |b| {
        b.iter(|| {
            let result = parse_metadata(&metadata_bytes).expect("parse");
            criterion::black_box(result);
        });
    });
}

criterion_group!(
    benches,
    bench_compact_u32_decode,
    bench_extrinsic_decode,
    bench_metadata_parse,
);
criterion_main!(benches);
